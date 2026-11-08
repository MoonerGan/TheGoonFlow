//! #17 (increment 1): extract a game archive into a real launchable folder.
//!
//! The launcher (#18) deliberately refuses to run a loose `.zip` — games need
//! a folder (cwd for saves/config). This closes that loop: a game item whose
//! only local source is a `.zip` gets it unpacked *next to the archive*
//! (`D:\games\Foo.zip` → `D:\games\Foo`), and the resulting folder is attached
//! as a second local source so the Launch block lights up. The archive itself
//! is never touched.
//!
//! Guards: zip-slip entries (absolute paths, drive letters, `..`) abort the
//! run and discard the partial folder; single top-level directories are
//! unwrapped so the game folder sits directly at the destination (repack
//! archives almost always contain one); the job stays cancellable and shows
//! progress. 7z/RAR archives are a later increment — only zip today.

use goon_core::db;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use crate::jobs::JobHandle;
use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractPlan {
    /// the archive we would extract
    pub zip: String,
    /// the folder it would land in
    pub dest: String,
    /// true when the folder already exists (re-run just links it)
    pub already_extracted: bool,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ExtractReport {
    pub dest: String,
    pub entries: usize,
    pub bytes: u64,
    pub source_added: bool,
    pub note: Option<String>,
}

/// The first local source that is a zip file.
pub fn zip_source(sources: &[db::SourceRow]) -> Option<PathBuf> {
    sources.iter().find_map(|s| {
        let rest = s.url.strip_prefix("file:///")?;
        let p = PathBuf::from(rest);
        p.extension()?
            .to_string_lossy()
            .eq_ignore_ascii_case("zip")
            .then_some(p)
    })
}

/// What extraction would do for this item (None when no zip source).
pub fn plan(sources: &[db::SourceRow]) -> Option<ExtractPlan> {
    let zip = zip_source(sources)?;
    let dest = dest_for(&zip);
    Some(ExtractPlan {
        zip: zip.display().to_string(),
        dest: dest.display().to_string(),
        already_extracted: dest.is_dir(),
    })
}

/// `D:\games\Foo.zip` → `D:\games\Foo` (keeps dots in names intact).
fn dest_for(zip: &Path) -> PathBuf {
    let stem = zip.file_stem().map(|s| s.to_string_lossy().to_string());
    match (zip.parent(), stem) {
        (Some(parent), Some(stem)) => parent.join(stem),
        _ => PathBuf::from("extracted"),
    }
}

/// Reject traversal/absolute entry names. zip's own enclosed_name() already
/// strips most of this — we double-check and abort instead of silently
/// rewriting a game file's path.
fn safe_join(dest: &Path, name: &str) -> Option<PathBuf> {
    let rel = Path::new(name);
    let mut out = dest.to_path_buf();
    for c in rel.components() {
        match c {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            // ParentDir, RootDir, Prefix (drive letters) all escape the folder
            _ => return None,
        }
    }
    Some(out)
}

#[derive(Debug)]
struct Extracted {
    entries: usize,
    bytes: u64,
}

/// Pull `zip` into `dest`. `tick(i, total, name)` feeds progress; returning
/// true from `cancel` stops and removes the partial folder.
pub fn extract_zip(
    zip: &Path,
    dest: &Path,
    cancel: &dyn Fn() -> bool,
    tick: &mut dyn FnMut(usize, usize, &str),
) -> Result<Extracted, String> {
    let file = fs::File::open(zip).map_err(|e| format!("cannot open {}: {e}", zip.display()))?;
    let mut za = zip::ZipArchive::new(file).map_err(|e| format!("unreadable zip: {e}"))?;
    let total = za.len();

    // Pre-flight: every entry name must stay inside dest. Checking up front
    // means a malicious archive never gets halfway unpacked.
    for i in 0..total {
        let entry = za.by_index(i).map_err(|e| format!("zip read failed: {e}"))?;
        let name = entry.name().to_string();
        if safe_join(dest, &name).is_none() {
            return Err(format!(
                "archive entry \"{name}\" tries to escape the game folder — nothing extracted"
            ));
        }
    }

    let created = !dest.exists();
    fs::create_dir_all(dest).map_err(|e| format!("cannot create {}: {e}", dest.display()))?;

    let mut got = Extracted {
        entries: 0,
        bytes: 0,
    };
    let run = (|| -> Result<(), String> {
        for i in 0..total {
            if cancel() {
                return Err("__cancelled__".into());
            }
            let mut entry = za.by_index(i).map_err(|e| format!("zip read failed: {e}"))?;
            let name = entry.name().to_string();
            let out = safe_join(dest, &name)
                .ok_or_else(|| format!("archive entry \"{name}\" escapes the folder"))?;
            if entry.is_dir() {
                fs::create_dir_all(&out).map_err(|e| format!("{name}: {e}"))?;
            } else {
                if let Some(parent) = out.parent() {
                    fs::create_dir_all(parent).map_err(|e| format!("{name}: {e}"))?;
                }
                let mut of = fs::File::create(&out).map_err(|e| format!("{name}: {e}"))?;
                let n = io::copy(&mut entry, &mut of).map_err(|e| format!("{name}: {e}"))?;
                got.bytes += n;
            }
            got.entries += 1;
            if i % 16 == 0 || i + 1 == total {
                tick(i + 1, total, &name);
            }
        }
        Ok(())
    })();
    if let Err(e) = run {
        if created {
            let _ = fs::remove_dir_all(dest); // never leave half a game behind
        }
        return Err(if e == "__cancelled__" {
            "stopped — partial extraction discarded".into()
        } else {
            e
        });
    }

    Ok(got)
}

/// Job-facing wrapper: extract (or adopt an existing folder) and attach the
/// folder as a local source so the launcher can run it.
pub fn extract_for_item(
    state: &AppState,
    item_id: &str,
    job: &JobHandle,
) -> Result<ExtractReport, String> {
    let sources = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        let Some(d) = db::get_item(&opened.conn, item_id).map_err(crate::err)? else {
            return Err("item not found".into());
        };
        d.sources
    };
    let zip = zip_source(&sources).ok_or("this game has no .zip source to extract")?;
    let dest = dest_for(&zip);

    let mut report = ExtractReport::default();

    if dest.is_dir() {
        // already unpacked (previous run or manual) — just make sure it's linked
        report.dest = dest.display().to_string();
        report.note = Some("the folder already existed — linked it without re-extracting".into());
    } else {
        job.set_progress(0, 1, &zip.display().to_string());
        let got = extract_zip(&zip, &dest, &|| job.cancelled(), &mut |i, n, name| {
            job.set_note(&format!("extracting {i}/{n}: {name}"));
        })?;
        report.dest = dest.display().to_string();
        report.entries = got.entries;
        report.bytes = got.bytes;
    }

    // Repacks usually wrap everything in one folder; link the inner one if the
    // outer dest holds exactly that (top-level exes must be one click deeper).
    let link = game_folder(&dest);

    let url = format!("file:///{}", link.to_string_lossy().replace('\\', "/"));
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault vanished mid-extract")?;
    report.source_added = db::insert_source(&opened.conn, item_id, "local", &url, None, Some(&crate::now_epoch()))
        .unwrap_or(false);
    let _ = db::touch_item(&opened.conn, item_id, &crate::now_epoch());
    Ok(report)
}

/// The folder a launcher should point at: `dest` itself, or its single child
/// directory when the archive wrapped everything in one top folder.
fn game_folder(dest: &Path) -> PathBuf {
    let rd = match fs::read_dir(dest) {
        Ok(rd) => rd,
        Err(_) => return dest.to_path_buf(),
    };
    let entries: Vec<_> = rd.flatten().collect();
    let dirs: Vec<_> = entries.iter().filter(|e| e.path().is_dir()).collect();
    let files = entries.len() - dirs.len();
    if files == 0 && dirs.len() == 1 {
        dirs[0].path()
    } else {
        dest.to_path_buf()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "goon-extract-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// Build a small zip: (entry name, bytes). Names with '/' create paths.
    fn make_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(file);
        for (name, bytes) in entries {
            let opts = zip::write::FileOptions::<()>::default()
                .compression_method(zip::CompressionMethod::Stored);
            zw.start_file(name, opts).unwrap();
            io::Write::write_all(&mut zw, bytes).unwrap();
        }
        zw.finish().unwrap();
    }

    #[test]
    fn dest_is_sibling_and_single_wrap_is_unwrapped() {
        let b = temp_base("dest");
        let z = b.join("Summer Witch [v1.2].zip");
        make_zip(
            &z,
            &[
                ("Summer Witch/game/data.rpy", b"//"),
                ("Summer Witch/Summer Witch.exe", b"MZ"),
            ],
        );
        let dest = dest_for(&z);
        assert_eq!(dest, b.join("Summer Witch [v1.2]"));

        let got = extract_zip(&dest.parent().unwrap().join("nope.zip"), &dest, &|| false, &mut |_, _, _| {});
        assert!(got.is_err(), "missing zip must fail cleanly");

        extract_zip(&z, &dest, &|| false, &mut |_, _, _| {}).unwrap();
        assert!(dest.join("Summer Witch/Summer Witch.exe").is_file());
        // single wrapper → link the inner folder (that's where the exe is)
        assert_eq!(game_folder(&dest), dest.join("Summer Witch"));
    }

    #[test]
    fn flat_archive_links_dest_itself() {
        let b = temp_base("flat");
        let z = b.join("flat.zip");
        make_zip(&z, &[("game.exe", b"MZ"), ("game/data.dat", b"x")]);
        let dest = b.join("flat");
        extract_zip(&z, &dest, &|| false, &mut |_, _, _| {}).unwrap();
        assert_eq!(game_folder(&dest), dest);
    }

    #[test]
    fn zip_slip_is_rejected_and_nothing_is_written() {
        let b = temp_base("slip");
        let z = b.join("evil.zip");
        make_zip(&z, &[("game/ok.txt", b"fine"), ("../escaped.txt", b"pwned")]);
        let dest = b.join("evil");
        let e = extract_zip(&z, &dest, &|| false, &mut |_, _, _| {}).unwrap_err();
        assert!(e.contains("escape"), "message was: {e}");
        assert!(!b.join("escaped.txt").exists(), "escaped file must never appear");
        assert!(!dest.exists(), "partial folder must be discarded");
    }

    #[test]
    fn cancel_mid_extract_leaves_nothing() {
        let b = temp_base("cancel");
        let z = b.join("big.zip");
        let many: Vec<(String, Vec<u8>)> =
            (0..64).map(|i| (format!("f{i}.bin"), vec![i as u8; 16])).collect();
        make_zip(
            &z,
            &many.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect::<Vec<_>>(),
        );
        let dest = b.join("big");
        let e = extract_zip(&z, &dest, &|| true, &mut |_, _, _| {}).unwrap_err();
        assert!(e.contains("stopped"), "message was: {e}");
        assert!(!dest.exists());
    }

    #[test]
    fn zip_source_picks_archives_only() {
        let src = |url: &str| db::SourceRow {
            id: 1,
            site: "local".into(),
            url: url.into(),
            source_id: None,
            fetched_at: None,
            dead: false,
        };
        let list = [src("file:///D:/games/Foo/game.exe"), src("file:///D:/games/Foo.zip")];
        assert_eq!(zip_source(&list), Some(PathBuf::from("D:/games/Foo.zip")));
        assert!(zip_source(&[src("file:///D:/games/Foo/game.exe")]).is_none());
    }
}
