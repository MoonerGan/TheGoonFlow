//! Ingest layer (PLAN §6): one pipeline — identify(url) → fetch meta → store item.
//!
//! Used by the bookmark importer, the in-app "+ Clip URL" action, and (M4)
//! the companion API `/api/clip` route. Everything classify()s to a SiteRef;
//! fetch_meta() pulls title/cover/tags where the site allows it; clip()
//! always stores the URL even when the metadata fetch fails — losing the
//! link is worse than a bare entry.

use crate::jobs::JobHandle;
use crate::{new_id, now_epoch, AppState};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, ItemKind, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::time::Duration;

pub const UA: &str = "TheGoonFlow/0.1 (personal library; user-initiated clip/import)";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Mode {
    /// danbooru-style dapi / JSON API with a numeric post id (gelbooru family, e621)
    BooruId,
    Ehentai,
    DlSite,
    F95,
    /// direct link to an image/video file on any host (http/https)
    DirectMedia,
    /// file:/// URL — media already on this machine
    LocalFile,
    /// repack-site product page (kimochi/otomi/ryuu) — DLsite mirrors
    Offsite,
}

#[derive(Clone, Debug)]
pub struct SiteRef {
    pub site: String,
    pub kind: ItemKind,
    pub remote_id: Option<String>,
    pub mode: Mode,
}

/// Repack mirrors are off by default (D8 / PLAN §6).
pub fn is_mirror_site(site: &str) -> bool {
    matches!(site, "kimochi" | "otomi" | "ryuugames" | "nagadoki" | "hentaihaven")
}

pub fn default_rate_ms(site: &str) -> i64 {
    match site {
        "ehentai" | "exhentai" => 10_500, // PLAN §6: hard ~10 s limiter
        s if is_mirror_site(s) => 1_200,
        _ => 700,
    }
}

/// Everything classify() knows about, for the Sources settings panel:
/// (site, what it yields). Mirrors default to off (D8).
pub const KNOWN_SITES: &[(&str, &'static str)] = &[
    ("gelbooru", "posts"),
    ("rule34", "posts"),
    ("e621", "posts"),
    ("danbooru", "posts"),
    ("safebooru", "posts"),
    ("yande", "posts"),
    ("konachan", "posts"),
    ("rule34hentai", "posts"),
    ("realbooru", "posts"),
    ("hypnohub", "posts"),
    ("ehentai", "galleries"),
    ("exhentai", "galleries"),
    ("dlsite", "games"),
    ("f95", "games"),
    ("kimochi", "games (repack mirror)"),
    ("otomi", "games (repack mirror)"),
    ("ryuugames", "games (repack mirror)"),
    ("nagadoki", "games (repack mirror)"),
    ("hentaihaven", "games (repack mirror)"),
    ("4chan", "posts"),
    ("local", "files on this machine"),
];

// ---------- host → site ----------

/// Host-suffix → canonical site name, so CDN subdomains and mirrors fold into
/// the site the user knows (and can toggle under Sources). Order is irrelevant;
/// longest suffix wins because matching is suffix-boundary aware.
const HOST_SUFFIXES: &[(&str, &str)] = &[
    ("gelbooru.com", "gelbooru"),
    ("rule34.xxx", "rule34"),
    ("e621.net", "e621"),
    ("e926.net", "e621"),
    ("e-hentai.org", "ehentai"),
    ("exhentai.org", "exhentai"),
    ("dlsite.com", "dlsite"),
    ("f95zone.to", "f95"),
    ("kimochi.net", "kimochi"),
    ("otomi.xyz", "otomi"),
    ("ryuugames.com", "ryuugames"),
    ("nagadoki.net", "nagadoki"),
    ("hentaihaven.xxx", "hentaihaven"),
    ("danbooru.donmai.us", "danbooru"),
    ("safebooru.donmai.us", "safebooru"),
    ("yande.re", "yande"),
    ("konachan.com", "konachan"),
    ("rule34hentai.net", "rule34hentai"),
    ("realbooru.com", "realbooru"),
    ("hypnohub.net", "hypnohub"),
    ("booru.allthefallen.moe", "allthefallen"),
    // 4cdn.org is 4chan's image host — "4cdn" used to leak out as site "i"
    // because the first host label was taken as the name.
    ("4cdn.org", "4chan"),
    ("4chan.org", "4chan"),
    ("4channel.org", "4chan"),
];

fn site_for_host(host: &str) -> Option<&'static str> {
    HOST_SUFFIXES
        .iter()
        .find(|(h, _)| host == *h || host.ends_with(&format!(".{h}")))
        .map(|(_, s)| *s)
}

/// Rough registrable domain: last two labels (enough for the hosts we meet).
fn registrable_domain(host: &str) -> String {
    let parts: Vec<&str> = host.split('.').collect();
    if parts.len() >= 2 {
        parts[parts.len() - 2..].join(".")
    } else {
        host.to_string()
    }
}

// ---------- identify ----------

const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "gif", "bmp", "avif"];
const VIDEO_EXTS: &[&str] = &["mp4", "webm", "mkv", "mov", "avi"];

pub fn media_ext(path: &str) -> Option<&'static str> {
    let last = path.rsplit('/').next()?;
    let stem = last.split('?').next()?;
    let ext = stem.rsplit('.').next()?.to_lowercase();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        Some("image")
    } else if VIDEO_EXTS.contains(&ext.as_str()) {
        Some("video")
    } else {
        None
    }
}

fn host_of(url: &str) -> (String, String) {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    let host = rest.split('/').next().unwrap_or("").to_lowercase();
    let path = rest[host.len()..].to_string();
    (host, path)
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

/// If the whole query string is digits (booru file-cache pattern
/// `…/file.gif?267940`), that number is the post id.
fn numeric_query_id(url: &str) -> Option<String> {
    let q = url.split_once('?')?.1.split('#').next()?.trim();
    (!q.is_empty() && q.chars().all(|c| c.is_ascii_digit())).then(|| q.to_string())
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

pub fn classify(url: &str) -> Option<SiteRef> {
    let url = url.trim();

    // file:// — media already on disk. Bookmarks to local .mp4/.png resolve
    // as media items (site "local"), not "unrecognized".
    if let Some(rest) = url.strip_prefix("file://") {
        let path = rest.split('?').next().unwrap_or(rest);
        if media_ext(path).is_none() {
            return None;
        }
        return Some(SiteRef {
            site: "local".into(),
            kind: ItemKind::Post,
            remote_id: None,
            mode: Mode::LocalFile,
        });
    }

    let (host, path) = host_of(url);
    if host.is_empty() {
        return None;
    }
    let host_no_port = host.split(':').next().unwrap_or("").to_string();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();

    // Known API-backed sites first.
    if host_no_port.ends_with("gelbooru.com") {
        return Some(SiteRef {
            site: "gelbooru".into(),
            kind: ItemKind::Post,
            // file-cache links (`img3.gelbooru.com//images/….jpg?4765984`)
            // carry the post id as the whole query string
            remote_id: query_param(url, "id")
                .or_else(|| query_param(url, "pid"))
                .or_else(|| numeric_query_id(url)),
            mode: Mode::BooruId,
        });
    }
    if host_no_port.ends_with("rule34.xxx") {
        return Some(SiteRef {
            site: "rule34".into(),
            kind: ItemKind::Post,
            remote_id: query_param(url, "id").or_else(|| numeric_query_id(url)),
            mode: Mode::BooruId,
        });
    }
    if host_no_port.ends_with("e621.net") || host_no_port.ends_with("e926.net") {
        let id = segments
            .iter()
            .position(|s| *s == "posts" || *s == "post")
            .and_then(|i| segments.get(i + 1))
            .map(|s| s.trim_end_matches(".json").to_string())
            .filter(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit()));
        // e621's own file CDN (`static1x1.e621.net/data/….jpg?md5`) is not a
        // post page — let it fall through to the direct-file branch instead of
        // dead-ending in BooruId with no id.
        if id.is_some() || media_ext(&path).is_none() {
            return Some(SiteRef {
                site: "e621".into(),
                kind: ItemKind::Post,
                remote_id: id,
                mode: Mode::BooruId,
            });
        }
    }
    if host_no_port.ends_with("e-hentai.org") {
        return Some(SiteRef {
            site: "ehentai".into(),
            kind: ItemKind::Gallery,
            remote_id: eh_gid(&segments),
            mode: Mode::Ehentai,
        });
    }
    if host_no_port.ends_with("exhentai.org") {
        return Some(SiteRef {
            site: "exhentai".into(),
            kind: ItemKind::Gallery,
            remote_id: eh_gid(&segments),
            mode: Mode::Ehentai,
        });
    }
    if host_no_port.ends_with("dlsite.com") {
        return Some(SiteRef {
            site: "dlsite".into(),
            kind: ItemKind::Game,
            remote_id: dlsite_product_id(url),
            mode: Mode::DlSite,
        });
    }
    if host_no_port.ends_with("f95zone.to") {
        return Some(SiteRef {
            site: "f95".into(),
            kind: ItemKind::Game,
            remote_id: segments.last().map(|s| (*s).to_string()),
            mode: Mode::F95,
        });
    }
    // Repack mirrors (off by default).
    for known in [
        "kimochi.net",
        "otomi.xyz",
        "ryuugames.com",
        "nagadoki.net",
        "hentaihaven.xxx",
    ] {
        if host_no_port.ends_with(known) {
            let s = known.split('.').next().unwrap_or(known).to_string();
            return Some(SiteRef {
                site: s,
                kind: ItemKind::Game,
                remote_id: None,
                mode: Mode::Offsite,
            });
        }
    }
    // Known booru-family hosts: post pages are title-only until site-packs;
    // their direct file links fall through to DirectMedia below.
    if media_ext(&path).is_none() {
        if let Some(site) = site_for_host(&host_no_port) {
            return Some(SiteRef {
                site: site.to_string(),
                kind: ItemKind::Post,
                remote_id: query_param(url, "id"),
                mode: Mode::BooruId,
            });
        }
    }

    // Direct media file on any host — catches hypnohub `//images/….mp4?NN`
    // style file links, CDNs, anything ending in a media extension.
    if path.starts_with('/') && media_ext(&path).is_some() {
        let site = site_for_host(&host_no_port)
            .map(str::to_string)
            .unwrap_or_else(|| registrable_domain(&host_no_port));
        // Booru file-cache links (`wimg.rule34.xxx//images/…jpeg?11353035`)
        // carry the post id as a numeric query. Going through the site's API
        // gets tags + file with the *right* referer — pulling bytes straight
        // from the CDN with the CDN as its own referer is what hotlink
        // protection kills.
        if site_for_host(&host_no_port).is_some() && media_ext(&path) == Some("image") {
            if let Some(id) = numeric_query_id(url) {
                let mode = match site.as_str() {
                    "gelbooru" | "rule34" | "e621" => Mode::BooruId,
                    _ => Mode::DirectMedia,
                };
                if mode == Mode::BooruId {
                    return Some(SiteRef {
                        site,
                        kind: ItemKind::Post,
                        remote_id: Some(id),
                        mode,
                    });
                }
            }
        }
        return Some(SiteRef {
            site,
            kind: ItemKind::Post,
            remote_id: numeric_query_id(url),
            mode: Mode::DirectMedia,
        });
    }
    None
}

// ---------- fetch ----------

#[derive(Debug, Default)]
pub struct Meta {
    pub title: Option<String>,
    /// original bytes to store inside the vault (images only — videos stay as links)
    pub store_bytes: Option<Vec<u8>>,
    /// bytes used to generate the thumbnail (may differ from the stored original)
    pub thumb_bytes: Option<Vec<u8>>,
    pub tags: Vec<String>,
    pub version: Option<String>,
    pub note: Option<String>,
}

#[derive(Default)]
pub struct FetchCtx<'a> {
    pub cookie: Option<&'a str>,
}

async fn get_text(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
    cookie: Option<&str>,
) -> Result<String, String> {
    let mut req = client.get(url).header(reqwest::header::REFERER, referer);
    if let Some(c) = cookie {
        req = req.header(reqwest::header::COOKIE, c);
    }
    let resp = req.send().await.map_err(|e| format!("fetch failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.text().await.map_err(|e| format!("read failed: {e}"))
}

async fn get_bytes(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
    cookie: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut req = client.get(url).header(reqwest::header::REFERER, referer);
    if let Some(c) = cookie {
        req = req.header(reqwest::header::COOKIE, c);
    }
    let resp = req.send().await.map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.bytes().await.map(|b| b.to_vec()).map_err(|e| format!("download interrupted: {e}"))
}

pub async fn fetch_meta(
    client: &reqwest::Client,
    s: &SiteRef,
    url: &str,
    ctx: &FetchCtx<'_>,
) -> Result<Meta, String> {
    match s.mode {
        Mode::BooruId => match s.site.as_str() {
            "gelbooru" | "rule34" => booru_dapi(client, &s.site, s.remote_id.as_deref()).await,
            "e621" => e621_post(client, s.remote_id.as_deref()).await,
            other => Ok(Meta {
                note: Some(format!("no metadata API wired for {other} yet — stored as bare link")),
                ..Default::default()
            }),
        },
        Mode::Ehentai => eh_gallery(client, url, ctx.cookie).await,
        Mode::DlSite => dl_product(client, url).await,
        Mode::Offsite => offsite_product(client, url).await,
        Mode::F95 => f95_thread(client, url, ctx.cookie).await,
        Mode::DirectMedia => direct_media(client, url).await,
        Mode::LocalFile => local_file(url),
    }
}

// ---- booru family (danbooru dapi: gelbooru/rule34) ----

async fn booru_dapi(client: &reqwest::Client, site: &str, id: Option<&str>) -> Result<Meta, String> {
    let id = id.ok_or_else(|| "no numeric post id in that url".to_string())?;
    let (api, referer) = match site {
        "gelbooru" => (
            format!("https://gelbooru.com/index.php?page=dapi&s=post&q=index&json=1&pid={id}"),
            "https://gelbooru.com",
        ),
        "rule34" => (
            format!("https://rule34.xxx/public/index.php?page=dapi&s=post&q=index&json=1&id={id}"),
            "https://rule34.xxx",
        ),
        _ => return Err("unsupported booru".into()),
    };
    let text = get_text(client, &api, referer, None).await?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "api reply was not json".to_string())?;
    let node = v.get("post").or_else(|| v.get("posts"));
    let post = match node {
        Some(n) if n.is_array() => n.as_array().and_then(|a| a.first()),
        other => other,
    }
    .ok_or_else(|| "post not found (deleted?)".to_string())?;
    if post.is_null() {
        return Err("post not found (deleted?)".into());
    }
    post_from_json(post, referer, client).await
}

async fn post_from_json(post: &Value, referer: &str, client: &reqwest::Client) -> Result<Meta, String> {
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
    let is_video = media_ext(file_url) == Some("video");
    let bytes = get_bytes(client, file_url, referer, None).await?;
    Ok(Meta {
        title: None,
        store_bytes: (!is_video).then_some(bytes.clone()),
        thumb_bytes: (!is_video).then_some(bytes),
        tags,
        note: is_video.then(|| "video post — kept as link, not stored in vault".into()),
        ..Default::default()
    })
}

async fn e621_post(client: &reqwest::Client, id: Option<&str>) -> Result<Meta, String> {
    let id = id.ok_or_else(|| "no numeric post id in that url".to_string())?;
    let text = get_text(client, &format!("https://e621.net/posts/{id}.json"), "https://e621.net", None).await?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "api reply was not json".to_string())?;
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
    let is_video = media_ext(file_url) == Some("video");
    let bytes = get_bytes(client, file_url, "https://e621.net", None).await?;
    Ok(Meta {
        title: None,
        store_bytes: (!is_video).then_some(bytes.clone()),
        thumb_bytes: (!is_video).then_some(bytes),
        tags,
        note: is_video.then(|| "video post — kept as link, not stored in vault".into()),
        ..Default::default()
    })
}

// ---- e-hentai ----

/// True when the page is e-hentai's IP-login gate rather than the gallery.
pub fn eh_is_gate(html: &str) -> bool {
    html.contains("IP Login")
        || html.contains("ip login")
        || html.contains("gtable ggate")
        || html.contains("10.114")
        || html.contains("10.124")
        || (html.contains("<h1>Access") && html.contains("restricted"))
}

/// Parse a gallery page: title from `<title>`, cover from the first gdtm img.
pub fn parse_eh_gallery(html: &str) -> (Option<String>, Option<String>) {
    let title = html
        .find("<title>")
        .map(|i| &html[i + 7..])
        .and_then(|s| s.split("</title>").next())
        .map(|t| {
            let t = t.split('|').next().unwrap_or(t).trim();
            // strip trailing language bracket: "Name [English]"
            match t.rsplit_once('[') {
                Some((a, b)) if b.ends_with(']') => a.trim_end().to_string(),
                _ => t.to_string(),
            }
        })
        .filter(|t| !t.is_empty());
    let cover = html
        .find("gdtm")
        .and_then(|i| html[i..].find("<img src=\"").map(|j| &html[i + j..]))
        .and_then(|tag| tag.find("src=\"").map(|k| &tag[k + 5..]))
        .and_then(|s| s.split('"').next())
        .map(|u| u.to_string());
    (title, cover)
}

async fn eh_gallery(client: &reqwest::Client, url: &str, cookie: Option<&str>) -> Result<Meta, String> {
    let html = get_text(client, url, "https://e-hentai.org", cookie).await?;
    if eh_is_gate(&html) {
        return Ok(Meta {
            note: Some(
                "e-hentai gate page hit — open the gallery once in your browser \
                 (or set its cookie under Sources), then re-clip to enrich"
                    .into(),
            ),
            ..Default::default()
        });
    }
    let (title, cover) = parse_eh_gallery(&html);
    let mut m = Meta { title, ..Default::default() };
    if let Some(cover) = cover {
        if let Ok(bytes) = get_bytes(client, &cover, "https://e-hentai.org", cookie).await {
            m.thumb_bytes = Some(bytes);
        }
    }
    Ok(m)
}

// ---- DLsite + repack mirrors ----

/// Parse a DLsite-style product page (mirrors use the same shape).
pub fn parse_dl_product(html: &str) -> (Option<String>, Option<String>, Option<String>) {
    let title = og_content(html, "og:title").map(|t| clean_title_from(&t));
    let cover = og_content(html, "og:image");
    let updated = ["itemprop=\"dateUpdated\"", "dateUpdated", "最終更新日", "Updated:"]
        .iter()
        .find_map(|marker| {
            let i = html.find(marker)?;
            let after = &html[i..(i + 160).min(html.len())];
            let digits: String = after
                .chars()
                .skip_while(|c| *c != '2')
                .take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '/')
                .collect();
            (digits.len() >= 8).then(|| digits.replace('/', "-")[..10].to_string())
        });
    (title, cover, updated)
}

fn clean_title_from(raw: &str) -> String {
    let t = raw.trim();
    // DLsite: "[RJ123456] Game Title [PC版]" → "Game Title [PC版]"
    let t = t
        .strip_prefix('[')
        .and_then(|s| s.find(']').map(|i| s[i + 1..].trim_start().to_string()))
        .unwrap_or_else(|| t.to_string());
    t.trim().to_string()
}

fn og_content(html: &str, prop: &str) -> Option<String> {
    let idx = html.find(prop)?;
    let after = &html[idx..];
    let cstart = after.find("content=\"")? + "content=\"".len();
    let v = after[cstart..].split('"').next()?.trim();
    (!v.is_empty()).then(|| v.to_string())
}

async fn dl_product(client: &reqwest::Client, url: &str) -> Result<Meta, String> {
    let html = get_text(client, url, "https://www.dlsite.com", None).await?;
    let (title, cover, updated) = parse_dl_product(&html);
    let mut m = Meta { title, version: updated, ..Default::default() };
    if let Some(cover) = cover {
        if let Ok(bytes) = get_bytes(client, &cover, "https://www.dlsite.com", None).await {
            m.thumb_bytes = Some(bytes);
        }
    }
    Ok(m)
}

async fn offsite_product(client: &reqwest::Client, url: &str) -> Result<Meta, String> {
    let referer = format!("https://{}", host_of(url).0);
    let html = get_text(client, url, &referer, None).await?;
    let (title, cover, _) = parse_dl_product(&html);
    let mut m = Meta {
        title,
        note: Some("repack mirror — tracked as a DLsite mirror source".into()),
        ..Default::default()
    };
    if let Some(cover) = cover {
        if let Ok(bytes) = get_bytes(client, &cover, &referer, None).await {
            m.thumb_bytes = Some(bytes);
        }
    }
    Ok(m)
}

// ---- f95zone ----

/// XenForo thread: og:title carries `Game Name [v0.12] [renpy] [WiP]`.
/// Returns the raw title text (brackets intact — [`crate::importer::clean_name`]
/// turns that into title + version) plus engine/status tags.
pub fn parse_f95_thread(html: &str) -> (Option<String>, Vec<String>) {
    let raw = match og_content(html, "og:title") {
        Some(r) => r,
        None => return (None, Vec::new()),
    };
    // Threads pages suffix the title: "… | f95zone"
    let mut raw = raw.as_str();
    for sep in [" | f95", " - f95", " – f95"] {
        if let Some((a, _)) = raw.rsplit_once(sep) {
            raw = a;
            break;
        }
    }
    let mut tags = Vec::new();
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        if c == '[' || c == '(' {
            let close = if c == '[' { ']' } else { ')' };
            let mut tok = String::new();
            i += 1;
            while i < chars.len() && chars[i] != close {
                tok.push(chars[i]);
                i += 1;
            }
            i += 1;
            let lower = tok.trim().to_lowercase();
            match lower.as_str() {
                "renpy" | "rpg maker" | "rpg-maker" | "rpgm" | "unity" | "gamemaker" | "twine" | "srpg maker" => {
                    tags.push(lower.replace(' ', "-"))
                }
                "completed" => tags.push("f95-completed".into()),
                _ => {} // version + status tokens handled by clean_name
            }
        } else {
            i += 1;
        }
    }
    (Some(raw.trim().to_string()), tags)
}

async fn f95_thread(client: &reqwest::Client, url: &str, cookie: Option<&str>) -> Result<Meta, String> {
    let html = get_text(client, url, "https://f95zone.to", cookie).await?;
    if html.contains("login required") || html.contains("You do not have permission") {
        return Ok(Meta {
            note: Some("f95 wants a login for this thread — set its cookie under Sources, then re-clip".into()),
            ..Default::default()
        });
    }
    let (raw_title, tags) = parse_f95_thread(&html);
    let (title, version) = match raw_title {
        Some(t) => {
            let (t2, v) = crate::importer::clean_name(&t);
            let title = if t2.is_empty() { t } else { t2 };
            (Some(title), v)
        }
        None => (None, None),
    };
    Ok(Meta { title, version, tags, ..Default::default() })
}

// ---- direct media & local files ----

async fn direct_media(client: &reqwest::Client, url: &str) -> Result<Meta, String> {
    let kind = media_ext(&host_of(url).1).ok_or("not a recognizable media file")?;
    if kind == "video" {
        return Ok(Meta {
            note: Some("video — kept as link (videos stay outside the encrypted DB)".into()),
            ..Default::default()
        });
    }
    // Hotlink protection checks the *site* as referer, not the CDN host, and
    // some bookmarks carry malformed `//images/…` paths — normalize both.
    let (host, path) = host_of(url);
    let clean_path = path.replace("//", "/");
    let clean = format!(
        "{}://{}{}",
        "https",
        host,
        clean_path
    );
    let referer = format!("https://{}/", registrable_domain(&host.split(':').next().unwrap_or(&host)));
    let bytes = match get_bytes(client, &clean, &referer, None).await {
        Ok(b) => b,
        Err(first) => {
            // second chance: the original URL (some CDNs are picky about paths)
            get_bytes(client, url, &referer, None)
                .await
                .map_err(|_| first)?
        }
    };
    Ok(Meta {
        store_bytes: Some(bytes.clone()),
        thumb_bytes: Some(bytes),
        ..Default::default()
    })
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `file:///C:/…` (Windows drive) and `file://host/share/…` (UNC) both have to
/// be turned back into a real path — browsers keep a slash that Windows would
/// choke on, and percent-escapes are already decoded by the caller.
fn local_path(decoded: &str) -> Result<std::path::PathBuf, String> {
    let s = decoded.replace('\\', "/");
    if let Some(rest) = s.strip_prefix("localhost/") {
        return ok_path(rest);
    }
    let candidate = if s.starts_with("//") {
        // UNC share: \\server\share\file.ext
        // keep the leading double backslash — that is what marks a UNC path
        format!(r"\\{}", s[2..].replace('/', "\\"))
    } else if s.starts_with('/') && is_drive_path(&s[1..]) {
        s[1..].to_string()
    } else {
        s.clone()
    };
    ok_path(&candidate)
}

fn ok_path(s: &str) -> Result<std::path::PathBuf, String> {
    if s.trim().is_empty() || s == "/" || s == "\\" {
        return Err("empty file path".into());
    }
    Ok(std::path::PathBuf::from(s))
}

/// "C:/dir/file.mp4" style (single letter + colon).
fn is_drive_path(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

/// file:/// URL → read from disk. Images get stored+thumbnailed; videos stay
/// as path links (opening uses the OS handler).
pub fn local_file(url: &str) -> Result<Meta, String> {
    let rest = url.strip_prefix("file://").ok_or("not a file url")?;
    let decoded = percent_decode(rest.split('?').next().unwrap_or(rest));
    let path = local_path(&decoded)?;
    if !path.is_file() {
        return Err("local file is not reachable right now".into());
    }
    let kind = media_ext(&path.to_string_lossy()).ok_or("not a recognizable media file")?;
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".into());
    if kind == "video" {
        return Ok(Meta {
            title: Some(stem),
            note: Some("video on disk — open from the item's source link".into()),
            ..Default::default()
        });
    }
    let bytes = std::fs::read(&path).map_err(|e| format!("cannot read local file: {e}"))?;
    Ok(Meta {
        title: Some(stem),
        store_bytes: Some(bytes.clone()),
        thumb_bytes: Some(bytes),
        ..Default::default()
    })
}

// ---------- the clip pipeline ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipReport {
    pub item_id: String,
    pub title: String,
    pub site: String,
    pub created: bool,
    pub stored_image: bool,
    pub thumb: bool,
    pub tags: usize,
    pub note: Option<String>,
}

pub async fn clip(state: &AppState, url: String) -> Result<ClipReport, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("no url".into());
    }
    let s = classify(&url).ok_or("can't identify that link (site support grows over time)")?;

    // Phase 1 (sync): settings, dedup, cookie for this site.
    let (cookie, existing) = {
        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().ok_or("vault is locked")?.conn;
        if !db::site_enabled(conn, &s.site, !is_mirror_site(&s.site)) {
            return Err(format!("{} is disabled — enable it under Sources", s.site));
        }
        (db::site_cookie(conn, &s.site), db::source_item_id(conn, &s.site, &url))
    };
    if let Some(existing) = existing {
        return Ok(ClipReport {
            item_id: existing,
            title: String::new(),
            site: s.site,
            created: false,
            stored_image: false,
            thumb: false,
            tags: 0,
            note: Some("already in the flow".into()),
        });
    }

    // Phase 2 (async): metadata where available. Never fatal — a failed
    // fetch still stores the link (bare entry beats lost bookmark).
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(25))
        .build()
        .map_err(|e| e.to_string())?;
    let ctx = FetchCtx { cookie: cookie.as_deref() };
    let mut meta = match fetch_meta(&client, &s, &url, &ctx).await {
        Ok(m) => m,
        Err(e) => Meta {
            note: Some(format!("link stored, metadata fetch failed: {e}")),
            ..Default::default()
        },
    };
    meta.note = meta
        .note
        .clone()
        .or_else(|| Some("no metadata available — stored as a bare link".into()));

    // Phase 3 (sync): store.
    let now = now_epoch();
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    let fallback = s
        .remote_id
        .clone()
        .map(|id| format!("{} {}", s.site, id))
        .unwrap_or_else(|| s.site.clone());
    let title = meta.title.filter(|t| !t.trim().is_empty()).unwrap_or(fallback);
    let id = new_id("c");
    let item = Item {
        id: id.clone(),
        kind: s.kind,
        title: title.clone(),
        original_title: None,
        circle: None,
        status: Status::Seen,
        dnf: false,
        rating: None,
        notes: String::new(),
        version: meta.version.clone(),
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    db::insert_item(conn, &item).map_err(|e| e.to_string())?;
    db::insert_source(conn, &id, &s.site, &url, s.remote_id.as_deref(), Some(&now))
        .map_err(|e| e.to_string())?;

    let mut stored_image = false;
    if let Some(bytes) = &meta.store_bytes {
        stored_image = media::put_full_image(conn, &id, bytes);
    }
    let mut thumb = false;
    if let Some(bytes) = &meta.thumb_bytes {
        thumb = media::put_thumbnail(conn, &id, bytes);
    }
    let mut tags = 0usize;
    for t in meta.tags.iter().take(40) {
        if db::add_item_tag(conn, &id, t, db::TYPE_CONTENT).is_ok() {
            tags += 1;
        }
    }
    Ok(ClipReport {
        item_id: id,
        title,
        site: s.site,
        created: true,
        stored_image,
        thumb,
        tags,
        note: meta.note,
    })
}

// ---------- update checking (M2) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateReport {
    pub checked: usize,
    pub updated: usize,
    pub failures: usize,
    pub stopped: bool,
    pub updated_titles: Vec<String>,
}

pub async fn check_updates(state: &AppState, job: &JobHandle) -> Result<UpdateReport, String> {
    let targets = {
        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().ok_or("vault is locked")?.conn;
        db::update_targets(conn)
    }
    .map_err(|e| e.to_string())?;

    let mut report = UpdateReport {
        checked: 0,
        updated: 0,
        failures: 0,
        stopped: false,
        updated_titles: Vec::new(),
    };
    if targets.is_empty() {
        return Ok(report);
    }
    let site_ctx: BTreeMap<String, (Option<String>, i64)> = {
        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().ok_or("vault is locked")?.conn;
        let mut m = BTreeMap::new();
        for t in &targets {
            m.entry(t.site.clone())
                .or_insert_with(|| (db::site_cookie(conn, &t.site), db::site_rate_ms(conn, &t.site, default_rate_ms(&t.site))));
        }
        m
    };

    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let targets_len = targets.len();
    for (i, t) in targets.into_iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(i, targets_len, &format!("{} ({})", t.title, t.site));
        let (cookie, rate) = site_ctx.get(&t.site).cloned().unwrap_or((None, default_rate_ms(&t.site)));
        // shared per-site lanes (#15), same politeness ledger as enrich
        if !state.queue.reserve(&t.site, rate, || job.cancelled()).await {
            report.stopped = true;
            break;
        }
        report.checked += 1;

        let mode = match t.site.as_str() {
            "f95" => Mode::F95,
            _ => Mode::DlSite,
        };
        let s = SiteRef { site: t.site.clone(), kind: ItemKind::Game, remote_id: None, mode };
        let ctx = FetchCtx { cookie: cookie.as_deref() };
        match fetch_meta(&client, &s, &t.url, &ctx).await {
            Ok(m) => {
                if let Some(nv) = m.version.filter(|v| !v.is_empty()) {
                    let changed = t.old_version.as_deref().map_or(true, |o| o != nv);
                    if changed {
                        let now = now_epoch();
                        let inner = state.inner.lock().unwrap();
                        if let Some(opened) = inner.opened.as_ref() {
                            let _ = db::set_item_version(&opened.conn, &t.item_id, &nv, &now);
                            let _ = db::record_update(&opened.conn, &t.item_id, &t.site, t.old_version.as_deref(), Some(&nv), &now);
                        }
                        report.updated += 1;
                        report.updated_titles.push(t.title.clone());
                    }
                }
            }
            Err(_) => report.failures += 1,
        }
    }
    Ok(report)
}

// ---------- apply fetched metadata + per-item re-enrichment (M2.6) ----------

/// Apply a fetched [`Meta`] onto an existing item: stored image, thumbnail,
/// tags, and — only where the current title is a placeholder — title/version.
/// Returns `(thumbnail, tags_added, title_updated)`.
pub fn apply_meta(conn: &rusqlite::Connection, item_id: &str, site: &str, meta: &Meta) -> (bool, usize, bool) {
    let mut thumb = false;
    if let Some(bytes) = &meta.store_bytes {
        media::put_full_image(conn, item_id, bytes);
    }
    if let Some(bytes) = &meta.thumb_bytes {
        thumb = media::put_thumbnail(conn, item_id, bytes);
    }
    let mut tags = 0usize;
    for t in meta.tags.iter().take(40) {
        if db::add_item_tag(conn, item_id, t, db::TYPE_CONTENT).is_ok() {
            tags += 1;
        }
    }
    let mut title_updated = false;
    if let Ok(Some(d)) = db::get_item(conn, item_id) {
        let looks_like_url = d.title.contains("://")
            || d.title.starts_with("www.")
            || d.title == site
            || d.title.starts_with(&format!("{site} "));
        let placeholder = d.title.trim().is_empty() || looks_like_url;
        let new_title = meta
            .title
            .as_deref()
            .filter(|t| !t.trim().is_empty() && placeholder);
        if new_title.is_some() || meta.version.is_some() {
            let _ = db::update_item(
                conn,
                item_id,
                new_title.unwrap_or(&d.title),
                d.circle.as_deref(),
                Status::from_str(&d.status).unwrap_or(Status::Seen),
                meta.version.as_deref().or(d.version.as_deref()),
                None,
                &now_epoch(),
            );
            title_updated = true;
        }
    }
    (thumb, tags, title_updated)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichFailure {
    pub url: String,
    pub reason: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EnrichReport {
    pub sources: usize,
    pub enriched: usize,
    pub stopped: bool,
    pub notes: Vec<String>,
    pub failures: Vec<EnrichFailure>,
}

/// "Re-fetch metadata" (#11): re-run every source of an item through the same
/// classify → fetch_meta → apply_meta pipeline that clip/bookmark-enrich use.
/// Local sources are re-read from disk (fixes covers that failed the first
/// time); remote sources honour their site rate and cookie.
pub async fn reenrich_item(state: &AppState, item_id: &str, job: &JobHandle) -> Result<EnrichReport, String> {
    let sources = {
        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().ok_or("vault is locked")?.conn;
        let detail = db::get_item(conn, item_id).map_err(|e| e.to_string())?;
        let Some(d) = detail else {
            return Err("item not found".into());
        };
        d.sources
            .into_iter()
            .map(|s| (s.site, s.url))
            .collect::<Vec<_>>()
    };

    let mut report = EnrichReport {
        sources: sources.len(),
        enriched: 0,
        stopped: false,
        notes: Vec::new(),
        failures: Vec::new(),
    };
    if sources.is_empty() {
        report.notes.push("this item has no sources to fetch from".into());
        return Ok(report);
    }

    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    for (i, (site, url)) in sources.iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(i, sources.len(), url);

        let Some(cls) = classify(url) else {
            report.failures.push(EnrichFailure {
                url: url.clone(),
                reason: "no longer classifiable — remove or fix this source".into(),
            });
            continue;
        };

        // remote fetches share the per-site lanes (#15); local files need none
        if !matches!(cls.mode, Mode::LocalFile) {
            let rate = {
                let inner = state.inner.lock().unwrap();
                match inner.opened.as_ref() {
                    Some(o) => db::site_rate_ms(&o.conn, site, default_rate_ms(site)),
                    None => {
                        report.stopped = true; // vault gone mid-run
                        break;
                    }
                }
            };
            if !state.queue.reserve(site, rate, || job.cancelled()).await {
                report.stopped = true;
                break;
            }
        }

        let cookie = {
            let inner = state.inner.lock().unwrap();
            inner.opened.as_ref().and_then(|o| db::site_cookie(&o.conn, site))
        };
        let ctx = FetchCtx { cookie: cookie.as_deref() };
        let meta = match fetch_meta(&client, &cls, url, &ctx).await {
            Ok(m) => m,
            Err(e) => {
                report.failures.push(EnrichFailure { url: url.clone(), reason: e });
                continue;
            }
        };

        let mut inner = state.inner.lock().unwrap();
        let Some(opened) = inner.opened.as_mut() else {
            report.stopped = true;
            break;
        };
        let (thumb, tags, title_updated) = apply_meta(&opened.conn, item_id, site, &meta);
        if thumb || tags > 0 || title_updated {
            report.enriched += 1;
        }
        if let Some(note) = &meta.note {
            report.notes.push(format!("{site}: {note}"));
        }
        if !thumb && tags == 0 && !title_updated && meta.note.is_none() {
            report.failures.push(EnrichFailure {
                url: url.clone(),
                reason: "the source answered but carried nothing new".into(),
            });
        }
    }
    Ok(report)
}

// ---------- #14: title search (online match suggestions) ----------
//
// Local-only games get *proposed* online matches, never merged automatically:
// we search DLsite's and f95zone's own site search, score the candidates
// against the game's title, and store the top hits as suggestions the user
// accepts (attach source + re-fetch) or dismisses in the Games view.

#[derive(Clone, Debug, PartialEq)]
pub struct SearchHit {
    pub url: String,
    pub title: String,
}

/// Normalize a title for fuzzy compare: bracketed noise (engine/language/
/// version tags) dropped, everything non-alphanumeric folded to spaces,
/// lowercased. "Summer Witch [renpy] [English] [v1.2]" → "summer witch".
pub fn normalize_title(s: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in s.chars() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => {
                if c.is_alphanumeric() {
                    for l in c.to_lowercase() {
                        out.push(l);
                    }
                } else {
                    out.push(' ');
                }
            }
            _ => {}
        }
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn char_bigrams(s: &str) -> std::collections::BTreeMap<(char, char), usize> {
    let cs: Vec<char> = s.chars().collect();
    let mut m = std::collections::BTreeMap::new();
    for w in cs.windows(2) {
        *m.entry((w[0], w[1])).or_default() += 1;
    }
    m
}

/// Character-bigram Dice coefficient over normalized titles, 0.0..=1.0.
/// Short titles are forgiving via a length-ratio guard so "Tomato" vs
/// "Tomato Story" doesn't read as a strong hit.
pub fn title_similarity(a: &str, b: &str) -> f64 {
    let a = normalize_title(a);
    let b = normalize_title(b);
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    let ba = char_bigrams(&a);
    let bb = char_bigrams(&b);
    let na: usize = ba.values().sum();
    let nb: usize = bb.values().sum();
    if na == 0 || nb == 0 {
        return 0.0;
    }
    let inter: usize = ba.iter().map(|(k, v)| bb.get(k).copied().unwrap_or(0).min(*v)).sum();
    let dice = 2.0 * inter as f64 / (na + nb) as f64;
    // penalize very different lengths ("cat" vs "cat who saved the world")
    let (short, long) = (a.chars().count().min(b.chars().count()), a.chars().count().max(b.chars().count()));
    let ratio = short as f64 / long as f64;
    dice * (0.5 + 0.5 * ratio)
}

fn unescape_html(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
}

async fn post_form_text(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
    cookie: Option<&str>,
    form: &[(&str, &str)],
) -> Result<String, String> {
    let mut req = client
        .post(url)
        .header(reqwest::header::REFERER, referer)
        .header(reqwest::header::ACCEPT, "text/html")
        .form(form);
    if let Some(c) = cookie {
        req = req.header(reqwest::header::COOKIE, c);
    }
    let resp = req.send().await.map_err(|e| format!("search failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.text().await.map_err(|e| format!("read failed: {e}"))
}

/// DLsite has no public JSON API — post the site's own /fs search form and
/// pull (link, title) from the result anchors. Verified live markup:
/// `href="https://www.dlsite.com/maniax/work/=/product_id/RJ01731048.html" title="…"`.
pub async fn dlsite_search(client: &reqwest::Client, query: &str) -> Result<Vec<SearchHit>, String> {
    let html = post_form_text(
        client,
        "https://www.dlsite.com/maniax/fs",
        "https://www.dlsite.com",
        None,
        &[
            ("_qf__fulltext_search", ""),
            ("_layout", "fs"),
            ("_site", "maniax"),
            ("_form_id", "FulltextSearchProductForm"),
            ("_view", "input"),
            ("from", "fs.header"),
            ("ana_flg", "all"),
            ("keywords", query),
        ],
    )
    .await?;
    Ok(parse_dl_search(&html))
}

pub fn parse_dl_search(html: &str) -> Vec<SearchHit> {
    let mut out: Vec<SearchHit> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut idx = 0usize;
    while let Some(rel) = html[idx..].find("product_id/") {
        let start = idx + rel + "product_id/".len();
        idx = start;
        let after = &html[start..];
        let id: String = after.chars().take_while(|c| c.is_ascii_alphanumeric()).collect();
        let alpha = id.chars().filter(|c| c.is_ascii_alphabetic()).count();
        let digits = id.chars().filter(|c| c.is_ascii_digit()).count();
        // real product ids are two uppercase letters + 6..8 digits; this also
        // rejects maker ids (RG…) because those appear under maker_id/, not here
        if alpha != 2 || !(6..=8).contains(&digits) {
            continue;
        }
        // title lives inside the same tag: title="…" (anchor) or alt="…" (thumb tile)
        let tag = &after[..after.find('>').unwrap_or(after.len().min(2000))];
        let title = ["title=\"", "alt=\""]
            .iter()
            .find_map(|m| tag.find(m).map(|i| &tag[i + m.len()..]))
            .map(|t| t.split('"').next().unwrap_or("").trim().to_string())
            .unwrap_or_default();
        let url = format!("https://www.dlsite.com/maniax/work/=/product_id/{id}.html");
        if title.is_empty() || !seen.insert(id.clone()) {
            continue;
        }
        out.push(SearchHit { url, title: unescape_html(&title) });
    }
    out
}

/// f95zone search lives behind XenForo login. With the user's saved f95
/// cookie (Sources panel) it works; without one the reply is the login page.
pub async fn f95_search(
    client: &reqwest::Client,
    query: &str,
    cookie: Option<&str>,
) -> Result<Vec<SearchHit>, String> {
    let mut req = client
        .get("https://f95zone.to/search/search")
        .header(reqwest::header::REFERER, "https://f95zone.to")
        .query(&[("q", query), ("c", "message")]);
    if let Some(c) = cookie {
        req = req.header(reqwest::header::COOKIE, c);
    }
    let resp = req.send().await.map_err(|e| format!("search failed: {e}"))?;
    if matches!(resp.status(), reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::UNAUTHORIZED) {
        return Err("f95 search wants a login — save your f95 cookie under Sources, then search again".into());
    }
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    let hits = parse_f95_search(&html);
    if hits.is_empty() && cookie.is_none() {
        return Err("f95 found nothing without login — set an f95 cookie under Sources to search threads".into());
    }
    Ok(hits)
}

pub fn parse_f95_search(html: &str) -> Vec<SearchHit> {
    let mut out: Vec<SearchHit> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    const P: &str = "href=\"https://f95zone.to/threads/";
    let mut idx = 0usize;
    while let Some(rel) = html[idx..].find(P) {
        let start = idx + rel;
        let after = &html[start + P.len()..];
        let raw_href: String = after.chars().take_while(|c| *c != '"').collect();
        idx = start + P.len();
        // thread form: slug.12345/, also seen as slug.12345/page-2#post-… or ?page=2
        let path = raw_href.split(['#', '?']).next().unwrap_or("").to_string();
        let Some(dot) = path.rfind('.') else { continue };
        let after_dot = &path[dot + 1..];
        let digits: String = after_dot.chars().take_while(|c| c.is_ascii_digit()).collect();
        let rest = &after_dot[digits.len()..];
        if digits.is_empty() || !(rest.is_empty() || rest.starts_with('/')) {
            continue;
        }
        // link text: from the first '>' after the href up to the next '<'
        let gt = after.find('>').unwrap_or(usize::MAX);
        if gt == usize::MAX || gt > 4000 {
            continue;
        }
        let text: String = after[gt + 1..].chars().take_while(|c| *c != '<').collect();
        let text = unescape_html(text.trim());
        if text.is_empty() || text.len() > 300 || !seen.insert(digits.clone()) {
            continue;
        }
        let url = format!("https://f95zone.to/threads/{}/", &path[..dot + 1 + digits.len()]);
        out.push(SearchHit { url, title: text });
    }
    out
}

// ---------- #14: title search (online match suggestions) ----------
//
// Local-only games get *proposed* online matches, never merged automatically:
// we search DLsite's and f95zone's own site search, score the candidates
// against the game's title, and store the top hits as suggestions the user
// accepts (attach source + re-fetch) or dismisses in the Games view.

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct MatchReport {
    pub scanned: usize,
    pub with_suggestions: usize,
    pub suggestions: usize,
    pub failures: Vec<String>,
    pub stopped: bool,
}

/// Keep the top few hits above the confidence floor and store them.
/// Returns how many rows were written.
fn store_hits(
    state: &crate::AppState,
    item_id: &str,
    site: &str,
    title: &str,
    hits: &[SearchHit],
) -> usize {
    let mut scored: Vec<(f64, &SearchHit)> =
        hits.iter().map(|h| (title_similarity(title, &h.title), h)).collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let keep: Vec<&(f64, &SearchHit)> =
        scored.iter().take(3).take_while(|(s, _)| *s >= 0.6).collect();
    if keep.is_empty() {
        return 0;
    }
    let Ok(inner) = state.inner.try_lock() else { return 0 };
    let Some(opened) = inner.opened.as_ref() else { return 0 };
    let now = crate::now_epoch();
    let n = keep.len();
    for (score, h) in keep {
        let _ = db::put_suggestion(
            &opened.conn,
            item_id,
            site,
            &h.url,
            &h.title,
            (score * 1000.0).round() / 1000.0,
            &now,
        );
    }
    n
}

/// Scan every game that is tracked only by a local source and store up to
/// three confident DLsite/f95 matches per game as suggestions.
pub async fn find_matches(
    state: &crate::AppState,
    job: &JobHandle,
) -> Result<MatchReport, String> {
    let targets = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        db::games_needing_match(&opened.conn).map_err(crate::err)?
    };
    let mut report = MatchReport::default();
    if targets.is_empty() {
        return Ok(report);
    }
    let client = reqwest::Client::builder()
        .user_agent(UA)
        .timeout(Duration::from_secs(20))
        .build()
        .map_err(crate::err)?;
    let total = targets.len();
    for (i, (id, title)) in targets.into_iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(i, total, &title);
        let query = normalize_title(&title);
        if query.replace(' ', "").chars().count() < 3 {
            continue; // nothing meaningful to search for
        }

        let (dl_on, dl_rate, f95_on, f95_rate, f95_cookie) = {
            let inner = state.inner.lock().unwrap();
            match inner.opened.as_ref() {
                Some(o) => (
                    db::site_enabled(&o.conn, "dlsite", true),
                    db::site_rate_ms(&o.conn, "dlsite", default_rate_ms("dlsite")),
                    db::site_enabled(&o.conn, "f95", true),
                    db::site_rate_ms(&o.conn, "f95", default_rate_ms("f95")),
                    db::site_cookie(&o.conn, "f95"),
                ),
                None => {
                    report.stopped = true; // vault gone mid-run
                    break;
                }
            }
        };

        // a rescan replaces this item's old proposals
        if let Ok(inner) = state.inner.try_lock() {
            if let Some(o) = inner.opened.as_ref() {
                let _ = db::clear_suggestions(&o.conn, &id);
            }
        }

        let mut wrote = 0usize;
        if dl_on {
            if !state.queue.reserve("dlsite", dl_rate, || job.cancelled()).await {
                report.stopped = true;
                break;
            }
            match dlsite_search(&client, &query).await {
                Ok(hits) => wrote += store_hits(state, &id, "dlsite", &title, &hits),
                Err(e) => report.failures.push(format!("dlsite: {e}")),
            }
        }
        if f95_on {
            if !state.queue.reserve("f95", f95_rate, || job.cancelled()).await {
                report.stopped = true;
                break;
            }
            match f95_search(&client, &query, f95_cookie.as_deref()).await {
                Ok(hits) => wrote += store_hits(state, &id, "f95", &title, &hits),
                Err(e) => report.failures.push(format!("f95: {e}")),
            }
        }
        report.scanned += 1;
        if wrote > 0 {
            report.with_suggestions += 1;
            report.suggestions += wrote;
        }
    }
    report.failures.sort();
    report.failures.dedup();
    Ok(report)
}

// ---------- tests: parsing only (no network) ----------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_urls_resolve_as_media() {
        let s = classify("file:///C:/Users/u/Videos/clip.mp4").unwrap();
        assert_eq!(s.mode, Mode::LocalFile);
        assert_eq!(s.site, "local");
        assert_eq!(s.kind, ItemKind::Post);
        let s = classify("file:///C:/Users/u/Media/art.png").unwrap();
        assert_eq!(s.mode, Mode::LocalFile);
        // non-media file urls stay unrecognized
        assert!(classify("file:///C:/Users/u/notes.txt").is_none());
    }

    /// Field report #1: `i.4cdn.org` used to become site "i" (first host label).
    #[test]
    fn cdn_hosts_fold_into_their_real_site() {
        let s = classify("https://i.4cdn.org/h/1743761874698456.jpg").unwrap();
        assert_eq!(s.site, "4chan", "4cdn must be 4chan, not 'i'");
        assert_eq!(s.mode, Mode::DirectMedia);

        // unknown hosts keep a sane name: registrable domain, not 'www'
        let s = classify("https://cdn.example-media.net/x/img.webp").unwrap();
        assert_eq!(s.site, "example-media.net");
    }

    /// Field report #7: `wimg.rule34.xxx//images/…jpeg?11353035` must route
    /// through the rule34 API (numeric query = post id), not the CDN directly —
    /// that is where the thumbnail 403 was coming from.
    #[test]
    fn booru_cdn_file_links_route_through_the_api() {
        let s = classify("https://wimg.rule34.xxx//images/3868/7412ed67862b691cceef71b918028f66.jpeg?11353035")
            .unwrap();
        assert_eq!(s.site, "rule34");
        assert_eq!(s.mode, Mode::BooruId);
        assert_eq!(s.remote_id.as_deref(), Some("11353035"));

        let s = classify("https://img3.gelbooru.com//images/2b/34/2b34cd.jpg?4765984").unwrap();
        assert_eq!(s.site, "gelbooru");
        assert_eq!(s.mode, Mode::BooruId);
        assert_eq!(s.remote_id.as_deref(), Some("4765984"));

        // hypnohub has no API wired → stays a direct file, but named correctly
        let s = classify("https://hypnohub.net//images/6c/e0/6ce01fd2de7cc88d217ac34e44f7601a.gif?267940")
            .unwrap();
        assert_eq!(s.site, "hypnohub");
        assert_eq!(s.mode, Mode::DirectMedia);

        // e621's file CDN (md5 query, not a post page) must fetch directly
        let s = classify("https://static1x1.e621.net/data/f1/2e/f12e32ab.jpg?a1b2c3").unwrap();
        assert_eq!(s.site, "e621");
        assert_eq!(s.mode, Mode::DirectMedia);
    }

    #[test]
    fn direct_media_on_booru_cdn_resolves() {
        let s = classify("https://hypnohub.net//images/6c/e0/6ce01.gif?267940").unwrap();
        assert_eq!(s.mode, Mode::DirectMedia);
        assert_eq!(s.site, "hypnohub");
        assert_eq!(s.remote_id.as_deref(), Some("267940"));
        let s = classify("https://hypnohub.net//images/ec/47/ec47.mp4?250356").unwrap();
        assert_eq!(s.mode, Mode::DirectMedia);
        // a post page still classifies as a booru post
        let s = classify("https://hypnohub.net/index.php?page=post&s=view&id=267940").unwrap();
        assert_eq!(s.mode, Mode::BooruId);
        assert_eq!(s.remote_id.as_deref(), Some("267940"));
    }

    #[test]
    fn local_file_image_reads_and_thumbs() {
        let dir = std::env::temp_dir().join("goon-clip-test");
        std::fs::create_dir_all(&dir).unwrap();
        let img = dir.join("sub zero.png");
        // 1x1 png via base64 (desktop crate has no image dep; goon-core makes the thumb)
        let png = base64::Engine::decode(
            &base64::engine::general_purpose::STANDARD,
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==",
        )
        .unwrap();
        std::fs::write(&img, &png).unwrap();
        let url = format!("file:///{}", img.to_string_lossy().replace('\\', "/"));
        let m = local_file(&url).unwrap();
        assert_eq!(m.title.as_deref(), Some("sub zero"));
        assert!(m.store_bytes.is_some());
        assert!(media::make_thumbnail(m.thumb_bytes.as_ref().unwrap()).is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- #14: title search ----

    #[test]
    fn normalize_title_drops_bracket_noise() {
        assert_eq!(
            normalize_title("Summer Witch [renpy] [English] [v1.2]"),
            "summer witch"
        );
        assert_eq!(normalize_title("Karryn's Prison (PC)"), "karryn s prison");
    }

    #[test]
    fn title_similarity_scores_titles() {
        // exact after normalization
        assert!((title_similarity("Summer Witch [v1.0]", "SUMMER  WITCH") - 1.0).abs() < 1e-9);
        // a real f95 title vs the same game on DLsite
        assert!(title_similarity("Karryn's Prison", "Karryn's Prison [v0.9.2] [renpy]") > 0.9);
        // short vs long with an unrelated tail must NOT read as strong
        assert!(title_similarity("cat", "cat who saved the whole world") < 0.6);
        // disjoint (common bigrams like "ta" keep unrelated words ~0.2)
        assert!(title_similarity("alpha beta", "gamma delta") < 0.5);
    }

    #[test]
    fn dl_search_anchors_parse_verified_markup() {
        let html = r#"
        <thumb-with-ng-filter-block link="https://www.dlsite.com/maniax/work/=/product_id/RJ01731048.html" :thumb-candidates="['x.webp']" alt="Summer Witch Remake [同人誌]" ></thumb-with-ng-filter-block>
        <a href="https://www.dlsite.com/maniax/work/=/product_id/RJ01731048.html" title="Summer Witch Remake [同人誌]">t</a>
        <a href="https://www.dlsite.com/maniax/work/=/product_id/RJ01752389.html" title="Summer &amp; Rain">t</a>
        <a href="https://www.dlsite.com/maniax/circle/profile/=/maker_id/RG22701.html">maker</a>
        <a href="https://www.dlsite.com/maniax/work/=/product_id/RJ123456.html" title="Old Six Digit">t</a>
        "#;
        let hits = parse_dl_search(html);
        assert_eq!(hits.len(), 3); // thumb+anchor for one product dedupe; maker id ignored
        assert!(hits.iter().any(|h| h.url.ends_with("RJ01731048.html") && h.title == "Summer Witch Remake [同人誌]"));
        assert!(hits.iter().any(|h| h.title == "Summer & Rain")); // entity unescaped
        assert!(hits.iter().any(|h| h.url.ends_with("RJ123456.html")));
    }

    #[test]
    fn f95_search_parses_thread_links_and_dedupes() {
        let html = r#"
        <div class="block-row"><a href="https://f95zone.to/threads/summer-witch.10599/"><img src="x.jpg"></a>
        <h3 class="structItem-title"><a href="https://f95zone.to/threads/summer-witch.10599/">Summer Witch [v1.2] [renpy] [WiP]</a></h3>
        <a href="https://f95zone.to/threads/other-game.99999/page-2#post-1">Other Game &amp; Sequel</a>
        <a href="https://f95zone.to/threads/">bad</a>
        "#;
        let hits = parse_f95_search(html);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].url, "https://f95zone.to/threads/summer-witch.10599/");
        assert_eq!(hits[0].title, "Summer Witch [v1.2] [renpy] [WiP]");
        assert_eq!(hits[1].title, "Other Game & Sequel");
    }

    /// The exact shapes from the M1 field report — these used to land in
    /// "not a known site yet".
    #[test]
    fn field_report_urls_all_resolve_as_media() {
        let urls = [
            "https://hypnohub.net//images/6c/e0/6ce01fd2de7cc88d217ac34e44f7601a.gif?267940",
            "https://hypnohub.net//images/ec/47/ec47078adbb43723acf78708b1a7f435.mp4?250356",
            "file:///C:/Users/Admin/Videos/EnomeSnake.mp4",
            "file:///C:/Users/Admin/Documents/New%20folder/Media/Renetan/Renetan_Animations_2025-08_Update/hasumi%20swimsuit%20zuri%20uncensored%20sound.mp4",
            "file:///C:/Users/Admin/Videos/[Kimochi]%20Cissia's%20Paizuri-fellatio%20animation/uncensored%20fix.mp4",
            "file:///C:/Users/Admin/Documents/New%20folder/Media/[%E3%82%A2%E3%83%8B%E3%83%A1]/[%E3%82%A2%E3%83%8B%E3%83%A1]%20%E3%83%8B%E3%82%B3%E3%81%AE%E6%B0%B4%E7%9D%80%E3%83%91%E3%82%A4%E3%82%BA%E3%83%AA.mp4",
        ];
        for u in urls {
            let s = classify(u).unwrap_or_else(|| panic!("unrecognized: {u}"));
            assert_eq!(s.kind, ItemKind::Post, "{u}");
            assert!(
                matches!(s.mode, Mode::LocalFile | Mode::DirectMedia),
                "{u} → {s:?}"
            );
        }
        // genuinely unknown stays unknown (no fake media detection)
        assert!(classify("https://support.mozilla.org/products/firefox").is_none());
        assert!(classify("https://www.mozilla.org/contribute/").is_none());
    }

    #[test]
    fn windows_file_urls_become_paths() {
        let p = local_path("/C:/Users/Admin/Videos/EnomeSnake.mp4").unwrap();
        assert_eq!(p.display().to_string(), "C:/Users/Admin/Videos/EnomeSnake.mp4");
        let p = local_path("//nas/share/clip.mp4").unwrap();
        assert_eq!(p.display().to_string(), r"\\nas\share\clip.mp4");
        assert!(local_path("/").is_err());
    }

    #[test]
    fn eh_gate_and_gallery_parse() {
        let gate = r##"<html><body><div class="gtable ggate">You must <a href="#">IP Login</a> to continue</div></body></html>"##;
        assert!(eh_is_gate(gate));
        let gallery = r#"<html><head><title>Kanojo no Kaomo | e-hentai.org</title></head>
        <body><div id="gdt"><div class="gdtm" style="..."><div class="g2"></div>
        <a href="https://e-hentai.org/s/abc/1234-1"><img src="https://ex.ph/cover1.jpg"></a></div></div>
        <div class="gld">This gallery has <em>24</em> pages.</div></body></html>"#;
        assert!(!eh_is_gate(gallery));
        let (title, cover) = parse_eh_gallery(gallery);
        assert_eq!(title.as_deref(), Some("Kanojo no Kaomo"));
        assert_eq!(cover.as_deref(), Some("https://ex.ph/cover1.jpg"));
    }

    #[test]
    fn f95_thread_parse_title_version_tags() {
        let page = r#"<html><head><meta property="og:title" content="Summer Witch [v0.12] [renpy] [Work in progress] | f95zone"></head><body></body></html>"#;
        let (raw, tags) = parse_f95_thread(page);
        assert!(tags.contains(&"renpy".to_string()));
        let (t, v) = crate::importer::clean_name(raw.as_deref().unwrap());
        assert_eq!(t, "Summer Witch");
        assert_eq!(v.as_deref(), Some("0.12"));
    }

    #[test]
    fn dlsite_product_parse() {
        let page = r#"<html><head>
        <meta property="og:title" content="[RJ123456] Summer Witch [PC版]">
        <meta property="og:image" content="https://img.dlsite.jp/works/RJ123456.jpg">
        </head><body><span itemprop="dateUpdated" content="2026-03-01T12:00:00+09:00"></span></body></html>"#;
        let (title, cover, updated) = parse_dl_product(page);
        assert_eq!(title.as_deref(), Some("Summer Witch [PC版]"));
        assert_eq!(cover.as_deref(), Some("https://img.dlsite.jp/works/RJ123456.jpg"));
        assert_eq!(updated.as_deref(), Some("2026-03-01"));
    }

    #[test]
    fn mirrors_classify_and_default_off() {
        let s = classify("https://kimochi.net/fgo-lilith/").unwrap();
        assert_eq!(s.site, "kimochi");
        assert!(is_mirror_site("kimochi"));
    }

    /// Manual smoke against the live sites (rate-limits ignored, run by hand):
    /// `cargo test -p goonflow-desktop --lib -- --ignored --nocapture match_live`
    #[tokio::test]
    #[ignore]
    async fn match_live_search_smoke() {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(Duration::from_secs(20))
            .build()
            .unwrap();
        let hits = dlsite_search(&client, "karryn").await.expect("dlsite search");
        assert!(!hits.is_empty(), "dlsite search returned nothing");
        assert!(hits.iter().all(|h| h.url.contains("/product_id/")));
        println!("dlsite hits: {}", hits.len());
        for h in hits.iter().take(3) {
            println!("  {} | {}", h.url, h.title);
        }
        // without a cookie f95 answers the login page — expect the friendly error
        let r = f95_search(&client, "spring seed", None).await;
        println!("f95 no-cookie result: {}", r.err().unwrap_or_else(|| "hits without a cookie?!".into()));
    }
}
