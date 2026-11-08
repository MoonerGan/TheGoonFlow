//! D13: browser bookmark export import (Netscape format — Firefox/Chrome).
//!
//! Phase 1 (sync): parse + classify + create items/sources. Phase 2 (async,
//! user-initiated "enrich online"): polite per-site fetches (booru JSON APIs,
//! DLsite og:image) for thumbnail + tags. e-hentai/f95 land title-only until
//! M2 session cookies. No fetch happens unless the user checks the box.

use crate::{new_id, now_epoch, AppState};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, ItemKind, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::time::Duration;

const UA: &str = "TheGoonFlow/0.1 (personal library; user-initiated bookmark import)";
/// Polite spacing between online enrichment requests.
const FETCH_DELAY: Duration = Duration::from_millis(700);

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
        let url = html[start..start + rel_end].trim().to_string();
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

#[derive(Clone, Debug, PartialEq)]
pub struct Classified {
    pub site: &'static str,
    pub kind: ItemKind,
    pub remote_id: Option<String>,
}

pub fn classify(url: &str) -> Option<Classified> {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest
        .split('/')
        .next()
        .unwrap_or("")
        .to_lowercase()
        .trim_end_matches('/')
        .to_string();
    let host_no_port = host.split(':').next().unwrap_or("").to_string();
    let path = rest[host.len()..].to_string();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    if host_no_port.ends_with("gelbooru.com") {
        return Some(Classified {
            site: "gelbooru",
            kind: ItemKind::Post,
            remote_id: query_param(url, "pid").or_else(|| query_param(url, "id")),
        });
    }
    if host_no_port.ends_with("rule34.xxx") || host_no_port == "rule34.xxx" {
        return Some(Classified {
            site: "rule34",
            kind: ItemKind::Post,
            remote_id: query_param(url, "id"),
        });
    }
    if host_no_port.ends_with("e621.net") || host_no_port.ends_with("e926.net") {
        let id = segments
            .iter()
            .position(|s| *s == "posts" || *s == "post")
            .and_then(|i| segments.get(i + 1))
            .map(|s| s.trim_end_matches(".json").to_string())
            .filter(|s| s.chars().all(|c| c.is_ascii_digit()) && !s.is_empty());
        return Some(Classified {
            site: "e621",
            kind: ItemKind::Post,
            remote_id: id,
        });
    }
    if host_no_port.ends_with("e-hentai.org") {
        return Some(Classified {
            site: "e-hentai",
            kind: ItemKind::Gallery,
            remote_id: eh_gid(&segments),
        });
    }
    if host_no_port.ends_with("exhentai.org") {
        return Some(Classified {
            site: "exhentai",
            kind: ItemKind::Gallery,
            remote_id: eh_gid(&segments),
        });
    }
    if host_no_port.ends_with("dlsite.com") {
        return Some(Classified {
            site: "dlsite",
            kind: ItemKind::Game,
            remote_id: dlsite_product_id(url),
        });
    }
    if host_no_port.ends_with("f95zone.to") {
        return Some(Classified {
            site: "f95",
            kind: ItemKind::Game,
            remote_id: segments.last().map(|s| s.to_string()),
        });
    }
    // Known booru-family hosts: import title-only until site-packs (M1+).
    for known in [
        "danbooru.donmai.us",
        "safebooru.donmai.us",
        "yande.re",
        "konachan.com",
        "rule34hentai.net",
        "realbooru.com",
    ] {
        if host_no_port.ends_with(known) {
            return Some(Classified {
                site: known.split('.').next().unwrap_or(known),
                kind: ItemKind::Post,
                remote_id: query_param(url, "id"),
            });
        }
    }
    None
}

fn query_param(url: &str, key: &str) -> Option<String> {
    let q = url.split_once('?')?.1.split('#').next()?;
    for pair in q.split('&') {
        let mut kv = pair.splitn(2, '=');
        if kv.next() == Some(key) {
            let v = kv.next().unwrap_or("").trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

fn eh_gid(segments: &[&str]) -> Option<String> {
    let i = segments.iter().position(|s| *s == "g")?;
    segments.get(i + 1).map(|s| s.to_string())
}

fn dlsite_product_id(url: &str) -> Option<String> {
    let up = url.to_uppercase();
    let idx = up.find("RJ")?;
    let digits: String = up[idx + 2..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (!digits.is_empty()).then(|| format!("RJ{digits}"))
}

fn looks_like_url(s: &str) -> bool {
    let s = s.trim();
    s.starts_with("http") || s.contains("://") || s.starts_with("www.")
}

// ---------- import ----------

struct Pending {
    item_id: String,
    site: &'static str,
    remote_id: String,
    url: String,
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
        thumbnailed: 0,
        tags_added: 0,
        fetch_failures: 0,
        by_site: BTreeMap::new(),
        failures: Vec::new(),
        unrecognized_sample: Vec::new(),
    };

    // Phase 1 — create items + sources (no network).
    let mut pending = Vec::new();
    {
        let mut inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
        let now = now_epoch();
        for bm in &bookmarks {
            let Some(cls) = classify(&bm.url) else {
                report.unrecognized += 1;
                if report.unrecognized_sample.len() < MAX_UNRECOGNIZED_SAMPLE {
                    report.unrecognized_sample.push(bm.url.clone());
                }
                continue;
            };
            if db::source_exists(conn, cls.site, &bm.url) {
                report.duplicates += 1;
                continue;
            }
            let title = if bm.title.is_empty() || looks_like_url(&bm.title) {
                format!("{} {}", cls.site, cls.remote_id.clone().unwrap_or_else(|| "?".into()))
            } else {
                bm.title.clone()
            };
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
                version: None,
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
            let created = db::insert_source(conn, &id, cls.site, &bm.url, cls.remote_id.as_deref(), None)
                .unwrap_or(false);
            if !created {
                let _ = db::delete_item(conn, &id);
                report.duplicates += 1;
                continue;
            }
            *report.by_site.entry(cls.site.to_string()).or_insert(0) += 1;
            report.imported += 1;
            if fetch_online {
                if let Some(rid) = cls.remote_id {
                    pending.push(Pending {
                        item_id: id,
                        site: cls.site,
                        remote_id: rid,
                        url: bm.url.clone(),
                    });
                }
            }
        }
    }

    // Phase 2 — optional online enrichment, politely rate-limited.
    if fetch_online && !pending.is_empty() {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(20))
            .build()
            .map_err(|e| e.to_string())?;
        for p in pending {
            tokio::time::sleep(FETCH_DELAY).await;
            let outcome = match p.site {
                "gelbooru" | "rule34" => gelbooru_family(&client, p.site, &p.remote_id).await,
                "e621" => e621_meta(&client, &p.remote_id).await,
                "dlsite" => dlsite_cover(&client, &p.url)
                    .await
                    .map(|bytes| (bytes, Vec::<String>::new())),
                _ => Err("no enrichment API for this site yet".to_string()),
            };
            match outcome {
                Ok((bytes, tags)) => {
                    let mut inner = state.inner.lock().unwrap();
                    let Ok(conn) = inner.opened.as_mut().map(|o| &mut o.conn).ok_or(()) else {
                        break; // vault locked mid-import; stop fetching
                    };
                    if media::put_thumbnail(conn, &p.item_id, &bytes) {
                        report.thumbnailed += 1;
                    }
                    media::put_full_image(conn, &p.item_id, &bytes);
                    for t in tags.into_iter().take(40) {
                        if let Ok(tid) = db::tag_get_or_create(conn, &t, db::TYPE_CONTENT) {
                            let _ = db::link_item_tag(conn, &p.item_id, tid);
                            report.tags_added += 1;
                        }
                    }
                }
                Err(reason) => {
                    report.fetch_failures += 1;
                    if report.failures.len() < MAX_FAILURE_DETAILS {
                        report.failures.push(FailureDetail { url: p.url, reason });
                    }
                }
            }
        }
    }

    Ok(report)
}

async fn gelbooru_family(
    client: &reqwest::Client,
    site: &str,
    id: &str,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let (api, referer) = match site {
        "gelbooru" => (
            format!("https://gelbooru.com/index.php?page=dapi&s=post&q=index&json=1&pid={id}"),
            "https://gelbooru.com",
        ),
        "rule34" => (
            format!("https://rule34.xxx/public/index.php?page=dapi&s=post&q=index&json=1&id={id}"),
            "https://rule34.xxx",
        ),
        _ => return Err("unsupported site".into()),
    };
    let resp = client
        .get(&api)
        .header(reqwest::header::REFERER, referer)
        .send()
        .await
        .map_err(|e| format!("api request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("api returned http {}", resp.status()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("api reply was not json: {e}"))?;
    let node = v.get("post").or_else(|| v.get("posts"));
    let post = match node {
        Some(n) if n.is_array() => n.as_array().and_then(|a| a.first()),
        other => other,
    }
    .ok_or_else(|| "post not found (deleted, or id not parsed from url)".to_string())?;
    if post.is_null() {
        return Err("post not found (deleted?)".into());
    }
    post_from_json(post, referer, client).await
}

async fn post_from_json(
    post: &Value,
    referer: &str,
    client: &reqwest::Client,
) -> Result<(Vec<u8>, Vec<String>), String> {
    let file_url = post
        .get("file_url")
        .and_then(|u| u.as_str())
        .filter(|u| u.starts_with("http"))
        .ok_or_else(|| "post json had no file url (video? deleted?)".to_string())?;
    let tags = post
        .get("tags")
        .and_then(|t| t.as_str())
        .map(|s| s.split_whitespace().map(String::from).collect())
        .unwrap_or_default();
    let bytes = client
        .get(file_url)
        .header(reqwest::header::REFERER, referer)
        .send()
        .await
        .map_err(|e| format!("file download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("file download interrupted: {e}"))?;
    Ok((bytes.to_vec(), tags))
}

async fn e621_meta(client: &reqwest::Client, id: &str) -> Result<(Vec<u8>, Vec<String>), String> {
    let resp = client
        .get(format!("https://e621.net/posts/{id}.json"))
        .send()
        .await
        .map_err(|e| format!("api request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("api returned http {}", resp.status()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("api reply was not json: {e}"))?;
    let post = v.get("post").ok_or_else(|| "post not found (deleted?)".to_string())?;
    let file_url = post
        .get("file")
        .and_then(|f| f.get("url"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| "post json had no file url".to_string())?;
    let tags = post
        .get("tags")
        .and_then(|t| t.get("general"))
        .and_then(|g| g.as_array())
        .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
        .unwrap_or_default();
    let bytes = client
        .get(file_url)
        .header(reqwest::header::REFERER, "https://e621.net")
        .send()
        .await
        .map_err(|e| format!("file download failed: {e}"))?
        .bytes()
        .await
        .map_err(|e| format!("file download interrupted: {e}"))?;
    Ok((bytes.to_vec(), tags))
}

async fn dlsite_cover(client: &reqwest::Client, url: &str) -> Result<Vec<u8>, String> {
    let page = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("page fetch failed: {e}"))?
        .text()
        .await
        .map_err(|e| format!("page read failed: {e}"))?;
    let idx = page
        .find("og:image")
        .ok_or_else(|| "no cover image on page (age gate or region block?)".to_string())?;
    let after = &page[idx..];
    let cstart = after
        .find("content=\"")
        .ok_or_else(|| "no og:image content attribute".to_string())?
        + "content=\"".len();
    let img = after[cstart..].split('"').next().unwrap_or("").trim();
    if !img.starts_with("http") {
        return Err("og:image url was not http(s)".into());
    }
    client
        .get(img)
        .header(reqwest::header::REFERER, "https://www.dlsite.com")
        .send()
        .await
        .map_err(|e| format!("cover download failed: {e}"))?
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("cover download interrupted: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_netscape_bookmarks() {
        let html = r##"<DL><p>
    <DT><A HREF="https://gelbooru.com/index.php?page=post&s=view&id=4765984">cool post - Gelbooru</A>
    <DT><A HREF="https://rule34.xxx/index.php?page=post&s=view&id=123456&amp;foo=1">esc &amp; test</A>
    <DT><A HREF="https://example.com/random">nope</A>
</DL><p>"##;
        let bms = parse_bookmarks(html);
        assert_eq!(bms.len(), 3);
        assert_eq!(bms[0].title, "cool post - Gelbooru");
        assert_eq!(bms[1].title, "esc & test");
    }

    #[test]
    fn classifies_known_hosts() {
        let g = classify("https://gelbooru.com/index.php?page=post&s=view&id=4765984").unwrap();
        assert_eq!(g.site, "gelbooru");
        assert_eq!(g.kind, ItemKind::Post);
        // gelbooru view pages use id= for the post; pid= is pagination. Prefer id= for view URLs.
        assert_eq!(g.remote_id.as_deref(), Some("4765984"));

        let r = classify("https://rule34.xxx/index.php?page=post&s=view&id=123456").unwrap();
        assert_eq!(r.remote_id.as_deref(), Some("123456"));

        let e6 = classify("https://e621.net/posts/1234567").unwrap();
        assert_eq!(e6.remote_id.as_deref(), Some("1234567"));

        let eh = classify("https://e-hentai.org/g/1234567/abcdef0123/").unwrap();
        assert_eq!(eh.site, "e-hentai");
        assert_eq!(eh.remote_id.as_deref(), Some("1234567/abcdef0123".split('/').next().unwrap()));

        let dl = classify("https://www.dlsite.com/maniax/product/link.html?id=RJ123456").unwrap();
        assert_eq!(dl.site, "dlsite");
        assert_eq!(dl.kind, ItemKind::Game);
        assert_eq!(dl.remote_id.as_deref(), Some("RJ123456"));

        let f95 = classify("https://f95zone.to/threads/summer-witch.12345/").unwrap();
        assert_eq!(f95.site, "f95");

        assert!(classify("https://example.com/whatever").is_none());
    }
}
