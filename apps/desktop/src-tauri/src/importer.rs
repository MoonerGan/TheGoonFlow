//! D13: game-folder and image-batch import.
//!
//! Game detection is fingerprint-based, no scraping: each engine leaves a
//! distinctive layout, and several store their *real* title on disk
//! (RPG Maker `www/data/System.json` / `Game.ini`, NW `package.json`,
//! Unity `*_Data` folder name). Versions come from `[v1.23]`-style folder
//! name tokens. Cover art is opportunistically extracted for thumbnails.
//! Everything lands as a normal item the user can then correct.

use crate::jobs::JobHandle;
use crate::{new_id, now_epoch, AppState};
use goon_core::db;
use goon_core::media;
use goon_core::model::{Item, ItemKind, Status};
use serde::Serialize;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub with_thumb: usize,
    /// True when the user stopped it (or the vault was locked) part-way.
    pub stopped: bool,
    pub titles: Vec<String>,
    pub failures: Vec<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageImportReport {
    pub item_id: String,
    pub title: String,
    pub images_stored: usize,
    pub failed: usize,
    pub stopped: bool,
}

/// How deep under the picked folder we look for games (library organizers
/// nest by genre/language/circle: `root/renpy/Summer Game [v1]/`).
const MAX_SCAN_DEPTH: usize = 5;

/// Archive extensions: seeing these during a scan means the game is probably
/// still packed — folders, not archives, get fingerprinted.
const ARCHIVE_EXTS: &[&str] = &["zip", "7z", "rar", "001", "z01", "gz", "tar"];

/// Folder names that are engine internals, never library branches — don't
/// descend into these when hunting for game folders.
fn is_internal_dir(name: &str) -> bool {
    const INTERNAL: &[&str] = &[
        "$recycle.bin",
        "system volume information",
        "node_modules",
        "savedata",
        "save",
        "saves",
        "logs",
        "data",
        "locales",
        "python",
        "redist",
        "tools",
        "x64",
        "x86",
        "dll",
    ];
    name.starts_with('.') || INTERNAL.iter().any(|i| name == *i) || name.starts_with("python")
}

/// What a scan noticed along the way — so a scan that finds *nothing* can
/// explain why instead of silently reporting 0/0/0 (field report: importing
/// `Summer Witch [renpy] [English]` said nothing at all about why).
#[derive(Default)]
struct ScanNotes {
    dirs_seen: usize,
    /// entry names at the picked folder's top level (for the "what's inside" line)
    root_top: Vec<String>,
    /// how many archive files were seen at any depth
    archives: usize,
    /// engine-internal dirs we refused to descend into
    skipped_internal: Vec<String>,
    /// folders that hold a `game/` subfolder but still didn't fingerprint
    near_misses: Vec<String>,
}

/// Recursively find game folders under `dir`. A folder that fingerprints as
/// a game is a candidate and we never descend into it (its engine internals
/// would produce false positives).
fn find_game_dirs(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, notes: &mut ScanNotes) {
    notes.dirs_seen += 1;
    if detect_engine(dir).engine.is_some() {
        out.push(dir.to_path_buf());
        return;
    }
    let is_root = notes.root_top.is_empty() && depth == MAX_SCAN_DEPTH;
    let Ok(rd) = fs::read_dir(dir) else {
        return;
    };
    let entries: Vec<PathBuf> = rd.flatten().map(|e| e.path()).collect();
    let mut sub: Vec<PathBuf> = Vec::new();
    for p in entries {
        let name = p.file_name().map(|n| n.to_string_lossy().to_lowercase()).unwrap_or_default();
        let is_dir = p.is_dir();
        if is_root && notes.root_top.len() < 10 {
            notes.root_top.push(p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default());
        }
        if is_dir {
            if is_internal_dir(&name) || name.ends_with("_data") {
                if notes.skipped_internal.len() < 6 {
                    notes.skipped_internal.push(
                        p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    );
                }
                continue;
            }
            if name == "game" && notes.near_misses.len() < 4 {
                notes.near_misses.push(dir.display().to_string());
            }
            sub.push(p);
        } else if p.extension().is_some_and(|x| {
            ARCHIVE_EXTS.iter().any(|a| x.to_string_lossy().eq_ignore_ascii_case(a))
        }) {
            notes.archives += 1;
        }
    }
    if depth == 0 {
        return;
    }
    sub.sort();
    for p in sub {
        find_game_dirs(&p, depth - 1, out, notes);
    }
}

/// Turn an empty scan into a sentence the user can act on.
fn describe_empty_scan(root: &Path, notes: &ScanNotes) -> String {
    let mut msg = format!(
        "no game folder recognized under {} (checked {} folder{} up to {} levels deep)",
        root.display(),
        notes.dirs_seen,
        if notes.dirs_seen == 1 { "" } else { "s" },
        MAX_SCAN_DEPTH + 1,
    );
    if notes.archives > 0 {
        msg.push_str(&format!(
            "; {archives} archive file{ext} seen — games inside 7z/rar archives must be extracted first (a .zip game can be added with + Game zip)",
            archives = notes.archives,
            ext = if notes.archives == 1 { "" } else { "s" },
        ));
    }
    if !notes.near_misses.is_empty() {
        msg.push_str(&format!(
            "; folders holding a `game/` subfolder that still didn't fingerprint: {}",
            notes.near_misses.join(", ")
        ));
    }
    if !notes.skipped_internal.is_empty() {
        msg.push_str(&format!(
            "; skipped engine-internal folders: {}",
            notes.skipped_internal.join(", ")
        ));
    }
    if !notes.root_top.is_empty() {
        msg.push_str(&format!("; top level of the picked folder contains: {}", notes.root_top.join(", ")));
    }
    msg.push_str(
        ". A folder counts as a game when it directly contains: game/ with Ren'Py files, Game.exe + Game.ini, *.rmvproject/*.rmmzproject, a <Name>_Data folder (Unity), data.win (GameMaker), package.json + index.html (NW.js), or a game .exe at its top level.",
    );
    msg
}

pub fn import_game_folder(
    state: &AppState,
    root: String,
    job: &JobHandle,
) -> Result<GameImportReport, String> {
    let root = PathBuf::from(root);
    if !root.is_dir() {
        return Err("folder does not exist".into());
    }

    job.set_progress(0, 0, "scanning for game folders…");
    let mut candidates = Vec::new();
    let mut notes = ScanNotes::default();
    find_game_dirs(&root, MAX_SCAN_DEPTH, &mut candidates, &mut notes);
    candidates.sort();
    candidates.dedup();
    job.set_note(&format!("{} game-shaped folders found", candidates.len()));

    if candidates.is_empty() {
        // Never a silent zero: say exactly what was looked at and why nothing
        // matched (field report: 0 imported / 0 failed / 0 unrecognized said
        // nothing at all).
        return Err(describe_empty_scan(&root, &notes));
    }

    let mut report = GameImportReport::default();
    import_game_dirs(state, &candidates, &mut report, job);
    Ok(report)
}

/// User pointed straight at a game's .exe — import its containing folder as
/// one game (an exe at top level always fingerprints as pc-game).
pub fn import_game_exe(
    state: &AppState,
    exe: String,
    job: &JobHandle,
) -> Result<GameImportReport, String> {
    let path = PathBuf::from(&exe);
    if !path.is_file() {
        return Err("file does not exist".into());
    }
    let dir = path
        .parent()
        .ok_or("cannot determine game folder from that exe")?
        .to_path_buf();
    let mut report = GameImportReport::default();
    import_game_dirs(state, &[dir], &mut report, job);
    Ok(report)
}

/// Import a zipped game: fingerprint engine files inside the archive,
/// extract title + cover art from the zip itself. The zip stays where it is;
/// the vault stores a pointer to it.
pub fn import_game_zip(
    state: &AppState,
    zip_path: String,
    job: &JobHandle,
) -> Result<GameImportReport, String> {
    let path = PathBuf::from(&zip_path);
    if !path.is_file() {
        return Err("zip file does not exist".into());
    }
    job.set_progress(0, 1, &zip_path);
    job.set_note("inspecting archive…");

    let mut report = GameImportReport::default();
    let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));

    let mut inner = state.inner.lock().unwrap();
    let Some(opened) = inner.opened.as_mut() else {
        return Err("vault is locked".into());
    };
    let conn = &mut opened.conn;
    if db::source_exists(conn, "local", &url) {
        report.skipped += 1;
        return Ok(report);
    }

    match zip_engine_info(&path) {
        Ok(info) => {
            let raw_name = path
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "Untitled".into());
            let (clean_title, version) = clean_name(&raw_name);
            let title = info
                .title
                .filter(|t| !t.trim().is_empty())
                .unwrap_or(clean_title);
            let now = now_epoch();
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
            db::insert_source(conn, &id, "local", &url, None, Some(&now))
                .map_err(|e| e.to_string())?;
            if let Some(tag) = info.tag {
                if let Ok(tag_id) = db::tag_get_or_create(conn, tag, db::TYPE_SITE_FLAG) {
                    let _ = db::link_item_tag(conn, &id, tag_id);
                }
            }
            if let Some(bytes) = info.cover_bytes {
                if media::put_thumbnail(conn, &id, &bytes) {
                    report.with_thumb += 1;
                }
            }
            report.imported += 1;
            report.titles.push(title);
        }
        Err(e) => report.failures.push(format!("{zip_path}: {e}")),
    }
    Ok(report)
}

/// Locks the vault once per folder: a big library scan+import never freezes the
/// rest of the app, and the user can stop it between items.
fn import_game_dirs(
    state: &AppState,
    dirs: &[PathBuf],
    report: &mut GameImportReport,
    job: &JobHandle,
) {
    let now = now_epoch();
    for (i, dir) in dirs.iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(i, dirs.len(), &dir.display().to_string());

        let url = format!("file:///{}", dir.to_string_lossy().replace('\\', "/"));
        let raw_name = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "Untitled".into());
        // Disk work (engine sniffing + cover read) stays outside the lock.
        let engine = detect_engine(dir);
        let (clean_title, version) = clean_name(&raw_name);
        let title = engine
            .title
            .filter(|t| !t.trim().is_empty())
            .unwrap_or(clean_title);
        let cover = engine.cover_path.as_ref().and_then(|p| fs::read(p).ok());

        let mut inner = state.inner.lock().unwrap();
        let Some(opened) = inner.opened.as_mut() else {
            report.stopped = true; // vault locked/nuke'd underneath us
            break;
        };
        let conn = &mut opened.conn;
        if db::source_exists(conn, "local", &url) {
            report.skipped += 1;
            continue;
        }

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
        if let Err(e) = db::insert_item(conn, &item) {
            report.failures.push(format!("{}: insert failed: {e}", raw_name));
            continue;
        }
        if let Err(e) = db::insert_source(conn, &id, "local", &url, None, Some(&now)) {
            report.failures.push(format!("{}: source failed: {e}", raw_name));
            let _ = db::delete_item(conn, &id);
            continue;
        }

        if let Some(tag) = engine.tag {
            if let Ok(tag_id) = db::tag_get_or_create(conn, tag, db::TYPE_SITE_FLAG) {
                let _ = db::link_item_tag(conn, &id, tag_id);
            }
        }
        if let Some(cover) = cover {
            if media::put_thumbnail(conn, &id, &cover) {
                report.with_thumb += 1;
            }
        }
        report.imported += 1;
        report.titles.push(title);
    }
    job.set_progress(dirs.len(), dirs.len(), "");
}

pub fn import_image_files(
    state: &AppState,
    paths: Vec<String>,
    job: &JobHandle,
) -> Result<ImageImportReport, String> {
    if paths.is_empty() {
        return Err("no files selected".into());
    }

    let title = batch_title(&paths);
    let id = {
        let mut inner = state.inner.lock().unwrap();
        let conn = &mut inner.opened.as_mut().ok_or("vault is locked")?.conn;
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
        id
    };

    // Read from disk outside the lock, write inside — a 500-image batch stays
    // stoppable and the rest of the app stays responsive.
    let mut stored = 0usize;
    let mut failed = 0usize;
    let mut stopped = false;
    let mut thumb_done = false;
    for (i, p) in paths.iter().enumerate() {
        if job.cancelled() {
            stopped = true;
            break;
        }
        job.set_progress(i, paths.len(), p);
        let bytes = match fs::read(p) {
            Ok(b) => b,
            Err(_) => {
                failed += 1;
                continue;
            }
        };
        let mut inner = state.inner.lock().unwrap();
        let Some(opened) = inner.opened.as_mut() else {
            stopped = true; // vault locked/nuke'd mid-import
            break;
        };
        let conn = &mut opened.conn;
        if media::put_full_image(conn, &id, &bytes) {
            stored += 1;
            if !thumb_done {
                thumb_done = media::put_thumbnail(conn, &id, &bytes);
            }
        } else {
            failed += 1;
        }
    }
    job.set_progress(paths.len(), paths.len(), "");

    if stored == 0 {
        let inner = state.inner.lock().unwrap();
        if let Some(opened) = inner.opened.as_ref() {
            let _ = db::delete_item(&opened.conn, &id);
        }
        return Err("no importable images among the selection".into());
    }
    Ok(ImageImportReport {
        item_id: id,
        title,
        images_stored: stored,
        failed,
        stopped,
    })
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoImportReport {
    pub imported: usize,
    pub skipped: usize,
    pub stopped: bool,
    pub titles: Vec<String>,
}

/// D13 + policy "videos stay links": picked video files become one media item
/// each (kind=post), tracked by their `file:///` path — nothing is copied into
/// the vault (vault-stored video is the M3 issue). Re-runs skip known paths.
pub fn import_video_files(
    state: &AppState,
    paths: Vec<String>,
    job: &JobHandle,
) -> Result<VideoImportReport, String> {
    if paths.is_empty() {
        return Err("no files selected".into());
    }
    let mut report = VideoImportReport::default();
    for (i, p) in paths.iter().enumerate() {
        if job.cancelled() {
            report.stopped = true;
            break;
        }
        job.set_progress(i, paths.len(), p);
        let path = Path::new(p);
        if crate::ingest::media_ext(&path.to_string_lossy().replace('\\', "/")) != Some("video") {
            report.skipped += 1;
            continue;
        }
        let url = format!("file:///{}", path.to_string_lossy().replace('\\', "/"));
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().trim().to_string())
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| "Untitled".into());

        let mut inner = state.inner.lock().unwrap();
        let Some(opened) = inner.opened.as_mut() else {
            report.stopped = true; // vault locked/nuke'd underneath us
            break;
        };
        let conn = &mut opened.conn;
        if db::source_exists(conn, "local", &url) {
            report.skipped += 1;
            continue;
        }
        let now = now_epoch();
        let id = new_id("v");
        let item = Item {
            id: id.clone(),
            kind: ItemKind::Post,
            title: title.clone(),
            original_title: None,
            circle: None,
            status: Status::Seen,
            dnf: false,
            rating: None,
            notes: "video — tracked by path; the file itself is not stored in the vault".into(),
            version: None,
            created_at: now.clone(),
            updated_at: now.clone(),
        };
        if db::insert_item(conn, &item).is_err() {
            report.skipped += 1;
            continue;
        }
        if !db::insert_source(conn, &id, "local", &url, None, Some(&now)).unwrap_or(false) {
            let _ = db::delete_item(conn, &id);
            report.skipped += 1;
            continue;
        }
        report.imported += 1;
        report.titles.push(title);
    }
    job.set_progress(paths.len(), paths.len(), "");
    Ok(report)
}

// ---------- zip archives ----------

/// Same fingerprinting as on-disk folders, but reading the archive's central
/// directory (we only touch the handful of tiny files that carry metadata).
pub struct ZipEngineInfo {
    pub tag: Option<&'static str>,
    pub title: Option<String>,
    pub cover_bytes: Option<Vec<u8>>,
}

pub fn zip_engine_info(path: &Path) -> Result<ZipEngineInfo, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut za = zip::ZipArchive::new(file).map_err(|e| format!("unreadable zip: {e}"))?;

    let names: Vec<String> = za
        .file_names()
        .map(|n| n.replace('\\', "/"))
        .filter(|n| !n.ends_with('/'))
        .collect();
    if names.is_empty() {
        return Err("zip contains no files".into());
    }

    // Repacks often wrap everything in one top folder: `GameName/www/...`.
    let prefix = common_zip_prefix(&names);
    let rel: Vec<String> = names
        .iter()
        .map(|n| n[prefix.len()..].to_string())
        .collect();
    let lower: Vec<String> = rel.iter().map(|s| s.to_lowercase()).collect();

    let find = |pred: &dyn Fn(&str) -> bool| lower.iter().position(|n| pred(n));

    // Cover image candidates.
    let cover_bytes = find(&|n| {
        n.starts_with("www/img/titles/")
            || n.starts_with("graphics/titles1/")
            || n.starts_with("graphics/title1/")
            || n == "game/gui/window_icon.png"
            || n == "game/images/ui/window_icon.png"
    })
    .and_then(|i| zip_read(&mut za, &names, i, 20 * 1024 * 1024));

    // RPG Maker MV/MZ
    if find(&|n| n == "game.rmmzproject" || n == "game.rmvproject").is_some() {
        let title = find(&|n| n == "www/data/system.json")
            .and_then(|i| zip_read_text(&mut za, &names, i))
            .and_then(|raw| {
                serde_json::from_str::<serde_json::Value>(&raw)
                    .ok()?
                    .get("gameTitle")?
                    .as_str()
                    .map(str::to_string)
            });
        return Ok(ZipEngineInfo { tag: Some("rpg-maker"), title, cover_bytes });
    }
    // RPG Maker XP/VX/Ace
    if find(&|n| n == "game.exe").is_some() && find(&|n| n == "game.ini").is_some() {
        let title = find(&|n| n == "game.ini")
            .and_then(|i| zip_read_text(&mut za, &names, i))
            .and_then(|raw| {
                raw.lines()
                    .find_map(|l| l.trim().strip_prefix("Title="))
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(str::to_string)
            });
        return Ok(ZipEngineInfo { tag: Some("rpg-maker"), title, cover_bytes });
    }
    // Ren'Py — releases ship *compiled* scripts: `game/script-xxxx.rpa`
    // archives, often with no `.rpy` sources and no `renpy/` folder (same
    // blind spot the folder import had — one fingerprint, both paths).
    if find(&|n| {
        (n.starts_with("game/")
            && (n.ends_with(".rpy") || n.ends_with(".rpa") || n.ends_with(".rpyc")))
            || n.starts_with("renpy/")
            || n.starts_with("renpydata/")
    })
    .is_some()
    {
        return Ok(ZipEngineInfo { tag: Some("renpy"), title: None, cover_bytes });
    }
    // Unity: a top-level `X_Data/` folder.
    if let Some(i) = lower.iter().position(|n| {
        n.split('/').next().is_some_and(|s| s.ends_with("_data")) && n.contains('/')
    }) {
        let name = rel[i]
            .split('/')
            .next()
            .unwrap_or_default()
            .trim_end_matches("_Data")
            .trim()
            .to_string();
        return Ok(ZipEngineInfo { tag: Some("unity"), title: Some(name), cover_bytes });
    }
    // GameMaker
    if find(&|n| n == "data.win").is_some() {
        return Ok(ZipEngineInfo { tag: Some("gamemaker"), title: None, cover_bytes });
    }
    // NW.js / HTML5
    if find(&|n| n == "package.json").is_some() && find(&|n| n == "index.html").is_some() {
        let title = find(&|n| n == "package.json")
            .and_then(|i| zip_read_text(&mut za, &names, i))
            .and_then(|raw| {
                let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
                ["title", "name"]
                    .iter()
                    .filter_map(|k| v.get(*k)?.as_str())
                    .find(|s| {
                        !s.trim().is_empty()
                            && !matches!(
                                s.trim().to_lowercase().as_str(),
                                "game" | "app" | "nw" | "index" | "project"
                            )
                    })
                    .map(str::to_string)
            });
        return Ok(ZipEngineInfo { tag: Some("html5"), title, cover_bytes });
    }
    // Generic: any exe at zip root.
    if find(&|n| !n.contains('/') && n.ends_with(".exe")).is_some() {
        return Ok(ZipEngineInfo { tag: Some("pc-game"), title: None, cover_bytes });
    }
    Err("no recognizable game files inside the zip (exe at root, game/ Ren'Py files, engine project files, ...)\nif the game came from a 7z or rar archive, extract it first and use + Game folder".to_string())
}

fn zip_read(za: &mut zip::ZipArchive<fs::File>, names: &[String], idx: usize, limit: u64) -> Option<Vec<u8>> {
    let mut entry = za.by_name(&names[idx]).ok()?;
    let mut buf = Vec::new();
    std::io::Read::take(&mut entry, limit)
        .read_to_end(&mut buf)
        .ok()?;
    Some(buf)
}

fn zip_read_text(za: &mut zip::ZipArchive<fs::File>, names: &[String], idx: usize) -> Option<String> {
    zip_read(za, names, idx, 2 * 1024 * 1024).map(|b| String::from_utf8_lossy(&b).into_owned())
}

fn common_zip_prefix(names: &[String]) -> String {
    let Some(first) = names.iter().find(|n| n.contains('/')) else {
        return String::new();
    };
    let seg = &first[..first.find('/').unwrap() + 1];
    names.iter().all(|n| n.starts_with(seg)).then(|| seg.to_string()).unwrap_or_default()
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
    // Ren'Py. Released games ship *compiled* scripts: `game/script-xxxx.rpa`
    // archives and often no `renpy/` folder at all — matching only `.rpy`
    // (source layout) missed real downloads (field report: a Ren'Py folder
    // imported as 0 games). Some packers also use `renpydata/`.
    let game_dir = dir.join("game");
    if game_dir.is_dir()
        && (dir.join("renpy").is_dir()
            || dir.join("renpydata").is_dir()
            || has_ext(&game_dir, "rpy")
            || has_ext(&game_dir, "rpa")
            || has_ext(&game_dir, "rpyc")
            || game_dir_has_contents(&game_dir))
    {
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

/// Last-resort Ren'Py check: a `game/` folder that holds real content anywhere
/// within two levels (ignoring its own cache/save dirs). A folder that ships
/// `game/` with content is a Ren'Py/RPG-style game in practice; the engine
/// marker files were just unusual.
fn game_dir_has_contents(game_dir: &Path) -> bool {
    let skip = |name: &str| {
        matches!(name, "cache" | "saves" | "save" | "samples") || name.starts_with('.')
    };
    let Ok(rd) = fs::read_dir(game_dir) else {
        return false;
    };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if skip(&name) {
            continue;
        }
        if p.is_file() {
            return true;
        }
        if p.is_dir() {
            // one more level down
            if let Ok(rd2) = fs::read_dir(&p) {
                if rd2.flatten().any(|e2| {
                    let n = e2.file_name().to_string_lossy().to_lowercase();
                    !skip(&n) && e2.path().is_file()
                }) {
                    return true;
                }
            }
        }
    }
    false
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

    #[test]
    fn find_game_dirs_handles_nested_library_layouts() {
        let base = std::env::temp_dir().join("goon-nest-test");
        let _ = fs::remove_dir_all(&base);

        // root/by-engine/renpy/Game A/ and root/JP/Deep/Game B/
        let a = base.join("by-engine").join("renpy").join("Game A [v1.0]");
        fs::create_dir_all(a.join("game")).unwrap();
        fs::create_dir_all(a.join("renpy")).unwrap();
        fs::write(a.join("game").join("s.rpy"), "//").unwrap();

        let b = base.join("JP").join("Deep").join("Game B");
        fs::create_dir_all(b.join("www").join("data")).unwrap();
        fs::write(b.join("game.rmmzproject"), "").unwrap();
        fs::write(
            b.join("www").join("data").join("System.json"),
            r#"{"gameTitle":"B Title"}"#,
        )
        .unwrap();

        // A non-game branch: contributes nothing.
        fs::create_dir_all(base.join("misc").join("txtfiles")).unwrap();
        fs::write(base.join("misc").join("txtfiles").join("readme.txt"), "x").unwrap();

        let mut out = Vec::new();
        let mut notes = ScanNotes::default();
        find_game_dirs(&base, MAX_SCAN_DEPTH, &mut out, &mut notes);
        assert_eq!(out.len(), 2, "found: {out:?}");
        let names: Vec<String> = out
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect();
        assert!(names.iter().any(|n| n == "Game A [v1.0]"), "names = {names:?}");
        assert!(names.iter().any(|n| n == "Game B"), "names = {names:?}");

        let _ = fs::remove_dir_all(&base);
    }

    /// Field report: a real *compiled* Ren'Py download — `game/` holding only
    /// `.rpa` archives, no `.rpy` sources, no `renpy/` folder — imported as
    /// 0 games because the fingerprint only knew the source layout.
    #[test]
    fn detects_compiled_renpy_release_layout() {
        let base = std::env::temp_dir().join("goon-renpy-release-test");
        let _ = fs::remove_dir_all(&base);
        let g = base.join("Summer Witch [renpy] [English]");
        fs::create_dir_all(g.join("game").join("gui")).unwrap();
        fs::write(g.join("game").join("script-0a3f2c8b.rpa"), "rpa").unwrap();
        fs::write(g.join("game").join("gui").join("window_icon.png"), "png").unwrap();
        let info = detect_engine(&g);
        assert_eq!(info.engine, Some("renpy"));
        assert_eq!(
            info.cover_path,
            Some(g.join("game").join("gui").join("window_icon.png"))
        );
        let _ = fs::remove_dir_all(&base);
    }

    /// Field report: "0 imported, 0 failed, 0 unrecognized — makes no sense".
    /// A scan that finds nothing must say what it saw instead.
    #[test]
    fn empty_scan_reports_what_it_saw() {
        let base = std::env::temp_dir().join("goon-empty-scan-test");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        fs::write(base.join("Summer Witch.7z.001"), "packed").unwrap();
        fs::write(base.join("readme.txt"), "extract me").unwrap();
        fs::create_dir_all(base.join("English Patch").join("game")).unwrap();

        let mut out = Vec::new();
        let mut notes = ScanNotes::default();
        find_game_dirs(&base, MAX_SCAN_DEPTH, &mut out, &mut notes);
        assert!(out.is_empty(), "found: {out:?}");
        let msg = describe_empty_scan(&base, &notes);
        assert!(msg.contains("archive"), "{msg}");
        assert!(msg.contains("Summer Witch.7z.001"), "{msg}");
        assert!(msg.contains("English Patch"), "{msg}");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn zip_engine_fingerprint_reads_inside_archive() {
        use std::io::Write;
        let path = std::env::temp_dir().join("goon-zip-test.zip");
        {
            let f = fs::File::create(&path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("Pumpkin Days/game.rmmzproject", opts).unwrap();
            w.write_all(b"").unwrap();
            w.start_file("Pumpkin Days/www/data/System.json", opts).unwrap();
            w.write_all(br#"{"gameTitle":"Pumpkin Days"}"#).unwrap();
            w.finish().unwrap();
        }
        let info = zip_engine_info(&path).unwrap();
        assert_eq!(info.tag, Some("rpg-maker"));
        assert_eq!(info.title.as_deref(), Some("Pumpkin Days"));
        let _ = fs::remove_file(&path);
    }

    /// Zips share the Ren'Py fingerprint — compiled `.rpa` layouts must work
    /// there too, not just in folder scans.
    #[test]
    fn zip_detects_compiled_renpy_rpa_layout() {
        use std::io::Write;
        let path = std::env::temp_dir().join("goon-zip-renpy-test.zip");
        {
            let f = fs::File::create(&path).unwrap();
            let mut w = zip::ZipWriter::new(f);
            let opts = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("Summer Witch/game/script-0a3f2c8b.rpa", opts).unwrap();
            w.write_all(b"rpa").unwrap();
            w.finish().unwrap();
        }
        let info = zip_engine_info(&path).unwrap();
        assert_eq!(info.tag, Some("renpy"));
        let _ = fs::remove_file(&path);
    }
}
