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

/// Can this build's decoder make a thumbnail out of `mime`?
///
/// AVIF is decodable since #52: `image`'s `avif-native` feature links dav1d
/// statically, which is what f95zone actually serves for its attachment art
/// (`image/avif` behind a `.jpg` URL, measured in #46).
///
/// HEIF/HEIC is **not** and will not be with this `image` version: the dav1d
/// path reads AVIF containers only (`mp4parse::read_avif`). A HEIF original is
/// still stored verbatim, and the job report says the grid thumbnail is missing
/// because this build cannot decode that format - it does not go quiet.
pub fn thumbnailable(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/gif" | "image/webp" | "image/bmp" | "image/avif"
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

/// Cut one rectangle out of a bigger image and re-encode it.
///
/// Why this exists (#46, 2026 shape): e-hentai's gallery page no longer marks a
/// thumbnail per `<img>`. The cover and the whole page grid are one sprite
/// sheet on a per-gallery host, and the page says where each tile sits —
/// `style="width:200px;height:141px;background:transparent url(<sheet>) -400px 0
/// no-repeat"`. Cropping the advertised box out of the sheet is the only way to
/// get a picture off that page, and the sheet itself answers to any client
/// (measured: `image/webp`, 200, no referer, no cookie).
///
/// A box that is not fully inside the decoded sheet is an error, never a silent
/// clamp: sheet and geometry disagreeing means the page is not the shape we
/// measured, and a shifted crop would be shown to the user as if it were art.
pub fn crop_tile(bytes: &[u8], x: u32, y: u32, w: u32, h: u32) -> Result<Thumbnail, String> {
    if w == 0 || h == 0 {
        return Err("that tile has no size".into());
    }
    // A page could advertise anything; keep one crop inside the memory budget
    // the rest of the media path already works with.
    if u64::from(w) * u64::from(h) > 12_000_000 {
        return Err(format!("that tile is {w}x{h} — too large to cut out"));
    }
    let img = image::load_from_memory(bytes).map_err(|_| "the sheet could not be decoded")?;
    let (sw, sh) = (img.width(), img.height());
    if x >= sw || y >= sh || x + w > sw || y + h > sh {
        return Err(format!(
            "the page asked for a {w}x{h} tile at ({x},{y}) of a {sw}x{sh} sheet"
        ));
    }
    let tile = img.crop_imm(x, y, w, h);
    let opaque = tile.to_rgba8().pixels().all(|p| p[3] == 255);
    let (width, height) = (tile.width(), tile.height());
    let mut buf = Vec::new();
    if opaque {
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 82);
        enc.encode_image(&tile)
            .map_err(|_| "the tile could not be re-encoded")?;
        Ok(Thumbnail {
            mime: "image/jpeg",
            width,
            height,
            bytes: buf,
        })
    } else {
        tile.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .map_err(|_| "the tile could not be re-encoded")?;
        Ok(Thumbnail {
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

/// One media row without its bytes: what the viewer's filmstrip lists (#62).
/// An item can carry a dozen originals, and a strip that pulled every BLOB to
/// draw one row of thumbnails would load the vault to paint 200 px squares.
pub struct MediaMeta {
    pub id: i64,
    pub kind: String,
    pub mime: String,
    /// bytes in the row, straight from `length(data)`: enough to size and label
    /// the frame without touching it
    pub size: i64,
}

/// Every viewable frame the item holds, oldest first (#62). [`best_for`] answers
/// "which single copy is the best one to show"; the strip wants all of them, and
/// `video` rows need no special case - they are frames with kind "video".
///
/// The generated `thumb` row is dropped as soon as a real original exists: it is
/// that same picture at 360 px ([`put_thumbnail`]), so it would show up in the
/// strip twice. An item whose only media is the tile - every single-image item
/// written before #62 - still returns the tile, so those keep rendering.
///
/// `item_id` leads both the WHERE clause and media_hash_idx, so this walks one
/// item's index range and `ORDER BY id` sorts a handful of rows.
pub fn frames_for(conn: &Connection, item_id: &str) -> Vec<MediaMeta> {
    let mut stmt = match conn
        .prepare("SELECT id, kind, mime, length(data) FROM media WHERE item_id = ?1 ORDER BY id")
    {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    if let Ok(rows) = stmt.query_map(params![item_id], |r| {
        Ok(MediaMeta {
            id: r.get(0)?,
            kind: r.get(1)?,
            mime: r.get(2)?,
            size: r.get(3)?,
        })
    }) {
        for row in rows.flatten() {
            out.push(row);
        }
    }
    if out.iter().any(|m| m.kind != "thumb") {
        out.retain(|m| m.kind != "thumb");
    }
    out
}

/// One frame's bytes, by the id [`frames_for`] handed out (#62). No item_id
/// argument on purpose: media.id is unique across the vault and the caller has
/// already proven the item lives in this one.
pub fn bytes_for_id(conn: &Connection, id: i64) -> Option<MediaBlob> {
    conn.query_row(
        "SELECT kind, mime, data FROM media WHERE id = ?1",
        params![id],
        |r| {
            Ok(MediaBlob {
                kind: r.get(0)?,
                mime: r.get(1)?,
                data: r.get(2)?,
            })
        },
    )
    .ok()
}

/// How many media rows the item holds - tile included, so this is what the
/// vault *stores*, not what the strip shows (#62 reports and tests).
pub fn media_count(conn: &Connection, item_id: &str) -> usize {
    let rows = conn
        .query_row(
            "SELECT count(*) FROM media WHERE item_id = ?1",
            params![item_id],
            |r| r.get::<_, i64>(0),
        )
        .unwrap_or(0);
    usize::try_from(rows).unwrap_or(0)
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

    /// PNG-headed bytes that differ by one byte, for tests that need several
    /// *distinct* originals on one item (#62). They are only ever sniffed and
    /// stored, never decoded, so the payload stays synthetic.
    fn fake_png(tag: u8) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n".to_vec();
        b.extend_from_slice(&[tag; 24]);
        b
    }

    /// One item row, inserted the way every test here does it.
    fn insert_post(conn: &Connection, id: &str) {
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES (?1,'post','t','0','0')",
            [id],
        )
        .unwrap();
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
        // #52: this build CAN decode AVIF (dav1d is linked in), so these
        // synthetic bytes produce no thumbnail because the *decode* fails, not
        // because the format is unsupported. That distinction is what keeps the
        // job report honest: unsupported is a fact about the build, a failed
        // decode is a fact about these bytes.
        assert!(thumbnailable("image/avif"));
        assert!(!put_thumbnail(&conn, "m6", &avif));
        assert!(!thumbnailable("image/heif"), "HEIF is still out of reach");
        assert!(get_thumb(&conn, "m6").is_none());
        assert!(thumbnailable("image/jpeg"));
        // an HTML error page never becomes "art"
        assert!(!put_full_image(&conn, "m6", b"<html>Access denied</html>"));
    }

    /// #52: the dav1d-backed AVIF decoder is a claim this build has to keep
    /// honouring. `image` only exports this type with the `avif-native` feature,
    /// so the line below stops compiling the moment the feature is dropped -
    /// which would otherwise pass every other test here and quietly take
    /// thumbnails away from f95's art.
    #[test]
    fn the_avif_decoder_is_actually_linked_in() {
        let never: Option<image::codecs::avif::AvifDecoder<std::io::Cursor<Vec<u8>>>> = None;
        assert!(never.is_none(), "the type exists only when dav1d is linked");
    }

    /// A real AVIF made by this project (a deterministic gradient, not a photo
    /// somebody else owns): the grid must get a thumbnail out of it. Written by
    /// `ravif`, read back by dav1d, so it also guards the two codecs agreeing on
    /// the container.
    #[test]
    fn a_real_avif_gets_a_thumbnail() {
        let bytes = include_bytes!("fixtures/avif-tiny.avif");
        assert_eq!(detect_mime(bytes), Some("image/avif"));
        assert!(thumbnailable("image/avif"));
        let t = make_thumbnail(bytes).expect("dav1d should decode what we wrote");
        assert!(
            t.width <= THUMB_MAX && t.height <= THUMB_MAX,
            "the thumbnail must fit the grid budget"
        );
        assert!(
            t.bytes.starts_with(&[0xff, 0xd8]),
            "an opaque source thumbnails to jpeg"
        );
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

    /// #62: one page carries many pictures, so the strip must list every stored
    /// original in the order they arrived - and must not add the tile, which is
    /// a copy of one of them, as a second frame.
    #[test]
    fn every_original_is_its_own_frame_in_insertion_order() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        insert_post(&conn, "f1");
        let (a, b, c) = (fake_png(1), fake_png(2), fake_png(3));
        for bytes in [&a, &b, &c] {
            assert!(put_full_image(&conn, "f1", bytes));
        }
        assert!(put_thumbnail(&conn, "f1", &sample_png()));

        let frames = frames_for(&conn, "f1");
        assert_eq!(frames.len(), 3, "the tile is not a frame of its own");
        let kinds: Vec<&str> = frames.iter().map(|f| f.kind.as_str()).collect();
        assert!(
            kinds.iter().all(|k| *k == "full"),
            "the tile must not pose as a second frame: kinds={kinds:?}"
        );
        let ids: Vec<i64> = frames.iter().map(|f| f.id).collect();
        assert!(ids.windows(2).all(|w| w[0] < w[1]), "ids ascend: {ids:?}");
        let sizes: Vec<i64> = frames.iter().map(|f| f.size).collect();
        assert_eq!(
            sizes,
            vec![a.len() as i64, b.len() as i64, c.len() as i64],
            "sizes are the stored byte counts, in insertion order"
        );
        assert!(frames.iter().all(|f| f.mime == "image/png"));
        assert!(get_thumb(&conn, "f1").is_some(), "the grid keeps its tile");
        assert_eq!(media_count(&conn, "f1"), 4, "3 originals + the tile");
    }

    /// The back-compat guarantee for a vault written before #62: an item whose
    /// only media is the generated tile still has exactly one frame to show.
    #[test]
    fn a_tile_only_item_still_yields_its_tile() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        insert_post(&conn, "f2");
        assert!(put_thumbnail(&conn, "f2", &sample_png()));

        let frames = frames_for(&conn, "f2");
        assert_eq!(frames.len(), 1, "the tile is the item's one frame");
        assert_eq!(frames[0].kind, "thumb");
        let tile = bytes_for_id(&conn, frames[0].id).expect("the tile's bytes are there");
        assert_eq!(tile.kind, "thumb");
        assert_eq!(tile.data.len() as i64, frames[0].size);
        // an item with no media at all is empty, not a phantom frame
        assert!(frames_for(&conn, "never-seen").is_empty());
        assert_eq!(media_count(&conn, "never-seen"), 0);
    }

    /// #62: listing frames is cheap, the bytes come one at a time by row id.
    /// Exact bytes, and an id that is not in this vault is a miss, not a panic.
    #[test]
    fn frame_bytes_come_back_exact_by_row_id() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        insert_post(&conn, "f3");
        let png = fake_png(7);
        assert!(put_full_image(&conn, "f3", &png));
        assert!(put_video(&conn, "f3", "video/mp4", b"ftypmp42"));

        let frames = frames_for(&conn, "f3");
        assert_eq!(frames.len(), 2, "a video row is just another frame kind");
        assert_eq!(frames[1].kind, "video");
        let full = bytes_for_id(&conn, frames[0].id).expect("frame 0");
        assert_eq!(full.kind, "full");
        assert_eq!(full.mime, "image/png");
        assert_eq!(full.data, png, "the bytes are what we stored");
        let movie = bytes_for_id(&conn, frames[1].id).expect("frame 1");
        assert_eq!(movie.mime, "video/mp4");
        assert_eq!(movie.data, b"ftypmp42".to_vec());
        assert!(
            bytes_for_id(&conn, 999_999).is_none(),
            "no such row in this vault"
        );
    }

    /// Content-hash dedup predates #62 and has to survive it: the same bytes
    /// fetched twice for one item are one frame, not two in the strip.
    #[test]
    fn identical_bytes_are_one_frame_not_two() {
        let conn = Connection::open_in_memory().unwrap();
        crate::db::init_schema(&conn).unwrap();
        insert_post(&conn, "f4");
        let png = fake_png(9);
        assert!(put_full_image(&conn, "f4", &png));
        assert!(put_full_image(&conn, "f4", &png));
        assert_eq!(media_count(&conn, "f4"), 1, "one row for identical bytes");
        let frames = frames_for(&conn, "f4");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].kind, "full");
        assert_eq!(frames[0].size, png.len() as i64);
    }

    /// #46 (2026 shape): e-hentai delivers its previews as **one sheet plus CSS
    /// offsets**, so the picture has to be cut out here. A crop that clamped, or
    /// read the offsets wrong, would be shown to the user as if it were the art —
    /// so the exact colour of each crop is checked, and a box that does not fit
    /// the sheet is refused instead of quietly moved inside it.
    #[test]
    fn a_sprite_sheet_is_cut_at_the_box_the_page_advertises() {
        use image::ImageEncoder as _;
        // 300x100, three 100x100 tiles in different colours: red, green, blue.
        let img = image::RgbImage::from_fn(300, 100, |x, _| {
            if x < 100 {
                image::Rgb([220, 10, 10])
            } else if x < 200 {
                image::Rgb([10, 200, 10])
            } else {
                image::Rgb([10, 10, 220])
            }
        });
        let mut sheet = Vec::new();
        image::codecs::png::PngEncoder::new(&mut sheet)
            .write_image(img.as_raw(), 300, 100, image::ExtendedColorType::Rgb8)
            .unwrap();

        // The gallery cover case: the sheet is exactly the advertised box.
        let cover = crop_tile(&sheet, 0, 0, 300, 100).expect("the whole sheet is one box");
        assert_eq!((cover.width, cover.height), (300, 100));
        assert_eq!(cover.mime, "image/jpeg", "an opaque tile comes back as jpeg");

        // And the grid case: the same sheet cut at the offsets the page names.
        let second = crop_tile(&sheet, 100, 0, 100, 100).expect("the second tile");
        assert_eq!((second.width, second.height), (100, 100));
        let decoded = image::load_from_memory(&second.bytes)
            .expect("the crop must be a real picture")
            .to_rgb8();
        let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
        for p in decoded.pixels() {
            r += u64::from(p[0]);
            g += u64::from(p[1]);
            b += u64::from(p[2]);
        }
        assert!(
            g > r && g > b,
            "the box at x=100 is the green tile, got averages r={r} g={g} b={b}"
        );

        // Refusals, each with the reason in the message.
        // `.err()` rather than `expect_err()`: `Thumbnail` carries the bytes, so
        // it deliberately has no `Debug` and a failed unwrap would print them.
        let off = crop_tile(&sheet, 250, 0, 100, 100).err().expect("hangs off the right edge");
        assert!(off.contains("300x100"), "the sheet size must be in the reason: {off}");
        assert!(crop_tile(&sheet, 0, 100, 10, 10).is_err(), "starts below the sheet");
        assert!(crop_tile(&sheet, 0, 0, 0, 10).is_err(), "a zero-sized box");
        assert!(
            crop_tile(&sheet, 0, 0, 5000, 4000).is_err(),
            "an advertised box bigger than any sane tile is refused"
        );
        assert!(
            crop_tile(b"not a picture at all", 0, 0, 10, 10).is_err(),
            "an undecodable sheet says so"
        );
    }
}
