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
    format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
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
        // ordinary browser UA. The referer (which boorus check) is unchanged,
        // and the polite lane above still spaces the requests.
        let client = crate::http::file_client_for(state, 600)?;
        let resp = client
            .get(&url)
            .header(reqwest::header::REFERER, format!("https://{site}"))
            .send()
            .await
            .map_err(|e| format!("download failed: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("http {}", resp.status()));
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
}
