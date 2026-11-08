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
use serde::Serialize;
use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

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
}
