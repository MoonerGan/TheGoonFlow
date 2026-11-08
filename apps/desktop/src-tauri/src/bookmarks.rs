//! D13: browser bookmark export import (Netscape format — Firefox/Chrome).
//!
//! Classification and enrichment live in [`crate::ingest`] so a bookmark, a
//! manual "+ Clip URL" and (later) the browser extension all go down the same
//! path — which is why `file:///…mp4` and direct image links resolve as media
//! instead of landing in "unrecognized".
//!
//! Phase 1 (sync): parse + classify + store items/sources, plus anything that
//! needs no network (local images get stored + thumbnailed immediately).
//! Phase 2 (async, opt-in "enrich online"): polite per-site fetches for
//! title/cover/tags. Nothing is fetched unless the user checks the box.

use crate::ingest::{self, Mode, SiteRef};
use crate::{new_id, now_epoch, AppState};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, Status};
use serde::Serialize;
use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

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

// ---------- import ----------

struct Pending {
    item_id: String,
    site: String,
    url: String,
    reference: SiteRef,
}

pub async fn import_bookmarks(
    state: &AppState,
    file_path: String,
    fetch_online: bool,
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
        by_site: BTreeMap::new(),
        failures: Vec::new(),
        unrecognized_sample: Vec::new(),
    };

    // Phase 1 — create items + sources (no network except local files).
    let mut pending = Vec::new();
    {
        let mut inner = state.inner.lock().unwrap();
        let conn = &mut inner.opened.as_mut().ok_or("vault is locked")?.conn;
        let now = now_epoch();
        for bm in &bookmarks {
            let Some(cls) = ingest::classify(&bm.url) else {
                report.unrecognized += 1;
                if report.unrecognized_sample.len() < MAX_UNRECOGNIZED_SAMPLE {
                    report.unrecognized_sample.push(bm.url.clone());
                }
                continue;
            };
            if !db::site_enabled(conn, &cls.site, !ingest::is_mirror_site(&cls.site)) {
                report.disabled += 1;
                if report.failures.len() < MAX_FAILURE_DETAILS {
                    report.failures.push(FailureDetail {
                        url: bm.url.clone(),
                        reason: format!("{} is switched off under Sources", cls.site),
                    });
                }
                continue;
            }
            if db::source_item_id(conn, &cls.site, &bm.url).is_some() {
                report.duplicates += 1;
                continue;
            }

            // Local media is already on disk: read it now, no network needed.
            let local = matches!(cls.mode, Mode::LocalFile)
                .then(|| ingest::local_file(&bm.url))
                .and_then(Result::ok);

            let title = pick_title(&bm.title, local.as_ref().and_then(|m| m.title.as_deref()), &cls);
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
                version: local.as_ref().and_then(|m| m.version.clone()),
                created_at: now.clone(),
                updated_at: now.clone(),
            };
            if let Err(e) = db::insert_item(conn, &item) {
                if report.failures.len() < MAX_FAILURE_DETAILS {
                    report.failures.push(FailureDetail {
                        url: bm.url.clone(),
                        reason: format!("could not store item: {e}"),
                    });
                }
                continue;
            }
            let created = db::insert_source(conn, &id, &cls.site, &bm.url, cls.remote_id.as_deref(), None)
                .unwrap_or(false);
            if !created {
                let _ = db::delete_item(conn, &id);
                report.duplicates += 1;
                continue;
            }
            *report.by_site.entry(cls.site.clone()).or_insert(0) += 1;
            report.imported += 1;

            if let Some(meta) = &local {
                if let Some(bytes) = &meta.store_bytes {
                    media::put_full_image(conn, &id, bytes);
                }
                if let Some(bytes) = &meta.thumb_bytes {
                    if media::put_thumbnail(conn, &id, bytes) {
                        report.thumbnailed += 1;
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
    }

    // Phase 2 — optional online enrichment, politely rate-limited per site.
    if !pending.is_empty() {
        let client = reqwest::Client::builder()
            .user_agent(ingest::UA)
            .timeout(Duration::from_secs(25))
            .build()
            .map_err(|e| e.to_string())?;
        let mut last: Option<(String, std::time::Instant)> = None;
        for p in pending {
            let rate = {
                let inner = state.inner.lock().unwrap();
                match inner.opened.as_ref() {
                    Some(o) => db::site_rate_ms(&o.conn, &p.site, ingest::default_rate_ms(&p.site)),
                    None => break, // vault locked mid-import: stop fetching
                }
            };
            if let Some((prev_site, when)) = &last {
                if prev_site == &p.site {
                    let need = Duration::from_millis(rate as u64);
                    if when.elapsed() < need {
                        tokio::time::sleep(need - when.elapsed()).await;
                    }
                }
            }
            last = Some((p.site.clone(), std::time::Instant::now()));

            let cookie = {
                let inner = state.inner.lock().unwrap();
                inner.opened.as_ref().and_then(|o| db::site_cookie(&o.conn, &p.site))
            };
            let ctx = ingest::FetchCtx { cookie: cookie.as_deref() };
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
                    continue;
                }
            };

            let mut inner = state.inner.lock().unwrap();
            let Ok(conn) = inner.opened.as_mut().map(|o| &mut o.conn).ok_or(()) else {
                break; // vault locked mid-import; stop fetching
            };

            let mut enriched = false;
            if let Some(bytes) = &meta.store_bytes {
                media::put_full_image(conn, &p.item_id, bytes);
                enriched = true;
            }
            if let Some(bytes) = &meta.thumb_bytes {
                if media::put_thumbnail(conn, &p.item_id, bytes) {
                    report.thumbnailed += 1;
                    enriched = true;
                }
            }
            for t in meta.tags.iter().take(40) {
                if db::add_item_tag(conn, &p.item_id, t, db::TYPE_CONTENT).is_ok() {
                    report.tags_added += 1;
                    enriched = true;
                }
            }
            // Enriched title/version only overwrite obvious placeholders.
            if let Some(detail) = db::get_item(conn, &p.item_id).map_err(|e| e.to_string())? {
                let placeholder = detail.title.is_empty()
                    || looks_like_url(&detail.title)
                    || detail.title == p.site
                    || detail.title.starts_with(&format!("{} ", p.site));
                let new_title = meta
                    .title
                    .as_deref()
                    .filter(|t| !t.trim().is_empty() && placeholder);
                if new_title.is_some() || meta.version.is_some() {
                    let _ = db::update_item(
                        conn,
                        &p.item_id,
                        new_title.unwrap_or(&detail.title),
                        detail.circle.as_deref(),
                        Status::from_str(&detail.status).unwrap_or(Status::Seen),
                        meta.version.as_deref().or(detail.version.as_deref()),
                        None,
                        &now_epoch(),
                    );
                }
            }
            if !enriched {
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
        }
    }

    Ok(report)
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

#[cfg(test)]
mod tests {
    use super::*;
    use goon_core::model::ItemKind;
    use std::io::Write;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    fn netscape(body: &str) -> String {
        let mut s = String::from("<!DOCTYPE NETSCAPE-Bookmark-file-1>\n<DL><p>\n");
        for line in body.lines() {
            s.push_str(&format!("    <DT><A HREF=\"{line}\">{line}</A>\n"));
        }
        s.push_str("</DL><p>\n");
        s
    }

    /// The reported bug: bookmarks pointing at local .mp4/.png files and at
    /// direct booru CDN links were "not a known site yet".
    #[tokio::test]
    async fn local_and_direct_media_bookmarks_import() {
        let tmp = std::env::temp_dir().join(format!("goonflow-bm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();

        // a real 1x1 png on disk so the local path actually resolves
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
        )
        .unwrap();
        let img = tmp.join("art.png");
        std::fs::write(&img, &png).unwrap();
        let vid = tmp.join("clip.mp4");
        std::fs::write(&vid, b"not really a video").unwrap();

        let file_url = |p: &std::path::Path| format!("file:///{}", p.to_string_lossy().replace('\\', "/"));
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

        let vault = tmp.join("vault");
        let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
        let conn = goon_vault::open_db(&vault, &key).unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        let state = AppState {
            inner: Mutex::new(crate::Inner {
                vault_root: Some(vault.clone()),
                opened: Some(crate::OpenedVault { conn, _key: key }),
            }),
            exe_dir: tmp.clone(),
            companion: None,
            locked: Arc::new(AtomicBool::new(false)),
        };

        // fetch_online = false: local files still resolve, CDN links stay bare.
        let report = import_bookmarks(&state, bm_file.display().to_string(), false)
            .await
            .unwrap();
        assert_eq!(report.parsed, 6);
        assert_eq!(report.unrecognized, 2, "mozilla links are genuinely unknown");
        assert_eq!(report.imported, 4, "imported = {:?}", report.by_site);
        assert_eq!(*report.by_site.get("local").unwrap_or(&0), 2);
        assert_eq!(*report.by_site.get("hypnohub").unwrap_or(&0), 2);
        assert!(report.thumbnailed >= 1, "local png should be thumbnailed");

        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().unwrap().conn;
        let media_items = goon_core::db::list_items(conn, 100, 0, None, Some("media"), None).unwrap();
        assert_eq!(media_items.len(), 4, "all four are media items");
        let titles: Vec<&str> = media_items.iter().map(|i| i.title.as_str()).collect();
        assert!(titles.iter().any(|t| t.contains("clip")), "video stem used as title: {titles:?}");
        drop(inner);

        let _ = std::fs::remove_dir_all(&tmp);
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
    fn entity_escaped_local_urls_still_resolve() {
        // Real Firefox exports escape `'` in HREF as `&#39;` — the path has to
        // come back unescaped or the file lookup fails.
        let html = r##"<DL><p>
    <DT><A HREF="file:///C:/Users/Admin/Videos/[Kimochi]%20Cissia&#39;s%20Paizuri-fellatio%20animation/uncensored%20fix.mp4">uncensored fix.mp4</A>
    <DT><A HREF="https://hypnohub.net/index.php?page=post&amp;s=view&amp;id=267940">hypnohub post</A>
</DL><p>"##;
        let bms = parse_bookmarks(html);
        assert_eq!(bms.len(), 2);
        assert!(bms[0].url.contains("Cissia's"), "url unescaped: {}", bms[0].url);
        let cls = ingest::classify(&bms[0].url).unwrap();
        assert_eq!(cls.mode, Mode::LocalFile);
        assert!(bms[1].url.contains("&s=view"), "url unescaped: {}", bms[1].url);
        let cls = ingest::classify(&bms[1].url).unwrap();
        assert_eq!(cls.site, "hypnohub");
        assert_eq!(cls.remote_id.as_deref(), Some("267940"));
    }

    #[test]
    fn mirrors_are_skipped_by_default() {
        let tmp = std::env::temp_dir().join(format!("goonflow-bm-mirror-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let vault = tmp.join("vault");
        let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
        let conn = goon_vault::open_db(&vault, &key).unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        assert!(!db::site_enabled(&conn, "kimochi", false));
        db::set_site_setting(&conn, "kimochi", Some(true), None, None).unwrap();
        assert!(db::site_enabled(&conn, "kimochi", false));
        assert_eq!(db::site_rate_ms(&conn, "kimochi", 1200), 700);
        assert_eq!(db::site_rate_ms(&conn, "e621", 1200), 1200);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
