//! #18: launch a tracked game.
//!
//! A game is tracked by a `site='local'` source — a folder, a `.zip`, or the
//! folder of a picked `.exe`. To launch it we resolve that path to a concrete
//! executable using the same engine fingerprinting the importer uses, honour a
//! per-game `launch_path` override (set in the editor when auto-detect picks
//! wrong), and start it with its own folder as the working directory (games
//! load assets relative to the exe, so cwd is not optional).
//!
//! Zips are deliberately *not* launched in place — a loose `.zip` has no cwd
//! and games cannot run from inside one, so we report that the user must
//! extract it first (the M3 downloader will own extraction into real folders).

use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct LaunchPlan {
    pub ok: bool,
    /// absolute path to the executable to start
    pub exe: Option<String>,
    /// working directory to start it in (the exe's own folder)
    pub cwd: Option<String>,
    /// engine we fingerprinted, when known
    pub engine: Option<&'static str>,
    /// human-readable why-not / hint when `ok` is false
    pub note: Option<String>,
    /// set only after a real launch succeeds
    pub pid: Option<u32>,
}

/// Names that look like a game exe but never launch the game.
fn is_helper_exe(stem_lower: &str) -> bool {
    const HINTS: &[&str] = &[
        "setup",
        "install",
        "uninstall",
        "update",
        "patch",
        "vcredist",
        "directx",
        "dotnet",
        "crashreport",
        "crash_report",
        "readme",
        "support",
    ];
    HINTS.iter().any(|h| stem_lower.contains(h))
}

fn top_level_exes(dir: &Path) -> Vec<PathBuf> {
    let Ok(rd) = fs(dir) else {
        return Vec::new();
    };
    let mut exes: Vec<PathBuf> = rd
        .into_iter()
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("exe"))
        })
        .collect();
    exes.sort();
    exes
}

fn fs(dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    Ok(std::fs::read_dir(dir)?
        .flatten()
        .map(|e| e.path())
        .collect())
}

/// Pick the real launcher among a folder's executables, engine-aware.
fn pick_exe(dir: &Path, engine: Option<&str>) -> Option<PathBuf> {
    let exes = top_level_exes(dir);
    if exes.is_empty() {
        return None;
    }
    // Unity: the exe shares its name with `<Name>_Data`.
    if engine == Some("unity") {
        if let Some(data) = crate::importer::sub_dirs_ending(dir, "_Data") {
            let name = data
                .file_name()
                .map(|s| s.to_string_lossy().trim_end_matches("_Data").to_lowercase())
                .unwrap_or_default();
            if let Some(hit) = exes.iter().find(|p| {
                p.file_stem().map(|s| s.to_string_lossy().to_lowercase()) == Some(name.clone())
            }) {
                return Some(hit.clone());
            }
        }
    }
    // RPG Maker XP/VX/Ace standard launcher.
    if engine == Some("rpg-maker") {
        if let Some(hit) = exes.iter().find(|p| {
            p.file_stem()
                .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case("Game"))
        }) {
            return Some(hit.clone());
        }
    }
    // Prefer an exe named after the folder (the common repack convention).
    let folder = dir
        .file_name()
        .map(|s| s.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if let Some(hit) = exes
        .iter()
        .find(|p| p.file_stem().map(|s| s.to_string_lossy().to_lowercase()) == Some(folder.clone()))
    {
        return Some(hit.clone());
    }
    // Otherwise the first non-helper exe; if all look like helpers, take the first.
    exes.iter()
        .find(|p| {
            !is_helper_exe(
                &p.file_stem()
                    .map(|s| s.to_string_lossy().to_lowercase())
                    .unwrap_or_default(),
            )
        })
        .cloned()
        .or_else(|| exes.first().cloned())
}

/// Resolve what launching this item would run — never spawns. `local_path` is
/// the decoded `file:///` path of a local source; `override_path` is the
/// per-game launch_path column.
pub fn resolve(local_path: Option<&Path>, override_path: Option<&str>) -> LaunchPlan {
    // 1. explicit override wins if it still exists
    if let Some(o) = override_path.map(PathBuf::from) {
        if o.is_file() {
            return LaunchPlan {
                ok: true,
                exe: Some(o.display().to_string()),
                cwd: o.parent().map(|p| p.display().to_string()),
                engine: None,
                note: Some("custom launcher (your override)".into()),
                pid: None,
            };
        }
        return LaunchPlan {
            ok: false,
            exe: None,
            cwd: None,
            engine: None,
            note: Some(format!(
                "your saved launcher is gone: {} — re-pick it or clear the override",
                o.display()
            )),
            pid: None,
        };
    }

    let Some(path) = local_path else {
        return LaunchPlan {
            ok: false,
            exe: None,
            cwd: None,
            engine: None,
            note: Some(
                "this game has no local file source to launch (add one under Sources)".into(),
            ),
            pid: None,
        };
    };

    if path.is_file() {
        let is_exe = path
            .extension()
            .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("exe"));
        if is_exe {
            return LaunchPlan {
                ok: true,
                exe: Some(path.display().to_string()),
                cwd: path.parent().map(|p| p.display().to_string()),
                engine: None,
                note: None,
                pid: None,
            };
        }
        let is_zip = path
            .extension()
            .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("zip"));
        return LaunchPlan {
            ok: false,
            exe: None,
            cwd: None,
            engine: None,
            note: Some(if is_zip {
                format!(
                        "{} is a zip — press Extract zip below to extract it into a launchable folder (the archive is never touched)",
                        path.display()
                    )
            } else {
                format!("{} is not an executable", path.display())
            }),
            pid: None,
        };
    }

    if path.is_dir() {
        let info = crate::importer::detect_engine(path);
        if let Some(exe) = pick_exe(path, info.engine) {
            return LaunchPlan {
                ok: true,
                exe: Some(exe.display().to_string()),
                cwd: Some(path.display().to_string()),
                engine: info.engine,
                note: None,
                pid: None,
            };
        }
        return LaunchPlan {
            ok: false,
            exe: None,
            cwd: None,
            engine: info.engine,
            note: Some(format!(
                "no launcher .exe found at the top of {} — set a launcher override in the editor",
                path.display()
            )),
            pid: None,
        };
    }

    LaunchPlan {
        ok: false,
        exe: None,
        cwd: None,
        engine: None,
        note: Some(format!(
            "{} is not reachable (moved, renamed, or the drive is unplugged)",
            path.display()
        )),
        pid: None,
    }
}

/// Actually start the resolved executable. Returns the plan with `pid` filled.
pub fn launch(plan: &LaunchPlan) -> Result<LaunchPlan, String> {
    if !plan.ok {
        return Err(plan
            .note
            .clone()
            .unwrap_or_else(|| "nothing to launch".into()));
    }
    let exe = plan.exe.as_ref().ok_or("nothing to launch")?;
    let mut cmd = std::process::Command::new(exe);
    if let Some(cwd) = &plan.cwd {
        cmd.current_dir(cwd);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("could not start {exe}: {e}"))?;
    let mut out = plan.clone();
    out.pid = Some(child.id());
    // We intentionally do not wait on the child; the game runs independently.
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_base(tag: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("goon-launch-{tag}"));
        let _ = std::fs::remove_dir_all(&b);
        std::fs::create_dir_all(&b).unwrap();
        b
    }

    #[test]
    fn unity_resolves_to_matching_exe() {
        let b = temp_base("unity");
        std::fs::create_dir_all(b.join("Karryn_Data")).unwrap();
        std::fs::write(b.join("Karryn.exe"), b"MZ").unwrap();
        std::fs::write(b.join("setup_directx.exe"), b"MZ").unwrap();
        let p = resolve(Some(&b), None);
        assert!(p.ok);
        assert_eq!(p.engine, Some("unity"));
        assert!(p.exe.unwrap().ends_with("Karryn.exe"));
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn rpgm_prefers_game_exe() {
        let b = temp_base("rpgm");
        std::fs::create_dir_all(b.join("www").join("data")).unwrap();
        std::fs::write(b.join("game.rmmzproject"), b"").unwrap();
        std::fs::write(b.join("Game.exe"), b"MZ").unwrap();
        let p = resolve(Some(&b), None);
        assert!(p.ok);
        assert!(p.exe.unwrap().ends_with("Game.exe"));
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn picks_exe_matching_folder_name_over_helpers() {
        let b = temp_base("renpy");
        let g = b.join("Summer Witch [renpy] [English]");
        std::fs::create_dir_all(g.join("game")).unwrap();
        std::fs::write(g.join("game").join("script.rpy"), b"//").unwrap();
        std::fs::write(g.join("Update.exe"), b"MZ").unwrap();
        std::fs::write(g.join("Summer Witch [renpy] [English].exe"), b"MZ").unwrap();
        let p = resolve(Some(&g), None);
        assert!(p.ok);
        assert_eq!(p.engine, Some("renpy"));
        assert!(p
            .exe
            .unwrap()
            .contains("Summer Witch [renpy] [English].exe"));
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn zip_and_missing_paths_explain_themselves() {
        let b = temp_base("zip");
        let z = b.join("game.zip");
        std::fs::write(&z, b"PK").unwrap();
        let p = resolve(Some(&z), None);
        assert!(!p.ok);
        assert!(p.note.unwrap().contains("extract"));
        let p = resolve(Some(&b.join("nope")), None);
        assert!(!p.ok);
        assert!(p.note.unwrap().contains("not reachable"));
        std::fs::remove_dir_all(&b).unwrap();
    }

    #[test]
    fn override_wins_and_missing_override_is_reported() {
        let b = temp_base("override");
        let real = b.join("Real.exe");
        std::fs::write(&real, b"MZ").unwrap();
        let p = resolve(None, Some(real.to_str().unwrap()));
        assert!(p.ok);
        assert_eq!(p.exe.as_deref(), Some(real.to_str().unwrap()));
        let p = resolve(None, Some(b.join("Gone.exe").to_str().unwrap()));
        assert!(!p.ok);
        assert!(p.note.unwrap().contains("saved launcher is gone"));
        std::fs::remove_dir_all(&b).unwrap();
    }
}
