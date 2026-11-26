//! #16: opt-in vault video storage.
//!
//! Videos stay links by default (D17). This module is the opt-in escape:
//! copy one video's bytes into the encrypted DB (`media` row kind='video'),
//! then try to make a first-frame thumbnail (videothumb) so the grid shows
//! something. Removing the stored copy later returns the item to link-only.
//! The size cap keeps a single BLOB inside SQLite's comfortable range and
//! protects the USB-drive story; Nuke shreds the DB, so stored videos die
//! with everything else — no separate deletion story needed.

use goon_core::media;
use serde::Serialize;
use std::path::PathBuf;

use crate::ingest;
use crate::jobs::JobHandle;
use crate::Core;

/// Hard cap per stored video (SQLite BLOB comfort + vault portability).
pub const STORE_CAP: u64 = 512 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoInfo {
    /// a video source exists for this item
    pub has_source: bool,
    /// byte size of that source (local stat or remote Content-Length if cheap), None = unknown
    pub source_size: Option<u64>,
    pub source_kind: Option<String>, // "local" | site name
    pub stored: bool,
    pub stored_size: Option<u64>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct VideoStoreReport {
    pub stored_bytes: u64,
    pub thumb_added: bool,
    pub note: Option<String>,
}

/// The item's video source, if it has one (first video-ish URL wins).
pub fn video_source(sources: &[goon_core::db::SourceRow]) -> Option<(String, String)> {
    sources
        .iter()
        .find(|s| ingest::media_ext(&s.url) == Some("video"))
        .map(|s| (s.site.clone(), s.url.clone()))
}

fn local_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file:///")?;
    Some(PathBuf::from(rest))
}

fn human_mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0)
}

/// `host[:port]` of an http(s) URL, lower-cased. Hand-rolled on purpose: no need
/// to fail a download because a URL parsed imperfectly, and this binary has no
/// URL-craving dependency here.
fn url_host(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, r)| r)?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    (!authority.is_empty()).then(|| authority.to_ascii_lowercase())
}

/// What the request ends up answering for, once redirects are followed.
fn redirect_hop(from: &str, to: &str) -> bool {
    url_host(from).is_some() && url_host(to).is_some() && url_host(from) != url_host(to)
}

/// Does this `Content-Type` mean "an HTML page answered", i.e. an interstitial
/// (CAPTCHA / hotlink warning / login wall) rather than the file we asked for?
/// A file host that does not name a type at all is not an HTML page.
fn is_html_ct(ct: Option<&str>) -> bool {
    match ct {
        Some(v) => {
            let v = v.to_ascii_lowercase();
            v.contains("text/html") || v.contains("application/xhtml")
        }
        None => false,
    }
}

/// The `Referer` a file request must carry (#64, measured 2026-11-25).
///
/// The old line formatted `https://{site}` from the **site key** the sources row
/// stores — `rule34`, not a hostname. Measured against the URL the user reported
/// (`…/71bd5b0c47e11f5db8675ca10484f5bd.mp4?8161771`, live in `http.rs`'s
/// `live_site_answers`):
///
/// ```text
/// referer https://rule34                -> 403 text/html 77 684 B  (Rule34.xxx CAPTCHA page)
/// referer https://ahri2mp4.rule34.xxx   -> 200 video/mp4 59 083 373 B
/// referer https://rule34.xxx            -> 200 video/mp4 59 083 373 B
/// ```
///
/// rule34's hotlink check treats a referer that is not a real page as a hotlink:
/// it 302s to `hl.rule34.xxx/public/hotlink.php`, and reqwest follows that hop
/// into a CAPTCHA page whose status is 403 — which is the `http 403 Forbidden`
/// the user saw for a file that plays fine in a browser. So the referer is
/// derived from **the URL that names the file**: the site key never enters it.
fn referer_for(url: &str) -> String {
    let origin_end = url
        .find("://")
        .and_then(|i| url[i + 3..].find(['/', '?', '#']).map(|j| i + 3 + j))
        .unwrap_or(url.len());
    url[..origin_end].trim_end_matches('/').to_string()
}

pub fn video_info(state: &Core, item_id: &str) -> Result<VideoInfo, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let Some(d) = goon_core::db::get_item(&opened.conn, item_id).map_err(crate::err)? else {
        return Err("item not found".into());
    };
    let mut info = VideoInfo {
        has_source: false,
        source_size: None,
        source_kind: None,
        stored: false,
        stored_size: None,
    };
    if let Some((site, url)) = video_source(&d.sources) {
        info.has_source = true;
        info.source_kind = Some(site);
        if let Some(p) = local_path(&url) {
            info.source_size = std::fs::metadata(&p).ok().map(|m| m.len());
        }
    }
    if let Some((_mime, size)) = media::has_video(&opened.conn, item_id) {
        info.stored = true;
        info.stored_size = Some(size as u64);
    }
    Ok(info)
}

/// Copy the item's video into the vault and try to extract a first-frame
/// thumbnail. Runs as a job (large files, remote downloads).
pub async fn store_video(
    state: &Core,
    item_id: &str,
    job: &JobHandle,
) -> Result<VideoStoreReport, String> {
    let (site, url) = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        let Some(d) = goon_core::db::get_item(&opened.conn, item_id).map_err(crate::err)? else {
            return Err("item not found".into());
        };
        video_source(&d.sources).ok_or("this item has no video source to store")?
    };

    let report = if let Some(path) = local_path(&url) {
        job.set_progress(0, 2, &path.display().to_string());
        let size = std::fs::metadata(&path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?
            .len();
        if size > STORE_CAP {
            return Err(format!(
                "that video is {} — over the {} vault cap; keep it as a link (D17)",
                human_mb(size),
                human_mb(STORE_CAP)
            ));
        }
        let bytes = std::fs::read(&path).map_err(|e| format!("read failed: {e}"))?;
        finish_store(state, item_id, &bytes, &path.display().to_string(), job)?
    } else {
        job.set_note(&format!("downloading {site} video…"));
        let rate = {
            let inner = state.inner.lock().unwrap();
            match inner.opened.as_ref() {
                Some(o) => {
                    goon_core::db::site_rate_ms(&o.conn, &site, ingest::default_rate_ms(&site))
                }
                None => return Err("vault vanished mid-store".into()),
            }
        };
        if !state.queue.reserve(&site, rate, || job.cancelled()).await {
            return Err("stopped".into());
        }
        // #50: this is file bytes from a content host, so the CDN sees an
        // ordinary browser UA. The polite lane above still spaces the requests.
        // #64: and the referer comes from the URL that names the file, never
        // from the app's site key — that is what got us the hotlink CAPTCHA.
        let client = crate::http::file_client_for(state, 600)?;
        let resp = client
            .get(&url)
            .header(reqwest::header::REFERER, referer_for(&url))
            .send()
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        let final_url = resp.url().to_string();
        let ct = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());
        if !resp.status().is_success() {
            // A file host that means "no" usually says so with an HTML page
            // (CAPTCHA, hotlink warning, login wall). Say which host answered,
            // and from where, instead of a bare status the user cannot act on.
            let hop = if redirect_hop(&url, &final_url) {
                format!(
                    " The host redirected the request to {} and that answered {}.",
                    url_host(&final_url).unwrap_or_else(|| "?".into()),
                    resp.status()
                )
            } else {
                String::new()
            };
            return Err(format!(
                "{} refused the download (http {}).{hop} Nothing was stored.",
                url_host(&url).unwrap_or_else(|| "the host".into()),
                resp.status()
            ));
        }
        if is_html_ct(ct.as_deref()) {
            // 200 with an HTML body is the other shape of the same answer.
            return Err(format!(
                "{} sent a web page instead of the video (content-type {}, from {}), \
                 so nothing was stored. Its hotlink protection does not accept this \
                 app's request — save the file in your browser and use + Video, \
                 or report the link so the ingestor can be taught.",
                url_host(&final_url).unwrap_or_else(|| "the host".into()),
                ct.unwrap_or_else(|| "?".into()),
                if redirect_hop(&url, &final_url) {
                    "a redirect hop"
                } else {
                    "the same url"
                }
            ));
        }
        if let Some(cl) = resp.content_length() {
            if cl > STORE_CAP {
                return Err(format!(
                    "that video is {} — over the {} vault cap; keep it as a link (D17)",
                    human_mb(cl),
                    human_mb(STORE_CAP)
                ));
            }
        }
        let mut bytes: Vec<u8> = Vec::new();
        let mut resp = resp;
        while let Some(chunk) = resp
            .chunk()
            .await
            .map_err(|e| format!("download interrupted: {e}"))?
        {
            if job.cancelled() {
                return Err("stopped".into());
            }
            if bytes.len() as u64 + chunk.len() as u64 > STORE_CAP {
                return Err(format!(
                    "stream passed the {} vault cap — nothing stored",
                    human_mb(STORE_CAP)
                ));
            }
            bytes.extend_from_slice(&chunk);
            job.set_progress(0, 2, &format!("{} MB", bytes.len() / (1024 * 1024) + 1));
        }
        finish_store(state, item_id, &bytes, &url, job)?
    };
    Ok(report)
}

fn finish_store(
    state: &Core,
    item_id: &str,
    bytes: &[u8],
    origin: &str,
    job: &JobHandle,
) -> Result<VideoStoreReport, String> {
    if job.cancelled() {
        return Err("stopped".into());
    }
    let mime = media::video_mime(
        origin
            .rsplit('.')
            .next()
            .unwrap_or("")
            .split(['?', '/'])
            .next()
            .unwrap_or(""),
    )
    .unwrap_or("application/octet-stream");
    {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        if !media::put_video(&opened.conn, item_id, mime, bytes) {
            return Err("could not write the video into the vault".into());
        }
    }
    let mut report = VideoStoreReport {
        stored_bytes: bytes.len() as u64,
        thumb_added: false,
        note: None,
    };

    // First-frame thumbnail only when the item has none yet — a site cover
    // image is a better tile than a mid-action frame.
    let want_thumb = {
        let inner = state.inner.lock().unwrap();
        match inner.opened.as_ref() {
            Some(o) => media::get_thumb(&o.conn, item_id).is_none(),
            None => false,
        }
    };
    if want_thumb {
        job.set_progress(1, 2, "extracting first frame…");
        if let Some(t) = first_frame_for(state, item_id, bytes, origin) {
            let inner = state.inner.lock().unwrap();
            if let Some(o) = inner.opened.as_ref() {
                report.thumb_added = media::set_thumbnail(&o.conn, item_id, &t);
            }
        } else {
            report.note = Some(
                "stored, but no first-frame thumbnail — Windows has no codec for that format"
                    .into(),
            );
        }
    }
    Ok(report)
}

/// Locate a playable file for first-frame extraction: reuse the local source,
/// else spill the vault bytes to a temp file (the Shell handler needs a path).
fn first_frame_for(
    state: &Core,
    item_id: &str,
    bytes: &[u8],
    origin: &str,
) -> Option<goon_core::media::Thumbnail> {
    let local = {
        let inner = state.inner.lock().unwrap();
        inner.opened.as_ref().and_then(|o| {
            goon_core::db::get_item(&o.conn, item_id)
                .ok()
                .flatten()
                .and_then(|d| video_source(&d.sources))
                .and_then(|(_, u)| local_path(&u))
                .filter(|p| p.is_file())
        })
    };
    let path = match local {
        Some(p) => p,
        None => {
            let ext = origin
                .rsplit('.')
                .next()
                .unwrap_or("mp4")
                .split(['?', '/'])
                .next()
                .unwrap_or("mp4");
            let tmp = std::env::temp_dir().join(format!("goon-ff-{item_id}.{ext}"));
            std::fs::write(&tmp, bytes).ok()?;
            tmp
        }
    };
    let frame = crate::videothumb::first_frame(&path);
    if path.starts_with(std::env::temp_dir()) {
        let _ = std::fs::remove_file(&path);
    }
    let f = frame?;
    media::make_thumbnail_from_rgba(&f.rgba, f.width, f.height)
}

pub fn remove_stored_video(state: &Core, item_id: &str) -> Result<(), String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    media::delete_video(&opened.conn, item_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_source_picks_the_video_url() {
        let src = |site: &str, url: &str| goon_core::db::SourceRow {
            id: 1,
            site: site.into(),
            url: url.into(),
            source_id: None,
            fetched_at: None,
            dead: false,
        };
        let list = [
            src("local", "file:///D:/media/clip.mp4"),
            src(
                "rule34",
                "https://rule34.xxx/index.php?page=post&s=view&id=1",
            ),
        ];
        let (site, url) = video_source(&list).unwrap();
        assert_eq!(site, "local");
        assert!(url.ends_with("clip.mp4"));
        assert!(video_source(&[src("rule34", "https://x/y.jpg")]).is_none());
    }

    #[test]
    fn mime_guess_and_cap() {
        assert_eq!(media::video_mime("mp4"), Some("video/mp4"));
        assert_eq!(media::video_mime("WEBM"), Some("video/webm"));
        assert_eq!(media::video_mime("exe"), None);
        assert_eq!(STORE_CAP, 512 * 1024 * 1024);
    }

    /// #64, the 403 the user reported: the referer must be built from the URL
    /// that names the file, never from the app's site key. `https://rule34` is
    /// not a page any hotlink check can accept, and rule34 answers it with a
    /// redirect into a CAPTCHA (measured in http.rs::live_site_answers: 403 /
    /// 77 684 B of HTML, where the URL's own origin gives 200 / 59 MB).
    #[test]
    fn referer_is_the_file_origin_not_the_site_key() {
        let cdn = "https://ahri2mp4.rule34.xxx//images/7158/71bd5b0c47e11f5db8675ca10484f5bd.mp4?8161771";
        assert_eq!(referer_for(cdn), "https://ahri2mp4.rule34.xxx");
        assert_eq!(
            referer_for("https://api-cdn-us-mp4.rule34.xxx/images/1440/x.mp4?10938771"),
            "https://api-cdn-us-mp4.rule34.xxx"
        );
        assert_eq!(referer_for("https://host.example"), "https://host.example");
        assert_eq!(
            referer_for("http://127.0.0.1:8080/a/b.webm"),
            "http://127.0.0.1:8080"
        );
        // the shape that caused the bug: a site key is not a hostname
        assert_ne!(referer_for(cdn), format!("https://{}", "rule34"));
        assert_eq!(url_host(cdn).as_deref(), Some("ahri2mp4.rule34.xxx"));
    }

    #[test]
    fn html_answers_are_named_not_guessed() {
        assert!(is_html_ct(Some("text/html; charset=UTF-8")));
        assert!(is_html_ct(Some("application/xhtml+xml")));
        assert!(!is_html_ct(Some("video/mp4")));
        assert!(!is_html_ct(None), "no content-type is not a web page");
        let mp4 = "https://ahri2mp4.rule34.xxx//images/7158/x.mp4?8161771";
        let hop = "https://hl.rule34.xxx/public/hotlink.php?img=x.mp4";
        assert!(redirect_hop(mp4, hop));
        assert!(!redirect_hop(mp4, mp4));
        assert!(!redirect_hop(mp4, "https://ahri2mp4.rule34.xxx/other.mp4"));
    }
}
