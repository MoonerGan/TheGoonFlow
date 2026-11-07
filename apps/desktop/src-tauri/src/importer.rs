//! D13: game-folder and image-batch import.
//!
//! Game detection is fingerprint-based, no scraping: each engine leaves a
//! distinctive layout, and several store their *real* title on disk
//! (RPG Maker `www/data/System.json` / `Game.ini`, NW `package.json`,
//! Unity `*_Data` folder name). Versions come from `[v1.23]`-style folder
//! name tokens. Cover art is opportunistically extracted for thumbnails.
//! Everything lands as a normal item the user can then correct.

use crate::{new_id, now_epoch, AppState};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, ItemKind, Status};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub with_thumb: usize,
    pub titles: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageImportReport {
    pub item_id: String,
    pub title: String,
    pub images_stored: usize,
    pub failed: usize,
}

pub fn import_game_folder(state: &AppState, root: String) -> Result<GameImportReport, String> {
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err("folder does not exist".into());
    }
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;

    // Either the picked folder *is* a game, or import its immediate game subfolders.
    let candidates: Vec<PathBuf> = if detect_engine(&root).engine.is_some() {
        vec![root]
    } else {
        let mut v = Vec::new();
        if let Ok(rd) = fs::read_dir(&root) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && detect_engine(&p).engine.is_some() {
                    v.push(p);
                }
            }
        }
        v.sort();
        v
    };

    let mut report = GameImportReport {
        imported: 0,
        skipped: 0,
        with_thumb: 0,
        titles: Vec::new(),
    };
    let now = now_epoch();

    for dir in candidates {
        let url = format!("file:///{}", dir.to_string_lossy().replace('\\', "/"));
        if db::source_exists(conn, "local", &url) {
            report.skipped += 1;
            continue;
        }
        let raw_name = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".into());
        let (clean_title, version) = clean_name(&raw_name);
        let engine = detect_engine(&dir);
        let title = engine
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(clean_title);

        let id = new_id("g");
        let item = Item {
            id: id.clone(),
            kind: ItemKind::Game,
            title: title.clone(),
            original_title: None,
            circle: None,
            status: Status::Downloaded,
            dnf: false,
            rating: None,
            notes: String::new(),
            version,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        db::insert_item(conn, &item).map_err(|e| e.to_string())?;
        db::insert_source(conn, &id, "local", &url, None, Some(&now)).map_err(|e| e.to_string())?;

        if let Some(tag) = engine.tag {
            if let Ok(tag_id) = db::tag_get_or_create(conn, tag, db::TYPE_SITE_FLAG) {
                let _ = db::link_item_tag(conn, &id, tag_id);
            }
        }
        if let Some(Ok(cover)) = engine.cover_path.as_ref().map(fs::read) {
            if media::put_thumbnail(conn, &id, &cover) {
                report.with_thumb += 1;
            }
        }
        report.imported += 1;
        report.titles.push(title);
    }
    Ok(report)
}

pub fn import_image_files(state: &AppState, paths: Vec<String>) -> Result<ImageImportReport, String> {
    if paths.is_empty() {
        return Err("no files selected".into());
    }
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;

    let title = batch_title(&paths);
    let now = now_epoch();
    let id = new_id("i");
    let item = Item {
        id: id.clone(),
        kind: ItemKind::Gallery,
        title: title.clone(),
        original_title: None,
        circle: None,
        status: Status::Downloaded,
        dnf: false,
        rating: None,
        notes: String::new(),
        version: None,
        created_at: now.clone(),
        updated_at: now.clone(),
    };
    db::insert_item(conn, &item).map_err(|e| e.to_string())?;

    let mut stored = 0usize;
    let mut failed = 0usize;
    let mut thumb_done = false;
    for p in paths {
        match fs::read(&p) {
            Ok(bytes) => {
                if media::put_full_image(conn, &id, &bytes) {
                    stored += 1;
                    if !thumb_done {
                        thumb_done = media::put_thumbnail(conn, &id, &bytes);
                    }
                } else {
                    failed += 1;
                }
            }
            Err(_) => failed += 1,
        }
    }
    if stored == 0 {
        let _ = db::delete_item(conn, &id);
        return Err("no importable images among the selection".into());
    }
    Ok(ImageImportReport {
        item_id: id,
        title,
        images_stored: stored,
        failed,
    })
}

// ---------- engine fingerprinting ----------

pub struct EngineInfo {
    pub engine: Option<&'static str>,
    pub tag: Option<&'static str>,
    pub title: Option<String>,
    pub cover_path: Option<PathBuf>,
}

pub fn detect_engine(dir: &Path) -> EngineInfo {
    // RPG Maker MV / MZ
    if dir.join("game.rmmzproject").is_file() || dir.join("game.rmvproject").is_file() {
        return EngineInfo {
            engine: Some("rpg-maker"),
            tag: Some("rpg-maker"),
            title: rpgm_title(dir),
            cover_path: first_image_in(&dir.join("www").join("img").join("titles")),
        };
    }
    // RPG Maker XP / VX / Ace
    if dir.join("Game.exe").is_file() && dir.join("Game.ini").is_file() {
        let cover = first_image_in(&dir.join("Graphics").join("Titles1"))
            .or_else(|| first_image_in(&dir.join("Graphics").join("Title1")));
        return EngineInfo {
            engine: Some("rpg-maker"),
            tag: Some("rpg-maker"),
            title: ini_title(&dir.join("Game.ini")),
            cover_path: cover,
        };
    }
    // Ren'Py
    let game_dir = dir.join("game");
    if game_dir.is_dir() && (dir.join("renpy").is_dir() || has_ext(&game_dir, "rpy")) {
        let cover = [
            game_dir.join("gui").join("window_icon.png"),
            game_dir.join("images").join("ui").join("window_icon.png"),
        ]
        .into_iter()
        .find(|p| p.is_file());
        return EngineInfo {
            engine: Some("renpy"),
            tag: Some("renpy"),
            title: None,
            cover_path: cover,
        };
    }
    // Unity
    if let Some(data_dir) = sub_dirs_ending(dir, "_Data") {
        let name = data_dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        return EngineInfo {
            engine: Some("unity"),
            tag: Some("unity"),
            title: Some(name.trim_end_matches("_Data").trim().to_string()),
            cover_path: None,
        };
    }
    // GameMaker
    if dir.join("data.win").is_file() {
        return EngineInfo {
            engine: Some("gamemaker"),
            tag: Some("gamemaker"),
            title: None,
            cover_path: None,
        };
    }
    // NW.js / HTML5
    if dir.join("package.json").is_file() && dir.join("index.html").is_file() {
        return EngineInfo {
            engine: Some("html5"),
            tag: Some("html5"),
            title: package_json_title(&dir.join("package.json")),
            cover_path: None,
        };
    }
    // Generic Windows game: exe at top level.
    if has_ext(dir, "exe") {
        return EngineInfo {
            engine: Some("pc-game"),
            tag: Some("pc-game"),
            title: None,
            cover_path: None,
        };
    }
    EngineInfo {
        engine: None,
        tag: None,
        title: None,
        cover_path: None,
    }
}

fn has_ext(dir: &Path, ext: &str) -> bool {
    fs::read_dir(dir)
        .map(|rd| {
            rd.flatten().any(|e| {
                e.path()
                    .extension()
                    .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case(ext))
            })
        })
        .unwrap_or(false)
}

fn sub_dirs_ending(dir: &Path, suffix: &str) -> Option<PathBuf> {
    fs::read_dir(dir).ok()?.flatten().find_map(|e| {
        let p = e.path();
        (p.is_dir()
            && p.file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(suffix)))
        .then_some(p)
    })
}

fn first_image_in(dir: &Path) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| {
                let x = x.to_string_lossy().to_lowercase();
                matches!(x.as_str(), "png" | "jpg" | "jpeg" | "webp" | "bmp" | "gif")
            })
        })
        .collect();
    entries.sort();
    entries.into_iter().next()
}

fn rpgm_title(dir: &Path) -> Option<String> {
    let raw = fs::read_to_string(dir.join("www").join("data").join("System.json")).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    v.get("gameTitle")
        .and_then(|t| t.as_str())
        .filter(|t| !t.trim().is_empty())
        .map(String::from)
}

fn ini_title(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    raw.lines()
        .find_map(|l| l.trim().strip_prefix("Title="))
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(String::from)
}

fn package_json_title(path: &Path) -> Option<String> {
    let raw = fs::read_to_string(path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
    ["title", "name"]
        .iter()
        .filter_map(|k| v.get(*k).and_then(|x| x.as_str()))
        .find(|s| {
            !s.trim().is_empty()
                && !matches!(
                    s.trim().to_lowercase().as_str(),
                    "game" | "app" | "nw" | "index" | "project"
                )
        })
        .map(String::from)
}

/// `Title [v0.12] [renpy] [18+]` → ("Title", Some("0.12"))
pub fn clean_name(raw: &str) -> (String, Option<String>) {
    let mut title = String::new();
    let mut version: Option<String> = None;
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
            if version.is_none() {
                version = version_from_token(&tok);
            }
        } else {
            title.push(c);
            i += 1;
        }
    }
    let trimmed = title.trim_matches(&[' ', '-', '–', '—', '_', '·', '|', ','][..]).trim();
    let title = if trimmed.is_empty() {
        raw.trim().to_string()
    } else {
        trimmed.split_whitespace().collect::<Vec<_>>().join(" ")
    };
    (title, version)
}

fn version_from_token(tok: &str) -> Option<String> {
    let t = tok.trim().to_lowercase();
    let rest = t.strip_prefix('v')?;
    if !rest.chars().next()?.is_ascii_digit() {
        return None;
    }
    let v: String = tok
        .trim()
        .trim_start_matches(['v', 'V'])
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    if v.is_empty() {
        None
    } else {
        Some(v.trim_matches('.').to_string())
    }
}

fn batch_title(paths: &[String]) -> String {
    if paths.len() == 1 {
        return Path::new(&paths[0])
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Imported image".into());
    }
    let first = Path::new(&paths[0]);
    let same_parent = paths.iter().all(|p| Path::new(p).parent() == first.parent());
    match (same_parent, first.parent().and_then(|p| p.file_name())) {
        (true, Some(name)) => name.to_string_lossy().to_string(),
        _ => format!("Imported images ({})", paths.len()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_name_strips_bracket_tokens_and_finds_version() {
        let (title, version) = clean_name("Summer Witch [v1.02 PR3] [oginomesoft] [renpy]");
        assert_eq!(title, "Summer Witch");
        assert_eq!(version.as_deref(), Some("1.02"));
    }

    #[test]
    fn clean_name_no_version() {
        let (title, version) = clean_name("Karryn's Prison [18+]");
        assert_eq!(title, "Karryn's Prison");
        assert_eq!(version, None);
    }

    #[test]
    fn clean_name_falls_back_to_raw() {
        let (title, _) = clean_name("[v1.0]");
        assert_eq!(title, "[v1.0]");
    }

    #[test]
    fn detects_renpy_and_rpgm_layouts() {
        let base = std::env::temp_dir().join("goon-import-test");
        let _ = fs::remove_dir_all(&base);

        let renpy = base.join("Some Game [v0.5]");
        fs::create_dir_all(renpy.join("game")).unwrap();
        fs::create_dir_all(renpy.join("renpy")).unwrap();
        fs::write(renpy.join("game").join("script.rpy"), "//").unwrap();
        let info = detect_engine(&renpy);
        assert_eq!(info.engine, Some("renpy"));

        let rpgm = base.join("RPGM Game");
        fs::create_dir_all(rpgm.join("www").join("data")).unwrap();
        fs::write(rpgm.join("game.rmmzproject"), "").unwrap();
        fs::write(
            rpgm.join("www").join("data").join("System.json"),
            r#"{"gameTitle":"Real Title Here"}"#,
        )
        .unwrap();
        let info = detect_engine(&rpgm);
        assert_eq!(info.engine, Some("rpg-maker"));
        assert_eq!(info.title.as_deref(), Some("Real Title Here"));

        // Not a game: random folder.
        let random = base.join("photos");
        fs::create_dir_all(&random).unwrap();
        fs::write(random.join("a.txt"), "hi").unwrap();
        assert!(detect_engine(&random).engine.is_none());

        let _ = fs::remove_dir_all(&base);
    }
}
