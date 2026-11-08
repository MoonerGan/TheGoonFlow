mod bookmarks;
mod companion;
mod importer;
mod ingest;

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
    tag: Option<String>,
) -> Result<Vec<goon_core::db::ItemSummary>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::list_items(
        &opened.conn,
        limit.unwrap_or(200),
        offset.unwrap_or(0),
        search.as_deref(),
        kind.as_deref(),
        tag.as_deref(),
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

// ---------- clip / ingest (M1) ----------

#[tauri::command]
async fn clip_url(
    state: tauri::State<'_, AppState>,
    url: String,
) -> Result<ingest::ClipReport, String> {
    ingest::clip(&state, url).await
}

#[tauri::command]
fn get_item(
    state: tauri::State<'_, AppState>,
    id: String,
) -> Result<Option<goon_core::db::ItemDetail>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::get_item(&opened.conn, &id).map_err(err)
}

// ---------- tags (M1) ----------

#[tauri::command]
fn top_tags(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<goon_core::db::TagCount>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::top_tags(&opened.conn, limit.unwrap_or(40)).map_err(err)
}

#[tauri::command]
fn add_item_tag(
    state: tauri::State<'_, AppState>,
    id: String,
    name: String,
    kind: Option<String>,
) -> Result<(), String> {
    let type_id = match kind.as_deref() {
        Some("site") => goon_core::db::TYPE_SITE_FLAG,
        Some("status") => goon_core::db::TYPE_USER_STATUS,
        _ => goon_core::db::TYPE_CONTENT,
    };
    let name = name.trim().to_lowercase().replace(' ', "-");
    if name.is_empty() {
        return Err("tag cannot be empty".into());
    }
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::add_item_tag(conn, &id, &name, type_id).map_err(err)?;
    Ok(())
}

#[tauri::command]
fn remove_item_tag(
    state: tauri::State<'_, AppState>,
    id: String,
    tag_id: i64,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::remove_item_tag(conn, &id, tag_id).map_err(err)
}

// ---------- sources (M2: enable, cookie, rate) ----------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SiteInfo {
    site: String,
    kind: &'static str,
    mirror: bool,
    default_rate_ms: i64,
}

#[tauri::command]
fn known_sites() -> Vec<SiteInfo> {
    ingest::KNOWN_SITES
        .iter()
        .map(|(site, kind)| SiteInfo {
            site: (*site).to_string(),
            kind: *kind,
            mirror: ingest::is_mirror_site(site),
            default_rate_ms: ingest::default_rate_ms(site),
        })
        .collect()
}

#[tauri::command]
fn get_site_settings(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<goon_core::db::SiteSetting>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::get_site_settings(&opened.conn).map_err(err)
}

#[tauri::command]
fn set_site_setting(
    state: tauri::State<'_, AppState>,
    site: String,
    enabled: Option<bool>,
    cookie: Option<String>,
    rate_ms: Option<i64>,
) -> Result<(), String> {
    let site = site.trim().to_lowercase();
    if site.is_empty() {
        return Err("no site".into());
    }
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::set_site_setting(
        conn,
        &site,
        enabled,
        cookie.as_deref().map(str::trim),
        rate_ms.map(|r| r.clamp(0, 600_000)),
    )
    .map_err(err)
}

// ---------- update feed (M2) ----------

#[tauri::command]
async fn check_updates(state: tauri::State<'_, AppState>) -> Result<ingest::UpdateReport, String> {
    ingest::check_updates(&state).await
}

#[tauri::command]
fn list_updates(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<goon_core::db::UpdateRow>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::list_updates(&opened.conn, limit.unwrap_or(100)).map_err(err)
}

// ---------- pairing / merge (M2) ----------

#[tauri::command]
fn pairing_suggestions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<goon_core::db::PairSuggestion>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::pairing_suggestions(&opened.conn).map_err(err)
}

#[tauri::command]
fn merge_items(
    state: tauri::State<'_, AppState>,
    keep_id: String,
    drop_id: String,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::merge_items(conn, &keep_id, &drop_id).map_err(err)
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
            clip_url,
            get_item,
            top_tags,
            add_item_tag,
            remove_item_tag,
            known_sites,
            get_site_settings,
            set_site_setting,
            check_updates,
            list_updates,
            pairing_suggestions,
            merge_items,
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
        let items = goon_core::db::list_items(conn, 300, 0, None, None, None).unwrap();
        assert_eq!(items.len(), 1, "imported but list showed nothing");
        assert_eq!(items[0].title, "Game X");
        assert_eq!(items[0].version.as_deref(), Some("2.0"));
        let games_only = goon_core::db::list_items(conn, 300, 0, None, Some("game"), None).unwrap();
        assert_eq!(games_only.len(), 1);
        drop(inner);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
