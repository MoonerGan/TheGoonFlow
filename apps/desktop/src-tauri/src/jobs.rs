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
    pub note: String,
    pub finished: bool,
    pub cancelled: bool,
    pub started_at: u64,
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
pub struct JobHandle {
    id: u64,
    jobs: Arc<Jobs>,
    cancel: Arc<AtomicBool>,
}

impl JobHandle {
    /// True once the user pressed Stop, or the vault was locked/nuke'd.
    /// Long loops must check this per item and bail out.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn set_progress(&self, done: usize, total: usize, current: &str) {
        self.jobs.patch(self.id, |info| {
            info.done = done;
            info.total = total;
            info.current = current.to_string();
        });
    }

    pub fn set_note(&self, note: &str) {
        self.jobs.patch(self.id, |info| info.note = note.to_string());
    }

    pub fn finish(&self, note: &str) {
        self.jobs.patch(self.id, |info| {
            info.finished = true;
            info.current = String::new();
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
        if entries.iter().any(|e| !e.info.finished && e.info.kind == kind) {
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
                note: String::new(),
                finished: false,
                cancelled: false,
                started_at: now_secs(),
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
        Some(JobHandle { id, jobs: self.clone(), cancel })
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
        let bm = jobs.start("bookmarks", "bookmarks.html").expect("first starts");
        assert!(jobs.start("bookmarks", "again.html").is_none(), "same kind is refused");
        assert!(jobs.start("games", "D:\\games").is_some(), "different kind runs alongside");

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
