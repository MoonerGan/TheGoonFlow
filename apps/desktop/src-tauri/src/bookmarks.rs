//! D13: browser bookmark export import (Netscape format — Firefox/Chrome).
//!
//! Classification and enrichment live in [`crate::ingest`] so a bookmark, a
//! manual "+ Clip URL" and (later) the browser extension all go down the same
//! path — which is why `file:///…mp4` and direct image links resolve as media
//! instead of landing in "unrecognized".
//!
//! Runs as a [`JobHandle`]: one vault lock **per bookmark** (so the rest of the
//! app stays usable and the grid fills up visibly), progress per item, and a
//! cancellation check on every iteration — locking or nuking stops it.

use crate::ingest::{self, Mode, SiteRef};
use crate::jobs::JobHandle;
use crate::{new_id, now_epoch, Core};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, Status};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;

#[derive(Clone, Debug)]
pub struct Bookmark {
    pub url: String,
    pub title: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FailureDetail {
    pub url: String,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookmarkReport {
    pub parsed: usize,
    pub imported: usize,
    pub duplicates: usize,
    pub unrecognized: usize,
    /// Bookmarks on sites that are switched off under Sources.
    pub disabled: usize,
    pub thumbnailed: usize,
    pub tags_added: usize,
    pub fetch_failures: usize,
    /// True when the user stopped it (or the vault was locked) part-way.
    pub stopped: bool,
    pub by_site: BTreeMap<String, usize>,
    /// Per-item failure detail (capped) so the user knows what failed and why.
    pub failures: Vec<FailureDetail>,
    /// Sample URLs of bookmarks we couldn't classify (capped).
    pub unrecognized_sample: Vec<String>,
}

const MAX_FAILURE_DETAILS: usize = 60;
const MAX_UNRECOGNIZED_SAMPLE: usize = 15;

// ---------- parsing ----------

pub fn parse_bookmarks(html: &str) -> Vec<Bookmark> {
    let mut out = Vec::new();
    let lower = html.to_lowercase();
    let mut pos = 0usize;
    while let Some(found) = lower[pos..].find("<a href=\"") {
        let start = pos + found + 9;
        let Some(rel_end) = html[start..].find('"') else {
            break;
        };
        let url_raw = html[start..start + rel_end].trim().to_string();
        // Attribute values in exports are entity-escaped (`&amp;`), same as titles.
        let url = decode_entities(&url_raw);
        let after = start + rel_end + 1;
        // Skip to the end of the opening tag (real exports add ADD_DATE etc).
        let text_start = html[after..]
            .find('>')
            .map(|i| after + i + 1)
            .unwrap_or(after);
        // Netscape exports use uppercase tags (`</A>`) — search the lowercased copy.
        let close = lower[after..]
            .find("</a>")
            .map(|i| after + i)
            .unwrap_or(text_start);
        let title = decode_entities(html[text_start..close.max(text_start)].trim());
        if !url.is_empty() {
            out.push(Bookmark { url, title });
        }
        pos = close;
    }
    out
}

fn decode_entities(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("http") || s.contains("://") || s.starts_with("www.")
}

/// Bookmark title vs. whatever the source (or the filename) gave us.
fn pick_title(bm_title: &str, meta_title: Option<&str>, cls: &SiteRef) -> String {
    let fallback = match &cls.remote_id {
        Some(rid) => format!("{} {}", cls.site, rid),
        None => cls.site.clone(),
    };
    if let Some(t) = meta_title.filter(|t| !t.trim().is_empty()) {
        return t.to_string();
    }
    let t = bm_title.trim();
    if t.is_empty() || looks_like_url(t) {
        fallback
    } else {
        t.to_string()
    }
}

// ---------- import ----------

struct Pending {
    item_id: String,
    site: String,
    url: String,
    reference: SiteRef,
}

enum Stored {
    Inserted { id: String, site: String },
    Duplicate,
    Disabled,
    Failed(String),
}

pub async fn import_bookmarks(
    state: &Core,
    file_path: String,
    fetch_online: bool,
    job: &JobHandle,
) -> Result<BookmarkReport, String> {
    let html = fs::read_to_string(&file_path).map_err(|e| e.to_string())?;
    let bookmarks = parse_bookmarks(&html);
    let mut report = BookmarkReport {
        parsed: bookmarks.len(),
        imported: 0,
        duplicates: 0,
        unrecognized: 0,
        disabled: 0,
        thumbnailed: 0,
        tags_added: 0,
        fetch_failures: 0,
        stopped: false,
        by_site: BTreeMap::new(),
        failures: Vec::new(),
        unrecognized_sample: Vec::new(),
    };
    job.set_progress(0, bookmarks.len(), "reading file…");

    // Phase 1 — store items + sources. No network except local files, and the
    // vault lock is taken per bookmark so the UI can keep working.
    let mut pending = Vec::new();
    for (idx, bm) in bookmarks.iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(idx, bookmarks.len(), &bm.url);

        let Some(cls) = ingest::classify(&bm.url) else {
            report.unrecognized += 1;
            if report.unrecognized_sample.len() < MAX_UNRECOGNIZED_SAMPLE {
                report.unrecognized_sample.push(bm.url.clone());
            }
            continue;
        };

        // Local media is already on disk: read it now, no network needed.
        let local = matches!(cls.mode, Mode::LocalFile)
            .then(|| ingest::local_file(&bm.url))
            .and_then(Result::ok);

        let outcome = {
            let mut inner = state.inner.lock().unwrap();
            match inner.opened.as_mut() {
                // vault locked underneath us (Lock or nuke) — stop cleanly
                None => Stored::Failed("vault is locked".into()),
                Some(opened) => {
                    let conn = &mut opened.conn;
                    if !db::site_enabled(conn, &cls.site, !ingest::is_mirror_site(&cls.site)) {
                        Stored::Disabled
                    } else if db::source_item_id(conn, &cls.site, &bm.url).is_some() {
                        Stored::Duplicate
                    } else {
                        store_one(conn, bm, &cls, local.as_ref())
                    }
                }
            }
        };

        match outcome {
            Stored::Inserted { id, site } => {
                *report.by_site.entry(site).or_insert(0) += 1;
                report.imported += 1;
                if let Some(meta) = &local {
                    if meta.store_bytes.is_some() || meta.thumb_bytes.is_some() {
                        let mut inner = state.inner.lock().unwrap();
                        if let Some(opened) = inner.opened.as_mut() {
                            let conn = &mut opened.conn;
                            if let Some(bytes) = &meta.store_bytes {
                                media::put_full_image(conn, &id, bytes);
                            }
                            if let Some(bytes) = &meta.thumb_bytes {
                                if media::put_thumbnail(conn, &id, bytes) {
                                    report.thumbnailed += 1;
                                }
                            }
                        }
                    }
                }
                if fetch_online && !matches!(cls.mode, Mode::LocalFile) {
                    pending.push(Pending {
                        item_id: id,
                        site: cls.site.clone(),
                        url: bm.url.clone(),
                        reference: cls,
                    });
                }
            }
            Stored::Duplicate => report.duplicates += 1,
            Stored::Disabled => {
                report.disabled += 1;
                if report.failures.len() < MAX_FAILURE_DETAILS {
                    report.failures.push(FailureDetail {
                        url: bm.url.clone(),
                        reason: format!("{} is switched off under Sources", cls.site),
                    });
                }
            }
            Stored::Failed(reason) => {
                // vault gone (Lock/Nuke): stop this job so phase 2 bails too
                job.cancel();
                if report.failures.len() < MAX_FAILURE_DETAILS {
                    report.failures.push(FailureDetail {
                        url: bm.url.clone(),
                        reason,
                    });
                }
                report.stopped = true;
                break;
            }
        }
    }

    // Phase 2 — optional online enrichment, politely rate-limited per site.
    if !pending.is_empty() && !report.stopped {
        // Round-robin across sites: with e-hentai at ~10.5 s/request a strict
        // file order stalls the whole queue on one slow lane. Interleaving
        // keeps every site visibly progressing while each still honours its
        // own delay.
        let mut queues: BTreeMap<String, std::collections::VecDeque<Pending>> = BTreeMap::new();
        for p in pending {
            queues.entry(p.site.clone()).or_default().push_back(p);
        }
        let mut ordered: Vec<Pending> = Vec::new();
        while queues.values().any(|q| !q.is_empty()) {
            for q in queues.values_mut() {
                if let Some(p) = q.pop_front() {
                    ordered.push(p);
                }
            }
        }

        let client = crate::http::client_for(state, 15)?;
        let total = bookmarks.len() + ordered.len();
        let mut done = bookmarks.len();
        for p in ordered {
            if job.cancelled() {
                report.stopped = true;
                break;
            }
            job.set_progress(done, total, &p.url);
            let rate = rate_of(state, &p.site);
            job.set_note(&format!("fetching {} (polite delay {rate} ms)", p.site));

            // Shared per-site lane (#15): guaranteed spacing against *all*
            // other fetch work, not just this loop's previous request.
            if !state.queue.reserve(&p.site, rate, || job.cancelled()).await {
                report.stopped = true;
                break;
            }

            let cookie = {
                let inner = state.inner.lock().unwrap();
                inner
                    .opened
                    .as_ref()
                    .and_then(|o| db::site_cookie(&o.conn, &p.site))
            };
            let ctx = ingest::FetchCtx {
                cookie: cookie.as_deref(),
                core: Some(state),
            };
            let meta = match ingest::fetch_meta(&client, &p.reference, &p.url, &ctx).await {
                Ok(m) => m,
                Err(e) => {
                    report.fetch_failures += 1;
                    if report.failures.len() < MAX_FAILURE_DETAILS {
                        report.failures.push(FailureDetail {
                            url: p.url.clone(),
                            reason: e,
                        });
                    }
                    done += 1;
                    continue;
                }
            };

            let mut inner = state.inner.lock().unwrap();
            let Some(opened) = inner.opened.as_mut() else {
                // vault locked mid-import (Lock or nuke): stop fetching
                job.cancel();
                report.stopped = true;
                break;
            };
            let conn = &mut opened.conn;

            let (thumb, tags, _title_updated) =
                ingest::apply_meta(conn, &p.item_id, &p.site, &meta);
            if thumb {
                report.thumbnailed += 1;
            }
            report.tags_added += tags;
            if !thumb && tags == 0 && meta.title.is_none() && meta.version.is_none() {
                report.fetch_failures += 1;
                if report.failures.len() < MAX_FAILURE_DETAILS {
                    report.failures.push(FailureDetail {
                        url: p.url.clone(),
                        reason: meta
                            .note
                            .clone()
                            .unwrap_or_else(|| "no metadata came back for this link".into()),
                    });
                }
            }
            done += 1;
        }
    }

    if report.stopped {
        job.set_note(&format!("stopped after {} imported", report.imported));
    }
    Ok(report)
}

fn rate_of(state: &Core, site: &str) -> i64 {
    let inner = state.inner.lock().unwrap();
    match inner.opened.as_ref() {
        Some(o) => db::site_rate_ms(&o.conn, site, ingest::default_rate_ms(site)),
        None => ingest::default_rate_ms(site),
    }
}

fn store_one(
    conn: &mut rusqlite::Connection,
    bm: &Bookmark,
    cls: &SiteRef,
    local: Option<&ingest::Meta>,
) -> Stored {
    let title = pick_title(&bm.title, local.and_then(|m| m.title.as_deref()), cls);
    let id = new_id("b");
    let item = Item {
        id: id.clone(),
        kind: cls.kind,
        title,
        original_title: None,
        circle: None,
        status: Status::Seen,
        dnf: false,
        rating: None,
        notes: String::new(),
        version: local.and_then(|m| m.version.clone()),
        created_at: now_epoch(),
        updated_at: now_epoch(),
    };
    if let Err(e) = db::insert_item(conn, &item) {
        return Stored::Failed(format!("could not store item: {e}"));
    }
    let created = db::insert_source(
        conn,
        &id,
        &cls.site,
        &bm.url,
        cls.remote_id.as_deref(),
        None,
    )
    .unwrap_or(false);
    if !created {
        let _ = db::delete_item(conn, &id);
        return Stored::Duplicate;
    }
    Stored::Inserted {
        id,
        site: cls.site.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs::Jobs;
    use goon_core::model::ItemKind;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    fn netscape(body: &str) -> String {
        let mut s = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<DL><p>\n");
        for line in body.lines() {
            s.push_str(&format!("    <DT><A HREF=\"{line}\">{line}</A>\n"));
        }
        s.push_str("</DL><p>\n");
        s
    }

    fn test_state(tag: &str) -> (crate::Core, std::path::PathBuf) {
        let tmp = std::env::temp_dir().join(format!("goonflow-bm-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let vault = tmp.join("vault");
        let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
        let conn = goon_vault::open_db(&vault, &key).unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        let state = crate::Core {
            inner: Arc::new(Mutex::new(crate::Inner {
                vault_root: Some(vault),
                opened: Some(crate::OpenedVault { conn, _key: key }),
                ..Default::default()
            })),
            jobs: Arc::new(Jobs::default()),
            queue: Arc::new(crate::queue::SiteQueue::default()),
        };
        (state, tmp)
    }

    /// The reported bug: bookmarks pointing at local .mp4/.png files and at
    /// direct booru CDN links were "not a known site yet".
    #[tokio::test]
    async fn local_and_direct_media_bookmarks_import() {
        let (state, tmp) = test_state("media");
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
        )
        .unwrap();
        let img = tmp.join("art.png");
        std::fs::write(&img, &png).unwrap();
        let vid = tmp.join("clip.mp4");
        std::fs::write(&vid, b"not really a video").unwrap();

        let file_url =
            |p: &std::path::Path| format!("file:///{}", p.to_string_lossy().replace('\\', "/"));
        let body = format!(
            "{}\n{}\n{}\n{}\n{}\n{}\n",
            file_url(&vid),
            file_url(&img),
            "https://hypnohub.net//images/6c/e0/6ce01fd2de7cc88d217ac34e44f7601a.gif?267940",
            "https://hypnohub.net/index.php?page=post&s=view&id=267940",
            "https://support.mozilla.org/products/firefox",
            "https://www.mozilla.org/contribute/",
        );
        let bm_file = tmp.join("bookmarks.html");
        let mut f = fs::File::create(&bm_file).unwrap();
        f.write_all(netscape(&body).as_bytes()).unwrap();
        drop(f);

        // fetch_online = false: local files still resolve, CDN links stay bare.
        let job = state.jobs.start("bookmarks", "bookmarks.html").unwrap();
        let report = import_bookmarks(&state, bm_file.display().to_string(), false, &job)
            .await
            .unwrap();
        job.finish("done");

        assert_eq!(report.parsed, 6);
        assert_eq!(
            report.unrecognized, 2,
            "mozilla links are genuinely unknown"
        );
        assert_eq!(report.imported, 4, "imported = {:?}", report.by_site);
        assert_eq!(*report.by_site.get("local").unwrap_or(&0), 2);
        assert_eq!(*report.by_site.get("hypnohub").unwrap_or(&0), 2);
        assert!(report.thumbnailed >= 1, "local png should be thumbnailed");
        assert!(!report.stopped);

        let snap = state.jobs.snapshot();
        let j = snap.iter().find(|j| j.kind == "bookmarks").unwrap();
        assert!(j.finished, "job must be closed out");
        assert_eq!(j.total, 6, "progress saw every bookmark");

        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().unwrap().conn;
        let media_items =
            goon_core::db::list_items(conn, 100, 0, None, Some("media"), None, None, None).unwrap();
        assert_eq!(media_items.len(), 4, "all four are media items");
        let titles: Vec<&str> = media_items.iter().map(|i| i.title.as_str()).collect();
        assert!(
            titles.iter().any(|t| t.contains("clip")),
            "video stem used as title: {titles:?}"
        );
        let local_only =
            goon_core::db::list_items(conn, 100, 0, None, Some("media"), None, Some("local"), None)
                .unwrap();
        assert_eq!(local_only.len(), 2);
        drop(inner);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Lock/nuke path: a cancelled job stops at once and says it stopped.
    #[tokio::test]
    async fn cancelling_stops_the_import() {
        let (state, tmp) = test_state("cancel");
        let body: String = (1..=200)
            .map(|i| format!("https://gelbooru.com/index.php?page=post&s=view&id={i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let bm_file = tmp.join("bm.html");
        fs::write(&bm_file, netscape(&body)).unwrap();

        let job = state.jobs.start("bookmarks", "bm.html").unwrap();
        let id = state
            .jobs
            .snapshot()
            .iter()
            .rev()
            .find(|j| j.kind == "bookmarks")
            .unwrap()
            .id;
        state.jobs.cancel(id);
        let report = import_bookmarks(&state, bm_file.display().to_string(), false, &job)
            .await
            .unwrap();
        assert!(report.stopped, "import noticed the cancel");
        assert!(
            report.imported < 200,
            "did not run to the end: {}",
            report.imported
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn entity_escaped_local_urls_still_resolve() {
        // Real Firefox exports escape `'` in HREF as `&#39;` — the path has to
        // come back unescaped or the file lookup fails.
        let html = r##"<DL><p>
    <DT><A HREF="file:///C:/Users/Admin/Videos/[Kimochi]%20Cissia&#39;s%20Paizuri-fellatio%20animation/uncensored%20fix.mp4">uncensored fix.mp4</A>
    <DT><A HREF="https://hypnohub.net/index.php?page=post&amp;s=view&amp;id=267940">hypnohub post</A>
</DL><p>"##;
        let bms = parse_bookmarks(html);
        assert_eq!(bms.len(), 2);
        assert!(
            bms[0].url.contains("Cissia's"),
            "url unescaped: {}",
            bms[0].url
        );
        let cls = ingest::classify(&bms[0].url).unwrap();
        assert_eq!(cls.mode, Mode::LocalFile);
        assert!(
            bms[1].url.contains("&s=view"),
            "url unescaped: {}",
            bms[1].url
        );
        let cls = ingest::classify(&bms[1].url).unwrap();
        assert_eq!(cls.site, "hypnohub");
        assert_eq!(cls.remote_id.as_deref(), Some("267940"));
    }

    #[test]
    fn parses_netscape_bookmarks() {
        let html = r##"<DL><p>
    <DT><A HREF="https://gelbooru.com/index.php?page=post&s=view&id=4765984">cool post - Gelbooru</A>
    <DT><A HREF="https://rule34.xxx/index.php?page=post&s=view&id=123456&amp;foo=1">esc &amp; test</A>
    <DT><A HREF="file:///C:/Users/Admin/Videos/EnomeSnake.mp4">EnomeSnake.mp4</A>
    <DT><A HREF="https://example.com/random">nope</A>
</DL><p>"##;
        let bms = parse_bookmarks(html);
        assert_eq!(bms.len(), 4);
        assert_eq!(bms[0].title, "cool post - Gelbooru");
        assert_eq!(bms[1].title, "esc & test");
        let cls = ingest::classify(&bms[2].url).unwrap();
        assert_eq!(cls.mode, Mode::LocalFile);
        assert_eq!(cls.kind, ItemKind::Post);
    }

    #[test]
    fn mirrors_are_skipped_by_default() {
        let (state, tmp) = test_state("mirror");
        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().unwrap().conn;
        assert!(!db::site_enabled(conn, "kimochi", false));
        db::set_site_setting(conn, "kimochi", Some(true), None, None, 1200).unwrap();
        assert!(db::site_enabled(conn, "kimochi", false));
        // #28: enabling a site seeds *its* rate, not a blanket 700
        assert_eq!(db::site_rate_ms(conn, "kimochi", 1200), 1200);
        assert_eq!(db::site_rate_ms(conn, "e621", 1200), 1200);
        drop(inner);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
