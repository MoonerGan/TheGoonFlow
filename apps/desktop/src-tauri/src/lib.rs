mod bookmarks;
mod companion;
mod cookies;
mod extract;
mod http;
mod importer;
mod ingest;
mod jobs;
mod launcher;
mod mods;
mod queue;
mod video;
mod videothumb;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use goon_core::model::Status;
use jobs::Jobs;
use rand::rngs::OsRng;
use rand::RngCore;
use rusqlite::Connection;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// Bumped whenever decrypted vault material stops being valid (lock, nuke) or
    /// a *different* vault gets attached, and reported by `app_status`. The
    /// frontend caches decrypted media in RAM while unlocked (#44); this counter
    /// is how it notices that the ground moved under it — including when the lock
    /// came from the panic hotkey or the erase watcher instead of a UI click,
    /// where nothing else would tell the webview anything.
    vault_gen: AtomicU64,
}

/// The shareable half of the app state — everything that touches the vault or
/// the network. Commands reach it through `AppState` (Deref), and the
/// companion HTTP API (#19) holds its own clone so browser clips run through
/// the exact same lock, job ledger and rate lanes as the UI.
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<Mutex<Inner>>,
    /// Long-running work (imports, update checks) runs as jobs: observable,
    /// stoppable, and cancelled by Lock/Nuke.
    pub(crate) jobs: Arc<Jobs>,
    /// #15: shared per-site rate lanes for every network fetch — one
    /// politeness ledger for all jobs, not one per work-loop.
    pub(crate) queue: Arc<queue::SiteQueue>,
}

/// A scratch vault for tests: a real encrypted vault in a temp folder, unlocked,
/// with the schema applied, inside a `Core` built the way the app builds one.
/// Both `companion` (#19 clips) and `ingest` (#38/#52 live rehearsals) need it,
/// and there is deliberately one copy: a hand-rolled second version of the app
/// state is how a test starts passing against something the app is not.
#[cfg(test)]
pub(crate) fn scratch_core(tag: &str, unlocked: bool) -> (Core, PathBuf) {
    // Tests run in parallel inside one process, so the pid alone does not keep
    // two vaults apart: #38 hit exactly that, one test deleting the directory
    // another was still using ("file is not a database").
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp =
        std::env::temp_dir().join(format!("goonflow-scratch-{tag}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    let vault = tmp.join("vault");
    let (conn, key) = if unlocked {
        let key = goon_vault::create_keyfile(&vault, "hunter2").unwrap();
        let conn = goon_vault::open_db(&vault, &key).unwrap();
        goon_core::db::init_schema(&conn).unwrap();
        (Some(conn), Some(key))
    } else {
        (None, None)
    };
    let core = Core {
        inner: Arc::new(Mutex::new(Inner {
            vault_root: Some(vault),
            opened: conn.map(|c| OpenedVault {
                conn: c,
                _key: key.unwrap(),
            }),
            ..Default::default()
        })),
        jobs: Arc::new(Jobs::default()),
        queue: Arc::new(queue::SiteQueue::default()),
    };
    (core, tmp)
}

pub struct AppState {
    core: Core,
    exe_dir: PathBuf,
    companion: Option<companion::CompanionInfo>,
    locked: Arc<AtomicBool>,
    /// #23: the panic hotkey hid the window (only the hotkey brings it back).
    hidden: Arc<AtomicBool>,
    /// #23: true while the panic key is physically held down; a watcher
    /// thread escalates to crypto-erase after PANIC_HOLD_MS.
    panic_hold: Arc<AtomicBool>,
    /// #23: bumped on every hold episode start; a watcher only escalates if
    /// its own episode is still the current one at the deadline (a stale
    /// watcher from an already-released tap must never fire).
    panic_gen: Arc<AtomicU64>,
    /// #23: set when a hold-erase fired, so the matching release does nothing
    nuked: Arc<AtomicBool>,
    /// #41: why the panic key could NOT be registered (another program or an
    /// earlier instance owns the combination). None = armed. The UI shows this:
    /// a panic path that silently does not exist is the one failure mode this
    /// feature must never have.
    panic_fault: Arc<Mutex<Option<String>>>,
}

impl std::ops::Deref for AppState {
    type Target = Core;
    fn deref(&self) -> &Self::Target {
        &self.core
    }
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
    /// See `Inner::vault_gen`: changes on lock/nuke/attach, so anything caching
    /// decrypted vault material in the webview knows to drop it (#44).
    generation: u64,
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
        generation: inner.vault_gen.load(Ordering::SeqCst),
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
    inner.vault_gen.fetch_add(1, Ordering::SeqCst);
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
fn unlock(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    passphrase: String,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let root = inner.vault_root.clone().ok_or("no vault folder attached")?;
    let key = goon_vault::open_keyfile(&root, &passphrase).map_err(err)?;
    let conn = goon_vault::open_db(&root, &key).map_err(err)?;
    goon_core::db::init_schema(&conn).map_err(err)?;
    // #23: this vault may carry its own panic key next to the default.
    let stored = goon_core::db::get_meta(&conn, "panic_hotkey");
    inner.opened = Some(OpenedVault { conn, _key: key });
    inner.vault_gen.fetch_add(1, Ordering::SeqCst);
    state.locked.store(false, Ordering::SeqCst);
    drop(inner);
    register_panic_keys(&app, stored.as_deref());
    Ok(())
}

#[tauri::command]
fn lock(state: tauri::State<'_, AppState>, app: tauri::AppHandle) {
    let scrub = scrub_on_lock_flag(&state.core);
    lock_core(&state.core, &state.locked, &app);
    if scrub {
        do_scrub(&app);
    }
}

/// #44: tell the webview that nothing decrypted may be kept any more. The viewer
/// caches full-res media in RAM while the vault is unlocked, and the panic hotkey
/// locks it without any frontend call, so waiting for the next poll would leave
/// seconds of decrypted bytes in the page. `app_status.generation` is the belt to
/// this brace: an event can be missed by a hidden or reloading webview, a changed
/// counter cannot.
fn end_vault_session(app: &tauri::AppHandle) {
    use tauri::Emitter;
    let _ = app.emit("vault-session-ended", ());
}

/// The lock half of #23 lives here so the panic hotkey and the Lock button
/// do exactly the same thing: every job stops, the vault closes, the data
/// key is zeroized.
fn lock_core(core: &Core, locked: &AtomicBool, app: &tauri::AppHandle) {
    core.jobs.cancel_all(); // imports/fetches must stop, not just hide
    core.queue.clear(); // drop lane timing history too
    {
        let mut inner = core.inner.lock().unwrap();
        inner.opened = None; // drops conn (checkpoint) + zeroizes data key
        inner.vault_gen.fetch_add(1, Ordering::SeqCst);
    }
    locked.store(true, Ordering::SeqCst);
    end_vault_session(app);
}

// ---------- scrubber (#23): OS-side traces, never vault contents ----------

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct ScrubReport {
    /// our WebView2 profile's cookies/cache/localStorage
    webview_cleared: bool,
    /// Windows Explorer thumbnail caches (they can hold game/media previews)
    thumbcache_removed: usize,
    thumbcache_locked: usize,
}

fn scrub_on_lock_flag(core: &Core) -> bool {
    let inner = core.inner.lock().unwrap();
    inner
        .opened
        .as_ref()
        .and_then(|o| goon_core::db::get_meta(&o.conn, "scrub_on_lock"))
        .is_some_and(|v| v == "1")
}

/// Delete `thumbcache_*.db` in `dir`, count removed vs locked-by-Explorer.
/// Nothing else in that folder is touched.
fn scrub_cache_dir(dir: &Path) -> (usize, usize) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    let (mut gone, mut busy) = (0usize, 0usize);
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if name.starts_with("thumbcache_") && name.ends_with(".db") {
            if std::fs::remove_file(e.path()).is_ok() {
                gone += 1;
            } else {
                busy += 1;
            }
        }
    }
    (gone, busy)
}

fn do_scrub(app: &tauri::AppHandle) -> ScrubReport {
    use tauri::Manager;
    let mut r = ScrubReport::default();
    if let Some(w) = app.get_webview_window("main") {
        r.webview_cleared = w.clear_all_browsing_data().is_ok();
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let explorer = Path::new(&local)
            .join("Microsoft")
            .join("Windows")
            .join("Explorer");
        let (gone, busy) = scrub_cache_dir(&explorer);
        r.thumbcache_removed = gone;
        r.thumbcache_locked = busy;
    }
    r
}

#[tauri::command]
fn scrub_now(app: tauri::AppHandle) -> ScrubReport {
    do_scrub(&app)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ScrubSettings {
    scrub_on_lock: bool,
}

#[tauri::command]
fn get_scrub_settings(state: tauri::State<'_, AppState>) -> Result<ScrubSettings, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    Ok(ScrubSettings {
        scrub_on_lock: goon_core::db::get_meta(&opened.conn, "scrub_on_lock").as_deref()
            == Some("1"),
    })
}

#[tauri::command]
fn set_scrub_on_lock(state: tauri::State<'_, AppState>, on: bool) -> Result<ScrubSettings, String> {
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    goon_core::db::set_meta(
        &opened.conn,
        "scrub_on_lock",
        if on { Some("1") } else { None },
    )
    .map_err(err)?;
    Ok(ScrubSettings { scrub_on_lock: on })
}

// ---------- deniability / decoy chrome (#27) ----------
//
// A plaintext `chrome.cfg` beside the exe may hold one line: a boring window
// title ("Flow Notes", "Sync Client", ...). It contains nothing sensitive —
// reading it when locked is fine — and covers the case where a renamed exe
// still shows a telltale title in the taskbar or an accidentally-unlocked
// window still shows the brand. Renaming the exe/folder itself already works
// (everything resolves relative to the exe), which we verified live.

fn chrome_path(exe_dir: &Path) -> PathBuf {
    exe_dir.join("chrome.cfg")
}

/// The stored decoy title, if any. Sanitized on read too, so a hand-edited
/// file can't smuggle control characters into a window title.
fn read_chrome(exe_dir: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(chrome_path(exe_dir)).ok()?;
    sanitize_chrome(raw.lines().next()?)
}

fn sanitize_chrome(s: &str) -> Option<String> {
    let t: String = s.trim().chars().filter(|c| !c.is_control()).collect();
    if t.is_empty() || t.chars().count() > 64 {
        return None;
    }
    Some(t)
}

fn apply_chrome(app: &tauri::AppHandle, exe_dir: &Path) {
    use tauri::Manager;
    // The tauri.conf default is already neutral ("Flow"); the decoy only
    // needs to be pushed when one is actually stored.
    if let Some(title) = read_chrome(exe_dir) {
        match app.get_webview_window("main") {
            Some(w) => {
                let r = w.set_title(&title);
                eprintln!("decoy chrome: {:?} -> set_title {:?}", title, r.is_ok());
            }
            None => eprintln!("decoy chrome: window not created yet, will retry at Ready"),
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ChromeInfo {
    /// active decoy title (None = stock neutral "Flow")
    decoy: Option<String>,
}

#[tauri::command]
fn get_chrome(state: tauri::State<'_, AppState>) -> ChromeInfo {
    ChromeInfo {
        decoy: read_chrome(&state.exe_dir),
    }
}

/// Store (or clear) the decoy title and apply it to the live window
/// immediately. Works whether or not a vault is open — it is the lock
/// screen's chrome we are also dressing.
#[tauri::command]
fn set_chrome(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    title: Option<String>,
) -> Result<ChromeInfo, String> {
    use tauri::Manager;
    let decoy = match title {
        Some(t) => Some(
            sanitize_chrome(&t)
                .ok_or("title is empty, all control characters, or over 64 chars")?,
        ),
        None => None,
    };
    let path = chrome_path(&state.exe_dir);
    match &decoy {
        Some(t) => std::fs::write(&path, format!("{t}\n")).map_err(err)?,
        None => {
            let _ = std::fs::remove_file(&path);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_title("Flow"); // the stock neutral title
            }
        }
    }
    if let Some(t) = &decoy {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.set_title(t);
        }
    }
    Ok(ChromeInfo { decoy })
}

// ---------- panic hotkey (#23) ----------

/// Registered at every launch; a vault may add its own key on unlock.
pub const DEFAULT_PANIC_HOTKEY: &str = "CommandOrControl+Alt+L";

/// Hold this long and the vault is crypto-erased (DESIGN §4 escalation, D19).
pub const PANIC_HOLD_MS: u64 = 3000;

/// Physical key state via the OS (same mechanism global-hotkey itself uses
/// to synthesize release events). Escalation must NEVER fire on a lost
/// release event alone: the watcher verifies the shortcut's own modifiers
/// are still physically down at the deadline — whatever the stored panic
/// key happens to be (Ctrl+Alt+L default, or e.g. Alt+F12).
#[cfg(windows)]
fn panic_mods_down(mods: tauri_plugin_global_shortcut::Modifiers) -> bool {
    use tauri_plugin_global_shortcut::Modifiers;
    #[link(name = "user32")]
    extern "system" {
        fn GetAsyncKeyState(virt_key: i32) -> i16;
    }
    let down = |vk: i32| unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 };
    if mods.contains(Modifiers::CONTROL) && !down(0x11) {
        return false;
    }
    if mods.contains(Modifiers::ALT) && !down(0x12) {
        return false;
    }
    if mods.contains(Modifiers::SHIFT) && !down(0x10) {
        return false;
    }
    if mods.contains(Modifiers::SUPER) && !(down(0x5B) || down(0x5C)) {
        return false;
    }
    true
}

#[cfg(not(windows))]
fn panic_mods_down(_mods: tauri_plugin_global_shortcut::Modifiers) -> bool {
    false // no reliable physical probe: never escalate, tap-only panic
}

/// Global hotkey state machine. Windows WM_HOTKEY auto-repeats while held
/// (many Pressed events) and global-hotkey synthesizes one Release per
/// Pressed (a storm on key-up), so the machine collapses both:
/// - first Pressed starts an episode + arms the 3 s escalation watcher
/// - Pressed repeats (episode live AND keys physically down) are ignored
/// - the first Release closes the episode = tap: Lock + hide (tap again
///   reveals the locked window); duplicate Releases fall through inert
/// - if the watcher escalated, the erase already ran; releases do nothing
///
/// A watcher only escalates if it is still the *current* episode when its
/// deadline lands (`panic_gen` matches): a stale watcher from an earlier
/// quick tap must never fire while a fresh hold is still under 3 s old.
fn panic_event(
    app: &tauri::AppHandle,
    pressed: bool,
    mods: tauri_plugin_global_shortcut::Modifiers,
) {
    use tauri::Manager;
    let state = app.state::<AppState>();
    if pressed {
        // auto-repeat while the episode is live AND keys physically down.
        // (episode live but keys up = lost release: start a fresh tap.)
        if state.panic_hold.load(Ordering::SeqCst) && panic_mods_down(mods) {
            return;
        }
        state.panic_hold.store(true, Ordering::SeqCst);
        let my_gen = state.panic_gen.fetch_add(1, Ordering::SeqCst) + 1;
        let app2 = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(PANIC_HOLD_MS));
            let st = app2.state::<AppState>();
            // Escalate only if this watcher is still the live episode AND
            // the shortcut's modifiers are verifiably still held. A newer
            // tap bumps panic_gen, so a stale watcher (or a released-then-
            // repressed hold that is not yet 3 s old) can never nuke alone.
            if st.panic_gen.load(Ordering::SeqCst) == my_gen
                && st.panic_hold.load(Ordering::SeqCst)
                && panic_mods_down(mods)
            {
                eprintln!("panic hold: crypto-erasing vault");
                st.nuked.store(true, Ordering::SeqCst);
                st.panic_hold.store(false, Ordering::SeqCst);
                let _ = nuke_core(&st, None, &app2);
                do_scrub(&app2);
                if let Some(w) = app2.get_webview_window("main") {
                    let _ = w.hide();
                }
                st.hidden.store(true, Ordering::SeqCst);
            }
        });
        return;
    }

    // release: the first one of an episode (or of an erase) counts, the
    // duplicate storm from the OS poll threads is inert.
    if state.nuked.swap(false, Ordering::SeqCst) {
        state.panic_hold.store(false, Ordering::SeqCst);
        return; // already erased mid-hold; nothing left to do
    }
    if !state.panic_hold.swap(false, Ordering::SeqCst) {
        return; // no live episode = duplicate release
    }

    // tap behavior
    let win = app.get_webview_window("main");
    if state.hidden.load(Ordering::SeqCst) {
        eprintln!("panic tap: revealing window");
        if let Some(w) = win {
            let _ = w.show();
            let _ = w.set_focus();
        }
        state.hidden.store(false, Ordering::SeqCst);
    } else {
        eprintln!("panic tap: locking + hiding");
        let scrub = scrub_on_lock_flag(&state.core);
        lock_core(&state.core, &state.locked, app);
        if let Some(w) = win {
            let _ = w.hide();
        }
        state.hidden.store(true, Ordering::SeqCst);
        if scrub {
            do_scrub(app);
        }
    }
}

fn register_panic_keys(app: &tauri::AppHandle, extra: Option<&str>) {
    use tauri::Manager;
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let _ = app.global_shortcut().unregister_all();
    let mut fault: Option<String> = None;
    if let Err(e) = app.global_shortcut().register(DEFAULT_PANIC_HOTKEY) {
        eprintln!("panic hotkey {DEFAULT_PANIC_HOTKEY} unavailable: {e}");
        fault = Some(format!("{DEFAULT_PANIC_HOTKEY}: {e}"));
    }
    if let Some(hk) = extra.filter(|h| *h != DEFAULT_PANIC_HOTKEY) {
        if let Err(e) = app.global_shortcut().register(hk) {
            eprintln!("stored panic hotkey {hk} unavailable: {e}");
            fault = Some(format!("{hk}: {e}"));
        }
    }
    // #41: an unarmed panic key must be visible in the UI, not just in stderr.
    // Measured in the field: with a hidden instance already holding the
    // combination, the copy the user was actually looking at registered nothing
    // and said nothing about it.
    if let Some(st) = app.try_state::<AppState>() {
        *st.panic_fault.lock().unwrap() = fault;
    }
}

/// #41: is the panic key live right now, and if not, why. Works whether or not
/// a vault is open — this is about the process, not the vault.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PanicStatus {
    hotkey: String,
    armed: bool,
    fault: Option<String>,
}

#[tauri::command]
fn panic_status(state: tauri::State<'_, AppState>) -> PanicStatus {
    let stored = {
        let inner = state.inner.lock().unwrap();
        inner
            .opened
            .as_ref()
            .and_then(|o| goon_core::db::get_meta(&o.conn, "panic_hotkey"))
    };
    let fault = state.panic_fault.lock().unwrap().clone();
    PanicStatus {
        hotkey: stored.unwrap_or_else(|| DEFAULT_PANIC_HOTKEY.into()),
        armed: fault.is_none(),
        fault,
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PanicInfo {
    /// the key that works right now (stored one, else the default)
    hotkey: String,
    /// what the vault has stored (None = using the default)
    stored: Option<String>,
    /// when the last successful crypto-erase drill ran
    last_drill: Option<String>,
    /// #41: is the key actually registered in this process?
    armed: bool,
    /// #41: why it is not, when it isn't
    fault: Option<String>,
}

#[tauri::command]
fn panic_info(state: tauri::State<'_, AppState>) -> Result<PanicInfo, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let stored = goon_core::db::get_meta(&opened.conn, "panic_hotkey");
    let fault = state.panic_fault.lock().unwrap().clone();
    Ok(PanicInfo {
        hotkey: stored
            .clone()
            .unwrap_or_else(|| DEFAULT_PANIC_HOTKEY.into()),
        stored,
        last_drill: goon_core::db::get_meta(&opened.conn, "nuke_last_drill"),
        armed: fault.is_none(),
        fault,
    })
}

/// Store a custom panic key in the vault (None = back to the default) and
/// re-register. Validated by parsing first — a typo must not silently
/// unarm the panic path.
#[tauri::command]
fn set_panic_hotkey(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    hotkey: Option<String>,
) -> Result<PanicInfo, String> {
    let hk = hotkey
        .map(|h| h.trim().to_string())
        .filter(|h| !h.is_empty() && h != DEFAULT_PANIC_HOTKEY);
    if let Some(cand) = &hk {
        cand.parse::<tauri_plugin_global_shortcut::Shortcut>()
            .map_err(|_| format!("\"{cand}\" is not a hotkey — try e.g. CommandOrControl+Alt+K"))?;
    }
    {
        let mut inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_mut().ok_or("vault is locked")?;
        goon_core::db::set_meta(&opened.conn, "panic_hotkey", hk.as_deref()).map_err(err)?;
    }
    register_panic_keys(&app, hk.as_deref());
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    // #41: report honestly whether the re-register worked (one guard: locking
    // the same mutex twice in one expression would self-deadlock).
    let fault = state.panic_fault.lock().unwrap().clone();
    Ok(PanicInfo {
        hotkey: hk.clone().unwrap_or_else(|| DEFAULT_PANIC_HOTKEY.into()),
        stored: hk,
        last_drill: goon_core::db::get_meta(&opened.conn, "nuke_last_drill"),
        armed: fault.is_none(),
        fault,
    })
}

/// Crypto-erase drill: build a throwaway vault, nuke it with the real nuke
/// path, and verify nothing survives. This is how you *know* the nuke still
/// works without gambling the real vault.
fn run_nuke_drill() -> Result<String, String> {
    let dir = std::env::temp_dir().join(format!("goonflow-drill-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let key = goon_vault::create_keyfile(&dir, "drill").map_err(err)?;
    {
        let conn = goon_vault::open_db(&dir, &key).map_err(err)?;
        goon_core::db::init_schema(&conn).map_err(err)?;
        conn.execute(
            "INSERT INTO items (id, kind, title, created_at, updated_at) VALUES ('drill','game','throwaway','0','0')",
            [],
        )
        .map_err(err)?;
        // drop conn + key BEFORE shredding (Windows file locks), like nuke_vault
    }
    goon_vault::nuke(&dir).map_err(err)?;
    if dir.exists() {
        let left: Vec<String> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.file_name().to_string_lossy().into())
                    .collect()
            })
            .unwrap_or_default();
        if !left.is_empty() {
            return Err(format!("drill FAILED: something survived: {left:?}"));
        }
        // empty husk is fine — no bytes to recover; tidy up anyway
        let _ = std::fs::remove_dir_all(&dir);
    }
    Ok("throwaway vault created, then crypto-erased through the real nuke path: key shredded first, DB+WAL overwritten and deleted".into())
}

#[tauri::command]
fn nuke_drill(state: tauri::State<'_, AppState>) -> Result<String, String> {
    let report = run_nuke_drill()?;
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    goon_core::db::set_meta(&opened.conn, "nuke_last_drill", Some(&crate::now_epoch()))
        .map_err(err)?;
    Ok(report)
}

/// Crypto-erase: keyfile shredded first, then every vault file overwritten
/// and deleted, then the pointer cleared. Irreversible by design.
/// `dir` lets the Reconnect screen nuke a vault that isn't currently attached
/// (e.g. the pointer points at a folder that was moved or partially deleted).
#[tauri::command]
fn nuke_vault(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    confirm: String,
    dir: Option<String>,
) -> Result<(), String> {
    if confirm != "NUKE" {
        return Err("type NUKE exactly to confirm".into());
    }
    nuke_core(&state, dir, &app)
}

/// The irreversibility lives exactly once here — the typed-NUKE flow and the
/// hold-3-seconds panic flow share it, so both really destroy everything.
fn nuke_core(state: &AppState, dir: Option<String>, app: &tauri::AppHandle) -> Result<(), String> {
    // Stop every job first: nothing should be fetching or writing while the
    // files are being shredded, and no loop may re-open anything afterwards.
    state.jobs.cancel_all();
    state.queue.clear();
    let root = {
        let mut inner = state.inner.lock().unwrap();
        let root = inner
            .vault_root
            .clone()
            .or_else(|| dir.map(PathBuf::from))
            .ok_or("no vault folder attached — pick its folder or use \"Forget pointer\"")?;
        inner.opened = None; // close DB before shredding (Windows file locks)
        inner.vault_root = None;
        inner.vault_gen.fetch_add(1, Ordering::SeqCst);
        goon_locator::clear(&state.exe_dir);
        state.locked.store(true, Ordering::SeqCst);
        root
    };
    if root.is_dir() {
        goon_vault::nuke(&root).map_err(err)?;
        let _ = std::fs::remove_dir_all(&root);
    }
    // Job history holds URLs and paths — wipe it along with the vault.
    state.jobs.clear();
    end_vault_session(app);
    Ok(())
}

/// Clear the pointer file (flow.cfg) without touching any vault folder.
/// Escape hatch when the pointer is stale/wrong and you want a clean start.
#[tauri::command]
fn reset_pointer(state: tauri::State<'_, AppState>, app: tauri::AppHandle) {
    state.jobs.cancel_all();
    state.queue.clear();
    goon_locator::clear(&state.exe_dir);
    let mut inner = state.inner.lock().unwrap();
    inner.vault_root = None;
    inner.opened = None;
    inner.vault_gen.fetch_add(1, Ordering::SeqCst);
    state.locked.store(true, Ordering::SeqCst);
    end_vault_session(&app);
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

/// The flat argument list is the wire contract: `api.ts listItems` sends these
/// keys and Tauri matches them by name. Nesting them into one struct argument
/// would be the cleaner Rust shape but would change every grid call's payload for
/// no gain the frontend can see, so #51 resolves this one by keeping the shape
/// and reshaping the *core* call (`db::ListQuery`) instead.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
fn list_items(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
    offset: Option<i64>,
    search: Option<String>,
    kind: Option<String>,
    tag: Option<String>,
    site: Option<String>,
    source: Option<String>,
) -> Result<Vec<goon_core::db::ItemSummary>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let q = goon_core::db::ListQuery {
        limit: limit.unwrap_or(200),
        offset: offset.unwrap_or(0),
        search: search.as_deref(),
        kind: kind.as_deref(),
        tag: tag.as_deref(),
        site: site.as_deref(),
        source: source.as_deref(),
    };
    goon_core::db::list_items(&opened.conn, &q).map_err(err)
}

#[tauri::command]
fn source_stats(
    state: tauri::State<'_, AppState>,
    kind: Option<String>,
) -> Result<goon_core::db::SourceStats, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::source_stats(&opened.conn, kind.as_deref()).map_err(err)
}

#[tauri::command]
fn site_stats(
    state: tauri::State<'_, AppState>,
    kind: Option<String>,
) -> Result<Vec<goon_core::db::SiteCount>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::counts_by_site(&opened.conn, kind.as_deref()).map_err(err)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ThumbPayload {
    id: String,
    mime: String,
    data: String,
}

#[tauri::command]
fn get_thumbs(
    state: tauri::State<'_, AppState>,
    ids: Vec<String>,
) -> Result<Vec<ThumbPayload>, String> {
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

// ---------- full viewer (#21, many frames per item #62) ----------

/// Biggest blob we inline into the webview for preview. Beyond this (rare,
/// opt-in stored videos) we show the poster and let the OS player handle it.
const INLINE_MAX: u64 = 96 * 1024 * 1024;

/// One row of the viewer's filmstrip (#62). An item can hold several originals -
/// a `+Images` folder import stores one per file, and #53's stored attachments do
/// the same - and the old single-picture read (`media_view`, `best_for` with a
/// `LIMIT 1`) could only ever reach one of them, so a 40-picture import showed one
/// picture. Listing frames separately from their bytes is the point: the strip
/// must not pull every BLOB to paint a row of tiles.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaFrame {
    id: i64,
    /// "full" | "video" | "thumb". A `thumb` appears only when it is the item's
    /// only media, which is every single-picture item written before #62.
    kind: String,
    mime: String,
    /// bytes the vault holds for this frame, so the viewer can label and plan
    /// without pulling anything
    size: i64,
}

#[tauri::command]
fn media_for_item(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<Vec<MediaFrame>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    Ok(goon_core::media::frames_for(&opened.conn, &item_id)
        .into_iter()
        .map(|f| MediaFrame {
            id: f.id,
            kind: f.kind,
            mime: f.mime,
            size: f.size,
        })
        .collect())
}

/// One frame's bytes, asked for by id.
///
/// `media_view` used to quietly relabel an oversized stored video as its item's
/// thumbnail so the viewer still showed something. That substitution is gone:
/// the tile is now a frame the strip can show honestly, and presenting a video
/// frame as a thumbnail is the kind of white lie #60 was about. The viewer keeps
/// the tile it already has and says the picture is too big to inline.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaFrameBytes {
    kind: String,
    mime: String,
    /// base64 inline bytes; None when a stored video is too big to ship whole
    data: Option<String>,
    /// the stored size when the bytes were not inlined
    too_big: Option<u64>,
}

#[tauri::command]
fn media_frame_bytes(
    state: tauri::State<'_, AppState>,
    id: i64,
) -> Result<Option<MediaFrameBytes>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let Some(b) = goon_core::media::bytes_for_id(&opened.conn, id) else {
        return Ok(None);
    };
    let size = b.data.len() as u64;
    if b.kind == "video" && size > INLINE_MAX {
        return Ok(Some(MediaFrameBytes {
            kind: b.kind,
            mime: b.mime,
            data: None,
            too_big: Some(size),
        }));
    }
    Ok(Some(MediaFrameBytes {
        kind: b.kind,
        mime: b.mime,
        data: Some(B64.encode(b.data)),
        too_big: None,
    }))
}

// ---------- pictures the page offered (#53) ----------

/// A candidate picture on an item: the url the fetch found, and whether the user
/// has stored it yet. Nothing here was downloaded - the list is the feature.
///
/// No `mime` on the wire: the type of a candidate is not knowable until its bytes
/// arrive (#46 - f95zone serves `image/avif` from a `.jpg` url, and a blocked
/// hotlink answers with HTML at status 200), so advertising one would be a column
/// that is always empty. The core row keeps the column for the day a site tells us.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CandidateImage {
    url: String,
    /// epoch seconds, as text, the way every other timestamp crosses to the UI
    found_at: String,
    stored_at: Option<String>,
}

#[tauri::command]
fn source_images(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<Vec<CandidateImage>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    Ok(goon_core::db::source_images_for(&opened.conn, &item_id)
        .into_iter()
        .map(|s| CandidateImage {
            url: s.url,
            found_at: s.found_at.to_string(),
            stored_at: s.stored_at.map(|v| v.to_string()),
        })
        .collect())
}

/// Store one picture the user picked out of that list. It is a job, exactly like
/// a clip (#58's lesson): a line in the activity strip, a Stop button, and
/// something Lock or Nuke can actually give up on.
#[tauri::command]
async fn store_source_image(
    state: tauri::State<'_, AppState>,
    item_id: String,
    url: String,
) -> Result<ingest::StorePictureReport, String> {
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("url is empty".into());
    }
    let job = state
        .jobs
        .start("image", &url)
        .ok_or("a picture fetch is still running \u{2014} the activity strip shows where it got to")?;
    let report = ingest::store_picture(&state, item_id, url).await;
    match &report {
        Ok(r) => job.finish(&format!(
            "{} ({} frames on the item){}",
            r.mime,
            r.frames,
            if r.already { ", already stored" } else { "" }
        )),
        Err(e) => job.finish(&format!("picture failed: {e}")),
    }
    report
}

/// #37: the vault refuses a nameless row. The library search matches on title,
/// so an empty one is invisible forever - refusing the write here beats letting
/// some future caller (companion route, importer bug) create an item nobody can
/// find again. The UI guards this too; the guard belongs at the door.
fn clean_title(title: &str) -> Result<&str, String> {
    let t = title.trim();
    if t.is_empty() {
        Err("a title is required".into())
    } else {
        Ok(t)
    }
}

#[tauri::command]
fn update_item(
    state: tauri::State<'_, AppState>,
    id: String,
    title: String,
    circle: Option<String>,
    status: String,
    version: Option<String>,
    notes: Option<String>,
) -> Result<(), String> {
    let title = clean_title(&title)?;
    let status = Status::parse(&status).ok_or("invalid status")?;
    let mut inner = state.inner.lock().unwrap();
    let conn = &inner.opened.as_mut().ok_or("vault is locked")?.conn;
    goon_core::db::update_item(
        conn,
        &id,
        title,
        circle.as_deref().filter(|c| !c.trim().is_empty()),
        status,
        version.as_deref().filter(|v| !v.trim().is_empty()),
        // None keeps the old note (the DB uses IFNULL); Some("") clears it, which
        // is what an emptied Notes box means.
        notes.as_deref(),
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
    // #58: this used to call `ingest::clip` with no job at all, so the one action
    // most users take had no line in the activity strip, no spinner, no Stop button,
    // and nothing for Lock or Nuke to cancel. The companion API's clip path
    // (companion.rs) already started one; the in-app path now does the same, so both
    // doors give the same observability and the same cancellation.
    let url = url.trim().to_string();
    if url.is_empty() {
        return Err("url is empty".into());
    }
    let job = state
        .jobs
        .start("clip", &url)
        .ok_or("a clip is still running \u{2014} the activity strip shows where it got to")?;
    let report = ingest::clip(&state, url).await;
    match &report {
        Ok(r) => job.finish(&format!("clipped ({})", r.site)),
        Err(e) => job.finish(&format!("clip failed: {e}")),
    }
    report
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
    kind: Option<String>,
) -> Result<Vec<goon_core::db::TagCount>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::top_tags(&opened.conn, limit.unwrap_or(40), kind.as_deref()).map_err(err)
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
    /// #38: does this site ask for a login at all. The UI hides the cookie box
    /// where this is false, so a site that never asked for one never collects a
    /// credential nobody needed.
    needs_cookie: bool,
}

#[tauri::command]
fn known_sites() -> Vec<SiteInfo> {
    ingest::KNOWN_SITES
        .iter()
        .map(|(site, kind)| SiteInfo {
            site: (*site).to_string(),
            kind,
            mirror: ingest::is_mirror_site(site),
            default_rate_ms: ingest::default_rate_ms(site),
            needs_cookie: cookies::needs_cookie(site),
        })
        .collect()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SiteRow {
    site: String,
    enabled: bool,
    cookie_set: bool,
    rate_ms: i64,
    /// #38: what the last cookie check said, or None when the saved cookie has
    /// never been checked. Unknown must not be drawn as a failure.
    cookie_ok: Option<bool>,
    cookie_checked_at: Option<String>,
    /// Whether this site asks for a login at all, so the UI does not offer a
    /// cookie box for a site that would only store junk.
    needs_cookie: bool,
}

#[tauri::command]
fn get_site_settings(state: tauri::State<'_, AppState>) -> Result<Vec<SiteRow>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let rows = goon_core::db::get_site_settings(&opened.conn).map_err(err)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            let verdict = cookies::recorded(&opened.conn, &r.site);
            SiteRow {
                site: r.site.clone(),
                enabled: r.enabled,
                cookie_set: r.cookie_set,
                rate_ms: r.rate_ms,
                cookie_ok: verdict.as_ref().map(|(ok, _)| *ok),
                cookie_checked_at: verdict
                    .and_then(|(_, at)| if at.is_empty() { None } else { Some(at) }),
                needs_cookie: cookies::needs_cookie(&r.site),
            }
        })
        .collect())
}

#[tauri::command]
async fn set_site_setting(
    state: tauri::State<'_, AppState>,
    site: String,
    enabled: Option<bool>,
    cookie: Option<String>,
    rate_ms: Option<i64>,
) -> Result<Option<cookies::Verdict>, String> {
    let site = cookies::canonical_site(&site);
    if site.is_empty() {
        return Err("no site".into());
    }
    // #38: validate the paste BEFORE it is stored. A paste that cannot be a
    // session used to sit in the vault until some later fetch failed, described
    // in a note about an unrelated item.
    let incoming = cookie.as_deref().map(str::trim);
    if let Some(c) = incoming.filter(|c| !c.is_empty()) {
        if !cookies::needs_cookie(&site) {
            return Err(format!("{site} does not ask for a login cookie"));
        }
        let v = cookies::structural(&site, c);
        if !v.ok {
            return Err(v.note);
        }
    }
    let probe_with = {
        let mut inner = state.inner.lock().unwrap();
        let conn = &mut inner.opened.as_mut().ok_or("vault is locked")?.conn;
        goon_core::db::set_site_setting(
            conn,
            &site,
            enabled,
            incoming,
            rate_ms.map(|r| r.clamp(0, 600_000)),
            ingest::default_rate_ms(&site),
        )
        .map_err(err)?;
        // A saved verdict describes the cookie that was just replaced.
        if incoming.is_some() {
            let _ = cookies::forget(conn, &site);
        }
        incoming
            .filter(|c| !c.is_empty())
            .and_then(|_| goon_core::db::site_cookie(conn, &site))
    };
    let Some(saved) = probe_with else {
        return Ok(None);
    };
    Ok(Some(cookies::check(&state.core, &site, &saved).await))
}

/// #38: re-check the cookie that is already in the vault.
#[tauri::command]
async fn check_site_cookie(
    state: tauri::State<'_, AppState>,
    site: String,
) -> Result<cookies::Verdict, String> {
    let site = cookies::canonical_site(&site);
    if !cookies::needs_cookie(&site) {
        return Err(format!("{site} does not ask for a login cookie"));
    }
    let saved = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        goon_core::db::site_cookie(&opened.conn, &site)
    };
    let saved = saved
        .filter(|c| !c.is_empty())
        .ok_or(format!("no cookie saved for {site}"))?;
    Ok(cookies::check(&state.core, &site, &saved).await)
}

/// #38: take the saved cookie back out of the vault for one site, leaving its
/// on/off switch and rate box alone.
///
/// This is the missing half of #38. The settings command took `Option<&str>` for
/// the cookie and its SQL is `IFNULL(?3, cookie)`, where `None` means *"keep the
/// stored value"* — so a Clear control wired through it reported success and
/// changed nothing, and a paste that turned out to be stale (or someone else's
/// session) kept being sent on every fetch. An explicit empty write is the only
/// honest way out.
#[tauri::command]
async fn clear_site_cookie(
    state: tauri::State<'_, AppState>,
    site: String,
) -> Result<bool, String> {
    let site = cookies::canonical_site(&site);
    if site.is_empty() {
        return Err("no site".into());
    }
    let mut inner = state.inner.lock().unwrap();
    let conn = &mut inner.opened.as_mut().ok_or("vault is locked")?.conn;
    let had = goon_core::db::site_cookie(conn, &site).is_some();
    goon_core::db::clear_site_cookie(conn, &site).map_err(err)?;
    // The recorded verdict belonged to the cookie that is gone. Left in place,
    // the Sources list would keep showing last month's "works".
    let _ = cookies::forget(conn, &site);
    Ok(had)
}

// ---------- update feed (M2) ----------

#[tauri::command]
async fn check_updates(state: tauri::State<'_, AppState>) -> Result<ingest::UpdateReport, String> {
    let job = state
        .jobs
        .start("updates", "tracked games")
        .ok_or("an update check is already running")?;
    let report = ingest::check_updates(&state, &job).await;
    match &report {
        Ok(r) => job.finish(&format!(
            "{} checked, {} updated{}",
            r.checked,
            r.updated,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
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
    kind: Option<String>,
) -> Result<Vec<goon_core::db::PairSuggestion>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::pairing_suggestions(&opened.conn, kind.as_deref()).map_err(err)
}

/// #31: "keep both" — this pair is not a duplicate, stop suggesting it.
#[tauri::command]
fn dismiss_pair(
    state: tauri::State<'_, AppState>,
    a_id: String,
    b_id: String,
) -> Result<(), String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::dismiss_pair(&opened.conn, &a_id, &b_id).map_err(err)
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

// ---------- launch (#18) ----------

/// The importer stores local files as `file:///<path>` — decode back.
fn local_source_path(sources: &[goon_core::db::SourceRow]) -> Option<PathBuf> {
    let locals: Vec<PathBuf> = sources
        .iter()
        .filter(|s| s.site == "local")
        .filter_map(|s| s.url.strip_prefix("file:///").map(PathBuf::from))
        .collect();
    // Prefer something launchable (folder or exe) over a loose archive, so an
    // extracted folder (#17) wins over the zip it came from.
    locals
        .iter()
        .find(|p| {
            p.is_dir()
                || p.extension()
                    .is_some_and(|x| x.to_string_lossy().eq_ignore_ascii_case("exe"))
        })
        .cloned()
        .or_else(|| locals.first().cloned())
}

fn plan_for(state: &AppState, item_id: &str) -> Result<launcher::LaunchPlan, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let Some(d) = goon_core::db::get_item(&opened.conn, item_id).map_err(err)? else {
        return Err("item not found".into());
    };
    Ok(launcher::resolve(
        local_source_path(&d.sources).as_deref(),
        d.launch_path.as_deref(),
    ))
}

/// What launching would run (and why not) — never spawns.
#[tauri::command]
fn preview_launch(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<launcher::LaunchPlan, String> {
    plan_for(&state, &item_id)
}

/// Resolve and start it. Games keep running after the app closes.
#[tauri::command]
fn launch_item(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<launcher::LaunchPlan, String> {
    let plan = plan_for(&state, &item_id)?;
    launcher::launch(&plan)
}

#[tauri::command]
fn set_launch_path(
    state: tauri::State<'_, AppState>,
    item_id: String,
    path: Option<String>,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    goon_core::db::set_launch_path(&opened.conn, &item_id, path.as_deref(), &now_epoch())
        .map(|_| ())
        .map_err(err)
}

// ---------- online match suggestions (#14) ----------

/// Search DLsite/f95 for every local-only game and store proposals.
#[tauri::command]
async fn find_matches(state: tauri::State<'_, AppState>) -> Result<ingest::MatchReport, String> {
    let job = state
        .jobs
        .start("matches", "online match search")
        .ok_or("a match search is already running — see the activity strip")?;
    let report = ingest::find_matches(&state, &job).await;
    match &report {
        Ok(r) => job.finish(&format!(
            "{} scanned, {} with suggestions{}",
            r.scanned,
            r.with_suggestions,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
fn list_match_suggestions(
    state: tauri::State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<goon_core::db::SuggestionRow>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::list_suggestions(&opened.conn, limit.unwrap_or(200)).map_err(err)
}

#[tauri::command]
fn dismiss_suggestion(
    state: tauri::State<'_, AppState>,
    item_id: String,
    site: String,
    url: String,
) -> Result<(), String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::delete_suggestion(&opened.conn, &item_id, &site, &url).map_err(err)
}

#[tauri::command]
fn clear_item_suggestions(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<(), String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::clear_suggestions(&opened.conn, &item_id).map_err(err)
}

// ---------- vault video storage (#16, opt-in) ----------

#[tauri::command]
fn video_info(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<video::VideoInfo, String> {
    video::video_info(&state, &item_id)
}

#[tauri::command]
async fn store_video(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<video::VideoStoreReport, String> {
    let job = state
        .jobs
        .start("video", &item_id)
        .ok_or("another video store is running — see the activity strip")?;
    let report = video::store_video(&state, &item_id, &job).await;
    match &report {
        Ok(r) => job.finish(&format!(
            "stored {} MB{}",
            r.stored_bytes / (1024 * 1024),
            if r.thumb_added {
                " + first-frame thumb"
            } else {
                ""
            }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
fn remove_stored_video(state: tauri::State<'_, AppState>, item_id: String) -> Result<(), String> {
    video::remove_stored_video(&state, &item_id)
}

// ---------- archive extraction (#17 increment 1) ----------

#[tauri::command]
fn extract_plan(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<Option<extract::ExtractPlan>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    let Some(d) = goon_core::db::get_item(&opened.conn, &item_id).map_err(err)? else {
        return Err("item not found".into());
    };
    Ok(extract::plan(&d.sources))
}

/// Unpack the item's zip source next to the archive and link the folder so
/// the game can actually be launched. Runs as an observable job.
#[tauri::command]
async fn extract_game(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<extract::ExtractReport, String> {
    let job = state
        .jobs
        .start("extract", &item_id)
        .ok_or("another extraction is running — see the activity strip")?;
    let report = extract::extract_for_item(&state, &item_id, &job);
    match &report {
        Ok(r) => job.finish(&format!(
            "{} → {} entries{}",
            r.dest,
            r.entries,
            if r.source_added { " + linked" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

// ---------- mods / tools (#25, D10) ----------

#[tauri::command]
fn list_mods(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<Vec<goon_core::db::ModRow>, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    goon_core::db::list_mods(&opened.conn, &item_id).map_err(err)
}

#[tauri::command]
fn add_mod(
    state: tauri::State<'_, AppState>,
    item_id: String,
    name: String,
    kind: String,
    path: String,
    args: Option<String>,
) -> Result<goon_core::db::ModRow, String> {
    if !matches!(kind.as_str(), "mtool" | "cheat" | "other" | "dll") {
        return Err("recipe kind must be mtool, cheat, other or dll".into());
    }
    let path = path.trim().to_string();
    if path.is_empty() {
        return Err("pick the tool's .exe first".into());
    }
    let name = {
        let n = name.trim().to_string();
        if n.is_empty() {
            std::path::Path::new(&path)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_else(|| "tool".into())
        } else {
            n
        }
    };
    let args = args.unwrap_or_default().trim().to_string();
    let now = crate::now_epoch();
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    let id = goon_core::db::add_mod(&opened.conn, &item_id, &name, &kind, &path, &args, &now)
        .map_err(err)?;
    let _ = goon_core::db::touch_item(&opened.conn, &item_id, &now);
    goon_core::db::get_mod(&opened.conn, &item_id, id)
        .map_err(err)?
        .ok_or_else(|| "mod vanished immediately after adding".to_string())
}

#[tauri::command]
fn delete_mod(
    state: tauri::State<'_, AppState>,
    item_id: String,
    mod_id: i64,
) -> Result<(), String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    if let Some(m) = goon_core::db::get_mod(&opened.conn, &item_id, mod_id).map_err(err)? {
        if m.status == "applied" {
            return Err("this mod is deployed — press Revert first (removing it now would strand the .flowbak backups)".into());
        }
    }
    if !goon_core::db::delete_mod(&opened.conn, &item_id, mod_id).map_err(err)? {
        return Err("that recipe is not on this item".into());
    }
    Ok(())
}

/// The folder a mod deploys into: the game's local source folder (or an exe
/// source's parent). None when the game isn't on disk.
fn game_folder_for(sources: &[goon_core::db::SourceRow]) -> Option<PathBuf> {
    let p = local_source_path(sources)?;
    if p.is_dir() {
        Some(p)
    } else if p.is_file() {
        p.parent().map(|x| x.to_path_buf())
    } else {
        None
    }
}

/// Deploy a dll/patch mod into the game folder with *.flowbak backups.
#[tauri::command]
fn apply_mod(
    state: tauri::State<'_, AppState>,
    item_id: String,
    mod_id: i64,
) -> Result<goon_core::db::ModRow, String> {
    let (m, game_dir) = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        let m = goon_core::db::get_mod(&opened.conn, &item_id, mod_id)
            .map_err(err)?
            .ok_or("recipe not found")?;
        if m.kind != "dll" {
            return Err("only dll/patch mods can be deployed".into());
        }
        if m.status == "applied" {
            return Err("already deployed — revert first".into());
        }
        let d = goon_core::db::get_item(&opened.conn, &item_id)
            .map_err(err)?
            .ok_or("item not found")?;
        let dir = game_folder_for(&d.sources)
            .ok_or("the game is not on disk (link a folder or exe source first)")?;
        (m, dir)
    };
    let manifest = mods::apply(Path::new(&m.path), &game_dir)?;
    let json = serde_json::to_string(&manifest).map_err(err)?;
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault vanished mid-apply")?;
    goon_core::db::set_mod_status(&opened.conn, &item_id, mod_id, "applied", Some(&json))
        .map_err(err)?;
    let _ = goon_core::db::touch_item(&opened.conn, &item_id, &crate::now_epoch());
    goon_core::db::get_mod(&opened.conn, &item_id, mod_id)
        .map_err(err)?
        .ok_or_else(|| "mod vanished".to_string())
}

/// Restore originals from the manifest; returns per-file notes (missing
/// backups after a game update, etc.).
#[tauri::command]
fn revert_mod(
    state: tauri::State<'_, AppState>,
    item_id: String,
    mod_id: i64,
) -> Result<Vec<String>, String> {
    let (m, game_dir) = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        let m = goon_core::db::get_mod(&opened.conn, &item_id, mod_id)
            .map_err(err)?
            .ok_or("recipe not found")?;
        if m.status != "applied" {
            return Err("nothing is deployed for this mod".into());
        }
        let d = goon_core::db::get_item(&opened.conn, &item_id)
            .map_err(err)?
            .ok_or("item not found")?;
        let dir =
            game_folder_for(&d.sources).ok_or("the game folder is not reachable right now")?;
        (m, dir)
    };
    let manifest: Vec<mods::ManifestEntry> =
        serde_json::from_str(m.manifest.as_deref().unwrap_or("[]")).map_err(err)?;
    let notes = mods::revert(&game_dir, &manifest);
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault vanished mid-revert")?;
    goon_core::db::set_mod_status(&opened.conn, &item_id, mod_id, "linked", None).map_err(err)?;
    Ok(notes)
}

/// What a recipe would run (and why not) — never spawns.
#[tauri::command]
fn preview_mod(
    state: tauri::State<'_, AppState>,
    item_id: String,
    mod_id: i64,
) -> Result<mods::ModLaunch, String> {
    let (m, game) = {
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        let m = goon_core::db::get_mod(&opened.conn, &item_id, mod_id)
            .map_err(err)?
            .ok_or("recipe not found")?;
        if m.kind == "dll" {
            return Err("dll mods are deployed with Apply/Revert, not launched".into());
        }
        let Some(d) = goon_core::db::get_item(&opened.conn, &item_id).map_err(err)? else {
            return Err("item not found".into());
        };
        let game = launcher::resolve(
            local_source_path(&d.sources).as_deref(),
            d.launch_path.as_deref(),
        );
        (m, game)
    };
    Ok(mods::recipe(&m, &game))
}

/// Start the tool with its stored recipe. Tools keep running after we exit.
#[tauri::command]
fn launch_mod(
    state: tauri::State<'_, AppState>,
    item_id: String,
    mod_id: i64,
) -> Result<mods::ModLaunch, String> {
    let plan = preview_mod(state, item_id, mod_id)?;
    mods::launch(&plan)
}

// ---------- proxy / Tor (#24) ----------

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ProxySettings {
    proxy_url: Option<String>,
}

#[tauri::command]
fn get_proxy(state: tauri::State<'_, AppState>) -> Result<ProxySettings, String> {
    let inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_ref().ok_or("vault is locked")?;
    Ok(ProxySettings {
        proxy_url: goon_core::db::get_meta(&opened.conn, "proxy_url"),
    })
}

/// Store (or clear with None) the proxy for every networked path.
/// Validated before saving so a typo can't silently disable enrichment.
#[tauri::command]
fn set_proxy(
    state: tauri::State<'_, AppState>,
    url: Option<String>,
) -> Result<ProxySettings, String> {
    let trimmed = url.map(|u| u.trim().to_string()).filter(|u| !u.is_empty());
    if let Some(u) = &trimmed {
        http::validate_proxy(u)?;
    }
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    goon_core::db::set_meta(&opened.conn, "proxy_url", trimmed.as_deref()).map_err(err)?;
    Ok(ProxySettings {
        proxy_url: goon_core::db::get_meta(&opened.conn, "proxy_url"),
    })
}

// ---------- import (D13) — every import runs as a job ----------
// Async so Tauri runs them off the main thread: a 3000-link bookmark import
// must never freeze the window or block other commands.

#[tauri::command]
async fn import_game_folder(
    state: tauri::State<'_, AppState>,
    dir: String,
) -> Result<importer::GameImportReport, String> {
    let job = state
        .jobs
        .start("games", &dir)
        .ok_or("a game import is already running — see the activity strip")?;
    let report = importer::import_game_folder(&state, dir, &job);
    match &report {
        Ok(r) => job.finish(&format!(
            "{} imported, {} skipped{}",
            r.imported,
            r.skipped,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
async fn import_game_exe(
    state: tauri::State<'_, AppState>,
    exe: String,
) -> Result<importer::GameImportReport, String> {
    let job = state
        .jobs
        .start("games", &exe)
        .ok_or("a game import is already running — see the activity strip")?;
    let report = importer::import_game_exe(&state, exe, &job);
    match &report {
        Ok(r) => job.finish(&format!("{} imported", r.imported)),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
async fn import_game_zip(
    state: tauri::State<'_, AppState>,
    zip_path: String,
) -> Result<importer::GameImportReport, String> {
    let job = state
        .jobs
        .start("games", &zip_path)
        .ok_or("a game import is already running — see the activity strip")?;
    let report = importer::import_game_zip(&state, zip_path, &job);
    match &report {
        Ok(r) => job.finish(&format!("{} imported", r.imported)),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
async fn import_image_files(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<importer::ImageImportReport, String> {
    let job = state
        .jobs
        .start("images", &format!("{} images", paths.len()))
        .ok_or("an image import is already running — see the activity strip")?;
    let report = importer::import_image_files(&state, paths, &job);
    match &report {
        Ok(r) => job.finish(&format!(
            "{} stored{}",
            r.images_stored,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
async fn import_bookmarks(
    state: tauri::State<'_, AppState>,
    path: String,
    fetch_online: bool,
) -> Result<bookmarks::BookmarkReport, String> {
    let job = state
        .jobs
        .start("bookmarks", &path)
        .ok_or("a bookmark import is already running — see the activity strip")?;
    job.set_note(if fetch_online {
        "storing links, then fetching art politely"
    } else {
        "storing links (no network)"
    });
    let report = bookmarks::import_bookmarks(&state, path, fetch_online, &job).await;
    match &report {
        Ok(r) => job.finish(&format!(
            "{} imported, {} unrecognized{}",
            r.imported,
            r.unrecognized,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

#[tauri::command]
async fn import_video_files(
    state: tauri::State<'_, AppState>,
    paths: Vec<String>,
) -> Result<importer::VideoImportReport, String> {
    let job = state
        .jobs
        .start("videos", &format!("{} videos", paths.len()))
        .ok_or("a video import is already running — see the activity strip")?;
    let report = importer::import_video_files(&state, paths, &job);
    match &report {
        Ok(r) => job.finish(&format!(
            "{} tracked, {} skipped{}",
            r.imported,
            r.skipped,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

// ---------- per-item re-enrichment (M2.6) ----------

#[tauri::command]
async fn re_enrich_item(
    state: tauri::State<'_, AppState>,
    item_id: String,
) -> Result<ingest::EnrichReport, String> {
    let job = state
        .jobs
        .start("enrich", &item_id)
        .ok_or("an enrichment is already running — see the activity strip")?;
    job.set_note("re-fetching every source");
    let report = ingest::reenrich_item(&state, &item_id, &job).await;
    match &report {
        Ok(r) => job.finish(&format!(
            "{} of {} sources gave something{}",
            r.enriched,
            r.sources,
            if r.stopped { " (stopped)" } else { "" }
        )),
        Err(e) => job.finish(&format!("failed: {e}")),
    }
    report
}

/// #36: batch re-fetch ("Re-fetch metadata" in select mode). One observable
/// job (progress, current item, Stop — D15) over the same code path the
/// per-item button uses; nothing else is blocked while it runs.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RefetchSummary {
    items: usize,
    enriched: usize,
    sources: usize,
    failed: usize,
    stopped: bool,
}

#[tauri::command]
async fn re_enrich_items(
    state: tauri::State<'_, AppState>,
    item_ids: Vec<String>,
) -> Result<RefetchSummary, String> {
    let n = item_ids.len();
    if n == 0 {
        return Err("nothing selected".into());
    }
    let job = state
        .jobs
        .start("enrich", &format!("{n} items"))
        .ok_or("an enrichment is already running — see the activity strip")?;
    let mut sum = RefetchSummary {
        items: n,
        enriched: 0,
        sources: 0,
        failed: 0,
        stopped: false,
    };
    for (i, id) in item_ids.iter().enumerate() {
        if job.cancelled() {
            sum.stopped = true;
            sum.items = i;
            break;
        }
        job.set_progress(i, n, id);
        // #49: the worker gets a *child* handle. Its per-source counts land in
        // the nested `sub_*` fields instead of replacing this batch's `i/n`,
        // which is what made the strip read `0/1` for the whole run.
        let scope = job.child();
        match ingest::reenrich_item(&state, id, &scope).await {
            Ok(r) => {
                sum.enriched += r.enriched;
                sum.sources += r.sources;
            }
            Err(_) => sum.failed += 1,
        }
    }
    let failed_note = if sum.failed > 0 {
        format!(" · {} item(s) failed", sum.failed)
    } else {
        String::new()
    };
    job.finish(&format!(
        "{} of {} items done · {} of {} sources gave something{}",
        sum.items,
        n,
        sum.enriched,
        sum.sources,
        if sum.stopped {
            " (stopped)"
        } else {
            failed_note.as_str()
        }
    ));
    Ok(sum)
}

#[tauri::command]
fn add_source(
    state: tauri::State<'_, AppState>,
    item_id: String,
    url: String,
) -> Result<String, String> {
    let cls = ingest::classify(&url).ok_or("not a recognised source link")?;
    {
        // honour the Sources toggles before linking (mirrors default off)
        let inner = state.inner.lock().unwrap();
        let opened = inner.opened.as_ref().ok_or("vault is locked")?;
        let default_on = !ingest::is_mirror_site(&cls.site);
        if !goon_core::db::site_enabled(&opened.conn, &cls.site, default_on) {
            return Err(format!("{} is switched off under Sources", cls.site));
        }
    }
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    if goon_core::db::source_exists(&opened.conn, &cls.site, &url) {
        return Err("that exact link is already tracked somewhere".into());
    }
    let created = goon_core::db::insert_source(
        &opened.conn,
        &item_id,
        &cls.site,
        &url,
        cls.remote_id.as_deref(),
        None,
    )
    .map_err(err)?;
    if !created {
        return Err("already tracked".into());
    }
    Ok(cls.site)
}

#[tauri::command]
fn remove_source(
    state: tauri::State<'_, AppState>,
    item_id: String,
    source_id: i64,
) -> Result<(), String> {
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    goon_core::db::delete_source(&opened.conn, &item_id, source_id)
        .map(|_| ())
        .map_err(err)
}

#[tauri::command]
fn delete_items(state: tauri::State<'_, AppState>, ids: Vec<String>) -> Result<usize, String> {
    let mut inner = state.inner.lock().unwrap();
    let opened = inner.opened.as_mut().ok_or("vault is locked")?;
    let mut n = 0;
    for id in &ids {
        if goon_core::db::delete_item(&opened.conn, id).is_ok() {
            n += 1;
        }
    }
    Ok(n)
}

// ---------- activity (jobs) ----------

#[tauri::command]
fn list_jobs(state: tauri::State<'_, AppState>) -> Vec<jobs::JobInfo> {
    state.jobs.snapshot()
}

#[tauri::command]
fn cancel_job(state: tauri::State<'_, AppState>, id: u64) {
    state.jobs.cancel(id);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;

    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."));

    let locked = Arc::new(AtomicBool::new(true));

    // Resolve vault at startup: pointer → volume label/path; fallback to a
    // default folder beside the exe (handy for dev and single-folder use).
    let mut vault_root = goon_locator::load(&exe_dir).and_then(|p| goon_locator::resolve(&p));
    if vault_root.is_none() {
        let default = exe_dir.join("goonflow-vault");
        if goon_vault::is_vault_dir(&default) {
            vault_root = Some(default);
        }
    }

    let core = Core {
        inner: Arc::new(Mutex::new(Inner {
            vault_root,
            opened: None,
            vault_gen: AtomicU64::new(0),
        })),
        jobs: Arc::new(Jobs::default()),
        queue: Arc::new(queue::SiteQueue::default()),
    };
    let companion = match companion::start(locked.clone(), core.clone()) {
        Ok(info) => Some(info),
        Err(e) => {
            eprintln!("companion API disabled: {e}");
            None
        }
    };

    let state = AppState {
        core,
        exe_dir,
        companion,
        locked,
        hidden: Arc::new(AtomicBool::new(false)),
        panic_hold: Arc::new(AtomicBool::new(false)),
        panic_gen: Arc::new(AtomicU64::new(0)),
        nuked: Arc::new(AtomicBool::new(false)),
        panic_fault: Arc::new(Mutex::new(None)),
    };

    let app = tauri::Builder::default()
        // #41: first plugin, per the plugin's own requirement. A second launch
        // reveals the instance that is already running (it may be sitting
        // hidden after a panic tap — that is exactly when you launch again) and
        // clears the hidden flag, instead of starting a second app that could
        // never register the panic key because the first one owns it.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
            if let Some(st) = app.try_state::<AppState>() {
                st.hidden.store(false, Ordering::SeqCst);
            }
        }))
        .plugin(tauri_plugin_dialog::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(move |app, shortcut, event| {
                    let pressed =
                        event.state() == tauri_plugin_global_shortcut::ShortcutState::Pressed;
                    eprintln!("panic hotkey event: {:?}", event.state());
                    panic_event(app, pressed, shortcut.mods);
                })
                .build(),
        )
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
            site_stats,
            list_items,
            source_stats,
            get_thumbs,
            media_for_item,
            media_frame_bytes,
            source_images,
            store_source_image,
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
            check_site_cookie,
            clear_site_cookie,
            check_updates,
            list_updates,
            pairing_suggestions,
            dismiss_pair,
            merge_items,
            import_game_folder,
            import_game_exe,
            import_game_zip,
            import_image_files,
            import_bookmarks,
            import_video_files,
            re_enrich_item,
            re_enrich_items,
            add_source,
            remove_source,
            delete_items,
            preview_launch,
            launch_item,
            set_launch_path,
            find_matches,
            list_match_suggestions,
            dismiss_suggestion,
            clear_item_suggestions,
            video_info,
            store_video,
            remove_stored_video,
            extract_plan,
            extract_game,
            list_mods,
            add_mod,
            delete_mod,
            preview_mod,
            launch_mod,
            apply_mod,
            revert_mod,
            get_proxy,
            set_proxy,
            panic_info,
            panic_status,
            set_panic_hotkey,
            nuke_drill,
            get_chrome,
            set_chrome,
            scrub_now,
            get_scrub_settings,
            set_scrub_on_lock,
            list_jobs,
            cancel_job
        ])
        .build(tauri::generate_context!())
        .expect("error while building The Goon Flow");

    // Arm the panic key before anything else can go wrong — it must always
    // be pressable, even if the vault is locked or a source is down.
    register_panic_keys(app.handle(), None);

    // Decoy window title (#27). Config windows are only guaranteed to exist
    // once the loop is running, so push it on Ready.
    let chrome_exe_dir = app.state::<AppState>().exe_dir.clone();

    app.run(move |app, event| {
        if let tauri::RunEvent::Ready = event {
            apply_chrome(app, &chrome_exe_dir);
        }
    });
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// #27: the decoy chrome file round-trips and sanitizes on read, so a
    /// hand-edited chrome.cfg can never smuggle junk into a window title.
    #[test]
    fn chrome_cfg_roundtrip_and_sanitizing() {
        let d = std::env::temp_dir().join(format!("goonflow-chrome-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();

        assert_eq!(read_chrome(&d), None, "no file = no decoy");
        std::fs::write(chrome_path(&d), "Flow Notes\nonly first line counts\n").unwrap();
        assert_eq!(read_chrome(&d).as_deref(), Some("Flow Notes"));

        assert_eq!(
            sanitize_chrome("  a\u{7}b  ").as_deref(),
            Some("ab"),
            "control chars stripped"
        );
        assert_eq!(sanitize_chrome("   "), None, "blank = absent");
        assert_eq!(sanitize_chrome(&"x".repeat(65)), None, "over-long refused");

        std::fs::write(chrome_path(&d), "\n").unwrap();
        assert_eq!(read_chrome(&d), None, "empty first line = no decoy");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// #23: the crypto-erase drill proves the real nuke path on a throwaway
    /// vault — build it, nuke it, nothing may survive.
    #[test]
    fn nuke_drill_leaves_nothing_behind() {
        let r = run_nuke_drill();
        assert!(r.is_ok(), "drill failed: {}", r.unwrap_err());
    }

    /// #23: the scrubber removes exactly thumbcache_*.db and nothing else.
    #[test]
    fn scrubber_only_touches_thumbcache_files() {
        let d = std::env::temp_dir().join(format!("goonflow-scrub-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in [
            "thumbcache_1.db",
            "thumbcache_99.db",
            "keepme.db",
            "thumbcache_notes.txt",
        ] {
            std::fs::write(d.join(f), b"x").unwrap();
        }
        let (gone, busy) = scrub_cache_dir(&d);
        assert_eq!((gone, busy), (2, 0));
        assert!(!d.join("thumbcache_1.db").exists());
        assert!(!d.join("thumbcache_99.db").exists());
        assert!(d.join("keepme.db").exists());
        assert!(d.join("thumbcache_notes.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

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
            core: Core {
                inner: Arc::new(Mutex::new(Inner {
                    vault_root: Some(vault.clone()),
                    opened: Some(OpenedVault { conn, _key: key }),
                    ..Default::default()
                })),
                jobs: Arc::new(Jobs::default()),
                queue: Arc::new(queue::SiteQueue::default()),
            },
            exe_dir: tmp.clone(),
            companion: None,
            locked: Arc::new(AtomicBool::new(false)),
            hidden: Arc::new(AtomicBool::new(false)),
            panic_hold: Arc::new(AtomicBool::new(false)),
            panic_gen: Arc::new(AtomicU64::new(0)),
            nuked: Arc::new(AtomicBool::new(false)),
            panic_fault: Arc::new(Mutex::new(None)),
        };

        let job = state.jobs.start("games", "games folder").unwrap();
        let report =
            importer::import_game_folder(&state, games.display().to_string(), &job).unwrap();
        job.finish("done");
        assert_eq!(report.imported, 1, "titles = {:?}", report.titles);

        let snap = state.jobs.snapshot();
        assert!(snap.iter().any(|j| j.kind == "games" && j.finished));

        let inner = state.inner.lock().unwrap();
        let conn = &inner.opened.as_ref().unwrap().conn;
        let items = goon_core::db::list_items(
            conn,
            &goon_core::db::ListQuery {
                limit: 300,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(items.len(), 1, "imported but list showed nothing");
        assert_eq!(items[0].title, "Game X");
        assert_eq!(items[0].version.as_deref(), Some("2.0"));
        let games_only = goon_core::db::list_items(
            conn,
            &goon_core::db::ListQuery {
                limit: 300,
                kind: Some("game"),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(games_only.len(), 1);
        drop(inner);

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
