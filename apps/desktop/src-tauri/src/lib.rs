mod bookmarks;
mod companion;
mod importer;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use goon_core::model::Status;
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::Connection;
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) fn now_epoch() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

pub(crate) fn new_id(prefix: &str) -> String {
    let mut raw = [0u8; 10];
    OsRng.fill_bytes(&mut raw);
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    format!("{prefix}-{hex}")
}

/// An unlocked vault: live DB connection + its data key. Locking drops this,
/// which zeroizes the key and closes (and checkpoints) the database.
struct OpenedVault {
    conn: Connection,
    _key: goon_vault::DataKey,
}

#[derive(Default)]
struct Inner {
    vault_root: Option<PathBuf>,
    opened: Option<OpenedVault>,
}

pub struct AppState {
    inner: Mutex<Inner>,
    exe_dir: PathBuf,
    companion: Option<companion::CompanionInfo>,
    locked: Arc<AtomicBool>,
}

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

// ---------- status ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CompanionStatus {
    port: u16,
    token: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AppStatus {
    /// A vault is known (pointer file present or attached this session).
    has_vault: bool,
    /// The vault folder is physically reachable right now.
    vault_resolved: bool,
    unlocked: bool,
    vault_root: Option<String>,
    companion: Option<CompanionStatus>,
}

#[tauri::command]
fn app_status(state: tauri::State<'_, AppState>) -> AppStatus {
    let inner = state.inner.lock().unwrap();
    let pointer_present = goon_locator::load(&state.exe_dir).is_some();
    AppStatus {
        has_vault: pointer_present || inner.vault_root.is_some(),
        vault_resolved: inner.vault_root.is_some(),
        unlocked: inner.opened.is_some(),
        vault_root: inner.vault_root.as_ref().map(|p| p.display().to_string()),
        companion: state.companion.as_ref().map(|c| CompanionStatus {
            port: c.port,
            token: c.token.clone(),
        }),
    }
}

#[tauri::command]
fn default_vault_dir(state: tauri::State<'_, AppState>) -> String {
    state.exe_dir.join("goonflow-vault").display().to_string()
}

#[tauri::command]
fn list_volumes() -> Vec<goon_locator::Volume> {
    goon_locator::volumes()
}

// ---------- vault lifecycle ----------

#[tauri::command]
fn create_vault(
    state: tauri::State<'_, AppState>,
    dir: String,
    passphrase: String,
) -> Result<(), String> {
    if passphrase.trim().is_empty() {
        return Err("passphrase must not be empty".into());
    }
    let path = PathBuf::from(&dir);
    if path.as_os_str().is_empty() {
        return Err("choose a vault folder first".into());
    }
    let key = goon_vault::create_keyfile(&path, &passphrase).map_err(err)?;
    let conn = goon_vault::open_db(&path, &key).map_err(err)?;
    goon_core::db::init_schema(&conn).map_err(err)?;

    if let Some(pointer) = goon_locator::make_pointer(&path) {
        let _ = goon_locator::save(&state.exe_dir, &pointer);
    }

    let mut inner = state.inner.lock().unwrap();
    inner.vault_root = Some(path);
    inner.opened = Some(OpenedVault { conn, _key: key });
    state.locked.store(false, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn attach_vault(state: tauri::State<'_, AppState>, dir: String) -> Result<(), String> {
    let path = PathBuf::from(dir);
    if !goon_vault::is_vault_dir(&path) {
        return Err("that folder is not a goonflow vault".into());
    }
    if let Some(pointer) = goon_locator::make_pointer(&path) {
        let _ = goon_locator::save(&state.exe_dir, &pointer);
    }
    state.inner.lock().unwrap().vault_root = Some(path);
    Ok(())
}

#[tauri::command]
fn unlock(state: tauri::State<'_, AppState>, passphrase: String) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let root = inner.vault_root.clone().ok_or("no vault folder attached")?;
    let key = goon_vault::open_keyfile(&root, &passphrase).map_err(err)?;
    let conn = goon_vault::open_db(&root, &key).map_err(err)?;
    goon_core::db::init_schema(&conn).map_err(err)?;
    inner.opened = Some(OpenedVault { conn, _key: key });
    state.locked.store(false, Ordering::SeqCst);
    Ok(())
}

#[tauri::command]
fn lock(state: tauri::State<'_, AppState>) {
    let mut inner = state.inner.lock().unwrap();
    inner.opened = None; // drops conn (checkpoint) + zeroizes data key
    state.locked.store(true, Ordering::SeqCst);
}

/// Crypto-erase: keyfile shredded first, then every vault file overwritten
/// and deleted, then the pointer cleared. Irreversible by design.
/// `dir` lets the Reconnect screen nuke a vault that isn't currently attached
/// (e.g. the pointer points at a folder that was moved or partially deleted).
#[tauri::command]
fn nuke_vault(
    state: tauri::State<'_, AppState>,
    confirm: String,
    dir: Option<String>,
) -> Result<(), String> {
    if confirm != "NUKE" {
        return Err("type NUKE exactly to confirm".into());
    }
    let root = {
        let mut inner = state.inner.lock().unwrap();
        let root = inner
            .vault_root
            .clone()
            .or_else(|| dir.map(PathBuf::from))
            .ok_or("no vault folder attached — pick its folder or use \"Forget pointer\"")?;
        inner.opened = None; // close DB before shredding (Windows file locks)
        inner.vault_root = None;
        goon_locator::clear(&state.exe_dir);
        state.locked.store(true, Ordering::SeqCst);
        root
    };
    if root.is_dir() {
        goon_vault::nuke(&root).map_err(err)?;
        let _ = std::fs::remove_dir_all(&root);
    }
    Ok(())
}

/// Clear the pointer file (flow.cfg) without touching any vault folder.
/// Escape hatch when the pointer is stale/wrong and you want a clean start.
#[tauri::command]
fn reset_pointer(state: tauri::State<'_, AppState>) {
    goon_locator::clear(&state.exe_dir);
    let mut inner = state.inner.lock().unwrap();
    inner.vault_root = None;
    inner.opened = None;
    state.locked.store(true, Ordering::SeqCst);
}

// ---------- data (post-unlock) ----------

#[tauri::command]
fn vault_stats(
    state: tauri::State<'_, AppState>,
    kind: Option<String>,
) -> Result<Vec<goon_core::db::StatusCount>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::counts_by_status(&opened.conn, kind.as_deref()).map_err(err)
}

#[tauri::command]
fn kind_stats(state: tauri::State<'_, AppState>) -> Result<Vec<(&'static str, i64)>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::counts_by_kind(&opened.conn).map_err(err)
}

#[tauri::command]
fn list_items(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
    search: Option<String>,
    kind: Option<String>,
) -> Result<Vec<goon_core::db::ItemSummary>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::list_items(
        &opened.conn,
        limit.unwrap_or(200),
        offset.unwrap_or(0),
        search.as_deref(),
        kind.as_deref(),
    )
    .map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbPayload {
    id: String,
    mime: String,
    data: String,
}

#[tauri::command]
fn get_thumbs(state: tauri::State<'_, AppState>, ids: Vec<String>) -> Result<Vec<ThumbPayload>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    Ok(goon_core::media::thumbs_for(&opened.conn, &ids)
        .into_iter()
        .map(|(id, mime, bytes)| ThumbPayload {
            id,
            mime,
            data: B64.encode(bytes),
        })
        .collect())
}

#[tauri::command]
fn update_item(
    state: tauri::State<'_, AppState>,
    id: String,
    title: String,
    circle: Option<String>,
    status: String,
    version: Option<String>,
) -> Result<(), String> {
    let status = Status::from_str(&status).ok_or("invalid status")?;
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::update_item(
        conn,
        &id,
        title.trim(),
        circle.as_deref().filter(|c| !c.trim().is_empty()),
        status,
        version.as_deref().filter(|v| !v.trim().is_empty()),
        None,
        &now_epoch(),
    )
    .map_err(err)
}

#[tauri::command]
fn delete_item(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::delete_item(conn, &id).map_err(err)
}

#[tauri::command]
fn set_complete(
    state: tauri::State<'_, AppState>,
    id: String,
    complete: bool,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::set_user_status_tag(conn, &id, complete.then_some("complete")).map_err(err)
}

// ---------- import (D13) ----------

#[tauri::command]
fn import_game_folder(
    state: tauri::State<'_, AppState>,
    dir: String,
) -> Result<importer::GameImportReport, String> {
    importer::import_game_folder(&state, dir)
}

#[tauri::command]
fn import_game_exe(
    state: tauri::State<'_, AppState>,
    exe: String,
) -> Result<importer::GameImportReport, String> {
    importer::import_game_exe(&state, exe)
}

#[tauri::command]
fn import_game_zip(
    state: tauri::State<'_, AppState>,
    zip_path: String,
) -> Result<importer::GameImportReport, String> {
    importer::import_game_zip(&state, zip_path)
}

#[tauri::command]
fn import_image_files(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<importer::ImageImportReport, String> {
    importer::import_image_files(&state, paths)
}

#[tauri::command]
async fn import_bookmarks(
    state: tauri::State<'_, AppState>,
    path: String,
    fetch_online: bool,
) -> Result<bookmarks::BookmarkReport, String> {
    bookmarks::import_bookmarks(&state, path, fetch_online).await
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let locked = Arc::new(AtomicBool::new(true));
    let companion = match companion::start(locked.clone()) {
        Ok(info) => Some(info),
        Err(e) => {
            eprintln!("companion API disabled: {e}");
            None
        }
    };

    // Resolve vault at startup: pointer → volume label/path; fallback to a
    // default folder beside the exe (handy for dev and single-folder use).
    let mut vault_root = goon_locator::load(&exe_dir).and_then(|p| goon_locator::resolve(&p));
    if vault_root.is_none() {
        let default = exe_dir.join("goonflow-vault");
        if goon_vault::is_vault_dir(&default) {
            vault_root = Some(default);
        }
    }

    let state = AppState {
        inner: Mutex::new(Inner { vault_root, opened: None }),
        exe_dir,
        companion,
        locked,
    };

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            app_status,
            default_vault_dir,
            list_volumes,
            create_vault,
            attach_vault,
            unlock,
            lock,
            nuke_vault,
            reset_pointer,
            vault_stats,
            kind_stats,
            list_items,
            get_thumbs,
            update_item,
            delete_item,
            set_complete,
            import_game_folder,
            import_game_exe,
            import_game_zip,
            import_image_files,
            import_bookmarks
        ])
        .run(tauri::generate_context!())
        .expect("error while running The Goon Flow");
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Regression for the M0 field report: nested game folders + the
    /// list_items binding bug made imports look like they stored nothing.
    #[test]
    fn import_then_list_roundtrip_on_real_vault() {
        let tmp = std::env::temp_dir().join(format!("goonflow-it-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let vault = tmp.join("vault");
        let games = tmp.join("games");

        let g = games.join("by-engine").join("Game X [v2.0]");
        std::fs::create_dir_all(g.join("game")).unwrap();
        std::fs::create_dir_all(g.join("renpy")).unwrap();
        std::fs::write(g.join("game").join("s.rpy"), "//").unwrap();

        let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
        let conn = goon_vault::open_db(&vault, &key).unwrap();
        goon_core::db::init_schema(&conn).unwrap();

        let state = AppState {
            inner: Mutex::new(Inner {
                vault_root: Some(vault.clone()),
                opened: Some(OpenedVault { conn, _key: key }),
            }),
            exe_dir: tmp.clone(),
            companion: None,
            locked: Arc::new(AtomicBool::new(false)),
        };

        let report = importer::import_game_folder(&state, games.display().to_string()).unwrap();
        assert_eq!(report.imported, 1, "titles = {:?}", report.titles);

        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().unwrap().conn;
        let items = goon_core::db::list_items(conn, 300, 0, None, None).unwrap();
        assert_eq!(items.len(), 1, "imported but list showed nothing");
        assert_eq!(items[0].title, "Game X");
        assert_eq!(items[0].version.as_deref(), Some("2.0"));
        let games_only = goon_core::db::list_items(conn, 300, 0, None, Some("game")).unwrap();
        assert_eq!(games_only.len(), 1);
        drop(inner);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
