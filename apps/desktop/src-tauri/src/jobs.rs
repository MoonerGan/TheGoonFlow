//! Job registry — imports, clips and update checks run as *jobs*, so one long
//! import never blocks the rest of the app and a nuke/lock can stop everything.
//!
//! The frontend polls [`Jobs::snapshot`] while something is running (cheap: a
//! dozen small rows) instead of receiving events — fewer moving parts, and a
//! late-loading webview still sees what happened.

use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobInfo {
    pub id: u64,
    /// bookmarks | games | images | clip | updates
    pub kind: String,
    /// what the user picked, for display ("bookmarks.html", "D:\games")
    pub label: String,
    pub done: usize,
    /// 0 = still unknown
    pub total: usize,
    /// the item currently being worked on
    pub current: String,
    /// Nested detail, written by [`JobHandle::child`] — what the owner is doing
    /// *inside* the current item (which source, which archive entry). Kept apart
    /// from `done`/`total` on purpose: see #49, where a per-source worker
    /// destroyed the batch's counts by reusing the same handle.
    pub sub_done: usize,
    pub sub_total: usize,
    pub sub_current: String,
    pub note: String,
    pub finished: bool,
    pub cancelled: bool,
    pub started_at: u64,
    /// set when the job finishes — the UI fades finished rows after a while
    pub finished_at: u64,
}

struct Entry {
    info: JobInfo,
    cancel: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct Jobs {
    next_id: AtomicU64,
    entries: Mutex<Vec<Entry>>,
}

/// Handle passed into the work loop. Cheap to clone, Send+Sync.
///
/// **One writer per level.** A job's `done`/`total` belong to whoever called
/// [`Jobs::start`]. A function that works *inside* the current item (one item's
/// sources, one archive's entries) must use [`JobHandle::child`], which writes to
/// the `sub_*` fields instead — otherwise the outer count is silently replaced by
/// the inner one (#49: an 85-item batch displayed `0/1` for its whole run).
pub struct JobHandle {
    id: u64,
    jobs: Arc<Jobs>,
    cancel: Arc<AtomicBool>,
    /// true = this handle writes the nested `sub_*` detail, not the job's counts
    nested: bool,
}

impl JobHandle {
    /// True once the user pressed Stop, or the vault was locked/nuke'd.
    /// Long loops must check this per item and bail out.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Handle for the work *inside* the current item. Cancellation is shared, the
    /// numbers are not.
    pub fn child(&self) -> JobHandle {
        JobHandle {
            id: self.id,
            jobs: self.jobs.clone(),
            cancel: self.cancel.clone(),
            nested: true,
        }
    }

    pub fn set_progress(&self, done: usize, total: usize, current: &str) {
        if self.nested {
            self.jobs.patch(self.id, |info| {
                info.sub_done = done;
                info.sub_total = total;
                info.sub_current = current.to_string();
            });
            return;
        }
        // The owner moving to the next item invalidates whatever the last item's
        // nested detail said.
        self.jobs.patch(self.id, |info| {
            info.done = done;
            info.total = total;
            info.current = current.to_string();
            info.sub_done = 0;
            info.sub_total = 0;
            info.sub_current = String::new();
        });
    }

    pub fn set_note(&self, note: &str) {
        if self.nested {
            return; // nested workers must not erase the owner's note
        }
        self.jobs
            .patch(self.id, |info| info.note = note.to_string());
    }

    pub fn finish(&self, note: &str) {
        self.jobs.patch(self.id, |info| {
            info.finished = true;
            info.finished_at = now_secs();
            info.current = String::new();
            info.sub_current = String::new();
            info.note = note.to_string();
        });
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default()
}

impl Jobs {
    /// One job per kind at a time (two simultaneous bookmark imports of the
    /// same file would only race each other). Returns None if one is running.
    pub fn start(self: &Arc<Self>, kind: &str, label: &str) -> Option<JobHandle> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let cancel = Arc::new(AtomicBool::new(false));
        let mut entries = self.entries.lock().unwrap();
        if entries
            .iter()
            .any(|e| !e.info.finished && e.info.kind == kind)
        {
            return None;
        }
        entries.push(Entry {
            info: JobInfo {
                id,
                kind: kind.to_string(),
                label: label.to_string(),
                done: 0,
                total: 0,
                current: "starting…".into(),
                sub_done: 0,
                sub_total: 0,
                sub_current: String::new(),
                note: String::new(),
                finished: false,
                cancelled: false,
                started_at: now_secs(),
                finished_at: 0,
            },
            cancel: cancel.clone(),
        });
        // Keep the recent-finished list bounded so the strip stays readable.
        const KEEP_FINISHED: usize = 8;
        let finished: Vec<u64> = entries
            .iter()
            .filter(|e| e.info.finished)
            .map(|e| e.info.id)
            .collect();
        if finished.len() > KEEP_FINISHED {
            let drop_upto = finished.len() - KEEP_FINISHED;
            let drop: Vec<u64> = finished.iter().copied().take(drop_upto).collect();
            entries.retain(|e| !drop.contains(&e.info.id));
        }
        Some(JobHandle {
            id,
            jobs: self.clone(),
            cancel,
            nested: false,
        })
    }

    fn patch(&self, id: u64, f: impl FnOnce(&mut JobInfo)) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.iter_mut().find(|e| e.info.id == id) {
            f(&mut e.info);
        }
    }

    pub fn snapshot(&self) -> Vec<JobInfo> {
        let entries = self.entries.lock().unwrap();
        entries.iter().map(|e| e.info.clone()).collect()
    }

    /// Stop one job. The work loop notices on its next item.
    pub fn cancel(&self, id: u64) {
        let mut entries = self.entries.lock().unwrap();
        if let Some(e) = entries.iter_mut().find(|e| e.info.id == id) {
            e.cancel.store(true, Ordering::Relaxed);
            e.info.cancelled = true;
            e.info.note = "stopping…".into();
        }
    }

    /// Lock/nuke: every running job is told to stop right now. Network fetches
    /// already in flight land nowhere, because the vault connection is closed
    /// and each loop re-checks `opened`.
    pub fn cancel_all(&self) {
        let mut entries = self.entries.lock().unwrap();
        for e in entries.iter_mut() {
            if !e.info.finished {
                e.cancel.store(true, Ordering::Relaxed);
                e.info.cancelled = true;
            }
        }
    }

    pub fn clear(&self) {
        self.entries.lock().unwrap().clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_job_per_kind_and_progress_visible() {
        let jobs = Arc::new(Jobs::default());
        let bm = jobs
            .start("bookmarks", "bookmarks.html")
            .expect("first starts");
        assert!(
            jobs.start("bookmarks", "again.html").is_none(),
            "same kind is refused"
        );
        assert!(
            jobs.start("games", "D:\\games").is_some(),
            "different kind runs alongside"
        );

        bm.set_progress(3, 10, "https://x/post/3");
        let snap = jobs.snapshot();
        let running = snap.iter().find(|j| j.kind == "bookmarks").unwrap();
        assert_eq!(running.done, 3);
        assert_eq!(running.total, 10);
        assert_eq!(running.current, "https://x/post/3");
        assert!(!running.finished);

        bm.finish("10 imported");
        let next = jobs.start("bookmarks", "next.html");
        assert!(next.is_some(), "kind frees up after finish");
        let snap = jobs.snapshot();
        let live: Vec<&str> = snap
            .iter()
            .filter(|j| !j.finished)
            .map(|j| j.kind.as_str())
            .collect();
        assert_eq!(live, vec!["games", "bookmarks"]);
    }

    #[test]
    fn nested_worker_cannot_destroy_the_owner_counts() {
        // #49: an 85-item batch re-fetch showed "0/1" for its whole run because
        // the per-item worker wrote progress on the parent's own handle.
        let jobs = Arc::new(Jobs::default());
        let job = jobs.start("enrich", "85 items").expect("first starts");
        let id = job.id;
        job.set_progress(0, 85, "item-1");

        let inner = job.child();
        inner.set_progress(0, 1, "https://rule34.xxx/post/1");
        inner.set_note("ignored");

        let snap = jobs.snapshot();
        let j = snap.iter().find(|j| j.id == id).unwrap();
        assert_eq!(j.done, 0);
        assert_eq!(j.total, 85, "the batch denominator survives");
        assert_eq!(j.current, "item-1");
        assert_eq!(j.sub_total, 1);
        assert_eq!(j.sub_current, "https://rule34.xxx/post/1");
        assert_eq!(j.note, "", "a nested worker may not rewrite the job note");

        // the owner moving on clears the stale nested detail
        drop(snap);
        job.set_progress(1, 85, "item-2");
        let snap = jobs.snapshot();
        let j = snap.iter().find(|j| j.id == id).unwrap();
        assert_eq!(j.done, 1);
        assert_eq!(j.sub_done, 0);
        assert_eq!(j.sub_current, "");

        // Stop still reaches the nested worker
        jobs.cancel(id);
        assert!(inner.cancelled());
    }

    #[test]
    fn cancel_reaches_the_work_loop() {
        let jobs = Arc::new(Jobs::default());
        let job = jobs.start("updates", "all").unwrap();
        assert!(!job.cancelled());
        let id = jobs.snapshot()[0].id;
        jobs.cancel(id);
        assert!(job.cancelled());

        let other = jobs.start("clip", "one url").unwrap();
        jobs.cancel_all();
        assert!(other.cancelled());
        assert_eq!(jobs.snapshot().iter().filter(|j| j.cancelled).count(), 2);
    }
}
