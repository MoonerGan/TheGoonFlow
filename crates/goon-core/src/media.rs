//! Media pipeline: hash/detect image bytes, generate normalized thumbnails,
//! store both as encrypted BLOBs. Thumbnails are always ≤ THUMB_MAX px; full
//! originals are stored verbatim (D13 imports, M3 downloads share this path).

use rusqlite::{params, Connection};
use sha2::{Digest, Sha256};
use std::io::Cursor;

pub const THUMB_MAX: u32 = 360;

pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// Magic bytes → image mime. Never the URL extension: f95zone answers
/// `content-type: image/avif` for a URL ending in `…_642380pr.jpg` (#46), so a
/// filename says nothing about the bytes we are about to store or thumbnail.
pub fn detect_mime(bytes: &[u8]) -> Option<&'static str> {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if starts(b"\xff\xd8\xff") {
        Some("image/jpeg")
    } else if starts(b"GIF87a") || starts(b"GIF89a") {
        Some("image/gif")
    } else if starts(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if starts(b"BM") {
        Some("image/bmp")
    } else if let Some(brand) = isobmff_brand(bytes) {
        // ISO-BMFF container: the major brand says whether it is a still image.
        match &brand {
            b"avif" | b"avis" => Some("image/avif"),
            b"heic" | b"heix" | b"heim" | b"mif1" | b"msf1" => Some("image/heif"),
            _ => None,
        }
    } else {
        None
    }
}

/// `ftyp` box → its 4-byte major brand (ISO-BMFF: mp4, AVIF, HEIF, …).
fn isobmff_brand(bytes: &[u8]) -> Option<[u8; 4]> {
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        let mut b = [0u8; 4];
        b.copy_from_slice(&bytes[8..12]);
        Some(b)
    } else {
        None
    }
}

/// Magic bytes → video mime, for the same reason as [`detect_mime`]. This is what
/// a caller should use instead of trusting a URL's extension (the old
/// [`video_mime`] helper stays for file names the user typed themselves).
pub fn detect_video_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x1a\x45\xdf\xa3") {
        // EBML: Matroska states its doctype, WebM does too
        let head = &bytes[..bytes.len().min(64)];
        if head.windows(8).any(|w| w == b"matroska") {
            Some("video/x-matroska")
        } else {
            Some("video/webm")
        }
    } else if bytes.starts_with(b"RIFF") && bytes.len() > 12 && &bytes[8..12] == b"AVI " {
        Some("video/x-msvideo")
    } else if bytes.starts_with(b"FLV") {
        Some("video/x-flv")
    } else if bytes.starts_with(b"\x00\x00\x01\xba") || bytes.starts_with(b"\x00\x00\x01\xb3") {
        Some("video/mpeg")
    } else if bytes.len() > 188 && bytes[0] == 0x47 && bytes[188] == 0x47 {
        Some("video/mp2t")
    } else if let Some(brand) = isobmff_brand(bytes) {
        match &brand {
            b"isom" | b"iso2" | b"iso4" | b"iso5" | b"iso6" | b"avc1" | b"avcs" | b"mp41"
            | b"mp42" | b"M4V " | b"MSNV" | b"dash" | b"mmp4" | b"3gp4" | b"3gp5" | b"3g2a"
            | b"f4v " | b"XAVC" | b"hev1" | b"hvc1" => Some("video/mp4"),
            _ => None,
        }
    } else {
        None
    }
}

/// What a body actually is, from magic bytes only (#46/#50). Callers use this to
/// decide what to store and what to hand the thumbnailer, never the URL suffix.
#[derive(Debug, PartialEq, Eq)]
pub enum Sniffed {
    Image(&'static str),
    Video(&'static str),
    Unknown,
}

pub fn sniff_media(bytes: &[u8]) -> Sniffed {
    if let Some(m) = detect_mime(bytes) {
        Sniffed::Image(m)
    } else if let Some(m) = detect_video_mime(bytes) {
        Sniffed::Video(m)
    } else {
        Sniffed::Unknown
    }
}

/// Can this build's decoder make a thumbnail out of `mime`? AVIF/HEIF are stored
/// verbatim but cannot be decoded until `image`'s `avif-native` feature (dav1d)
/// is enabled for goon-core — say so in the job report instead of going quiet.
pub fn thumbnailable(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp"
    )
}

pub struct Thumbnail {
    pub mime: &'static str,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

/// Decode → aspect-preserving scale → re-encode (JPEG if the source has no
/// transparency, PNG if it does). Opacity is judged on the *decoded source*,
/// not the scaled result — upscale filters round edge alphas to 254.
/// Returns None for undecodable input.
pub fn make_thumbnail(bytes: &[u8]) -> Option<Thumbnail> {
    let img = image::load_from_memory(bytes).ok()?;
    let opaque = img.to_rgba8().pixels().all(|p| p[3] == 255);
    let thumb = img.thumbnail(THUMB_MAX, THUMB_MAX);
    let (width, height) = (thumb.width(), thumb.height());
    let mut buf = Vec::new();
    if opaque {
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82);
        enc.encode_image(&thumb).ok()?;
        Some(Thumbnail {
            mime: "image/jpeg",
            width,
            height,
            bytes: buf,
        })
    } else {
        thumb
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .ok()?;
        Some(Thumbnail {
            mime: "image/png",
            width,
            height,
            bytes: buf,
        })
    }
}

/// Replace the item's single `thumb` row with a generated thumbnail.
/// Returns false when `source_bytes` cannot be decoded.
pub fn put_thumbnail(conn: &Connection, item_id: &str, source_bytes: &[u8]) -> bool {
    let Some(t) = make_thumbnail(source_bytes) else {
        return false;
    };
    set_thumbnail(conn, item_id, &t)
}

/// Replace the item's single `thumb` row with an already-generated thumbnail.
pub fn set_thumbnail(conn: &Connection, item_id: &str, t: &Thumbnail) -> bool {
    let _ = conn.execute(
        "DELETE FROM media WHERE item_id = ?1 AND kind = 'thumb'",
        params![item_id],
    );
    conn.execute(
        "INSERT INTO media (item_id, kind, hash, mime, width, height, data)
         VALUES (?1, 'thumb', ?2, ?3, ?4, ?5, ?6)",
        params![
            item_id,
            sha256_hex(&t.bytes),
            t.mime,
            t.width as i64,
            t.height as i64,
            t.bytes
        ],
    )
    .is_ok()
}

/// Thumbnail from raw RGBA pixels — the video first-frame path (#16). Alpha is
/// forced opaque (video frames have none) and the result is always JPEG.
pub fn make_thumbnail_from_rgba(rgba: &[u8], width: u32, height: u32) -> Option<Thumbnail> {
    let w = usize::try_from(width).ok()?;
    let h = usize::try_from(height).ok()?;
    if w == 0 || h == 0 || rgba.len() != w * h * 4 {
        return None;
    }
    let mut px = rgba.to_vec();
    for p in px.chunks_mut(4) {
        p[3] = 255;
    }
    let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(width, height, px)?);
    let thumb = img.thumbnail(THUMB_MAX, THUMB_MAX);
    let (tw, th) = (thumb.width(), thumb.height());
    let mut buf = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82);
    enc.encode_image(&thumb.to_rgb8()).ok()?;
    Some(Thumbnail {
        mime: "image/jpeg",
        width: tw,
        height: th,
        bytes: buf,
    })
}

// ---------- vault-stored video (#16, strictly opt-in) ----------

/// Extension → video mime; None for non-video extensions. Only for file names we
/// have no bytes for — anything fetched goes through [`detect_video_mime`].
pub fn video_mime(ext: &str) -> Option<&'static str> {
    Some(match ext.to_ascii_lowercase().as_str() {
        "mp4" | "m4v" => "video/mp4",
        "webm" => "video/webm",
        "mkv" => "video/x-matroska",
        "mov" => "video/quicktime",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" => "video/mp2t",
        _ => return None,
    })
}

/// Store the video bytes as the item's single `video` row (deduped by hash).
/// The mime that gets **recorded** is the one the bytes show (#46/#50); the
/// caller's mime — usually read off a file name or URL suffix — is only the
/// fallback for containers we cannot sniff, so a row never claims to be an
/// mp4 it is not.
pub fn put_video(conn: &Connection, item_id: &str, mime: &str, bytes: &[u8]) -> bool {
    let sniffed = detect_video_mime(bytes).unwrap_or(mime);
    conn.execute(
        "INSERT OR IGNORE INTO media (item_id, kind, hash, mime, width, height, data)
         VALUES (?1, 'video', ?2, ?3, NULL, NULL, ?4)",
        params![item_id, sha256_hex(bytes), sniffed, bytes],
    )
    .is_ok()
}

/// (mime, byte size) of the stored video, if the item has one.
pub fn has_video(conn: &Connection, item_id: &str) -> Option<(String, i64)> {
    conn.query_row(
        "SELECT mime, length(data) FROM media WHERE item_id = ?1 AND kind = 'video' LIMIT 1",
        params![item_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

pub fn get_video_bytes(conn: &Connection, item_id: &str) -> Option<Vec<u8>> {
    conn.query_row(
        "SELECT data FROM media WHERE item_id = ?1 AND kind = 'video' LIMIT 1",
        params![item_id],
        |r| r.get(0),
    )
    .ok()
}

/// The best viewable copy an item holds (#21): full-res original first,
/// then a vault-stored video, then the thumbnail fallback.
pub fn best_for(conn: &Connection, item_id: &str) -> Option<MediaBlob> {
    for kind in ["full", "video", "thumb"] {
        if let Ok(row) = conn.query_row(
            "SELECT kind, mime, data FROM media WHERE item_id = ?1 AND kind = ?2 LIMIT 1",
            params![item_id, kind],
            |r| {
                Ok(MediaBlob {
                    kind: r.get(0)?,
                    mime: r.get(1)?,
                    data: r.get(2)?,
                })
            },
        ) {
            return Some(row);
        }
    }
    None
}

pub struct MediaBlob {
    pub kind: String,
    pub mime: String,
    pub data: Vec<u8>,
}

pub fn delete_video(conn: &Connection, item_id: &str) {
    let _ = conn.execute(
        "DELETE FROM media WHERE item_id = ?1 AND kind = 'video'",
        params![item_id],
    );
}

/// Store original image bytes as a `full` media row (deduped by content hash).
/// Returns false for unknown/undetectable formats.
pub fn put_full_image(conn: &Connection, item_id: &str, bytes: &[u8]) -> bool {
    let Some(mime) = detect_mime(bytes) else {
        return false;
    };
    conn.execute(
        "INSERT OR IGNORE INTO media (item_id, kind, hash, mime, width, height, data)
         VALUES (?1, 'full', ?2, ?3, NULL, NULL, ?4)",
        params![item_id, sha256_hex(bytes), mime, bytes],
    )
    .is_ok()
}

/// (mime, bytes) of the item's thumbnail, if any.
pub fn get_thumb(conn: &Connection, item_id: &str) -> Option<(String, Vec<u8>)> {
    conn.query_row(
        "SELECT mime, data FROM media WHERE item_id = ?1 AND kind = 'thumb' LIMIT 1",
        params![item_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )
    .ok()
}

/// Batch thumbnail fetch for grid views: (item_id, mime, bytes).
pub fn thumbs_for(conn: &Connection, item_ids: &[String]) -> Vec<(String, String, Vec<u8>)> {
    if item_ids.is_empty() {
        return Vec::new();
    }
    let placeholders = item_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
    let sql = format!(
        "SELECT item_id, mime, data FROM media WHERE kind = 'thumb' AND item_id IN ({placeholders})"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(item_ids), |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Vec<u8>>(2)?,
        ))
    }) {
        for row in rows.flatten() {
            out.push(row);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 8x8 solid PNG used by several tests.
    fn sample_png() -> Vec<u8> {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([10, 200, 30, 255]),
        ));
        let mut buf = Vec::new();
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn mime_detection() {
        assert_eq!(detect_mime(&sample_png()), Some("image/png"));
        assert_eq!(detect_mime(b"GIF89a..."), Some("image/gif"));
        assert_eq!(detect_mime(b"not an image"), None);
        // #50: a real JPEG body (the 403 error page rule34's CDN hands out when
        // it does not like the User-Agent must never be stored as an image)
        assert_eq!(
            detect_mime(b"\xff\xd8\xff\xe0\x00\x10JFIF"),
            Some("image/jpeg")
        );
        assert_eq!(
            detect_mime(b"<html><body>Access denied</body></html>"),
            None
        );
    }

    /// #46: f95zone answers `image/avif` for a URL ending in `.jpg`. The type
    /// comes from the bytes, and the container brand keeps AVIF from reading as
    /// the MP4 that shares its ISO-BMFF wrapper.
    #[test]
    fn avif_is_detected_from_magic_bytes_not_the_name() {
        let avif = b"ftypavif";
        let mut body = vec![0u8, 0, 0, 0x18];
        body.extend_from_slice(avif);
        body.extend_from_slice(&[0; 24]);
        assert_eq!(detect_mime(&body), Some("image/avif"));
        assert_eq!(sniff_media(&body), Sniffed::Image("image/avif"));

        // same container, video brand → video, not image
        let mut mp4 = vec![0u8, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftyp");
        mp4.extend_from_slice(b"isom");
        mp4.extend_from_slice(&[0; 24]);
        assert_eq!(detect_mime(&mp4), None);
        assert_eq!(sniff_media(&mp4), Sniffed::Video("video/mp4"));

        assert_eq!(detect_mime(b"\x00\x00\x00\x18ftypheic"), Some("image/heif"));
    }

    #[test]
    fn video_containers_sniff_from_magic_bytes() {
        assert_eq!(
            detect_video_mime(b"\x1a\x45\xdf\xa3\x01\x00\x00\x00\x00\x00\x00\x1fmatroska\x90"),
            Some("video/x-matroska")
        );
        assert_eq!(
            detect_video_mime(b"\x1a\x45\xdf\xa3\x01\x00\x00\x00\x00\x00\x00\x1fwebm\x90abc"),
            Some("video/webm")
        );
        assert_eq!(
            detect_video_mime(b"RIFF\x00\x00\x00\x00AVI LIST"),
            Some("video/x-msvideo")
        );
        assert_eq!(detect_video_mime(b"FLV\x01\x05\x00"), Some("video/x-flv"));
        assert_eq!(detect_video_mime(b"\x89PNG\r\n\x1a\n"), None);
        assert_eq!(
            sniff_media(b"RIFF\x00\x00\x00\x00AVI LIST"),
            Sniffed::Video("video/x-msvideo")
        );
        assert_eq!(sniff_media(b"whatever"), Sniffed::Unknown);
    }

    /// #46/#50: the mime we *record* for stored art is the sniffed one, so a
    /// `.jpg` URL that carries AVIF is filed as AVIF and the viewer is told.
    #[test]
    fn stored_type_is_recorded_from_the_bytes() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('m6','game','t','0','0')",
            [],
        )
        .unwrap();
        let mut avif = vec![0u8, 0, 0, 0x1c];
        avif.extend_from_slice(b"ftypavif");
        avif.extend_from_slice(&[0; 32]);
        assert!(put_full_image(&conn, "m6", &avif));
        let row = best_for(&conn, "m6").expect("full row stored");
        assert_eq!(row.kind, "full");
        assert_eq!(row.mime, "image/avif");
        assert_eq!(row.data, avif);
        // this build cannot decode AVIF → no thumbnail, and that stays visible
        assert!(!thumbnailable("image/avif"));
        assert!(!put_thumbnail(&conn, "m6", &avif));
        assert!(get_thumb(&conn, "m6").is_none());
        assert!(thumbnailable("image/jpeg"));
        // an HTML error page never becomes "art"
        assert!(!put_full_image(&conn, "m6", b"<html>Access denied</html>"));
    }

    /// A `.mkv`-looking source carrying WebM bytes is recorded as WebM: the row's
    /// mime is what the viewer and the hash-pairing read back, so it must be the
    /// container the bytes actually are.
    #[test]
    fn video_rows_record_the_sniffed_container() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        for id in ["m7", "m8"] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'post','t','0','0')",
                [id],
            )
            .unwrap();
        }
        let webm = b"\x1a\x45\xdf\xa3\x01\x00\x00\x00\x00\x00\x00\x1fwebm\x90abc";
        assert!(put_video(&conn, "m7", "video/x-matroska", webm));
        assert_eq!(has_video(&conn, "m7").unwrap().0, "video/webm");
        // an unsniffable body keeps whatever the caller asserted
        assert!(put_video(&conn, "m8", "video/mp4", b"ftypmp42"));
        assert_eq!(has_video(&conn, "m8").unwrap().0, "video/mp4");
    }

    #[test]
    fn thumbnail_generation_and_roundtrip_in_db() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('m1','post','t','0','0')",
            [],
        )
        .unwrap();
        let png = sample_png();
        assert!(put_thumbnail(&conn, "m1", &png));
        assert!(put_full_image(&conn, "m1", &png));
        let (mime, bytes) = get_thumb(&conn, "m1").expect("thumb stored");
        assert_eq!(mime, "image/jpeg"); // opaque → jpeg
        assert!(!bytes.is_empty());
        let batch = thumbs_for(&conn, &["m1".to_string()]);
        assert_eq!(batch.len(), 1);
    }

    #[test]
    fn thumbnail_replaced_not_duplicated() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('m2','post','t','0','0')",
            [],
        )
        .unwrap();
        let png = sample_png();
        let _ = put_thumbnail(&conn, "m2", &png);
        let _ = put_thumbnail(&conn, "m2", &png);
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM media WHERE item_id='m2' AND kind='thumb'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(count, 1);
    }

    /// #21: the viewer picks the best copy that exists.
    #[test]
    fn best_for_prefers_full_then_video_then_thumb() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        for id in ["m3", "m4", "m5"] {
            conn.execute(
                "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'post','t','0','0')",
                [id],
            )
            .unwrap();
        }
        // m3: thumb + full → full wins
        let png = sample_png();
        assert!(put_thumbnail(&conn, "m3", &png));
        assert!(put_full_image(&conn, "m3", &png));
        assert_eq!(best_for(&conn, "m3").unwrap().kind, "full");
        // m4: thumb + video → video wins
        assert!(put_thumbnail(&conn, "m4", &png));
        assert!(put_video(&conn, "m4", "video/mp4", b"ftypmp42"));
        assert_eq!(best_for(&conn, "m4").unwrap().kind, "video");
        // m5: thumb only → thumb
        assert!(put_thumbnail(&conn, "m5", &png));
        assert_eq!(best_for(&conn, "m5").unwrap().kind, "thumb");
        assert!(best_for(&conn, "m1").is_none());
    }
}
