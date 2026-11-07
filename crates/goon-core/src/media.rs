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
    } else {
        None
    }
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
    let _ = conn.execute(
        "DELETE FROM media WHERE item_id = ?1 AND kind = 'thumb'",
        params![item_id],
    );
    conn.execute(
        "INSERT INTO media (item_id, kind, hash, mime, width, height, data)
         VALUES (?1, 'thumb', ?2, ?3, ?4, ?5, ?6)",
        params![item_id, sha256_hex(&t.bytes), t.mime, t.width as i64, t.height as i64, t.bytes],
    )
    .is_ok()
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
    let placeholders = item_ids
        .iter()
        .map(|_| "?")
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT item_id, mime, data FROM media WHERE kind = 'thumb' AND item_id IN ({placeholders})"
    );
    let mut stmt = match conn.prepare(&sql) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Ok(rows) = stmt.query_map(rusqlite::params_from_iter(item_ids), |r| {
        Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
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
            .query_row("SELECT count(*) FROM media WHERE item_id='m2' AND kind='thumb'", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }
}
