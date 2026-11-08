//! #25 (increment 1): mod/tool launch recipes (D10).
//!
//! Thin recipes, not reimplementations: the app records a tool's `.exe`
//! (MTool, Cheat Engine, anything else) per game and can start it with the
//! stored arguments — exactly the user's current workflow, one click shorter.
//! The app never injects, wraps or patches anything.
//!
//! Recipe kinds:
//! - `mtool`  → start the tool with the *game's* resolved exe as its argument
//!              (MTool's "pass the game to inject" flow).
//! - `cheat`  → start Cheat Engine; attaching stays inside CE (its CLI is
//!              unreliable, so we don't pretend otherwise).
//! - `other`  → start the tool with the stored args; `{game}` inside an arg
//!              is replaced by the game's exe path.
//!
//! DLL mod apply/revert (flowbak backups + manifests) is the next increment
//! and will grow this table, not fork it.

use crate::launcher::LaunchPlan;
use goon_core::db::ModRow;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ModLaunch {
    pub ok: bool,
    pub exe: Option<String>,
    pub args: Vec<String>,
    pub cwd: Option<String>,
    pub note: Option<String>,
    /// filled only after a successful spawn
    pub pid: Option<u32>,
}

/// Split a stored args string like a shell would: whitespace separates,
/// double quotes group (quotes are stripped). Not full POSIX escaping —
/// recipes are short, and the UI preview shows exactly what will run.
pub fn split_args(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has = false;
    for c in s.chars() {
        match c {
            '"' => in_quotes = !in_quotes,
            c if c.is_whitespace() && !in_quotes => {
                if has {
                    out.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => {
                cur.push(c);
                has = true;
            }
        }
    }
    if has {
        out.push(cur);
    }
    out
}

/// Replace `{game}` with the game exe path in every arg.
fn expand_game(args: Vec<String>, game_exe: Option<&str>) -> Vec<String> {
    args.into_iter()
        .map(|a| match game_exe {
            Some(g) => a.replace("{game}", g),
            None => a,
        })
        .collect()
}

/// What launching this recipe would run — never spawns.
/// `game` is the game's own resolved launch plan (may be !ok).
pub fn recipe(m: &ModRow, game: &LaunchPlan) -> ModLaunch {
    let path = PathBuf::from(&m.path);
    let base = |exe: Option<String>, args: Vec<String>, note: Option<String>| ModLaunch {
        ok: exe.is_some(),
        exe,
        args,
        cwd: path.parent().map(|p| p.display().to_string()),
        note,
        pid: None,
    };

    if !path.exists() {
        return base(
            None,
            Vec::new(),
            Some(format!(
                "{} is not reachable (moved, renamed, or the drive is unplugged)",
                path.display()
            )),
        );
    }
    let is_exe = path
        .extension()
        .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("exe"));
    if path.is_dir() || !is_exe {
        return base(
            None,
            Vec::new(),
            Some(format!(
                "{} is not an .exe — point the recipe at the tool's executable",
                path.display()
            )),
        );
    }

    let game_exe = game.exe.clone();
    let stored = split_args(&m.args);
    match m.kind.as_str() {
        "mtool" => {
            let note = match &game_exe {
                Some(_) => None,
                None => Some(
                    "the game has no resolvable launcher exe — the tool starts without a target (set a launcher override or link the game folder)"
                        .into(),
                ),
            };
            let args = game_exe.into_iter().collect();
            base(Some(m.path.clone()), args, note)
        }
        "cheat" => base(
            Some(m.path.clone()),
            expand_game(stored, game_exe.as_deref()),
            Some("attach to the game inside Cheat Engine".into()),
        ),
        _ => base(
            Some(m.path.clone()),
            expand_game(stored, game_exe.as_deref()),
            None,
        ),
    }
}

/// Actually start it. Returns the plan with `pid` filled.
pub fn launch(l: &ModLaunch) -> Result<ModLaunch, String> {
    if !l.ok {
        return Err(l.note.clone().unwrap_or_else(|| "nothing to launch".into()));
    }
    let exe = l.exe.as_ref().ok_or("nothing to launch")?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(&l.args);
    if let Some(cwd) = &l.cwd {
        cmd.current_dir(cwd);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start {exe}: {e}"))?;
    let mut out = l.clone();
    out.pid = Some(child.id());
    // Like games: the tool runs independently of the app.
    Ok(out)
}

// ---------- dll / patch mod deploy (#25 increment 2) ----------
//
// Apply = deploy the mod's files into the game folder, but *never* destroy
// anything: every file we replace is first moved to `<name>.flowbak` (only if
// no flowbak exists yet — the original game file always wins), and the exact
// file list is recorded in the manifest so Revert can undo it. Everything is
// pre-screened (zip-slip names, directory collisions) so a bad payload never
// lands half-applied; a mid-run failure rolls the whole attempt back.

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ManifestEntry {
    /// path inside the game folder, relative, with '/' separators
    pub file: String,
    /// true when an original was backed up to `<file>.flowbak`
    pub had_backup: bool,
    /// sha256 of the deployed mod bytes (integrity + clobber detection)
    pub sha256: String,
}

pub const BAK_SUFFIX: &str = ".flowbak";

/// Reject traversal/absolute entry names (same discipline as extract.rs).
fn safe_rel(name: &str) -> Option<()> {
    let rel = Path::new(name);
    for c in rel.components() {
        match c {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(())
}

/// List the mod payload's files (relative names, '/' separators) from a
/// folder or zip, pre-screened for safety.
fn payload_files(payload: &Path) -> Result<Vec<String>, String> {
    if payload.is_dir() {
        let mut out = Vec::new();
        let mut stack = vec![payload.to_path_buf()];
        while let Some(dir) = stack.pop() {
            for e in fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))? {
                let e = e.map_err(|e| e.to_string())?;
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    let rel = p
                        .strip_prefix(payload)
                        .map_err(|_| "payload walk slipped".to_string())?
                        .to_string_lossy()
                        .replace('\\', "/");
                    safe_rel(&rel).ok_or_else(|| format!("payload entry \"{rel}\" escapes the game folder"))?;
                    out.push(rel);
                }
            }
        }
        Ok(out)
    } else if payload
        .extension()
        .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("zip"))
    {
        let file = fs::File::open(payload).map_err(|e| format!("cannot open {}: {e}", payload.display()))?;
        let mut za = zip::ZipArchive::new(file).map_err(|e| format!("unreadable zip: {e}"))?;
        let mut out = Vec::new();
        for i in 0..za.len() {
            let entry = za.by_index(i).map_err(|e| e.to_string())?;
            if entry.is_dir() {
                continue;
            }
            let rel = entry.name().replace('\\', "/");
            safe_rel(&rel).ok_or_else(|| format!("archive entry \"{rel}\" escapes the game folder"))?;
            out.push(rel);
        }
        Ok(out)
    } else {
        // single loose file (a bare .dll) — deploys under its own name
        let name = payload
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .ok_or("payload has no file name")?;
        Ok(vec![name])
    }
}

/// Read one payload entry's bytes (folder file, zip entry, or loose file).
fn payload_bytes(payload: &Path, rel: &str) -> Result<Vec<u8>, String> {
    if payload.is_dir() {
        fs::read(payload.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)))
            .map_err(|e| format!("{rel}: {e}"))
    } else if payload
        .extension()
        .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("zip"))
    {
        let file = fs::File::open(payload).map_err(|e| e.to_string())?;
        let mut za = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
        let mut entry = za.by_name(rel).map_err(|e| format!("{rel}: {e}"))?;
        let mut buf = Vec::new();
        io::Read::read_to_end(&mut entry, &mut buf).map_err(|e| format!("{rel}: {e}"))?;
        Ok(buf)
    } else {
        fs::read(payload).map_err(|e| e.to_string())
    }
}

/// Deploy the payload into `game_dir`. Returns the manifest on success; on
/// any failure everything already done is rolled back.
pub fn apply(payload: &Path, game_dir: &Path) -> Result<Vec<ManifestEntry>, String> {
    if !game_dir.is_dir() {
        return Err(format!("game folder {} is not reachable", game_dir.display()));
    }
    let files = payload_files(payload)?;
    if files.is_empty() {
        return Err("the mod payload contains no files".into());
    }

    // Pre-flight: no mod file may land on top of a directory.
    for rel in &files {
        let target = game_dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
        if target.is_dir() {
            return Err(format!(
                "{} is a folder in the game — a mod file cannot replace it",
                rel
            ));
        }
    }

    let mut done: Vec<ManifestEntry> = Vec::new();
    let run = (|| -> Result<(), String> {
        for rel in &files {
            let bytes = payload_bytes(payload, rel)?;
            let target = game_dir.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
            let bak = {
                let mut s = target.clone().into_os_string();
                s.push(BAK_SUFFIX);
                PathBuf::from(s)
            };
            let had_backup = target.is_file();
            if had_backup && !bak.exists() {
                fs::rename(&target, &bak)
                    .map_err(|e| format!("could not back up {rel}: {e}"))?;
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(|e| format!("{rel}: {e}"))?;
            }
            fs::write(&target, &bytes).map_err(|e| format!("{rel}: {e}"))?;
            done.push(ManifestEntry {
                file: rel.clone(),
                had_backup,
                sha256: goon_core::media::sha256_hex(&bytes),
            });
        }
        Ok(())
    })();
    if let Err(e) = run {
        // Roll everything back so the game is exactly as it was before.
        for entry in done.iter().rev() {
            let target = game_dir.join(entry.file.replace('/', std::path::MAIN_SEPARATOR_STR));
            let _ = fs::remove_file(&target);
            if entry.had_backup {
                let mut s = target.into_os_string();
                s.push(BAK_SUFFIX);
                let _ = fs::rename(PathBuf::from(s), game_dir.join(entry.file.replace('/', std::path::MAIN_SEPARATOR_STR)));
            }
        }
        return Err(e);
    }
    Ok(done)
}

/// Undo a recorded deploy. Missing flowbaks (a game update already wiped
/// them) are reported per file instead of aborting the whole revert.
pub fn revert(game_dir: &Path, manifest: &[ManifestEntry]) -> Vec<String> {
    let mut notes = Vec::new();
    for entry in manifest.iter().rev() {
        let sep = entry.file.replace('/', std::path::MAIN_SEPARATOR_STR);
        let target = game_dir.join(&sep);
        if entry.had_backup {
            let mut s = target.clone().into_os_string();
            s.push(BAK_SUFFIX);
            let bak = PathBuf::from(s);
            if bak.exists() {
                let _ = fs::remove_file(&target);
                if let Err(e) = fs::rename(&bak, &target) {
                    notes.push(format!("could not restore {}: {e}", entry.file));
                }
            } else {
                notes.push(format!(
                    "{}: no .flowbak backup to restore (an update probably removed it)",
                    entry.file
                ));
            }
        } else if target.exists() {
            let _ = fs::remove_file(&target);
        }
    }
    notes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(ok: bool, exe: Option<&str>) -> LaunchPlan {
        LaunchPlan {
            ok,
            exe: exe.map(String::from),
            cwd: None,
            engine: None,
            note: None,
            pid: None,
        }
    }

    fn row(kind: &str, path: &Path, args: &str) -> ModRow {
        ModRow {
            id: 1,
            name: "t".into(),
            kind: kind.into(),
            path: path.display().to_string(),
            args: args.into(),
            created_at: "0".into(),
            status: "linked".into(),
            manifest: None,
        }
    }

    #[test]
    fn args_split_handles_quotes() {
        assert_eq!(split_args(""), Vec::<String>::new());
        assert_eq!(split_args("a b\tc"), vec!["a", "b", "c"]);
        assert_eq!(
            split_args(r#"--open "C:\My Game\save slot 1.sav" -x"#),
            vec!["--open", r"C:\My Game\save slot 1.sav", "-x"]
        );
    }

    #[test]
    fn mtool_recipe_passes_game_exe_as_argument() {
        let b = std::env::temp_dir().join("goon-mods-mtool");
        let _ = std::fs::remove_dir_all(&b);
        std::fs::create_dir_all(&b).unwrap();
        let tool = b.join("MTool.exe");
        std::fs::write(&tool, b"MZ").unwrap();

        let r = recipe(&row("mtool", &tool, ""), &plan(true, Some(r"D:\g\Karryn.exe")));
        assert!(r.ok);
        assert_eq!(r.args, vec![r"D:\g\Karryn.exe"]);
        assert_eq!(r.cwd.as_deref(), Some(b.display().to_string().as_str()));

        // no game exe resolved: still launches the tool, with an explanation
        let r = recipe(&row("mtool", &tool, ""), &plan(false, None));
        assert!(r.ok);
        assert!(r.args.is_empty());
        assert!(r.note.unwrap().contains("no resolvable launcher"));
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn other_recipe_expands_game_placeholder() {
        let b = std::env::temp_dir().join("goon-mods-other");
        let _ = std::fs::remove_dir_all(&b);
        std::fs::create_dir_all(&b).unwrap();
        let tool = b.join("patcher.exe");
        std::fs::write(&tool, b"MZ").unwrap();

        let r = recipe(
            &row("other", &tool, r#"--target "{game}" --silent"#),
            &plan(true, Some(r"D:\g\Karryn.exe")),
        );
        assert!(r.ok);
        assert_eq!(r.args, vec!["--target", r"D:\g\Karryn.exe", "--silent"]);
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn dead_or_wrong_paths_explain_themselves() {
        let b = std::env::temp_dir().join("goon-mods-dead");
        let _ = std::fs::remove_dir_all(&b);
        std::fs::create_dir_all(&b).unwrap();
        let dir = b.join("toolfolder");
        std::fs::create_dir_all(&dir).unwrap();

        let r = recipe(&row("cheat", &b.join("gone.exe"), ""), &plan(false, None));
        assert!(!r.ok);
        assert!(r.note.unwrap().contains("not reachable"));

        let r = recipe(&row("cheat", &dir, ""), &plan(false, None));
        assert!(!r.ok);
        assert!(r.note.unwrap().contains("not an .exe"));
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// #25 increment 2: apply backs originals up as *.flowbak, revert
    /// restores them and removes files the mod added.
    #[test]
    fn apply_revert_roundtrip_folder_payload() {
        let b = std::env::temp_dir().join("goon-mods-apply");
        let _ = std::fs::remove_dir_all(&b);
        let game = b.join("game");
        let payload = b.join("modfiles");
        std::fs::create_dir_all(&game).unwrap();
        std::fs::create_dir_all(payload.join("data")).unwrap();
        std::fs::write(game.join("Game.exe"), b"ORIGINAL").unwrap();
        std::fs::write(payload.join("Game.exe"), b"PATCHED").unwrap();
        std::fs::write(payload.join("data").join("extra.dat"), b"NEW").unwrap();

        let m = apply(&payload, &game).expect("apply");
        assert_eq!(m.len(), 2);
        assert_eq!(fs::read(game.join("Game.exe")).unwrap(), b"PATCHED");
        assert_eq!(fs::read(game.join("Game.exe.flowbak")).unwrap(), b"ORIGINAL");
        assert!(game.join("data").join("extra.dat").is_file());

        let notes = revert(&game, &m);
        assert!(notes.is_empty(), "notes: {notes:?}");
        assert_eq!(fs::read(game.join("Game.exe")).unwrap(), b"ORIGINAL");
        assert!(!game.join("Game.exe.flowbak").exists());
        assert!(!game.join("data").join("extra.dat").exists());
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// A zip payload deploys the same way; the sha256 in the manifest matches
    /// the deployed bytes (clobber detection will use it).
    #[test]
    fn apply_zip_payload_records_hashes() {
        let b = std::env::temp_dir().join("goon-mods-zip");
        let _ = std::fs::remove_dir_all(&b);
        let game = b.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let z = b.join("mod.zip");
        let f = fs::File::create(&z).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::FileOptions::<()>::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("sub/mod.dll", opts).unwrap();
        io::Write::write_all(&mut zw, b"DLLBYTES").unwrap();
        zw.finish().unwrap();

        let m = apply(&z, &game).expect("apply zip");
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].file, "sub/mod.dll");
        assert!(!m[0].had_backup, "new file has no flowbak");
        assert_eq!(
            m[0].sha256,
            goon_core::media::sha256_hex(b"DLLBYTES")
        );
        assert!(game.join("sub").join("mod.dll").is_file());
        revert(&game, &m);
        assert!(!game.join("sub").join("mod.dll").exists());
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// A zip-slip entry aborts the whole apply before anything is written.
    #[test]
    fn apply_refuses_escaping_entries() {
        let b = std::env::temp_dir().join("goon-mods-slip");
        let _ = std::fs::remove_dir_all(&b);
        let game = b.join("game");
        std::fs::create_dir_all(&game).unwrap();
        let z = b.join("evil.zip");
        let f = fs::File::create(&z).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts = zip::write::FileOptions::<()>::default()
            .compression_method(zip::CompressionMethod::Stored);
        zw.start_file("ok.dll", opts).unwrap();
        io::Write::write_all(&mut zw, b"x").unwrap();
        zw.start_file("../escaped.dll", opts).unwrap();
        io::Write::write_all(&mut zw, b"x").unwrap();
        zw.finish().unwrap();

        let e = apply(&z, &game).unwrap_err();
        assert!(e.contains("escapes"), "message was: {e}");
        assert!(!game.join("ok.dll").exists(), "nothing may be written");
        assert!(!b.join("escaped.dll").exists());
        std::fs::remove_dir_all(&b).unwrap();
    }

    /// A mod file that would overwrite an existing *directory* aborts clean.
    #[test]
    fn apply_refuses_directory_collisions() {
        let b = std::env::temp_dir().join("goon-mods-dircol");
        let _ = std::fs::remove_dir_all(&b);
        let game = b.join("game");
        std::fs::create_dir_all(game.join("Data")).unwrap();
        let payload = b.join("payload");
        std::fs::create_dir_all(&payload).unwrap();
        std::fs::write(payload.join("Data"), b"i am a file pretending to be a folder").unwrap();

        let e = apply(&payload, &game).unwrap_err();
        assert!(e.contains("is a folder"), "message was: {e}");
        assert!(game.join("Data").is_dir());
        std::fs::remove_dir_all(&b).unwrap();
    }
}
