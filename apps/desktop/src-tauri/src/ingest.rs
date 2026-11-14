//! Ingest layer (PLAN §6): one pipeline — identify(url) → fetch meta → store item.
//!
//! Used by the bookmark importer, the in-app "+ Clip URL" action, and (M4)
//! the companion API `/api/clip` route. Everything classify()s to a SiteRef;
//! fetch_meta() pulls title/cover/tags where the site allows it; clip()
//! always stores the URL even when the metadata fetch fails — losing the
//! link is worse than a bare entry.

use crate::jobs::JobHandle;
use crate::{new_id, now_epoch, Core};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, ItemKind, Status};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

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
    matches!(
        site,
        "kimochi" | "otomi" | "ryuugames" | "nagadoki" | "hentaihaven"
    )
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
pub const KNOWN_SITES: &[(&str, &str)] = &[
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

/// gid out of an e-hentai/exhentai URL. `/g/<gid>/<token>/` is the gallery page.
/// `/s/<token>/<gid>-<n>` is the slideshow (#46) — the gid is still in the URL,
/// and it is the only thing that page can give us.
fn eh_gid(segments: &[&str]) -> Option<String> {
    if let Some(i) = segments.iter().position(|s| *s == "g") {
        return segments.get(i + 1).map(|s| s.to_string());
    }
    if segments.first().map(|s| *s == "s").unwrap_or(false) {
        let gid = segments.last()?.split('-').next().unwrap_or("");
        if !gid.is_empty() && gid.chars().all(|c| c.is_ascii_digit()) {
            return Some(gid.to_string());
        }
    }
    None
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
        media_ext(path)?;
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
    //
    // #50, measured 2026-11-14: the danbooru-family metadata api answers
    // `403 text/html` + `cf-mitigated: challenge` to this app — honest UA, a
    // browser UA, even no UA — while the same post's file host answers
    // `200 image/jpeg`. So a link that **names a file** is fetched as a file
    // and never sent to the api, even when a post id can be read out of it (the
    // id is still kept, by the direct-file branch below, so the report can name
    // the post). A link that does not name a file is a post page: that still
    // goes to the api, where the fallback note says what could not be read.
    if host_no_port.ends_with("gelbooru.com") {
        let id = query_param(url, "id")
            .or_else(|| query_param(url, "pid"))
            .or_else(|| numeric_query_id(url));
        // direct CDN file links (img3.gelbooru.com/images/….jpg) with no
        // recoverable post id must fall through to the direct-file branch
        // below, which at least stores the image (#39).
        if media_ext(&path).is_none() {
            return Some(SiteRef {
                site: "gelbooru".into(),
                kind: ItemKind::Post,
                remote_id: id,
                mode: Mode::BooruId,
            });
        }
    }
    if host_no_port.ends_with("rule34.xxx") {
        let id = query_param(url, "id")
            .or_else(|| numeric_query_id(url))
            .or_else(|| {
                // /index.php?...&id=N and /post/show/N both count
                let i = segments.iter().position(|s| *s == "show")?;
                segments
                    .get(i + 1)
                    .filter(|s| s.chars().all(|c| c.is_ascii_digit()))
                    .map(|s| s.to_string())
            });
        if media_ext(&path).is_none() {
            return Some(SiteRef {
                site: "rule34".into(),
                kind: ItemKind::Post,
                remote_id: id,
                mode: Mode::BooruId,
            });
        }
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
        // carry the post id as a numeric query, and #50 routed them through the
        // site's API because that got tags + file with the right referer. That
        // routing is what broke rule34: measured 2026-11-14, the danbooru dapi
        // answers `403` + `cf-mitigated: challenge` to *both* client identities
        // (the API is behind a bot check now), while the CDN link answers `200
        // image/jpeg` to anything. The file is what the user pasted, so the file
        // is what gets fetched; the id is kept only to report which post it was.
        if site_for_host(&host_no_port).is_some() && media_ext(&path) == Some("image") {
            if let Some(id) = numeric_query_id(url) {
                return Some(SiteRef {
                    site,
                    kind: ItemKind::Post,
                    remote_id: Some(id),
                    mode: Mode::DirectMedia,
                });
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
    /// image links the source page exposed but we did not download (#46).
    /// Reported as a count up the chain: "the art is there, we only stored the
    /// cover" must never read as "the source carried nothing".
    pub images: Vec<String>,
    pub tags: Vec<String>,
    pub version: Option<String>,
    pub note: Option<String>,
}

#[derive(Default)]
pub struct FetchCtx<'a> {
    pub cookie: Option<&'a str>,
    /// #38: the running app, so a fetch that comes back showing the site's login
    /// wall can mark the cookie we just sent as stale. `None` for the live probe
    /// tests, which have no vault.
    pub core: Option<&'a Core>,
}

/// A logged-in fetch was answered by the site's login wall: the cookie in the
/// vault is stale, and that is worth recording instead of leaving the user to
/// deduce it from a string in an item's note.
///
/// **Never call this while holding the inner lock** - it takes the lock itself, and
/// `std::sync::Mutex` is not reentrant, so a caller that holds it freezes its own
/// thread instead of failing: the #55 test did exactly that and the whole suite hung
/// with no output. Every fetch path calls this after its settings block has released
/// the guard.
pub fn mark_cookie_gate(core: Option<&Core>, site: &str) {
    let Some(core) = core else { return };
    let mut inner = core.inner.lock().unwrap();
    if let Some(o) = inner.opened.as_mut() {
        let _ = crate::cookies::note_gate(&o.conn, site, &crate::now_epoch());
    }
}

/// A status alone is not a report: a 4xx says our request was refused, a 5xx
/// says the site's own server failed. The user's next step differs (fix the link
/// vs wait and retry), so the note has to differ too — an e621 `http 520` used to
/// read as if this app were broken.
fn http_error(status: reqwest::StatusCode) -> String {
    if status.is_server_error() {
        format!(
            "http {status} — the site's own server failed this request, try again in a minute"
        )
    } else {
        format!("http {status}")
    }
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
        return Err(http_error(resp.status()));
    }
    resp.text().await.map_err(|e| format!("read failed: {e}"))
}

/// Fetch **file bytes** (image/video). #50: goes out with the browser UA — the
/// other half of the split documented in [`crate::http`]. Applied per request
/// rather than by swapping clients so a caller that only handed us its page
/// client (bookmark import, re-enrich) keeps that client's vault proxy, while
/// the CDN still sees a browser. reqwest fills *default* headers only where the
/// request set none, so this override wins.
async fn get_bytes(
    client: &reqwest::Client,
    url: &str,
    referer: &str,
    cookie: Option<&str>,
) -> Result<Vec<u8>, String> {
    let mut req = client
        .get(url)
        .header(reqwest::header::REFERER, referer)
        .header(reqwest::header::USER_AGENT, crate::http::FILE_UA);
    if let Some(c) = cookie {
        req = req.header(reqwest::header::COOKIE, c);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let bytes = resp
        .bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("download interrupted: {e}"))?;
    // f95zone answers a guest with 204 and no body: "success" that gave us
    // nothing. Saying so beats storing an empty file.
    if bytes.is_empty() {
        return Err("the host answered with no bytes (art is gated behind a login?)".into());
    }
    Ok(bytes)
}

/// Ceiling for cover art pulled into the vault. An oversized body is refused
/// with a note — never silently truncated, never stored as a thumbnail.
const MAX_COVER_BYTES: usize = 16 * 1024 * 1024;

/// Decide what to keep from fetched bytes by **sniffing them**, never by the URL
/// suffix (#46/#50: f95zone serves `image/avif` from a URL ending `.jpg`, and a
/// blocked hotlink request can answer with an HTML page at status 200). Sets
/// `meta.note` when the bytes are not art we can keep. Returns the image mime.
fn store_bytes_in(meta: &mut Meta, bytes: Vec<u8>) -> Option<&'static str> {
    match media::sniff_media(&bytes) {
        media::Sniffed::Image(mime) if bytes.len() <= MAX_COVER_BYTES => {
            meta.thumb_bytes = Some(bytes.clone());
            meta.store_bytes = Some(bytes);
            Some(mime)
        }
        media::Sniffed::Image(mime) => {
            meta.note = Some(format!(
                "the image came back as {mime} but is {} bytes — too large to store; \
                 the link is kept",
                bytes.len()
            ));
            None
        }
        media::Sniffed::Video(mime) => {
            meta.note = Some(format!(
                "video ({mime}) — kept as a link, not stored in the vault"
            ));
            None
        }
        media::Sniffed::Unknown => {
            meta.note = Some(
                "the host answered, but the bytes were not a recognizable image/video — \
                 nothing stored"
                    .into(),
            );
            None
        }
    }
}

pub async fn fetch_meta(
    client: &reqwest::Client,
    s: &SiteRef,
    url: &str,
    ctx: &FetchCtx<'_>,
) -> Result<Meta, String> {
    fetch_meta_with(client, s, url, ctx, true).await
}

/// `want_images = false`: read the page only. Update sweeps use this so a
/// version check stays one request per item instead of pulling cover art for
/// every tracked game (#46 added per-item art work; it must not silently
/// double the traffic the polite per-site lanes are budgeted for).
async fn fetch_meta_with(
    client: &reqwest::Client,
    s: &SiteRef,
    url: &str,
    ctx: &FetchCtx<'_>,
    want_images: bool,
) -> Result<Meta, String> {
    match s.mode {
        Mode::BooruId => match s.site.as_str() {
            "gelbooru" | "rule34" => {
                match booru_dapi(client, &s.site, s.remote_id.as_deref()).await {
                    Ok(m) => Ok(m),
                    // #50: the metadata api is the polite way in, but the
                    // danbooru-family api is behind a bot check now (measured
                    // 2026-11-14: `403` with `cf-mitigated: challenge` for both
                    // client identities). A link that names a file can still be
                    // fetched from the cdn, so take the picture and say plainly
                    // that the tags did not come.
                    Err(e) => {
                        let (_, path) = host_of(url);
                        if media_ext(&path).is_some() {
                            let mut m = direct_media(client, url, Some(&s.site)).await?;
                            m.note = Some(match m.note.take() {
                                Some(n) => format!("{n}; the {} metadata api refused us ({e})", s.site),
                                None => format!(
                                    "the {} metadata api refused us ({e}) — the picture was taken \
                                     from the link itself, so there are no tags",
                                    s.site
                                ),
                            });
                            Ok(m)
                        } else {
                            Err(format!(
                                "the {} metadata api refused us ({e}) and that link is not a file",
                                s.site
                            ))
                        }
                    }
                }
            }
            "e621" => e621_post(client, s.remote_id.as_deref()).await,
            other => Ok(Meta {
                note: Some(format!(
                    "no metadata API wired for {other} yet — stored as bare link"
                )),
                ..Default::default()
            }),
        },
        Mode::Ehentai => eh_gallery(client, url, ctx, &s.site).await,
        Mode::DlSite => dl_product(client, url).await,
        Mode::Offsite => offsite_product(client, url).await,
        Mode::F95 => f95_thread(client, url, ctx, want_images).await,
        Mode::DirectMedia => {
            let mut m = direct_media(client, url, Some(&s.site)).await?;
            // #50, measured 2026-11-14: the danbooru-family metadata api
            // (`…/index.php?page=dapi…`) answers `403` with `cf-mitigated:
            // challenge` to *both* client identities this app has — honest UA,
            // browser UA, even no UA. The file host answers the same link at
            // `200 image/jpeg` to everything. So a file link gets its picture
            // and does not get its tags, and the item has to say which half it is.
            if matches!(s.site.as_str(), "rule34" | "gelbooru") && m.tags.is_empty() {
                m.note = note_with(
                    m.note.take(),
                    Some(format!(
                        "{}'s metadata api answers a Cloudflare bot check to this app, so the \
                         tags could not be read; the picture came from the file host, which \
                         answers normally{}",
                        s.site,
                        match s.remote_id.as_deref() {
                            Some(id) => format!(" (post {id} — open its page in your browser)"),
                            None => String::new(),
                        }
                    )),
                );
            }
            Ok(m)
        }
        Mode::LocalFile => local_file(url),
    }
}

// ---- booru family (danbooru dapi: gelbooru/rule34) ----

async fn booru_dapi(
    client: &reqwest::Client,
    site: &str,
    id: Option<&str>,
) -> Result<Meta, String> {
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

async fn post_from_json(
    post: &Value,
    referer: &str,
    client: &reqwest::Client,
) -> Result<Meta, String> {
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
    let mut m = Meta {
        tags,
        ..Default::default()
    };
    // #50: image vs video is decided by the bytes we got, not by the file URL's
    // extension (and a CDN that dislikes us answers HTML at 200 — that is not art).
    if store_bytes_in(&mut m, bytes).is_none() && m.note.is_none() {
        m.note = Some(if is_video {
            "video post — kept as link, not stored in vault".into()
        } else {
            "post json had no storable file (deleted?)".into()
        });
    }
    Ok(m)
}

async fn e621_post(client: &reqwest::Client, id: Option<&str>) -> Result<Meta, String> {
    let id = id.ok_or_else(|| "no numeric post id in that url".to_string())?;
    let text = get_text(
        client,
        &format!("https://e621.net/posts/{id}.json"),
        "https://e621.net",
        None,
    )
    .await?;
    let v: Value = serde_json::from_str(&text).map_err(|_| "api reply was not json".to_string())?;
    let post = v
        .get("post")
        .ok_or_else(|| "post not found (deleted?)".to_string())?;
    let file_url = post
        .get("file")
        .and_then(|f| f.get("url"))
        .and_then(|u| u.as_str())
        .ok_or_else(|| "post json had no file url".to_string())?;
    let tags = post
        .get("tags")
        .and_then(|t| t.get("general"))
        .and_then(|g| g.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|t| t.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();
    let is_video = media_ext(file_url) == Some("video");
    let bytes = get_bytes(client, file_url, "https://e621.net", None).await?;
    let mut m = Meta {
        tags,
        ..Default::default()
    };
    // #50: bytes decide, not the URL extension
    if store_bytes_in(&mut m, bytes).is_none() && m.note.is_none() {
        m.note = Some(if is_video {
            "video post — kept as link, not stored in vault".into()
        } else {
            "post json had no storable file (deleted?)".into()
        });
    }
    Ok(m)
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
        // 2026: a gallery link without its key is answered by this plain text,
        // not by a gate page (#46's `gdtm` grid and `og:image` are both gone).
        || html.contains("Key missing, or incorrect key provided")
}

/// Page count, in the words the page actually uses.
///
/// `Length:</td><td class="gdt2">20 pages</td>` is the row, and the grid header
/// says `Showing 1 - 20 of 20 images`. The number is taken by walking **back**
/// from the word it precedes: scanning for digits *after* the label picks up the
/// `2` inside `gdt2` and then keeps going into the tile urls, which is how this
/// parser first read a real 20-page gallery as a nine-digit one.
pub fn eh_pages(html: &str) -> Option<u32> {
    for (label, word) in [("Length:", " pages"), ("Showing", " of ")] {
        let Some(i) = html.find(label) else { continue };
        let from = i + label.len();
        let window = &html[from..(from + 200).min(html.len())];
        let Some(w) = window.find(word) else { continue };
        let digits: String = window[..w]
            .trim_end()
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        if let Ok(n) = digits.chars().rev().collect::<String>().parse::<u32>() {
            return Some(n);
        }
    }
    None
}

/// One tile of a sprite sheet: which sheet, and the box inside it.
///
/// Why this exists at all: e-hentai's gallery page used to carry a `<div
/// class="gdtm">` with one `<img>` per thumbnail, and `og:image` for the cover.
/// Measured 2026-11-14, neither is in the page any more — the cover and the
/// whole page grid are sprite sheets on hosts that change per gallery, and the
/// only place the tiles are described is CSS:
/// `style="width:200px;height:141px;background:transparent
///  url(https://puzaqhxrbn.hath.network/c2/…/4089666-0.webp) -400px 0 no-repeat"`.
/// So a thumbnail is (sheet, x, y, w, h), not a url.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EhTile {
    pub sheet: String,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// What a gallery page gave us.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EhGallery {
    pub title: Option<String>,
    pub tags: Vec<String>,
    /// declared page count (`Length: 20 pages`), `None` when the row is absent
    pub pages: Option<u32>,
    pub cover: Option<EhTile>,
    /// the slideshow link of every page on this grid page, in order
    pub slides: Vec<String>,
}

/// Read one tile out of a `style` attribute. CSS background-position says where
/// the **sheet's** top-left corner sits relative to the box, so a tile drawn at
/// `-400px 0` starts 400px into the sheet.
pub fn eh_tile(style: &str) -> Option<EhTile> {
    let px = |key: &str| -> Option<u32> {
        let i = style.find(key)? + key.len();
        let digits: String = style[i..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse().ok()
    };
    let w = px("width:")?;
    let h = px("height:")?;
    let u = style.find("url(")? + 4;
    let sheet: String = style[u..].chars().take_while(|c| *c != ')' && *c != '"').collect();
    if !sheet.starts_with("http") {
        return None;
    }
    // the two offsets after the closing paren: `-400px 0`, `0 0`
    let after = &style[u + sheet.len()..];
    let nums: Vec<i64> = after
        .split([';', ' ', 'n'])
        .filter_map(|t| {
            let t = t.trim().trim_end_matches("px");
            t.parse().ok()
        })
        .collect();
    Some(EhTile {
        sheet,
        x: nums.first().map(|v| v.unsigned_abs() as u32).unwrap_or(0),
        y: nums.get(1).map(|v| v.unsigned_abs() as u32).unwrap_or(0),
        w,
        h,
    })
}

/// The host a `/g/` occurrence was written with, when the page spelled its url
/// out in full. `None` for a relative href, which then takes the site root.
fn eh_host_at(html: &str, i: usize) -> Option<String> {
    let start = html[..i].rfind("://")? + 3;
    let between = &html[start..i];
    // close by, and no tag or attribute boundary in the way: `…href="https://
    // e-hentai.org/g/` is a real absolute url, an earlier link in the page is not
    if between.len() > 48 || between.contains('"') || between.contains('\'') || between.contains('<')
    {
        return None;
    }
    let host: String = between.chars().take_while(|c| *c != '/').collect();
    (!host.is_empty() && host.contains('.')).then_some(host)
}

/// The gallery link a page carries — `<a href="…/g/<gid>/<key>/">`.
///
/// #46 folded a slideshow link onto `https://host/g/<gid>/`, which used to work.
/// It is an apache 404 now (measured 2026-11-14: 236 B, "Not Found"), while the
/// `/s/` shell page still names the gallery with its key. This is how the key is
/// recovered, so the key never has to be guessed.
pub fn eh_gallery_link(html: &str, root: &str) -> Option<String> {
    let mut from = 0usize;
    while let Some(i) = html[from..].find("/g/") {
        let i = from + i + 3;
        from = i;
        let rest = &html[i..];
        let gid: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        if gid.is_empty() {
            continue;
        }
        let key: String = rest[gid.len()..]
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        // e-hentai's gallery key is a short hex token; a bare `/g/` nav link or a
        // numeric segment is not one.
        if key.len() >= 8 && key.chars().all(|c| c.is_ascii_hexdigit()) {
            let host = eh_host_at(html, i).unwrap_or_else(|| {
                root.trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .to_string()
            });
            return Some(format!("https://{host}/g/{gid}/{key}/"));
        }
    }
    None
}

/// The gallery name. `<h1 id="gn">` is the name as the page states it;
/// `<title>` is the same text with " - E-Hentai Galleries" glued on, and until
/// 2026 it was the only thing this parser read.
fn eh_title(html: &str) -> Option<String> {
    ["<h1 id=\"gn\">", "<h1 id=\"gj\">"]
        .iter()
        .find_map(|m| html.find(m).map(|i| &html[i + m.len()..]))
        .map(|s| s.split("</h1>").next().unwrap_or("").trim().to_string())
        .filter(|t| !t.is_empty())
        .or_else(|| {
            html.find("<title>")
                .map(|i| html[i + 7..].split("</title>").next().unwrap_or("").trim())
                .map(|t| {
                    t.strip_suffix(" - E-Hentai Galleries")
                        .unwrap_or(t)
                        .trim()
                        .to_string()
                })
                .filter(|t| !t.is_empty())
        })
}

/// The cover box: the first sprite tile in the page. The grid tiles come after
/// it, and the 720x80 ad banner above them has no `url()` in its style.
fn eh_cover_tile(html: &str) -> Option<EhTile> {
    let mut at = 0usize;
    while let Some(i) = html[at..].find("background:transparent url(") {
        let i = at + i;
        let end = html[i..].find('"').map(|e| i + e).unwrap_or(html.len());
        let start = html[..i].rfind("style=\"").map(|s| s + 7).unwrap_or(i);
        if let Some(tile) = eh_tile(&html[start..end]) {
            return Some(tile);
        }
        at = end;
    }
    None
}

/// Tags, in the order the page lists them: `id="ta_artist:horosuke"`.
fn eh_tags(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut at = 0usize;
    while let Some(i) = html[at..].find("id=\"ta_") {
        let i = at + i + 7;
        let tag: String = html[i..].chars().take_while(|c| *c != '"').collect();
        if tag.contains(':') {
            out.push(tag);
        }
        at = i;
    }
    out
}

/// Every slideshow link on this grid page, in page order. They are absolute
/// urls in the real page; a relative one would still work because the item
/// keeps its source host.
fn eh_slides(html: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut at = 0usize;
    while let Some(i) = html[at..].find("/s/") {
        let i = at + i;
        let start = html[..i].rfind("href=\"").map(|s| s + 6).unwrap_or(i);
        let end = html[start..].find('"').map(|e| start + e).unwrap_or(html.len());
        let href = &html[start..end];
        if href.contains("/s/") && seen.insert(href.to_string()) {
            out.push(href.to_string());
        }
        at = end;
    }
    out
}

/// Parse a gallery page (2026 shape): title, tags, page count, the cover sprite
/// box, and one link per page.
pub fn parse_eh_gallery(html: &str) -> EhGallery {
    EhGallery {
        title: eh_title(html),
        tags: eh_tags(html),
        pages: eh_pages(html),
        cover: eh_cover_tile(html),
        slides: eh_slides(html),
    }
}

/// True when a link is e-hentai's slideshow page (`/s/<imgkey>/<gid>-<page>`)
/// rather than a gallery page. The slideshow page is not decoration: it carries
/// the full-size art of one page (#46, measured below).
pub fn eh_is_show(url: &str) -> bool {
    let (_host, path) = host_of(url);
    path.starts_with("/s/")
}

/// One page of the slideshow, as its own page describes it.
///
/// Measured 2026-11-14 on `https://e-hentai.org/s/3c7a2c86a5/4089666-1` with
/// this app's own UA and **no cookie at all**:
/// * `<img id="img" src="https://fivpjap.ezrzluxzxslb.hath.network/h/<hash>-…
///   /keystamp=1794642300-…;fileindex=…;xres=org/12349451_0.jpg">` — the real
///   page art. It answered `200 image/jpeg 177 497 B` with our UA, with or
///   without a referer and with or without a cookie. The host and the
///   `keystamp` are handed out per request, so this url is never a candidate to
///   keep: the **page link** is the thing worth remembering.
/// * `<div>12349451_0.jpg :: 1200 x 630 :: 173.3 KiB</div>` — name and size.
/// * `<span>1</span> / <span>20</span>` — this page and the page count.
/// * `<div id="i5"><a href="https://e-hentai.org/g/4089666/b9d914bef4/">` — the
///   gallery, **with the key the gallery page needs**.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EhShow {
    pub title: Option<String>,
    /// full-size art of this one page, `None` when the page refused to show it
    pub image: Option<String>,
    /// this page number (`1` in `1 / 20`)
    pub page: Option<u32>,
    /// page count (`20` in `1 / 20`)
    pub pages: Option<u32>,
}

pub fn parse_eh_show(html: &str) -> EhShow {
    let title = ["<h1>", "<title>"]
        .iter()
        .find_map(|m| html.find(m).map(|i| &html[i + m.len()..]))
        .map(|t| t.split("</").next().unwrap_or("").trim().to_string())
        .filter(|t| !t.is_empty());
    // `id="img"` is the art; the nav arrows are plain `ehgt.org/g/*.png` icons.
    let image = html
        .find("id=\"img\"")
        .and_then(|i| html[i..].find("src=\"").map(|j| &html[i + j + 5..]))
        .and_then(|t| t.split('"').next())
        .map(|u| u.trim().to_string())
        .filter(|u| u.starts_with("http"));
    // `1</span> / <span>20` is the page counter, and the file line gives the
    // same numbers as `:: 1200 x 630 ::`. The counter is the shorter read.
    let (page, pages) = match html.find("</span> / <span>") {
        Some(i) => {
            let before: String = html[..i]
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            let after: String = html[i + 16..]
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            (before.chars().rev().collect::<String>().parse().ok(), after.parse().ok())
        }
        None => (None, None),
    };
    EhShow { title, image, page, pages }
}

async fn eh_page_art(
    client: &reqwest::Client,
    page_url: &str,
    referer: &str,
    ctx: &FetchCtx<'_>,
) -> Result<(Vec<u8>, EhShow), String> {
    let html = get_text(client, page_url, referer, ctx.cookie).await?;
    let show = parse_eh_show(&html);
    let image = show
        .image
        .clone()
        .filter(|_| !eh_is_gate(&html) && !html.trim().is_empty())
        .ok_or("the slideshow page named no picture (its art is not shown to this client)")?;
    let bytes = get_bytes(client, &image, page_url, ctx.cookie).await?;
    Ok((bytes, show))
}

/// The site root, as the referer its own pages expect.
fn eh_root(site: &str) -> &'static str {
    if site == "exhentai" {
        "https://exhentai.org"
    } else {
        "https://e-hentai.org"
    }
}

async fn eh_gallery(
    client: &reqwest::Client,
    url: &str,
    ctx: &FetchCtx<'_>,
    site: &str,
) -> Result<Meta, String> {
    // #46: the gallery url needs its **key**. A `/s/` slideshow link used to be
    // folded onto `https://host/g/<gid>/`; since 2026-11 that is a bare apache
    // 404 (`http 404 Not Found`, exactly what the user saw when clipping
    // e-hentai.org/s/fd0163b571/4089666-7), while the shell page still names the
    // gallery with its key. So the key comes from the page, never from a guess.
    let show_link = eh_is_show(url);
    let root = eh_root(site);
    let (gallery, gallery_html) = if show_link {
        let shell = get_text(client, url, root, ctx.cookie).await?;
        if eh_is_gate(&shell) || shell.trim().is_empty() {
            return Ok(eh_gate_meta(ctx, site, &shell));
        }
        let link = eh_gallery_link(&shell, root).ok_or_else(|| {
            "the slideshow page named no gallery (no /g/<gid>/<key>/ link in it)".to_string()
        })?;
        let html = get_text(client, &link, url, ctx.cookie).await?;
        (link, html)
    } else {
        let page = url.to_string();
        let html = get_text(client, &page, root, ctx.cookie).await?;
        (page, html)
    };

    if eh_is_gate(&gallery_html) || gallery_html.trim().is_empty() {
        return Ok(eh_gate_meta(ctx, site, &gallery_html));
    }

    let g = parse_eh_gallery(&gallery_html);
    let mut m = Meta {
        title: g.title.clone(),
        tags: g.tags.clone(),
        // Every page of the gallery is a candidate: the link, never the
        // per-request image url it would hand out (#53 — one picture per click).
        images: g.slides.clone(),
        ..Default::default()
    };
    let pages = g
        .pages
        .map(|p| p.to_string())
        .unwrap_or_else(|| g.slides.len().to_string());

    if show_link {
        // The link names one page, so that page's own art is the picture.
        match eh_page_art(client, url, &gallery, ctx).await {
            Ok((bytes, show)) => {
                // `store_bytes_in` writes its own reason when the bytes are not a
                // keepable picture, so only the success path adds a note here.
                if let Some(mime) = store_bytes_in(&mut m, bytes) {
                    let page = show
                        .page
                        .map(|p| format!("page {p}"))
                        .unwrap_or_else(|| "a page".into());
                    m.note = Some(format!(
                        "{page} stored from e-hentai's image host as {mime}; the gallery has \
                         {} page(s), listed under Sources where any one of them can be stored",
                        show.pages.map(|p| p.to_string()).unwrap_or_else(|| pages.clone())
                    ));
                }
            }
            Err(e) => m.note = Some(format!(
                "the gallery was read, but this page's own art could not be taken ({e}) — \
                 the title and tags were stored"
            )),
        }
        return Ok(m);
    }

    match g.cover {
        Some(tile) => match get_bytes(client, &tile.sheet, &gallery, ctx.cookie).await {
            Ok(sheet) => match media::crop_tile(&sheet, tile.x, tile.y, tile.w, tile.h) {
                Ok(t) => {
                    m.thumb_bytes = Some(t.bytes.clone());
                    m.store_bytes = Some(t.bytes);
                    m.note = Some(format!(
                        "cover cut out of e-hentai's gallery sheet ({}x{} from a {} B sheet); \
                         {pages} page(s) listed under Sources, where any one of them can be \
                         stored — a gallery page is only ever a preview",
                        t.width,
                        t.height,
                        sheet.len()
                    ));
                }
                Err(e) => m.note = Some(format!(
                    "e-hentai's gallery sheet arrived but the cover box could not be cut from \
                     it ({e}) — the title and tags were stored"
                )),
            },
            Err(e) => m.note = Some(format!(
                "the gallery sheet at {} could not be fetched ({e}) — the title was stored",
                tile.sheet
            )),
        },
        None => {
            if m.title.is_some() {
                m.note = Some(format!(
                    "e-hentai gallery page named no cover box to take a picture from \
                     ({pages} page(s) listed under Sources)"
                ));
            }
        }
    }
    Ok(m)
}

/// What to say when the page came back as a gate instead of a gallery. #55: an
/// e-hentai/exhentai cookie could die and the Sources line stayed green, because
/// nothing recorded what the site had just answered. Same rule, same helper, as
/// the f95 login wall.
fn eh_gate_meta(ctx: &FetchCtx<'_>, site: &str, html: &str) -> Meta {
    if ctx.cookie.is_some() {
        mark_cookie_gate(ctx.core, site);
    }
    Meta {
        note: Some(if html.trim().is_empty() {
            "the site answered with an empty page — its anti-bot layer did not accept this \
             client and cookie pair (measured: exhentai 200 with 0 bytes, any user-agent)"
                .into()
        } else {
            "e-hentai gate page hit — open the gallery once in your browser (or set its \
             cookie under Sources), then re-clip to enrich"
                .into()
        }),
        ..Default::default()
    }
}

// ---- DLsite + repack mirrors ----

/// Parse a DLsite-style product page (mirrors use the same shape).
pub fn parse_dl_product(html: &str) -> (Option<String>, Option<String>, Option<String>) {
    let title = og_content(html, "og:title").map(|t| clean_title_from(&t));
    let cover = og_content(html, "og:image");
    let updated = [
        "itemprop=\"dateUpdated\"",
        "dateUpdated",
        "最終更新日",
        "Updated:",
    ]
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
    let mut m = Meta {
        title,
        version: updated,
        ..Default::default()
    };
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

/// Hosts that carry a thread's real art. XenForo keeps smilies, badges, avatars,
/// signatures and ad creatives under `f95zone.to/data/…` and `/assets/…`, so the
/// host allow-list — not a class filter alone — is what keeps them out (#46).
const F95_ATTACHMENT_HOSTS: &[&str] = &["attachments.f95zone.to"];

/// Read one attribute out of an already-sliced tag body (`name="v"` / `name='v'`).
fn html_attr(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        for pat in [format!("{name}={quote}"), format!("{name} = {quote}")] {
            if let Some(i) = tag.find(&pat) {
                let rest = &tag[i + pat.len()..];
                if let Some(j) = rest.find(quote) {
                    let v = rest[..j].trim();
                    if !v.is_empty() {
                        return Some(v.to_string());
                    }
                }
            }
        }
    }
    None
}

/// #46: the real post-image URLs of a XenForo thread page, in document order.
/// Lazy-load means `src` is a `data:image/svg+xml…` placeholder — the URL we want
/// is in `data-src` (`data-url` repeats it for the lightbox). Verified markup:
///
/// ```html
/// <img src="data:image/svg+xml;charset=utf-8,%3Csvg … width='560' height='420' …"
///      data-src="https://attachments.f95zone.to/2026-11-6671101_d_642380pr.jpg"
///      data-url="https://attachments.f95zone.to/2026-11-6671101_d_642380pr.jpg"
///      class="bbImage lazyload" data-zoom-target="1" alt= …>
/// ```
///
/// `src` is only a fallback when it is not a `data:` placeholder. Deduped; the
/// first entry is post #1's preview, which is what the cover is taken from.
pub fn parse_f95_post_images(html: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut idx = 0usize;
    while let Some(rel) = html[idx..].find("<img") {
        let tag_start = idx + rel + "<img".len();
        let tag_end = html[tag_start..]
            .find('>')
            .map(|e| tag_start + e)
            .unwrap_or(html.len());
        let tag = &html[tag_start..tag_end];
        idx = tag_end;
        // the class says what the tag *is*: smilies/emoji are never art
        let class = html_attr(tag, "class").unwrap_or_default();
        if class.contains("smilie") || class.contains("smiley") || class.contains("emoji") {
            continue;
        }
        let raw = html_attr(tag, "data-src")
            .or_else(|| html_attr(tag, "data-url"))
            .or_else(|| html_attr(tag, "src"));
        let Some(raw) = raw.map(|v| unescape_html(v.trim())) else {
            continue;
        };
        // the svg/gif placeholder is not a link to anything
        if raw.is_empty() || raw.starts_with("data:") || raw.starts_with("javascript:") {
            continue;
        }
        let url = if raw.starts_with("//") {
            format!("https:{raw}")
        } else {
            raw
        };
        let host = host_of(&url).0.to_lowercase();
        if !F95_ATTACHMENT_HOSTS
            .iter()
            .any(|h| host == *h || host.ends_with(&format!(".{h}")))
        {
            continue;
        }
        if seen.insert(url.clone()) {
            out.push(url);
        }
    }
    out
}

/// #46: the guest login wall, measurable on a cookie-less fetch of thread
/// 321420: XenForo replaces each hidden post with
/// `class="messageHide messageHide--attach"` + "Log in or register to see
/// attachments" (20 "register to" hits, zero `attachments.f95zone.to` URLs).
/// With the user's saved cookie the same page carries 26 lazy images.
pub fn f95_login_wall(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("messagehide--attach")
        || lower.contains("register to see attachments")
        || lower.contains("register to see links")
        || lower.contains("log in or register to")
        || lower.contains("you must register")
        || html.contains("login required")
        || html.contains("You do not have permission")
}

/// What a fetched f95 thread page says about its art (#46). Three verdicts, so
/// the report never sends the user to the wrong fix — the old code lumped "needs
/// a login" in with "nothing there" and the UI printed the misleading verdict.
#[derive(Debug, PartialEq, Eq)]
enum F95Art {
    /// the page is the guest wall: the art exists, we are not allowed to see it
    NeedsLogin,
    /// we could see the post and the thread really has no post images
    NoArt,
    /// real attachment URLs, document order (first = post #1's preview)
    Art(Vec<String>),
}

fn f95_art(html: &str, cookie: Option<&str>, images: &[String]) -> F95Art {
    if !images.is_empty() {
        return F95Art::Art(images.to_vec());
    }
    if f95_login_wall(html) || cookie.is_none() {
        F95Art::NeedsLogin
    } else {
        F95Art::NoArt
    }
}

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
                "renpy" | "rpg maker" | "rpg-maker" | "rpgm" | "unity" | "gamemaker" | "twine"
                | "srpg maker" => tags.push(lower.replace(' ', "-")),
                "completed" => tags.push("f95-completed".into()),
                _ => {} // version + status tokens handled by clean_name
            }
        } else {
            i += 1;
        }
    }
    (Some(raw.trim().to_string()), tags)
}

/// #46: an f95 thread carries its art in lazy-loaded `data-src` images on
/// `attachments.f95zone.to`, and a cookie-less fetch only sees the login wall.
/// `og:image` on f95 threads is **the site logo** — never a cover. The three
/// outcomes (needs a login / no art at all / art fetched) each get their own
/// note so the UI stops pointing users at the wrong thing. `want_images=false`
/// (version sweeps) reads title/version/tags and downloads nothing.
async fn f95_thread(
    client: &reqwest::Client,
    url: &str,
    ctx: &FetchCtx<'_>,
    want_images: bool,
) -> Result<Meta, String> {
    let html = get_text(client, url, "https://f95zone.to", ctx.cookie).await?;
    let (raw_title, tags) = parse_f95_thread(&html);
    let (title, version) = match raw_title {
        Some(t) => {
            let (t2, v) = crate::importer::clean_name(&t);
            let title = if t2.is_empty() { t } else { t2 };
            (Some(title), v)
        }
        None => (None, None),
    };
    let found = parse_f95_post_images(&html);
    let mut m = Meta {
        title,
        version,
        tags,
        images: found.clone(),
        ..Default::default()
    };
    if !want_images {
        return Ok(m);
    }
    let images = match f95_art(&html, ctx.cookie, &found) {
        F95Art::NeedsLogin => {
            // #38: we sent a cookie and f95 still showed the wall, so that cookie
            // is dead. Say so where the user can see it (Sources), not only in
            // this item's note.
            if ctx.cookie.is_some() {
                mark_cookie_gate(ctx.core, "f95");
            }
            m.note = Some(
                "f95 answered, but this page shows its art to members only — save your f95 \
                 cookie under Sources and re-fetch this item to pull the images"
                    .into(),
            );
            return Ok(m);
        }
        F95Art::NoArt => {
            m.note = Some(
                "this f95 thread page carries no post images — title/tags came from it, \
                 there is no cover to fetch"
                    .into(),
            );
            return Ok(m);
        }
        F95Art::Art(list) => list,
    };
    // Cover = post #1's preview; a thread may open with a banner, so a few
    // candidates get one try each. Nothing else is downloaded — the rest stay
    // links, and the note says that out loud.
    let mut why = String::new();
    for cand in images.iter().take(3) {
        let bytes = match get_bytes(client, cand, "https://f95zone.to", ctx.cookie).await {
            Ok(b) => b,
            Err(e) => {
                why = e;
                continue;
            }
        };
        // #46/#50: the type comes from the bytes (f95 answers image/avif for a
        // URL ending .jpg); the extension decides nothing here.
        if let Err(e) = keep_f95_cover(&mut m, bytes, images.len()) {
            why = format!("{cand} {e}");
            continue;
        }
        return Ok(m);
    }
    m.note = Some(format!(
        "f95 lists {} attachment image(s) but none came through ({why}) — f95 gates its art \
         behind a login: save your f95 cookie under Sources and re-fetch",
        images.len()
    ));
    Ok(m)
}

/// Decide what one fetched f95 cover contributes to the vault, and what the
/// note says about it. Split out of the fetch loop so the storage rule is
/// testable without a network.
///
/// Both halves are kept whenever the bytes are a keepable image: the
/// **original**, so the item view shows the real picture, and the bytes the
/// thumbnailer works from. They used to be either/or here, which is how #52
/// went wrong: making AVIF `thumbnailable` moved f95's covers out of the "no
/// decoder, store the original" branch and into the "make a tile" branch, and
/// that branch kept *only* the tile - so the full cover quietly stopped being
/// stored while the note went on claiming "cover stored". A live clip of one
/// thread proved it: thumbnail row present, full image row absent. The rest of
/// the ingest path (`store_bytes_in`) has always kept both, so this matches it.
///
/// `Err(reason)` means these bytes are not a keepable cover: the caller tries
/// the next candidate and the reason still reaches the report.
fn keep_f95_cover(m: &mut Meta, bytes: Vec<u8>, found: usize) -> Result<(), String> {
    let mime = match media::sniff_media(&bytes) {
        media::Sniffed::Image(mime) => mime,
        media::Sniffed::Video(mime) => return Err(format!("is {mime}, not an image")),
        media::Sniffed::Unknown => return Err("was not a recognizable image".into()),
    };
    if bytes.len() > MAX_COVER_BYTES {
        return Err(format!("is {} bytes — too big for a cover", bytes.len()));
    }
    m.thumb_bytes = Some(bytes.clone());
    m.store_bytes = Some(bytes);
    if media::thumbnailable(mime) {
        m.note = Some(format!(
            "cover stored from post #1 as {mime}; {found} attachment image(s) on the page — \
             listed under Sources, where any one of them can be stored"
        ));
    } else {
        // No decoder for this format in this build (HEIF/HEIC, anything `image`
        // 0.25.10 cannot read): the original is kept so the item view still shows
        // real art, and the note says which of the two facts it is.
        m.note = Some(format!(
            "cover is {mime} from post #1 — the original is stored, but this build cannot \
             decode {mime} into a thumbnail, so the grid tile stays missing; {found} \
             attachment image(s) on the page, listed under Sources"
        ));
    }
    Ok(())
}

// ---------- one picture at a time (#53) ----------

/// What storing one candidate picture did, in the same honest shape as a clip
/// report: how many bytes, whether the vault already had them, and what the item
/// can now show.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorePictureReport {
    pub url: String,
    pub mime: String,
    pub bytes: usize,
    /// media rows the item holds now, so the viewer knows to re-list its strip
    pub frames: usize,
    /// true when the vault already had these exact bytes (content-hash dedup)
    pub already: bool,
    /// true when this picture also became the item's grid tile
    pub tile: bool,
    pub note: Option<String>,
}

/// May these bytes become a stored picture? Decided by **sniffing**, never by the
/// URL suffix (#46: f95zone serves `image/avif` from a URL ending `.jpg`, and a
/// blocked hotlink answers with an HTML page at status 200), under the same
/// ceiling a cover gets. Split out of [`store_picture`] so the rule is testable
/// without a network.
fn storable_picture(bytes: &[u8]) -> Result<&'static str, String> {
    let mime = match media::sniff_media(bytes) {
        media::Sniffed::Image(mime) => mime,
        media::Sniffed::Video(mime) => return Err(format!("that link is {mime}, not a picture")),
        media::Sniffed::Unknown => return Err("was not a recognizable image".into()),
    };
    if bytes.len() > MAX_COVER_BYTES {
        return Err(format!(
            "is {} bytes, over the {} MB picture ceiling",
            bytes.len(),
            MAX_COVER_BYTES / (1024 * 1024)
        ));
    }
    Ok(mime)
}

/// Which polite lane a picture fetch belongs to when the item has no source row
/// to borrow a site from: the host it was fetched from, so a repeat visit to the
/// same host still waits for the same lane.
fn host_lane(url: &str) -> String {
    let rest = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    rest.split('/').next().unwrap_or(rest).to_string()
}

/// Unix seconds, for the `source_images` bookkeeping columns (the rest of the
/// schema keeps epoch seconds as text; [`crate::now_epoch`] is that string).
fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// #53: remember the pictures a page offered, as candidates on the item. Nothing
/// is fetched here: the default stays one downloaded cover per clip, because
/// pulling every attachment on enrich would turn a polite fetch into a mirror of
/// the thread. What lands in `source_images` is the list the item's Sources
/// section shows and what [`store_picture`] acts on.
pub fn remember_candidates(conn: &rusqlite::Connection, item_id: &str, images: &[String]) -> usize {
    if images.is_empty() {
        return 0;
    }
    db::remember_source_images(conn, item_id, images, now_secs())
}

/// #53: fetch **one** picture the user picked and store it on the item.
///
/// Same guarantees as a clip, because it is the same machinery: the site's rate
/// box is honoured through the queue lane (#57), the cookie comes from the same
/// per-site store, the bytes are typed by sniffing (#46), the size ceiling is the
/// cover ceiling, and a Lock or Nuke mid-fetch gives the picture up instead of
/// writing into a closing vault. This is the deliberate alternative to downloading
/// a whole thread: the user chose this image, so it is the only one fetched.
pub async fn store_picture(core: &Core, item_id: String, url: String) -> Result<StorePictureReport, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("no url".into());
    }

    // Phase 1 (sync): the item, the site whose rate box and cookie govern this
    // fetch, and the page to name as referer (attachment hosts gate hotlinks).
    let (site, rate_ms, cookie, referer) = {
        let inner = core.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().ok_or("vault is locked")?.conn;
        let d = db::get_item(conn, &item_id)
            .map_err(|e| e.to_string())?
            .ok_or("no such item in this vault")?;
        let site = d
            .sources
            .first()
            .map(|s| s.site.clone())
            .unwrap_or_else(|| host_lane(&url));
        (
            site.clone(),
            db::site_rate_ms(conn, &site, default_rate_ms(&site)),
            db::site_cookie(conn, &site),
            d.sources
                .first()
                .map(|s| s.url.clone())
                .unwrap_or_else(|| url.clone()),
        )
    };

    // #57: one lane slot, and the same give-up-if-the-vault-closed rule as clip().
    let gone = || match core.inner.try_lock() {
        Ok(guard) => guard.opened.is_none(),
        Err(_) => false,
    };
    if !core.queue.reserve(&site, rate_ms, gone).await {
        return Err(format!(
            "the vault was locked before {site} could answer - the picture was not stored"
        ));
    }

    // Phase 2 (async): the bytes. get_bytes carries the file UA per request and
    // refuses the empty 204 a guest gets back, so a gated host says so here.
    let client = crate::http::client_for(core, 25)?;
    // An e-hentai candidate is a *page* link, not a picture link (#46): the url
    // the page hands out for its art carries a per-request host and keystamp, so
    // it is not worth remembering. The page is fetched, and its art is stored.
    let bytes = if eh_is_show(&url) {
        let ctx = FetchCtx {
            cookie: cookie.as_deref(),
            core: Some(core),
        };
        eh_page_art(&client, &url, &referer, &ctx).await?.0
    } else {
        get_bytes(&client, &url, &referer, cookie.as_deref()).await?
    };
    let mime = storable_picture(&bytes)?;

    // Phase 3 (sync): store, mark the candidate stored, and report.
    let mut inner = core.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    let fresh = media::put_full_image(conn, &item_id, &bytes);
    db::mark_source_image_stored(conn, &item_id, &url, now_secs());
    // An item with no tile yet gets one from this picture, so the grid stops
    // showing a nameless card; an item that already has its cover keeps it.
    let mut tile = false;
    let mut note = None;
    if media::get_thumb(conn, &item_id).is_none() {
        match media::make_thumbnail(&bytes) {
            Some(t) => tile = media::set_thumbnail(conn, &item_id, &t),
            None if media::thumbnailable(mime) => {
                note = Some(format!(
                    "stored as {mime}; the picture would not decode into a tile, so the grid \
                     tile stays missing"
                ))
            }
            None => note = Some(format!(
                "stored as {mime}; this build cannot decode {mime} into a tile, so the grid \
                 tile stays missing"
            )),
        }
    }
    if !fresh {
        note = note_with(
            note.take(),
            Some("the vault already had these exact bytes (stored once, deduped by content hash)".into()),
        );
    }
    Ok(StorePictureReport {
        url,
        mime: mime.to_string(),
        bytes: bytes.len(),
        frames: media::media_count(conn, &item_id),
        already: !fresh,
        tile,
        note,
    })
}

// ---- direct media & local files ----


async fn direct_media(
    client: &reqwest::Client,
    url: &str,
    site: Option<&str>,
) -> Result<Meta, String> {
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
    let clean = format!("{}://{}{}", "https", host, clean_path);
    let referer = format!(
        "https://{}/",
        registrable_domain(host.split(':').next().unwrap_or(&host))
    );
    let bytes = match get_bytes(client, &clean, &referer, None).await {
        Ok(b) => b,
        Err(first) => {
            // second chance: the original URL (some CDNs are picky about paths)
            get_bytes(client, url, &referer, None)
                .await
                .map_err(|_| first)?
        }
    };

    // #46/#50: what these bytes are is decided by magic bytes, never by the
    // link's suffix — a `.jpg` link can carry AVIF, and a CDN that dislikes us
    // answers an HTML error page at status 200. Unusable bodies are never stored
    // and never handed to the thumbnailer, and the note says what happened.
    let size = bytes.len();
    let mut meta = Meta::default();
    let Some(mime) = store_bytes_in(&mut meta, bytes) else {
        if meta.note.is_none() {
            meta.note = Some("direct file link — the host's bytes were not storable art".into());
        }
        return Ok(meta);
    };

    // #39: booru CDN filenames embed the post's md5 (`<md5><crc32>.png`). If
    // the site's API has exactly one post with that md5 AND the same byte
    // size, upgrade the bare file link to full post tags. Verification is
    // strict — a wrong post is worse than no post; otherwise we keep just the
    // stored image and the user fixes sources/tags by hand later.
    if matches!(site, Some("rule34") | Some("gelbooru") | Some("e621")) {
        let stem = clean_path.rsplit('/').next().unwrap_or("");
        let stem = stem.split('.').next().unwrap_or("").to_lowercase();
        let hex = stem.chars().all(|c| c.is_ascii_hexdigit());
        let md5 = if hex && stem.len() == 40 {
            stem[..32].to_string()
        } else if hex && stem.len() == 32 {
            stem.clone()
        } else {
            String::new()
        };
        if !md5.is_empty() {
            if let Some(post) = find_post_by_md5(client, site.unwrap_or(""), &md5).await {
                let claimed_md5 = post.get("md5").and_then(|m| m.as_str()).or_else(|| {
                    post.get("file")
                        .and_then(|f| f.get("md5"))
                        .and_then(|m| m.as_str())
                });
                let claimed_size = post.get("filesize").and_then(|f| f.as_i64()).or_else(|| {
                    post.get("file")
                        .and_then(|f| f.get("size"))
                        .and_then(|s| s.as_i64())
                });
                let md5_ok = claimed_md5.is_some_and(|m| m.eq_ignore_ascii_case(&md5));
                let size_ok = claimed_size.is_none_or(|s| s as usize == size);
                if md5_ok && size_ok {
                    let pid = post
                        .get("id")
                        .and_then(|i| i.as_i64())
                        .map(|i| i.to_string())
                        .unwrap_or_default();
                    let tags: Vec<String> = post
                        .get("tags")
                        .and_then(|t| t.as_str())
                        .map(|s| s.split_whitespace().map(String::from).collect())
                        .or_else(|| {
                            post.get("tags")
                                .and_then(|t| t.get("general"))
                                .and_then(|g| g.as_array())
                                .map(|a| {
                                    a.iter()
                                        .filter_map(|t| t.as_str().map(String::from))
                                        .collect()
                                })
                        })
                        .unwrap_or_default();
                    meta.tags = tags;
                    meta.note = Some(format!(
                        "reverse-matched to post {pid} by file hash — link the post page as a source if you want it official"
                    ));
                    return Ok(meta);
                }
            }
        }
    }

    meta.note = Some(format!(
        "direct file link — stored as-is as {mime} (type read from the bytes, not the link); \
         no post could be identified, edit tags/source later"
    ));
    Ok(meta)
}

/// One-post lookup by file md5 on the sites that support it. Anything but a
/// clean single hit is treated as "no match".
async fn find_post_by_md5(client: &reqwest::Client, site: &str, md5: &str) -> Option<Value> {
    let (api, referer) = match site {
        "rule34" => (
            format!(
                "https://rule34.xxx/public/index.php?page=dapi&s=post&q=index&json=1&md5={md5}"
            ),
            "https://rule34.xxx",
        ),
        "gelbooru" => (
            format!("https://gelbooru.com/index.php?page=dapi&s=post&q=index&json=1&md5={md5}"),
            "https://gelbooru.com",
        ),
        "e621" => (
            format!("https://e621.net/posts.json?tags=md5%3A{md5}&limit=1"),
            "https://e621.net",
        ),
        _ => return None,
    };
    let text = get_text(client, &api, referer, None).await.ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    let posts = v.get("posts").or_else(|| v.get("post"))?;
    posts.as_array()?.first().cloned()
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
    let candidate = if let Some(unc) = s.strip_prefix("//") {
        // UNC share: \\server\share\file.ext
        // keep the leading double backslash — that is what marks a UNC path
        format!(r"\\{}", unc.replace('/', "\\"))
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
    let mut m = Meta {
        title: Some(stem),
        ..Default::default()
    };
    // #50: even for a file the user pointed at, the type we store is the type the
    // bytes have — a `.png` that is really an AVI stays a link, not a broken row.
    if store_bytes_in(&mut m, bytes).is_none() && m.note.is_none() {
        m.note = Some("the file's bytes were not a recognizable image — kept as a link".into());
    }
    Ok(m)
}

// ---------- the clip pipeline ----------

/// `Debug` so a failing test can print what the clip actually reported instead of
/// a trait bound error.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipReport {
    pub item_id: String,
    pub title: String,
    pub site: String,
    pub created: bool,
    pub stored_image: bool,
    pub thumb: bool,
    pub tags: usize,
    /// #46: images the source page offered beyond the cover we stored. Reported,
    /// never dropped, so "20 images on the page, 1 cover stored" is visible.
    pub images_found: usize,
    pub note: Option<String>,
}

pub async fn clip(state: &Core, url: String) -> Result<ClipReport, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("no url".into());
    }
    let s = classify(&url).ok_or("can't identify that link (site support grows over time)")?;

    // Phase 1 (sync): settings, dedup, cookie for this site.
    let (cookie, rate_ms, existing) = {
        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().ok_or("vault is locked")?.conn;
        if !db::site_enabled(conn, &s.site, !is_mirror_site(&s.site)) {
            return Err(format!("{} is disabled — enable it under Sources", s.site));
        }
        (
            db::site_cookie(conn, &s.site),
            db::site_rate_ms(conn, &s.site, default_rate_ms(&s.site)),
            db::source_item_id(conn, &s.site, &url),
        )
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
            images_found: 0,
            note: Some("already in the flow".into()),
        });
    }

    // #57: the clip used to fetch immediately no matter what the site's rate box
    // said, so two clips in a row could hit e-hentai inside its ~10 s lane while
    // every *job* politely queued behind it. A clip is the most frequent thing this
    // app does to a site, and the companion API's own comment promises "the exact
    // same lock, job ledger and rate lanes" — so it claims a slot like a job does.
    // First request to a site costs nothing (the lane opens immediately); a Lock or
    // Nuke while waiting gives the clip up instead of fetching into a closed vault.
    // One slot per clip, not per request: a clip can still make several (f95 fetches
    // its page plus up to three cover candidates). That is the same compromise #15
    // already makes for enrich, and closing it means counting requests inside the
    // fetchers rather than at the call site.
    if !matches!(s.mode, Mode::LocalFile) {
        let gone = || match state.inner.try_lock() {
            Ok(guard) => guard.opened.is_none(),
            Err(_) => false, // someone else holds the lock; not a cancel signal
        };
        if !state.queue.reserve(&s.site, rate_ms, gone).await {
            return Err(format!(
                "the vault was locked before {site} could answer — the link was not clipped",
                site = s.site
            ));
        }
    }

    // Phase 2 (async): metadata where available. Never fatal — a failed
    // fetch still stores the link (bare entry beats lost bookmark).
    // #50: a direct file link on a host we have no API for is *only* file bytes,
    // so it uses the browser-UA client. Booru CDN links keep the honest client:
    // their md5 reverse-lookup talks to the site's API, and that traffic must
    // stay identifiable — the file bytes they fetch still carry the browser UA,
    // per request, inside `get_bytes`.
    let client = match s.mode {
        Mode::DirectMedia if !matches!(s.site.as_str(), "rule34" | "gelbooru" | "e621") => {
            crate::http::file_client_for(state, 25)?
        }
        _ => crate::http::client_for(state, 25)?,
    };
    let ctx = FetchCtx {
        cookie: cookie.as_deref(),
        core: Some(state),
    };
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
    let title = meta
        .title
        .take()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(fallback);
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
    // A thumbnail this build cannot make stays visible in the report (#46 found
    // AVIF coming back from f95 with no decoder; #52 linked dav1d in, so what
    // lands here now is HEIF and anything else `image` cannot read).
    let missing = undecodable_note(&meta, thumb);
    meta.note = note_with(meta.note.take(), missing);
    // #53: whatever else the page offered is kept as a candidate list on the item,
    // so its Sources section can offer them one picture at a time. Still exactly
    // one download per clip: remembering 15 urls costs nothing, mirroring a thread
    // costs the site.
    remember_candidates(conn, &id, &meta.images);
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
        images_found: meta.images.len(),
        note: meta.note,
    })
}

/// Join two report notes without duplicating prose.
fn note_with(existing: Option<String>, extra: Option<String>) -> Option<String> {
    match (existing, extra) {
        (Some(a), Some(b)) if a != b => Some(format!("{a} — {b}")),
        (a, b) => a.or(b),
    }
}

/// The honest sentence for "we have bytes but cannot thumbnail them". Never
/// silently reported as success, never silently skipped.
fn undecodable_note(meta: &Meta, thumb_stored: bool) -> Option<String> {
    if meta.thumb_bytes.is_none() || thumb_stored {
        return None;
    }
    let mime = meta
        .thumb_bytes
        .as_deref()
        .and_then(|b| match media::sniff_media(b) {
            media::Sniffed::Image(m) => Some(m),
            _ => None,
        })
        .unwrap_or("an unknown format");
    // Two different facts, and they must not share a sentence (#52): a format
    // this build has no decoder for (HEIF) is a fact about the app, while a
    // decodable format whose bytes refused to decode is a fact about those
    // bytes. Saying "this build cannot decode image/avif" after dav1d was
    // linked in would be a lie that sent users looking for a missing feature.
    if media::thumbnailable(mime) {
        Some(format!(
            "no thumbnail: the stored {mime} would not decode (the stored image itself \
             is fine and shows in the item view)"
        ))
    } else {
        Some(format!(
            "no thumbnail: this build cannot decode {mime} (the stored image itself is \
             fine and shows in the item view)"
        ))
    }
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

pub async fn check_updates(state: &Core, job: &JobHandle) -> Result<UpdateReport, String> {
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
            m.entry(t.site.clone()).or_insert_with(|| {
                (
                    db::site_cookie(conn, &t.site),
                    db::site_rate_ms(conn, &t.site, default_rate_ms(&t.site)),
                )
            });
        }
        m
    };

    let client = crate::http::client_for(state, 15)?;

    let targets_len = targets.len();
    for (i, t) in targets.into_iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(i, targets_len, &format!("{} ({})", t.title, t.site));
        let (cookie, rate) = site_ctx
            .get(&t.site)
            .cloned()
            .unwrap_or((None, default_rate_ms(&t.site)));
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
        let s = SiteRef {
            site: t.site.clone(),
            kind: ItemKind::Game,
            remote_id: None,
            mode,
        };
        let ctx = FetchCtx {
            cookie: cookie.as_deref(),
            core: Some(state),
        };
        match fetch_meta_with(&client, &s, &t.url, &ctx, false).await {
            Ok(m) => {
                if let Some(nv) = m.version.filter(|v| !v.is_empty()) {
                    let changed = t.old_version.as_deref().is_none_or(|o| o != nv);
                    if changed {
                        let now = now_epoch();
                        let inner = state.inner.lock().unwrap();
                        if let Some(opened) = inner.opened.as_ref() {
                            let _ = db::set_item_version(&opened.conn, &t.item_id, &nv, &now);
                            let _ = db::record_update(
                                &opened.conn,
                                &t.item_id,
                                &t.site,
                                t.old_version.as_deref(),
                                Some(&nv),
                                &now,
                            );
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
pub fn apply_meta(
    conn: &rusqlite::Connection,
    item_id: &str,
    site: &str,
    meta: &Meta,
) -> (bool, usize, bool) {
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
                Status::parse(&d.status).unwrap_or(Status::Seen),
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
    /// #46: image links the sources offered beyond what was stored.
    pub images_found: usize,
}

/// "Re-fetch metadata" (#11): re-run every source of an item through the same
/// classify → fetch_meta → apply_meta pipeline that clip/bookmark-enrich use.
/// Local sources are re-read from disk (fixes covers that failed the first
/// time); remote sources honour their site rate and cookie.
pub async fn reenrich_item(
    state: &Core,
    item_id: &str,
    job: &JobHandle,
) -> Result<EnrichReport, String> {
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
        images_found: 0,
    };
    if sources.is_empty() {
        report
            .notes
            .push("this item has no sources to fetch from".into());
        return Ok(report);
    }

    let client = crate::http::client_for(state, 15)?;

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
            inner
                .opened
                .as_ref()
                .and_then(|o| db::site_cookie(&o.conn, site))
        };
        let ctx = FetchCtx {
            cookie: cookie.as_deref(),
            core: Some(state),
        };
        let meta = match fetch_meta(&client, &cls, url, &ctx).await {
            Ok(m) => m,
            Err(e) => {
                report.failures.push(EnrichFailure {
                    url: url.clone(),
                    reason: e,
                });
                continue;
            }
        };

        let mut inner = state.inner.lock().unwrap();
        let Some(opened) = inner.opened.as_mut() else {
            report.stopped = true;
            break;
        };
        // #46: "did this item already have art?" is the question that decides
        // whether a source with no new thumbnail is a dead end or simply already
        // stored. Without it the only verdict left was the misleading
        // "the source answered but carried nothing new".
        let had_art = media::best_for(&opened.conn, item_id).is_some();
        let (thumb, tags, title_updated) = apply_meta(&opened.conn, item_id, site, &meta);
        report.images_found += meta.images.len();
        if thumb || tags > 0 || title_updated {
            report.enriched += 1;
        }
        let already = !thumb && had_art && (meta.thumb_bytes.is_some() || !meta.images.is_empty());
        let note = note_with(meta.note.clone(), undecodable_note(&meta, thumb));
        if let Some(note) = &note {
            report.notes.push(format!("{site}: {note}"));
        } else if already {
            report.notes.push(format!(
                "{site}: this item already has art stored — {site} had nothing new to add"
            ));
        }
        if !thumb && tags == 0 && !title_updated && note.is_none() && !already {
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
    let inter: usize = ba
        .iter()
        .map(|(k, v)| bb.get(k).copied().unwrap_or(0).min(*v))
        .sum();
    let dice = 2.0 * inter as f64 / (na + nb) as f64;
    // penalize very different lengths ("cat" vs "cat who saved the world")
    let (short, long) = (
        a.chars().count().min(b.chars().count()),
        a.chars().count().max(b.chars().count()),
    );
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
    let resp = req
        .send()
        .await
        .map_err(|e| format!("search failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    resp.text().await.map_err(|e| format!("read failed: {e}"))
}

/// DLsite has no public JSON API — post the site's own /fs search form and
/// pull (link, title) from the result anchors. Verified live markup:
/// `href="https://www.dlsite.com/maniax/work/=/product_id/RJ01731048.html" title="…"`.
pub async fn dlsite_search(
    client: &reqwest::Client,
    query: &str,
) -> Result<Vec<SearchHit>, String> {
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
        let id: String = after
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
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
        out.push(SearchHit {
            url,
            title: unescape_html(&title),
        });
    }
    out
}

/// f95zone search lives behind XenForo login, so **no cookie means no request**:
/// the guide's promise (*f95 search runs only with your saved f95 cookie*) has to
/// hold before anything leaves the machine. An anonymous query to a login-gated
/// search endpoint is traffic the user never asked for and a hit on f95's limiter
/// for nothing; the old code sent it and only then noticed the reply was the wall.
pub async fn f95_search(
    client: &reqwest::Client,
    query: &str,
    cookie: Option<&str>,
) -> Result<Vec<SearchHit>, String> {
    let Some(cookie) = cookie else {
        return Err(
            "f95 search wants a login — save your f95 cookie under Sources, then search again"
                .into(),
        );
    };
    let req = client
        .get("https://f95zone.to/search/search")
        .header(reqwest::header::REFERER, "https://f95zone.to")
        .header(reqwest::header::COOKIE, cookie)
        .query(&[("q", query), ("c", "message")]);
    let resp = req
        .send()
        .await
        .map_err(|e| format!("search failed: {e}"))?;
    if matches!(
        resp.status(),
        reqwest::StatusCode::FORBIDDEN | reqwest::StatusCode::UNAUTHORIZED
    ) {
        // The cookie was sent and refused: a different fact from having none.
        return Err(
            "f95 refused the saved cookie — re-copy it from a logged-in window and set it under Sources"
                .into(),
        );
    }
    if !resp.status().is_success() {
        return Err(format!("http {}", resp.status()));
    }
    let html = resp.text().await.map_err(|e| format!("read failed: {e}"))?;
    let hits = parse_f95_search(&html);
    if hits.is_empty() {
        // A cookie was definitely sent (the guard above), so an empty result page
        // means "no such title" — not "log in". The old code could not tell those
        // apart and blamed the login on every empty page.
        return Err(format!("f95 found no thread for \"{query}\""));
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
        let digits: String = after_dot
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
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
        let url = format!(
            "https://f95zone.to/threads/{}/",
            &path[..dot + 1 + digits.len()]
        );
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
    state: &crate::Core,
    item_id: &str,
    site: &str,
    title: &str,
    hits: &[SearchHit],
) -> usize {
    let mut scored: Vec<(f64, &SearchHit)> = hits
        .iter()
        .map(|h| (title_similarity(title, &h.title), h))
        .collect();
    scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let keep: Vec<&(f64, &SearchHit)> = scored
        .iter()
        .take(3)
        .take_while(|(s, _)| *s >= 0.6)
        .collect();
    if keep.is_empty() {
        return 0;
    }
    let Ok(inner) = state.inner.try_lock() else {
        return 0;
    };
    let Some(opened) = inner.opened.as_ref() else {
        return 0;
    };
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
pub async fn find_matches(state: &crate::Core, job: &JobHandle) -> Result<MatchReport, String> {
    let targets = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        db::games_needing_match(&opened.conn).map_err(crate::err)?
    };
    let mut report = MatchReport::default();
    if targets.is_empty() {
        return Ok(report);
    }
    let client = crate::http::client_for(state, 20)?;
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
            if !state
                .queue
                .reserve("dlsite", dl_rate, || job.cancelled())
                .await
            {
                report.stopped = true;
                break;
            }
            match dlsite_search(&client, &query).await {
                Ok(hits) => wrote += store_hits(state, &id, "dlsite", &title, &hits),
                Err(e) => report.failures.push(format!("dlsite: {e}")),
            }
        }
        if f95_on {
            // #56: no saved cookie means no request, and no lane slot spent waiting
            // for a fetch that cannot happen. The old code queued politely behind
            // f95's lane and then asked for a login it already knew was missing.
            match f95_cookie.as_deref() {
                None => {
                    let why = "f95: search skipped — no login cookie saved (Sources)".to_string();
                    if !report.failures.contains(&why) {
                        // once per run: the same reason for 40 titles is noise, not news
                        report.failures.push(why);
                    }
                }
                Some(saved) => {
                    if !state
                        .queue
                        .reserve("f95", f95_rate, || job.cancelled())
                        .await
                    {
                        report.stopped = true;
                        break;
                    }
                    match f95_search(&client, &query, Some(saved)).await {
                        Ok(hits) => wrote += store_hits(state, &id, "f95", &title, &hits),
                        Err(e) => report.failures.push(format!("f95: {e}")),
                    }
                }
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

    /// Field report #7, **re-measured 2026-11-14**. This test used to insist that
    /// a rule34/gelbooru CDN link must route through the site API ("that is where
    /// the thumbnail 403 was coming from"). That premise was a guess, and it is
    /// backwards: the API answers `403` + `cf-mitigated: challenge` to every
    /// identity this app has, while the CDN answers `200 image/jpeg`. A link that
    /// names a file is fetched as a file now; the post id in the query is still
    /// recovered, because the report and the fallback note name the post with it.
    #[test]
    fn booru_cdn_file_links_keep_their_post_id_but_fetch_as_files() {
        let s = classify(
            "https://wimg.rule34.xxx//images/3868/7412ed67862b691cceef71b918028f66.jpeg?11353035",
        )
        .unwrap();
        assert_eq!(s.site, "rule34");
        assert_eq!(s.mode, Mode::DirectMedia);
        assert_eq!(s.remote_id.as_deref(), Some("11353035"));

        let s = classify("https://img3.gelbooru.com//images/2b/34/2b34cd.jpg?4765984").unwrap();
        assert_eq!(s.site, "gelbooru");
        assert_eq!(s.mode, Mode::DirectMedia);
        assert_eq!(s.remote_id.as_deref(), Some("4765984"));

        // hypnohub has no API wired → stays a direct file, but named correctly
        let s = classify(
            "https://hypnohub.net//images/6c/e0/6ce01fd2de7cc88d217ac34e44f7601a.gif?267940",
        )
        .unwrap();
        assert_eq!(s.site, "hypnohub");
        assert_eq!(s.mode, Mode::DirectMedia);

        // e621's file CDN (md5 query, not a post page) must fetch directly
        let s = classify("https://static1x1.e621.net/data/f1/2e/f12e32ab.jpg?a1b2c3").unwrap();
        assert_eq!(s.site, "e621");
        assert_eq!(s.mode, Mode::DirectMedia);
    }

    /// Field report #39: booru file links with *no* post id used to dead-end
    /// with "no numeric post id in that url" and store nothing at all. They
    /// must fall through to the direct-file path so the image lands in the
    /// vault even when no post can be identified.
    #[test]
    fn idless_booru_file_links_still_store_the_image() {
        let s = classify("https://img3.gelbooru.com/images/2b/34/2b34cd76ef.jpg").unwrap();
        assert_eq!(
            (s.site.as_str(), s.mode, s.remote_id.as_deref()),
            ("gelbooru", Mode::DirectMedia, None)
        );

        let s =
            classify("https://wimg.rule34.xxx/images/3868/7412ed67862b691cceef71b918028f66.jpeg")
                .unwrap();
        assert_eq!(
            (s.site.as_str(), s.mode, s.remote_id.as_deref()),
            ("rule34", Mode::DirectMedia, None)
        );

        // a post page URL that carries its id in the path still resolves as a post
        let s = classify("https://rule34.xxx/index.php?page=post&s=view&id=11353035").unwrap();
        assert_eq!(
            (s.site.as_str(), s.mode, s.remote_id.as_deref()),
            ("rule34", Mode::BooruId, Some("11353035"))
        );
        let s = classify("https://rule34.xxx/post/show/11353035").unwrap();
        assert_eq!(
            (s.site.as_str(), s.mode, s.remote_id.as_deref()),
            ("rule34", Mode::BooruId, Some("11353035"))
        );
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

    /// #50, the part that was actually broken. The app used to send every rule34
    /// file link through the site's metadata api because that also carried tags —
    /// and the api is a bot wall now (measured 2026-11-14: `403 text/html` plus
    /// `cf-mitigated: challenge` for the honest UA, a browser UA and no UA at all,
    /// while the same post's CDN file answered `200 image/jpeg 678 634 B` to all
    /// three). A link that names a file must therefore be fetched as a file.
    #[test]
    fn a_booru_file_link_is_fetched_as_a_file_not_through_the_api() {
        let s = classify("https://wimg.rule34.xxx//images/3043/a6cf0e6d194728b189303b9da8c904e6.jpeg?3411909")
            .expect("rule34 file link");
        assert_eq!(
            (s.site.as_str(), s.mode),
            ("rule34", Mode::DirectMedia),
            "the api is a bot wall; the file host is not"
        );
        assert_eq!(
            s.remote_id.as_deref(),
            Some("3411909"),
            "the post id is kept, so the report can name the post"
        );
        // same rule for gelbooru, and the doubled slash the bookmark carries is
        // normalised later, in direct_media, not here
        let s = classify("https://img3.gelbooru.com//images/f2/1e/f21e0ce1d9ff33ba2b7fdc75f01e6a3d.jpeg?11353035")
            .expect("gelbooru file link");
        assert_eq!((s.site.as_str(), s.mode), ("gelbooru", Mode::DirectMedia));
        // a post *page* still goes to the api, where the fallback note explains
        // what could not be read
        let s = classify("https://rule34.xxx/index.php?page=post&s=view&id=3411909").unwrap();
        assert_eq!((s.site.as_str(), s.mode), ("rule34", Mode::BooruId));
        // e621 is the one booru whose api answers this app (measured `200
        // application/json`, and its file host `200 image/png`), so its post
        // pages still take the api path and keep their tags
        let s = classify("https://e621.net/posts/4914601").unwrap();
        assert_eq!((s.site.as_str(), s.mode), ("e621", Mode::BooruId));
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
        assert!(hits.iter().any(
            |h| h.url.ends_with("RJ01731048.html") && h.title == "Summer Witch Remake [同人誌]"
        ));
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
        assert_eq!(
            hits[0].url,
            "https://f95zone.to/threads/summer-witch.10599/"
        );
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
        assert_eq!(
            p.display().to_string(),
            "C:/Users/Admin/Videos/EnomeSnake.mp4"
        );
        let p = local_path("//nas/share/clip.mp4").unwrap();
        assert_eq!(p.display().to_string(), r"\\nas\share\clip.mp4");
        assert!(local_path("/").is_err());
    }

    #[test]
    fn eh_gate_pages_are_recognised() {
        let gate = r##"<html><body><div class="gtable ggate">You must <a href="#">IP Login</a> to continue</div></body></html>"##;
        assert!(eh_is_gate(gate));
        assert!(!eh_is_gate("<html><body><h1 id=\"gn\">Real gallery</h1></body></html>"));

        // The shape #46 was written against (`gdtm` + `<img src>` per thumbnail)
        // no longer exists on the site, and the parser must not invent a cover
        // for it: an empty result is what leads to an honest note.
        let legacy = r#"<html><head><title>Kanojo no Kaomo | e-hentai.org</title></head>
        <body><div id="gdt"><div class="gdtm" style="..."><div class="g2"></div>
        <a href="https://e-hentai.org/s/abc/1234-1"><img src="https://ex.ph/cover1.jpg"></a></div></div>
        <div class="gld">This gallery has <em>24</em> pages.</div></body></html>"#;
        let g = parse_eh_gallery(legacy);
        assert!(g.cover.is_none(), "no sprite box means no cover, not a guess");
        assert_eq!(g.pages, None);
        // a title is still a title, and that must be reported as partial success
        assert_eq!(g.title.as_deref(), Some("Kanojo no Kaomo | e-hentai.org"));
        assert_eq!(g.slides, vec!["https://e-hentai.org/s/abc/1234-1".to_string()]);
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

    // ---- #46: f95zone post art, e-hentai slideshow links, sniffed types ----

    /// The exact XenForo lazy-load shape from the field report (a member page),
    /// plus everything that must NOT become a cover: avatar, badge, smiley, ad
    /// creative and the `og:image` site logo.
    const F95_MEMBER_PAGE: &str = r##"<html><head>
        <meta property="og:title" content="Lewd Customs in the Tsugai Mating Village 2 Final [v1.0] [renpy] [Completed] | f95zone">
        <meta property="og:image" content="https://f95zone.to/assets/images/logo-sqw192.png">
        </head><body>
        <div class="message-userAvatar"><img src="https://f95zone.to/data/avatars/m/112/1121990.jpg" class="board-avatar-image" alt=""></div>
        <img class="smilie smilie--smile" src="https://f95zone.to/data/smililies/custom/smiley4.png" alt=":o">
        <img src="data:image/svg+xml;charset=utf-8,%3Csvg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 560 420'%3E%3Crect width='560' height='420' fill='%2323262e'/%3E%3C/svg%3E"
             data-src="https://attachments.f95zone.to/2026-11-6671101_d_642380pr.jpg"
             data-url="https://attachments.f95zone.to/2026-11-6671101_d_642380pr.jpg"
             class="bbImage lazyload" data-zoom-target="1" alt="preview">
        <img src="data:image/svg+xml;charset=utf-8,%3Csvg%3E%3C/svg%3E" class="bbImage lazyload" data-src="https://attachments.f95zone.to/2026-11-6671101_d_642380pr.jpg">
        <img src="//attachments.f95zone.to/2026-11-6671101_second.png" class="bbImage lazyload">
        <img src="https://attachments.f95zone.to/2026-11-6671101_third.jpg?sig=1&amp;t=2" class="bbImage lazyload">
        <img src="https://f95zone.to/data/badges/2022/wholesome.png" class="message-userInfo-badge">
        <div class="samAd"><img src="https://ads.example.net/banner.gif" data-src="https://ads.example.net/banner.gif"></div>
        </body></html>"##;

    /// Measured guest fetch of thread 321420 (286 128 B, zero attachment URLs):
    /// the post body is replaced by XenForo `messageHide` divs.
    const F95_GUEST_PAGE: &str = r##"<html><head>
        <meta property="og:title" content="Lewd Customs in the Tsugai Mating Village 2 Final [v1.0] [renpy] | f95zone">
        <meta property="og:image" content="https://f95zone.to/assets/images/logo-sqw192.png">
        </head><body>
        <div class="bbWrapper"><div style="text-align: center"><div class="messageHide messageHide--attach"><a href="/login/" data-xf-click="overlay" data-follow-redirects="on" rel="nofollow">Log in or register to see attachments</a></div><br /> <b>Overview</b>:<br /></div>
        <div class="messageHide messageHide--link"><a href="/login/" rel="nofollow">Log in or register to see links</a></div>
        <img src="https://f95zone.to/assets/images/logo-sqw192.png" class="message-avatar">
        </body></html>"##;

    #[test]
    fn f95_post_images_come_from_lazy_data_src() {
        let imgs = parse_f95_post_images(F95_MEMBER_PAGE);
        assert_eq!(
            imgs,
            vec![
                "https://attachments.f95zone.to/2026-11-6671101_d_642380pr.jpg",
                "https://attachments.f95zone.to/2026-11-6671101_second.png",
                "https://attachments.f95zone.to/2026-11-6671101_third.jpg?sig=1&t=2",
            ],
            "the svg placeholder must not win over data-src, dupes/ad/ badge hosts must drop"
        );
        assert!(!imgs.iter().any(|u| u.contains("logo")));
        assert!(!imgs.iter().any(|u| u.contains("smiley")));
        assert!(!imgs.iter().any(|u| u.contains("avatars")));
        assert!(!f95_login_wall(F95_MEMBER_PAGE));
        assert_eq!(
            f95_art(F95_MEMBER_PAGE, Some("xf_cookies=x"), &imgs),
            F95Art::Art(imgs.clone())
        );
    }

    #[test]
    fn f95_guest_page_reads_as_a_login_wall_not_as_no_art() {
        let imgs = parse_f95_post_images(F95_GUEST_PAGE);
        assert!(
            imgs.is_empty(),
            "a guest page must not invent art: {imgs:?}"
        );
        assert!(f95_login_wall(F95_GUEST_PAGE));
        assert_eq!(f95_art(F95_GUEST_PAGE, None, &imgs), F95Art::NeedsLogin);
        // wall gone, cookie sent, page genuinely has no post images → other verdict
        assert_eq!(
            f95_art(
                "<html><body><div class=\"bbWrapper\">no images here</div></body></html>",
                Some("c=1"),
                &imgs
            ),
            F95Art::NoArt
        );
        // title/tags do come through even for a guest — that is not a failure
        let (raw, tags) = parse_f95_thread(F95_GUEST_PAGE);
        assert!(raw.as_deref().unwrap().starts_with("Lewd Customs"));
        assert!(tags.contains(&"renpy".to_string()));
    }

    /// #46, the 2026 shape. A slideshow link is **not** folded onto
    /// `https://host/g/<gid>/` any more: measured, that url is a bare apache 404
    /// (236 B), which is exactly the `http 404 Not Found` the user hit. The
    /// slideshow page names its gallery with the key the gallery url needs.
    #[test]
    fn eh_slideshow_links_are_resolved_from_the_page_not_a_guess() {
        let shell = r##"<html><head><title>[horosuke] Asuka Dairy</title><script>
            var gid=4089666;var startpage=1;var startkey="3c7a2c86a5";var showkey="3w32t1laoni";
            </script></head><body><div id="i3"><a onclick="return load_image(2, '24e4797087')"
            href="https://e-hentai.org/s/24e4797087/4089666-2"><img id="img" src="https://fivpjap.ezrzluxzxslb.hath.network/h/3c7a2c86a58d-177497-1200-630-jpg/keystamp=1794642300-f03a00b680;fileindex=254604586;xres=org/12349451_0.jpg"
            style="width:1200px;height:630px" /></a></div>
            <div>12349451_0.jpg :: 1200 x 630 :: 173.3 KiB</div>
            <div><span>1</span> / <span>20</span></div>
            <div id="i5"><a href="https://e-hentai.org/g/4089666/b9d914bef4/"></a></div></body></html>"##;
        assert_eq!(
            eh_gallery_link(shell, "https://e-hentai.org").as_deref(),
            Some("https://e-hentai.org/g/4089666/b9d914bef4/")
        );
        let show = parse_eh_show(shell);
        assert_eq!(show.title.as_deref(), Some("[horosuke] Asuka Dairy"));
        assert_eq!(show.page, Some(1));
        assert_eq!(show.pages, Some(20));
        assert!(
            show.image
                .as_deref()
                .unwrap()
                .contains("keystamp=1794642300-f03a00b680"),
            "the art url is per-request: it is fetched, never stored as a candidate"
        );

        // the gid is still what the item records, and both link kinds are eh
        let s = classify("https://e-hentai.org/s/fd0163b571/4089666-7").unwrap();
        assert_eq!(
            (s.site.as_str(), s.mode, s.remote_id.as_deref()),
            ("ehentai", Mode::Ehentai, Some("4089666"))
        );
        assert!(eh_is_show("https://e-hentai.org/s/fd0163b571/4089666-7"));
        assert!(!eh_is_show("https://e-hentai.org/g/4089666/b9d914bef4/"));

        // #46 as reported: the gallery key missing from the url is an error the
        // site states in words, and it must not read as a gallery with no art.
        assert!(eh_is_gate("<html><head><title>Key Missing</title></head><body>Key missing, or incorrect key provided.</body></html>"));
    }

    /// #46, the 2026 gallery page: title, tags, page count and the sprite boxes
    /// are all markup this app has never read before.
    #[test]
    fn eh_gallery_page_reads_the_2026_shape() {
        // Trimmed from the real page for gid 4089666 (saved live). What stays is
        // what the parser depends on: `id="gn"`, `id="ta_"`, `Length:`, and the
        // `background:transparent url(<sheet>) -Xpx 0` boxes.
        let page = r##"<html><head><title>[horosuke] Asuka Dairy - E-Hentai Galleries</title></head>
        <body><div class="gm"><div id="gleft"><div id="gd1"><div style="width:250px; height:131px;
        background:transparent url(https://ehgt.org/w/02/546/04586-20dsbt2q.webp) 0 0 no-repeat"></div></div></div>
        <div id="gd2"><h1 id="gn">[horosuke] Asuka Dairy</h1><h1 id="gj"></h1></div>
        <a id="ta_artist:horosuke" href="https://e-hentai.org/tag/artist:horosuke">horosuke</a>
        <a id="ta_female:big_breasts" href="https://e-hentai.org/tag/female:big+breasts">big breasts</a>
        <td class="gdt1">Length:</td><td class="gdt2">20 pages</td>
        <div id="gdt" class="gt200">
        <a href="https://e-hentai.org/s/3c7a2c86a5/4089666-1"><div title="Page 1: 12349451_0.jpg"
          style="width:200px;height:105px;background:transparent url(https://puzaqhxrbn.hath.network/c2/du3afh26zz2jhx1a05/4089666-0.webp) -0px 0 no-repeat"></div></a>
        <a href="https://e-hentai.org/s/24e4797087/4089666-2"><div title="Page 2: 12349451_1.jpg"
          style="width:200px;height:141px;background:transparent url(https://puzaqhxrbn.hath.network/c2/du3afh26zz2jhx1a05/4089666-0.webp) -200px 0 no-repeat"></div></a>
        </div></body></html>"##;
        let g = parse_eh_gallery(page);
        assert_eq!(g.title.as_deref(), Some("[horosuke] Asuka Dairy"));
        assert_eq!(g.pages, Some(20));
        assert_eq!(
            g.tags,
            vec!["artist:horosuke".to_string(), "female:big_breasts".to_string()]
        );
        assert_eq!(
            g.slides,
            vec![
                "https://e-hentai.org/s/3c7a2c86a5/4089666-1".to_string(),
                "https://e-hentai.org/s/24e4797087/4089666-2".to_string()
            ],
            "every page of the gallery becomes one candidate, not a download"
        );
        // The cover is the box in `gd1`, and the box is a position *inside a
        // sheet*: what has to be stored is a crop, not the sheet url.
        let cover = g.cover.expect("the cover box must be found");
        assert_eq!(cover.sheet, "https://ehgt.org/w/02/546/04586-20dsbt2q.webp");
        assert_eq!((cover.x, cover.y, cover.w, cover.h), (0, 0, 250, 131));

        // and the grid tiles: the page's own offsets say where each one sits
        let t0 = eh_tile(r#"width:200px;height:105px;background:transparent url(https://x.hath.network/c2/a/4089666-0.webp) -0px 0 no-repeat"#).unwrap();
        assert_eq!((t0.x, t0.y, t0.w, t0.h), (0, 0, 200, 105));
        let t7 = eh_tile(r#"width:200px;height:141px;background:transparent url(https://x.hath.network/c2/a/4089666-0.webp) -1200px 0 no-repeat"#).unwrap();
        assert_eq!((t7.x, t7.y, t7.w, t7.h), (1200, 0, 200, 141));
        // a banner box with no sheet in it is not a picture
        assert!(eh_tile("width:720px;height:80px;margin:0 auto").is_none());
    }

    /// #46/#50: `attachments.f95zone.to/…_642380pr.jpg` answers `image/avif`.
    /// What gets stored and what goes to the thumbnailer follows the bytes.
    #[test]
    fn sniffed_bytes_not_the_url_decide_what_is_stored() {
        let mut avif = vec![0u8, 0, 0, 0x1c];
        avif.extend_from_slice(b"ftypavif");
        avif.extend_from_slice(&[7u8; 40]);
        let mut m = Meta::default();
        assert_eq!(store_bytes_in(&mut m, avif.clone()), Some("image/avif"));
        assert_eq!(m.store_bytes.as_deref(), Some(avif.as_slice()));
        assert!(m.thumb_bytes.is_some());
        assert!(m.note.is_none());

        // a hotlink block page served at HTTP 200 is not art
        let mut blocked = Meta::default();
        assert_eq!(
            store_bytes_in(
                &mut blocked,
                b"<html><body>Access denied</body></html>".to_vec()
            ),
            None
        );
        assert!(blocked.store_bytes.is_none() && blocked.thumb_bytes.is_none());
        assert!(blocked.note.unwrap().contains("not a recognizable"));

        // `.jpg` link carrying an mp4 body → link, not stored art
        let mut mp4 = vec![0u8, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(&[9u8; 24]);
        let mut mov = Meta::default();
        assert_eq!(store_bytes_in(&mut mov, mp4), None);
        assert!(mov.note.as_deref().unwrap().contains("video/mp4"));

        // Two kinds of "no thumbnail", and the note must tell them apart (#52).
        // These synthetic `ftypavif` bytes are not a decodable AVIF, so this is
        // a failed decode, not an unsupported format: the wording may not claim
        // the build has no AVIF decoder, because dav1d is linked in.
        let mut avif_meta = Meta::default();
        store_bytes_in(&mut avif_meta, avif.clone());
        assert!(media::make_thumbnail(avif_meta.thumb_bytes.as_ref().unwrap()).is_none());
        let note = undecodable_note(&avif_meta, false).expect("must stay visible");
        assert!(note.contains("image/avif"), "{note}");
        assert!(!note.contains("cannot decode"), "{note}");
        assert!(undecodable_note(&avif_meta, true).is_none());

        // HEIF has no decoder in this build (image 0.25.10 reads AVIF containers
        // only), and that stays stated as the missing capability it is.
        let mut heif = vec![0u8, 0, 0, 0x18];
        heif.extend_from_slice(b"ftypheic");
        heif.extend_from_slice(&[7u8; 24]);
        let mut heif_meta = Meta::default();
        assert_eq!(store_bytes_in(&mut heif_meta, heif), Some("image/heif"));
        assert!(!media::thumbnailable("image/heif"));
        let note = undecodable_note(&heif_meta, false).expect("must stay visible");
        assert!(note.contains("cannot decode"), "{note}");
        assert!(note.contains("image/heif"), "{note}");
    }

    /// #54 follow-up of #52, offline: a thumbnailable cover must keep **both**
    /// the original and the tile bytes. The live rehearsal below proves it with
    /// real f95 bytes; this pins the rule so it cannot quietly regress again.
    #[test]
    fn f95_covers_keep_the_original_and_the_tile() {
        const AVIF: &[u8] =
            include_bytes!("../../../../crates/goon-core/src/fixtures/avif-tiny.avif");
        let mut m = Meta::default();
        keep_f95_cover(&mut m, AVIF.to_vec(), 15).expect("a real AVIF is a keepable cover");
        assert_eq!(
            m.store_bytes.as_deref(),
            Some(AVIF),
            "the original must reach the vault, not just the tile"
        );
        assert!(m.thumb_bytes.is_some(), "and the tile bytes too");
        let note = m.note.clone().unwrap();
        assert!(note.contains("image/avif"), "{note}");
        assert!(note.contains("15 attachment image(s)"), "{note}");
        assert!(
            note.contains("listed under Sources"),
            "#53: the other attachments must be named as storable candidates, not as dead \
             links: {note}"
        );
        assert!(
            !note.contains("cannot"),
            "a thumbnailable format must not be described as a missing capability: {note}"
        );

        // HEIF: no decoder in this build, so the tile is missing - but the bytes
        // are still kept, and the note says which of the two facts it is.
        let mut heif = vec![0u8, 0, 0, 0x18];
        heif.extend_from_slice(b"ftypheic");
        heif.extend_from_slice(&[7u8; 24]);
        let mut m2 = Meta::default();
        keep_f95_cover(&mut m2, heif.clone(), 3).expect("HEIF is still a keepable cover");
        assert_eq!(m2.store_bytes.as_deref(), Some(heif.as_slice()));
        let note = m2.note.unwrap();
        assert!(
            note.contains("image/heif") && note.contains("cannot"),
            "{note}"
        );

        // A hotlink block page is not a cover: rejected, so the loop tries the
        // next candidate and the reason still reaches the report.
        let mut m3 = Meta::default();
        let why = keep_f95_cover(&mut m3, b"<html>Access denied</html>".to_vec(), 1)
            .expect_err("html is not a cover");
        assert!(why.contains("recognizable"), "{why}");
        assert!(
            m3.store_bytes.is_none() && m3.thumb_bytes.is_none(),
            "nothing may be stored from a rejected candidate"
        );
    }

    /// #53, offline: the store-one-picture rule decides from the **bytes**, never
    /// from the url, under the same ceiling a cover gets. This is the whole safety
    /// story of the feature: a picture is fetched only because the user picked it,
    /// and what arrives is still type-checked before it can enter the vault.
    #[test]
    fn a_chosen_picture_is_sniffed_and_capped() {
        const AVIF: &[u8] =
            include_bytes!("../../../../crates/goon-core/src/fixtures/avif-tiny.avif");
        assert_eq!(
            storable_picture(AVIF).expect("a real avif is a picture"),
            "image/avif",
            "the type comes from the bytes, not the url"
        );

        // f95zone answers a blocked hotlink with an HTML body at status 200.
        // Storing that is how a vault fills with error pages.
        let why = storable_picture(b"<html>Access denied</html>".as_slice())
            .expect_err("an html body is not a picture");
        assert!(why.contains("recognizable"), "{why}");

        // A `.jpg` link carrying video (the #50 shape) is refused, in words that
        // say what it is.
        let mut mp4 = vec![0u8, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(&[9u8; 24]);
        let why = storable_picture(&mp4).expect_err("a video link is not a picture");
        assert!(why.contains("video/mp4"), "{why}");

        let mut big = b"\x89PNG\r\n\x1a\n".to_vec();
        big.resize(MAX_COVER_BYTES + 1, b'x');
        let why = storable_picture(&big).expect_err("the ceiling stays the cover ceiling");
        assert!(why.contains("picture ceiling"), "{why}");
    }

    /// #53: an item with no source row still gets a named polite lane - the host
    /// the picture came from - so a second click on the same host waits for it.
    #[test]
    fn a_picture_lane_is_named_by_host() {
        assert_eq!(
            host_lane("https://attachments.f95zone.to/2024/05/tiny.avif"),
            "attachments.f95zone.to"
        );
        assert_eq!(host_lane("not a url"), "not a url");
    }

    /// #53, offline: the candidate list survives the fetch, dedups by url and
    /// keeps page order - the three things the Sources section depends on. Run
    /// against a real scratch vault so the unique index and the FK cascade are the
    /// ones the app actually uses.
    #[test]
    fn candidate_pictures_are_kept_once_each_in_page_order() {
        let (core, tmp) = crate::scratch_core("cands", true);
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            let now = now_epoch();
            let item = Item {
                id: "c-cands-1".into(),
                kind: ItemKind::Game,
                title: "candidate probe".into(),
                original_title: None,
                circle: None,
                status: Status::Seen,
                dnf: false,
                rating: None,
                notes: String::new(),
                version: None,
                created_at: now.clone(),
                updated_at: now,
            };
            db::insert_item(conn, &item).unwrap();

            let urls: Vec<String> = ["https://x/a.jpg", "https://x/b.jpg", "https://x/a.jpg"]
                .iter()
                .map(|s| s.to_string())
                .collect();
            assert_eq!(
                remember_candidates(conn, "c-cands-1", &urls),
                2,
                "the repeat url must not become a second row"
            );
            assert_eq!(
                remember_candidates(conn, "c-cands-1", &urls),
                0,
                "remembering the same page twice adds nothing"
            );
            let rows = db::source_images_for(conn, "c-cands-1");
            assert_eq!(rows.len(), 2, "{rows:?}");
            assert_eq!(rows[0].url, "https://x/a.jpg", "page order, not sorted");
            assert!(
                rows.iter().all(|r| r.stored_at.is_none()),
                "a candidate is not yet a stored picture"
            );
            assert_eq!(
                remember_candidates(conn, "c-cands-1", &[]),
                0,
                "a page that offered nothing writes nothing"
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn dlsite_product_parse() {
        let page = r#"<html><head>
        <meta property="og:title" content="[RJ123456] Summer Witch [PC版]">
        <meta property="og:image" content="https://img.dlsite.jp/works/RJ123456.jpg">
        </head><body><span itemprop="dateUpdated" content="2026-03-01T12:00:00+09:00"></span></body></html>"#;
        let (title, cover, updated) = parse_dl_product(page);
        assert_eq!(title.as_deref(), Some("Summer Witch [PC版]"));
        assert_eq!(
            cover.as_deref(),
            Some("https://img.dlsite.jp/works/RJ123456.jpg")
        );
        assert_eq!(updated.as_deref(), Some("2026-03-01"));
    }

    #[test]
    fn mirrors_classify_and_default_off() {
        let s = classify("https://kimochi.net/fgo-lilith/").unwrap();
        assert_eq!(s.site, "kimochi");
        assert!(is_mirror_site("kimochi"));
    }

    /// #56: the guide says f95 search runs only with the saved cookie, so the
    /// cookie has to stop the *request*, not just colour the error afterwards. An
    /// anonymous query to a login-gated search endpoint is traffic the user never
    /// asked for. This test needs no network because the guard returns before any
    /// client is used — remove the guard and it starts hitting f95zone.to.
    #[tokio::test]
    async fn f95_search_sends_nothing_without_a_cookie() {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let e = f95_search(&client, "anything at all", None)
            .await
            .expect_err("no cookie means no search");
        assert!(e.contains("wants a login"), "{e}");
    }

    /// #57: a clip is the most frequent thing this app does to a site, and until now
    /// it fetched immediately while every *job* politely queued. These three checks
    /// use `konachan` deliberately: a known site with no metadata API wired, so the
    /// fetch path answers from the code ("no metadata API wired for konachan yet")
    /// and the test proves lane behaviour without sending a single request.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clip_waits_its_sites_lane() {
        let (core, tmp) = crate::scratch_core("lane", true);
        let url = "https://konachan.com/post/show/1".to_string();
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked").conn;
            db::set_site_setting(conn, "konachan", Some(true), None, Some(900), 900).unwrap();
        }

        // First clip: the lane is empty, so it costs nothing.
        let at = std::time::Instant::now();
        let first = clip(&core, url.clone()).await.expect("clip 1");
        let fast = at.elapsed();
        println!("clip 1 to a quiet lane: {fast:?}");
        assert!(first.created, "the link must still be stored");

        // Second clip, different link on the same site: it must wait the site's
        // 900 ms rather than fetch now. (The *same* link returns earlier, from the
        // dedup lookup, and correctly costs no lane slot.)
        let at = std::time::Instant::now();
        let second = clip(&core, "https://konachan.com/post/show/2".to_string())
            .await
            .expect("clip 2");
        let waited = at.elapsed();
        println!("clip 2 behind a 900 ms lane: {waited:?} (clip 1 cost {fast:?})");
        assert!(second.created, "a different link is a new item");
        // The lane opens when the first clip starts its work, so the second clip
        // waits the lane minus whatever the first clip itself cost. A flat 800 ms
        // floor failed on a busy machine (clip 1 measured 152 ms, the lane was
        // fine) — the quantity under test is "the gap between the two lane
        // slots is the lane", so compare against that, with clock slack.
        let floor = 900u128.saturating_sub(fast.as_millis() + 150);
        assert!(
            waited.as_millis() >= floor,
            "the clip fetched {waited:?} after the last one, but konachan's lane is 900 ms \
             (floor {floor} ms)"
        );
        assert!(
            fast.as_millis() < 800,
            "the first clip to a quiet lane must not wait: {fast:?}"
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// #57, the other half: Lock (or Nuke) while a clip is waiting in a lane must
    /// give the clip up instead of fetching into a closed vault. The guide promises
    /// a Lock stops running work; a clip that skipped the lane skipped that too.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn clip_gives_up_when_the_vault_locks_while_it_waits() {
        let (core, tmp) = crate::scratch_core("lockwait", true);
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked").conn;
            db::set_site_setting(conn, "konachan", Some(true), None, Some(1200), 1200).unwrap();
        }
        // Occupy the lane, then lock the vault underneath the waiting clip.
        core.queue.reserve("konachan", 1200, || false).await;
        let closer = {
            let core2 = core.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                // Exactly what lock_core does to the shared state.
                core2.inner.lock().unwrap().opened = None;
            })
        };
        let e = clip(&core, "https://konachan.com/post/show/2".to_string())
            .await
            .expect_err("a clip may not fetch into a locked vault");
        assert!(e.contains("locked before"), "{e}");
        closer.await.unwrap();
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Manual smoke against the live sites (rate-limits ignored, run by hand):
    /// `cargo test -p goonflow-desktop --lib -- --ignored --nocapture match_live`
    #[tokio::test]
    #[ignore]
    async fn match_live_search_smoke() {
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .unwrap();
        let hits = dlsite_search(&client, "karryn")
            .await
            .expect("dlsite search");
        assert!(!hits.is_empty(), "dlsite search returned nothing");
        assert!(hits.iter().all(|h| h.url.contains("/product_id/")));
        println!("dlsite hits: {}", hits.len());
        for h in hits.iter().take(3) {
            println!("  {} | {}", h.url, h.title);
        }
        // #56: no cookie means no request at all — nothing leaves the machine.
        let r = f95_search(&client, "spring seed", None).await;
        println!(
            "f95 no-cookie result: {}",
            r.err().unwrap_or_else(|| "hits without a cookie?!".into())
        );
        // With the cookie it is a real capability, so prove that half too instead of
        // only ever testing the refusal.
        match std::env::var("GOON_F95_COOKIE") {
            Ok(c) => {
                let hits = f95_search(&client, "spring seed", Some(c.trim()))
                    .await
                    .expect("f95 search with the saved cookie");
                println!("f95 hits with cookie: {}", hits.len());
                for h in hits.iter().take(3) {
                    println!("  {} | {}", h.url, h.title);
                }
            }
            Err(_) => {
                println!("GOON_F95_COOKIE not set: f95 search *with* a cookie was not exercised")
            }
        }
    }

    /// The #46 probe, kept runnable instead of re-invented: this bug was
    /// invisible offline, because a guest page parses perfectly and simply has
    /// no art. The cookie comes from the environment; nothing here is committed
    /// as a fixture and no cookie is ever stored in the repo.
    ///
    ///   $env:GOON_F95_COOKIE = (Get-Content "$env:USERPROFILE\Downloads\f95cookie.txt" -Raw)
    ///   cargo test -p goonflow-desktop --lib -- --ignored --nocapture f95_live_art_probe
    #[tokio::test]
    #[ignore]
    async fn f95_live_art_probe() {
        let Ok(cookie) = std::env::var("GOON_F95_COOKIE") else {
            eprintln!("GOON_F95_COOKIE not set - skipping (which is exactly the bug)");
            return;
        };
        let cookie = cookie.trim();
        let url = "https://f95zone.to/threads/lewd-customs-in-the-tsugai-mating-village-2-final-itsumoeswaninatteorimasu.321420/";
        // page fetch: the honest UA. The attachment fetch inside get_bytes
        // overrides it with the browser UA, which is the #50 half.
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap();
        let meta = f95_thread(
            &client,
            url,
            &FetchCtx {
                cookie: Some(cookie),
                core: None,
            },
            true,
        )
        .await
        .expect("f95 thread with a cookie");
        println!("title   = {:?}", meta.title);
        println!("version = {:?}", meta.version);
        println!("tags    = {}", meta.tags.len());
        println!("images  = {}", meta.images.len());
        println!("note    = {:?}", meta.note);
        println!(
            "thumb   = {} bytes",
            meta.thumb_bytes.as_ref().map_or(0, |b| b.len())
        );
        assert!(
            !meta.images.is_empty(),
            "a logged-in f95 thread must expose lazy attachment images"
        );
        // the same page as a guest, to show what the cookie is worth
        let guest = f95_thread(
            &client,
            url,
            &FetchCtx {
                cookie: None,
                core: None,
            },
            true,
        )
        .await
        .unwrap_or_else(|e| Meta {
            title: None,
            store_bytes: None,
            thumb_bytes: None,
            images: Vec::new(),
            tags: Vec::new(),
            version: None,
            note: Some(e),
        });
        println!(
            "guest   = images {}, note {:?}",
            guest.images.len(),
            guest.note
        );
        if let Some(first) = meta.images.first() {
            let bytes = get_bytes(&client, first, "https://f95zone.to", Some(cookie))
                .await
                .expect("attachment bytes");
            println!(
                "first attachment = {} bytes, sniffed {:?}",
                bytes.len(),
                goon_core::media::detect_mime(&bytes)
            );
        }
    }

    /// The same thread, but through the real pipeline: cookie in `site_settings`,
    /// `clip()` doing the fetch, and then what the **vault** actually holds. The
    /// probe above proves the parser; #52 was proven offline against a 680-byte
    /// fixture, and manual TESTING item 21 ("clip a new f95 thread, the tile is
    /// there") is exactly this flow — so here it is as a machine check with live
    /// bytes: stored cover + a real thumbnail row + the grid reporting `has_thumb`.
    ///
    ///   $env:GOON_F95_COOKIE = (Get-Content "$env:USERPROFILE\Downloads\f95cookie.txt" -Raw).Trim()
    ///   powershell -ExecutionPolicy Bypass -Command ". tools\dev-env.ps1; cargo test -p goonflow-desktop --lib -- --ignored --nocapture f95_live_clip_rehearsal"
    #[tokio::test]
    #[ignore]
    async fn f95_live_clip_rehearsal() {
        let Ok(cookie) = std::env::var("GOON_F95_COOKIE") else {
            panic!(
                "GOON_F95_COOKIE must be set — silently skipping this is how #46 hid for a release"
            );
        };
        let cookie = cookie.trim().to_string();
        let url = "https://f95zone.to/threads/lewd-customs-in-the-tsugai-mating-village-2-final-itsumoeswaninatteorimasu.321420/";

        let (core, tmp) = crate::scratch_core("f95clip", true);
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            db::set_site_setting(
                conn,
                "f95",
                Some(true),
                Some(&cookie),
                None,
                default_rate_ms("f95"),
            )
            .unwrap();
        }
        let report = clip(&core, url.to_string())
            .await
            .expect("clip an f95 thread with a cookie");
        println!(
            "clip     = created {} stored_image {} thumb {} tags {} images_found {}",
            report.created, report.stored_image, report.thumb, report.tags, report.images_found
        );
        println!("clip     = note {:?}", report.note);
        assert!(report.created, "the thread must become an item");
        assert!(report.stored_image, "the cover must be stored in the vault");
        assert!(
            report.thumb,
            "the cover must decode into a thumbnail (#52 with live bytes)"
        );
        assert!(
            report.images_found >= 10,
            "the page's attachments must be reported, not dropped"
        );

        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            let full = media::best_for(conn, &report.item_id).expect("stored cover");
            println!(
                "vault    = full {} bytes mime {} (sniffed {:?})",
                full.data.len(),
                full.mime,
                media::detect_mime(&full.data)
            );
            // Pinned to what f95 serves today (AVIF behind a `.jpg` URL, #46/#50).
            // If this ever fails because the CDN changed, the expectation is what
            // needs updating — and the `thumbnailable` rule below is the real test.
            assert_eq!(
                full.mime, "image/avif",
                "the row must say what the bytes are"
            );
            assert_eq!(
                media::detect_mime(&full.data),
                Some(full.mime.as_str()),
                "the stored mime must match the stored bytes"
            );
            assert!(media::thumbnailable(&full.mime), "this build can decode it");
            let (tmime, tbytes) = media::get_thumb(conn, &report.item_id).expect("thumbnail row");
            println!("vault    = thumb {} bytes mime {}", tbytes.len(), tmime);
            assert_eq!(
                tmime, "image/jpeg",
                "dav1d decoded it, image wrote the tile"
            );
            assert!(
                tbytes.len() < full.data.len(),
                "a tile must be smaller than what it came from"
            );
            // Which grid it lands in is part of the claim: an f95 **thread** is a
            // game, so it belongs to the Games tab (`kind = 'game'`), not Media.
            // Getting this wrong in a test is how a tile "disappears" in a report.
            let games = db::list_items(
                conn,
                &db::ListQuery {
                    limit: 300,
                    kind: Some("game"),
                    site: Some("f95"),
                    ..Default::default()
                },
            )
            .unwrap();
            let me = games.iter().find(|i| i.id == report.item_id);
            assert!(me.is_some(), "the games grid must list this item");
            assert!(me.unwrap().has_thumb, "and the card must show a tile");
            println!(
                "grid     = {} game row(s) for site f95, this one has_thumb",
                games.len()
            );
            let media_rows = db::list_items(
                conn,
                &db::ListQuery {
                    limit: 300,
                    kind: Some("media"),
                    ..Default::default()
                },
            )
            .unwrap();
            assert!(
                !media_rows.iter().any(|i| i.id == report.item_id),
                "an f95 thread is a game, not a media item"
            );

            // #53: the rest of the page's attachments must survive the fetch as
            // candidates. If they are thrown away here the Sources section has
            // nothing to offer and the parser's work is lost - so it is measured.
            let cands = db::source_images_for(conn, &report.item_id);
            println!(
                "sources  = {} candidate picture(s) kept for the item, first {}",
                cands.len(),
                cands.first().map(|c| c.url.as_str()).unwrap_or("-")
            );
            assert!(
                cands.len() >= 10,
                "the attachment urls must be kept as candidates, not dropped (#53)"
            );
            assert!(
                cands.iter().all(|c| c.stored_at.is_none()),
                "a clip stores one cover; no candidate may claim to be stored"
            );
            assert_eq!(
                media::media_count(conn, &report.item_id),
                2,
                "cover + tile only, until the user asks for another picture"
            );
        }

        // #53 end to end: the user picks ONE of those candidates. It goes through
        // the same polite lane as a clip (#57) and the same sniffed-type rule
        // (#46), and it is the only extra download this does - chosen, not scraped.
        let target = {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            db::source_images_for(conn, &report.item_id)
                .last()
                .expect("a candidate to store")
                .url
                .clone()
        };
        let stored = store_picture(&core, report.item_id.clone(), target.clone())
            .await
            .expect("storing one chosen picture");
        println!(
            "store    = {} bytes {} already {} tile {} frames {} note {:?}",
            stored.bytes, stored.mime, stored.already, stored.tile, stored.frames, stored.note
        );
        assert!(
            !stored.already,
            "the last attachment is not the cover, so these are new bytes"
        );
        // The type must come from the bytes, not the file name (#46). f95zone
        // serves avif/webp/jpeg from urls that all end in `.jpg`, so pinning one
        // mime here only measures which format the site happened to serve today.
        // What stays true next week is: the mime is one the sniffer can produce.
        assert!(
            matches!(
                stored.mime.as_str(),
                "image/png"
                    | "image/jpeg"
                    | "image/gif"
                    | "image/webp"
                    | "image/bmp"
                    | "image/avif"
                    | "image/heif"
            ),
            "the stored picture must carry a sniffer-known image mime, got {:?}",
            stored.mime
        );
        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            let frames = media::frames_for(conn, &report.item_id);
            println!(
                "strip    = {} frame(s): {}",
                frames.len(),
                frames
                    .iter()
                    .map(|f| format!("{} {} B", f.kind, f.size))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            assert_eq!(
                frames.len(),
                2,
                "the cover and the chosen picture; the tile is not a second frame (#62)"
            );
            let me = db::source_images_for(conn, &report.item_id)
                .into_iter()
                .find(|c| c.url == target)
                .expect("the candidate is still listed");
            assert!(me.stored_at.is_some(), "and it must be marked stored");
        }
        let _ = std::fs::remove_dir_all(&tmp);

        // The same link with no cookie in the vault. The invariant is not "it
        // fails": the link is still stored and the login wall is said out loud
        // (#38/#46), because a bare entry beats a lost bookmark.
        let (guest_core, tmp2) = crate::scratch_core("f95guest", true);
        let guest = clip(&guest_core, url.to_string())
            .await
            .expect("clip an f95 thread without a cookie");
        println!(
            "guest    = created {} stored_image {} thumb {} images_found {} note {:?}",
            guest.created, guest.stored_image, guest.thumb, guest.images_found, guest.note
        );
        assert!(guest.created, "the link must survive the login wall");
        assert!(!guest.stored_image, "a guest page has no art to store");
        // Where the reason lives matters: it is the clip report's note (the
        // activity strip), *not* `items.notes` - that column is the user's own
        // notes field and a fetch must never overwrite it. So the check is on the
        // report, and the vault check is that nothing art-shaped got stored.
        let note = guest.note.unwrap_or_default();
        assert!(
            note.contains("members only"),
            "the report must say why there is no art: {note}"
        );
        {
            let inner = guest_core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            assert!(
                media::best_for(conn, &guest.item_id).is_none(),
                "nothing was stored, so the viewer must have nothing"
            );
            let detail = db::get_item(conn, &guest.item_id)
                .unwrap()
                .expect("the guest clip created an item");
            assert!(
                detail.notes.is_empty(),
                "a fetch must not write into the user's notes field: {:?}",
                detail.notes
            );
        }
        let _ = std::fs::remove_dir_all(&tmp2);
    }

    /// #46 with live bytes: the whole e-hentai chain through the real pipeline.
    /// Nothing here is assumed — the gallery link is the one the site hands out,
    /// the cover has to be **cut out of the sprite sheet** the page names (there
    /// is no `<img>` to fetch any more), and one remembered page is stored
    /// through the same one-picture-at-a-time path an f95 attachment uses.
    ///
    ///   $env:GOON_EH_COOKIE = (Get-Content "$env:USERPROFILE\Downloads\ehentaicookie.txt" -Raw).Trim()
    ///   powershell -ExecutionPolicy Bypass -Command "& tools\dev-env.ps1; `
    ///     cargo test -p goonflow-desktop --lib -- --ignored --nocapture --test-threads 1 eh_live_clip_rehearsal"
    #[tokio::test]
    #[ignore]
    async fn eh_live_clip_rehearsal() {
        let (core, tmp) = crate::scratch_core("ehclip", true);
        match std::env::var("GOON_EH_COOKIE") {
            Ok(c) if !c.trim().is_empty() => {
                let inner = core.inner.lock().unwrap();
                let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
                db::set_site_setting(
                    conn,
                    "ehentai",
                    Some(true),
                    Some(c.trim()),
                    None,
                    default_rate_ms("ehentai"),
                )
                .unwrap();
            }
            _ => println!(
                "GOON_EH_COOKIE not set: the art below needs no cookie (measured), only the \
                 cookie verdict does"
            ),
        }

        // 1. a gallery link. The gid+key are this gallery's real ones; the gid
        //    alone is an apache 404 now, which is the bug #46 was filed as.
        let gallery = clip(&core, "https://e-hentai.org/g/4089666/b9d914bef4/".to_string())
            .await
            .expect("clip an e-hentai gallery");
        println!(
            "gallery  = created {} stored_image {} thumb {} tags {} images_found {}",
            gallery.created,
            gallery.stored_image,
            gallery.thumb,
            gallery.tags,
            gallery.images_found
        );
        println!("gallery  = note {:?}", gallery.note);
        assert!(gallery.created, "the gallery must become an item");
        assert!(
            gallery.stored_image,
            "the cover must be cut out of the gallery sheet (#46: no <img> to fetch)"
        );
        assert!(gallery.thumb, "and it must decode into a grid tile");
        assert!(gallery.tags > 0, "the tags are on the gallery page: {gallery:?}");
        assert!(
            gallery.images_found >= 20,
            "every page of the gallery is a candidate, none of them fetched"
        );

        // 2. store one of those page links. It is a *page*, so the store path has
        //    to resolve the page and take the art the site names inside it.
        let page = {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            let cands = db::source_images_for(conn, &gallery.item_id);
            cands
                .get(6)
                .expect("page 7 of the gallery as a candidate")
                .url
                .clone()
        };
        println!("page     = resolving candidate {page}");
        let stored = store_picture(&core, gallery.item_id.clone(), page.clone())
            .await
            .expect("storing one chosen e-hentai page");
        println!(
            "page     = {} bytes {} already {} tile {} frames {}",
            stored.bytes, stored.mime, stored.already, stored.tile, stored.frames
        );
        assert!(
            stored.bytes > 50_000,
            "a gallery page is full-size art, not a preview tile: {} B",
            stored.bytes
        );
        assert!(
            media::thumbnailable(&stored.mime),
            "the page arrives in a format this build can show: {}",
            stored.mime
        );

        // 3. the link shape the bug came in as: `/s/<imgkey>/<gid>-<n>`. The key
        //    is per-session, so it is taken from the live page instead of a
        //    hardcoded one that would rot — the point is that a slideshow link
        //    resolves onto its gallery instead of 404ing.
        let slide = clip(&core, page.clone())
            .await
            .expect("a slideshow link must resolve through its gallery, not 404");
        println!(
            "slideshow= created {} stored_image {} thumb {} tags {} images_found {}",
            slide.created,
            slide.stored_image,
            slide.thumb,
            slide.tags,
            slide.images_found
        );
        println!("slideshow= note {:?}", slide.note);
        assert!(slide.created, "the same page as a link is still an item");
        assert!(
            slide.stored_image,
            "and its own art is what gets stored for it"
        );

        {
            let inner = core.inner.lock().unwrap();
            let conn = &inner.opened.as_ref().expect("unlocked vault").conn;
            println!(
                "strip    = {} frame(s) on the gallery item (cover + tile + the stored page)",
                media::media_count(conn, &gallery.item_id)
            );
            assert!(
                media::media_count(conn, &gallery.item_id) >= 3,
                "cover + tile + the one chosen page"
            );
            let row = db::source_images_for(conn, &gallery.item_id)
                .into_iter()
                .find(|c| c.url == page)
                .expect("the page is still listed under Sources");
            assert!(row.stored_at.is_some(), "and it is marked stored");
            let frames = media::frames_for(conn, &gallery.item_id);
            println!(
                "frames   = {}",
                frames
                    .iter()
                    .map(|f| format!("#{} {} {} {} B", f.id, f.kind, f.mime, f.size))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// #38 with live bytes: what the two EH sites answer to the cookie check,
    /// for no cookie, a right-shape junk cookie and the real paste. Asserts the
    /// verdicts that were measured on 2026-11-14, because the old marker
    /// (`/g/`) answered `404` to every one of them and could only accuse a good
    /// cookie of being stale.
    ///
    ///   $env:GOON_EH_COOKIE = (Get-Content "$env:USERPROFILE\Downloads\ehentaicookie.txt" -Raw).Trim()
    #[tokio::test]
    #[ignore]
    async fn eh_cookie_verdicts_live() {
        let real = match std::env::var("GOON_EH_COOKIE") {
            Ok(c) if !c.trim().is_empty() => c.trim().to_string(),
            _ => panic!("GOON_EH_COOKIE must be set to measure the cookie verdicts"),
        };
        let client = reqwest::Client::builder()
            .user_agent(UA)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(25))
            .build()
            .unwrap();
        let junk = "ipb_member_id=1; ipb_pass_hash=deadbeefdeadbeefdeadbeefdeadbeef";

        for site in ["ehentai", "exhentai"] {
            for (label, cookie) in [
                ("no cookie", ""),
                ("right shape, wrong hash", junk),
                ("the real paste", real.as_str()),
            ] {
                let v = crate::cookies::ask(&client, site, cookie).await;
                println!("  {site:<9} {label:<24} {v:?}");
            }
        }
        assert_eq!(
            crate::cookies::ask(&client, "ehentai", "").await.ok(),
            Some(crate::cookies::Live::No),
            "a guest is bounced to login: that side is exact"
        );
        assert_eq!(
            crate::cookies::ask(&client, "ehentai", &real).await.ok(),
            Some(crate::cookies::Live::Yes),
            "the real session gets a page: that side is exact too"
        );
        assert_eq!(
            crate::cookies::ask(&client, "ehentai", junk).await.ok(),
            Some(crate::cookies::Live::No),
            "a paste with the right key names and a dead hash is caught"
        );
        assert_eq!(
            crate::cookies::ask(&client, "exhentai", &real).await.ok(),
            Some(crate::cookies::Live::Inconclusive),
            "exhentai answers 200 with an empty page to every cookie from this client, \
             so it must not be reported as working"
        );
    }
}
