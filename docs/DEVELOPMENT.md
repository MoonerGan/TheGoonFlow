# The Goon Flow — Developer Guide

Milestone plan, architecture rationale, and the decision log all live in
[DESIGN.md](DESIGN.md) and the [project board](https://github.com/MoonerGan/TheGoonFlow/projects/1). This doc is the practical "how to build, where the code
is, what invariants matter" companion.

## 1. Prerequisites (Windows)

| Tool | Why | Install |
|---|---|---|
| Rust (MSVC toolchain) | everything native | `winget install Rustlang.Rustup` |
| VS 2022 Build Tools, C++ workload | linking + C builds (SQLCipher, OpenSSL) | `winget install Microsoft.VisualStudio.2022.BuildTools` + `--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended` |
| Strawberry Perl + NASM | vendored OpenSSL build (SQLCipher backend) | `winget install StrawberryPerl.StrawberryPerl` `winget install NASM.NASM` |
| Node 18+ | frontend toolchain | `winget install OpenJS.NodeJS.LTS` |
| WebView2 runtime | Tauri webview | preinstalled on Win 11 |
| dav1d, **static** build | AVIF decoding (#52) — a C library linked into the exe | `vcpkg install dav1d:x64-windows-static` + copy into `%LOCALAPPDATA%\goonflow-build\dav1d-x64-static` (see "dav1d, the AVIF decoder" below) |

### The dev-env wrapper
Plain `cargo` in a bare shell **fails for the C deps** (they need the MSVC
env, not just `cl` on PATH — and since #52 also the static dav1d, so
`dav1d-sys` dies in its build script with a pkg-config stack trace). For the
everyday commands there is a wrapper per job; each one sources `dev-env.ps1`
itself, writes a log under `tools\`, and leaves a summary file whose numbers any
claim in a report can be checked against:

```powershell
powershell -ExecutionPolicy Bypass -File tools\run-test.ps1   -tag mNN   # cargo test --workspace
powershell -ExecutionPolicy Bypass -File tools\run-clippy.ps1 -tag cN    # cargo clippy --workspace --all-targets
powershell -ExecutionPolicy Bypass -File tools\run-release.ps1 -tag mNN  # release build + sha256
```

Anything else that needs cargo imports the environment first. Dot-sourcing needs
a bypassed policy (`powershell -ExecutionPolicy Bypass -Command ". tools\dev-env.ps1; …"`),
and then the cargo call goes through `cmd /c`:

```powershell
cmd /c "cargo test --workspace > tools\cargo-test.log 2>&1"
```

`cmd /c "... > log 2>&1"` is deliberate: PowerShell 5.1 mangles native-command
stderr progress into `NativeCommandError` records — cargo works, but the log
capture gets noisy/abort-prone without the redirection wrapper.

**Never round-trip source files through PowerShell 5 text cmdlets.** The system
ANSI codepage here is Shift_JIS, so `Get-Content -Raw` (no `-Encoding`) decodes
BOM-less UTF-8 as Shift_JIS and `Set-Content`/`WriteAllText` writes the mojibake
back — `—` becomes `窶・`, an em dash at end of line even swallows the newline.
Source edits go through the editor; if a scripted rewrite is unavoidable, use
`[System.IO.File]::ReadAllText($f, [Text.Encoding]::UTF8)`. Damage is repaired
with `node tools/restore-nonascii.js <path>` (aligns each damaged line to its
HEAD version and takes HEAD's exact bytes), and `node tools/char-audit.js
<path>...` reports any remaining unexpected code points.

First full build compiles vendored OpenSSL + SQLCipher amalgamation + Tauri —
expect several minutes. Incremental afterwards is fast.

Known harmless noise: linking emits hundreds of `LNK4099: PDB 'ossl_static.pdb'
was not found` lines from the vendored OpenSSL objects (no debug info shipped).
Ignore; it does not affect the build. Grep test runs with e.g.
`Select-String -Pattern 'test result|error'` instead of reading raw tails.

### dav1d, the AVIF decoder (#52)

`crates/goon-core/Cargo.toml` turns on `image`'s `avif-native` feature, so an
AVIF cover stops being "stored verbatim, no thumbnail" and gets a real tile.
That matters more than it sounds: #46/#50 measured f95zone answering
`content-type: image/avif` for a URL that ends in `.jpg` (see §6), and a live
f95 cover is 59 478 bytes of AVIF. The booru CDNs measured in #52 all answered
`image/jpeg` (e621 and yande.re have **zero** AVIF originals today), so f95 is
where AVIF comes from - which is also why asking the source for a sized variant
could not fix this. `image` 0.25.10 splits AVIF into two features and only one
of them reads files:

- `avif = ["dep:ravif", "dep:rgb"]` — **encode only**: ravif writes AV1/AVIF, it
  cannot decode them. Enabling `avif` alone buys nothing at import time.
- `avif-native = ["dep:mp4parse", "dep:dav1d"]` — **the decoder**: mp4parse reads
  the ISOBMFF/AVIF container, dav1d (C, VideoLAN) decodes the AV1 frame.

So there is now a C library in the link, and it has to be the *static* one.
`goon_core::media::thumbnailable` is the single place that says what this build
can decode; keep it truthful. HEIF/HEIC is **still not decodable** — this path is
AV1-in-AVIF, HEVC-in-HEIF is a different container and codec, and dav1d has no
HEVC decoder — so `image/heif` must stay out of `thumbnailable` and the ingest
report must keep its "stored as-is, this type cannot be decoded" line.

**Why the environment variables exist.** `dav1d-sys` 0.8.3 contains no C source;
it is a `system-deps` binding whose `build.rs` probes `dav1d >= 1.3.0` with
pkg-config, and only falls back to cloning dav1d (its `build.rs` pins tag 1.5.0)
and building it with meson + ninja. Both routes fail here: the only `pkg-config`
on PATH is Strawberry Perl's (`C:\Strawberry\perl\bin\pkg-config.bat`, plus a
shebang-perl `pkg-config` beside it) — neither is a Win32 executable, so Rust's
`Command`/CreateProcess cannot exec it — and meson is not installed. So the
library is handed to the build explicitly, with exactly these five variables:

| Variable | What it does |
|---|---|
| `SYSTEM_DEPS_DAV1D_NO_PKG_CONFIG=1` | skips the pkg-config probe that cannot work here |
| `SYSTEM_DEPS_DAV1D_LIB=dav1d` | names the library to link (bare name, so `dav1d.lib`) |
| `SYSTEM_DEPS_DAV1D_SEARCH_NATIVE=<root>\lib` | folder holding `dav1d.lib` |
| `SYSTEM_DEPS_DAV1D_SEARCH_INCLUDE=<root>\include` | folder holding `include\dav1d\*.h` |
| `SYSTEM_DEPS_DAV1D_LINK=static` | forces a static link — import lib + DLL is not an option |

**Getting dav1d (pick one):**

1. `vcpkg install dav1d:x64-windows-static` (**static triplet, not the default
   `x64-windows`**), then copy `installed\x64-windows-static\include\dav1d` and
   `installed\x64-windows-static\lib\dav1d.lib` into
   `%LOCALAPPDATA%\goonflow-build\dav1d-x64-static`, keeping the `include\` +
   `lib\` layout. That is what this machine holds: dav1d 1.5.4, `lib\dav1d.lib`
   8 110 908 bytes.
2. Any prebuilt static dav1d `>= 1.3` — set `GOON_DAV1D_DIR` to its root (the
   folder with `include\` and `lib\` in it).

**What `tools\dev-env.ps1` does for you.** It resolves the dav1d root as
`GOON_DAV1D_DIR` when set, and otherwise uses
`%LOCALAPPDATA%\goonflow-build\dav1d-x64-static`; if `lib\dav1d.lib` **and**
`include\dav1d\dav1d.h` both exist it sets the five variables above from that
root (never overwriting one you exported yourself) and reports on its ready
line:

```
dev env ready: cl.exe=yes, perl=yes, nasm=yes, dav1d=static
```

When the two files are missing it does not go quiet: the ready line says
`dav1d=missing` and an `!!!` block prints where it looked and the three fixes
(vcpkg static copy, `GOON_DAV1D_DIR`, or the five variables by hand). It sets
nothing in that case, so the next `cargo build` dies with dav1d-sys' own probe
error — read the `!!!` block rather than re-deriving the cause from cargo.

**Static, or the artifact is wrong.** The shipping rule is one portable exe with
no DLLs beside it (§8), so a shared dav1d — the `x64-windows` triplet, or someone
dropping `dav1d.dll` next to the exe — is not acceptable even if it links and
runs. Prove it after every release build:

```powershell
dumpbin /dependents target\release\GoonFlow.exe
```

Expected: only Windows system DLLs. Measured with AVIF on (2026-11): **20
dependent DLLs, no `dav1d*.dll`**. A dav1d entry there means the build picked up
a shared library and the exe is no longer carryable.

**Measured cost** (2026-11, release build, so nobody re-measures it):

- `target\release\GoonFlow.exe` grows **+2 275 840 bytes**;
- `cargo tree -p goon-core`: **57 → 85 packages**;
- dav1d decode alone: **2.5 ms** for a 706 B / 360x240 AVIF, **11.6 ms** for a
  50 101 B / 1600x1067 one;
- the whole `put_thumbnail` path on those files: **~7-21 ms**, output ~**14 KB**.

**Licences for the new code.** DESIGN D5 keeps the project MIT; there is no
NOTICE file in the repo yet, and when one lands these are the lines it needs.
The crate licences were read off the vendored manifests under
`%USERPROFILE%\.cargo\registry\src\index.crates.io-*\<crate>\Cargo.toml`
(dav1d-sys-0.8.3, dav1d-0.11.1, mp4parse-0.17.0, av-data-0.4.4,
bitreader-0.3.11); dav1d's own BSD-2-Clause terms were read from the copyright
block in the installed `include\dav1d\version.h` rather than from the web:

| Component | Licence |
|---|---|
| dav1d 1.5.4 (C library, statically linked) | BSD-2-Clause |
| dav1d-sys 0.8.3, dav1d 0.11.1 (rust-av FFI bindings) | MIT |
| mp4parse 0.17.0 (ISOBMFF/AVIF container parser) | MPL-2.0 |
| av-data 0.4.4 (shared codec data types) | MIT |
| bitreader 0.3.11 | MIT OR Apache-2.0 |

MPL-2.0 is file-level copyleft, which is fine as long as mp4parse stays an
unmodified dependency — another reason not to vendor-patch it into `goon-core`.

### Frontend
```powershell
cd apps\desktop
npm install
npm install-scripts approve esbuild   # npm ≥ 12 blocks postinstall by default
npm run dev          # vite only (http://localhost:1420)
npm run tauri dev    # full app, hot reload
npm run build        # production bundle to dist/
npm run tauri build  # release exe + installers (see §8)
```

## 2. Repo layout

```
crates/
  vault/        crypto core: Argon2id KDF → XChaCha20-Poly1305-wrapped data key
                → SQLCipher PRAGMA key; nuke = shred keyfile first, then files
  locator/      flow.cfg pointer + volume-label resolution (drive-letter agnostic)
  goon-core/    domain model, SQLite schema (v8), media pipeline (thumbnail gen,
                sha256 dedup, BLOB storage)
apps/desktop/
  src/          Svelte 5 (runes) + Vite frontend
    views/      LibraryView (tab shell) + GamesView / MediaView (separate views)
    components/ TileGrid, ItemEditor, Viewer (#21 full-res viewer + slideshow),
                ActivityBar, ClipDialog, SourcesPanel, ConfirmDialog, ReportBar
    lib/        api.ts (command bindings), activity.svelte.ts (job polling store)
  src-tauri/
    src/
      lib.rs        AppState, vault lifecycle commands, all tauri commands
      jobs.rs       job registry: progress, cancellation, one job per kind
      queue.rs      #15: per-site rate lanes shared by every network fetch
      http.rs       #24: single reqwest client policy (vault proxy_url)
      extract.rs    #17: zip → launchable game folder (safe_join guards)
      mods.rs       #25: tool launch recipes (mtool/cheat/other, {game} arg)
      launcher.rs   #18: engine-aware game-launch resolution + spawn
      video.rs      #16: opt-in vault video storage (cap, job, first-frame)
      videothumb.rs #16: first-frame grabs via the Windows Shell thumbnail handler
      companion.rs  127.0.0.1 axum API: handshake + ping + real /api/clip (#19)
      ingest.rs     DESIGN §6: one clip pipeline — classify(url) → fetch_meta → store
      importer.rs   D13: game-folder scan, engine fingerprinting, image import
      bookmarks.rs  D13: Netscape bookmark parse + polite per-site enrichment
    capabilities/   Tauri 2 permission sets (core + dialog)
extensions/
  goonflow-clip/    #19 MV3 browser extension (manifest, background, options)
tools/
  dev-env.ps1       MSVC/toolchain env import (+ the static dav1d vars for #52)
  run-test.ps1      dev-env + cargo test --workspace, log + summary file
  run-clippy.ps1    dev-env + cargo clippy --workspace --all-targets
  run-release.ps1   dev-env + release build, writes size/sha256 evidence
  verify-artifact.ps1  runs the live checks against the built exe as a set
  smoke-release.ps1 launches target\release\GoonFlow.exe, checks the companion
                    handshake, closes it (run after every tauri build)
  close-exit-test.ps1 live check for #41: close exits, panic tap hides, a second
                    launch hands off to the running instance (sandboxed exe copy)
  window-probe.ps1  every top-level window of a running instance with size +
                    WS_EX styles — for "is there a phantom/hidden window?" bugs
  make-icon.ps1     regenerates src-tauri/icons
```

## 3. Data model cheat sheet (schema v8, `goon-core/src/db.rs`)

- `items` — one row per tracked thing. `kind`: game/gallery/post. `status`:
  pipeline stage (seen/tracking/wishlist/downloaded/played). `version` is a
  first-class column. **Completion is NOT a status** — it's a `user_status`
  tag (D9): exactly one per item, e.g. `complete` (green, priority 100).
  Media items never get it (D16 — enforced in the editor UI). `launch_path`
  (v4) is the optional per-game launcher override for #18.
  `match_suggestions` (v5) stores proposed online matches per item:
  `(item_id, site, url)` unique, title + confidence + found_at; rows cascade
  with the item.
- `sources` — many→one to items; `UNIQUE(site, url)`; `site='local'` +
  `file:///…` url for imported games, **local media bookmarks and `+ Video`
  entries**; raw snapshot column reserved for M3 selector recovery. Because of
  that unique key, `merge_items` moves sources with `UPDATE OR IGNORE` (a copy
  would collide with the keeper's own rows). `delete_source(item_id, row_id)`
  detaches one link from the item editor.
- `tags`/`tag_types`/`item_tags` — types 1 `user_status`, 2 `site_flag`,
  3 `content`. `add_item_tag` / `remove_item_tag` / `top_tags` are the M1
  read+write paths; `list_items(…, tag)` filters on them.
- `media` — thumbnails (`kind='thumb'`, one per item, replaced on update) and
  originals (`kind='full'`), content-hash deduped BLOBs. Keeping them in the
  DB keeps the wipe story to "shred one directory". Media dupe suggestions
  (`pairing_suggestions(conn, Some("media"))`) pair *different items sharing
  one stored hash* — titles are useless for `1743761874698456.jpg`.
- `site_settings` (v2) — per site: `enabled`, `cookie`, `rate_ms`. Read with
  `site_enabled(conn, site, default)` — the default differs per site (repack
  mirrors ship **off**, D8), so never assume `true`.
- `updates` (v2) — the change feed: `record_update` + `list_updates`;
  `update_targets` picks the game rows worth re-visiting (dlsite/f95 sources,
  non-DNF statuses).
- `mods` (v6, +`status`/`manifest` in v7) — per-game launch/deploy recipes
  (#25): `kind` mtool/cheat/dll/other, `path`, `args` (`{game}` placeholder),
  `status` linked/applied, `manifest` = deployed files as JSON (path + sha256).
  Cascades with the item.
- `pair_dismissals` (v8) — "Keep both" (#31): `a_id`/`b_id` + `dismissed_at`,
  stored canonically (smaller id first) so `dismiss_pair` is symmetric and
  `pairing_suggestions` can skip a pair the user already judged. Deleting/
  merging either item drops the row; the items themselves are never touched.
- **Game dupe suggestions** are `pairing_key(title)` equality (brackets and
  punctuation stripped, so `Summer Witch` matches `Summer Witch [v0.12] [renpy]`)
  with two guards from #47: `title_is_placeholder` skips titles that name the
  *tool* rather than the game (`mmnz-game` — what RPG Maker's `System.json`
  `gameTitle` and repack installers leave behind — plus `game`, `demo`, `sample`,
  `untitled`, `new folder`), and a bucket emits cross-site pairs before same-site
  ones and stops at 12 rows, so one shared title cannot produce N·(N-1)/2 noise.
  If you really do have more than a dozen games under one title, the rest are
  paired by hand from the item editor.
- **Migrations**: `init_schema` seeds at v1 then runs ordered, idempotent
  `if v < N` steps up to `SCHEMA_VERSION` (v8). v3 renames hosts-labeled sites
  from early field reports (`i` → `4chan`, `wimg` → `rule34`, `booru` →
  `allthefallen`); v4 adds `items.launch_path`; v5 `match_suggestions`; v6/v7
  the `mods` table and its deploy columns; v8 `pair_dismissals`. Any new schema
  change is a new numbered step — never edit the `CREATE TABLE` batch for
  existing columns.

## 4. Invariants (do not break without updating DESIGN.md)

1. **`PRAGMA key` before any query** on a SQLCipher connection (see
   `vault::open_db`); wrong key must surface as `InvalidPassphrase`, never as
   a silently-open empty DB. **Zeroize the PRAGMA string only after
   `execute_batch` returns** — wiping it first sends a degenerate key and
   silently disables real encryption (this exact bug happened during M0;
   `cargo run -p goon-vault --example sqlcipher_probe` is the fast check that
   the linked engine is genuinely SQLCipher and rejects wrong keys).
2. **Key hygiene**: `DataKey` is `Zeroize + ZeroizeOnDrop`; locking drops the
   `OpenedVault`, which closes the DB (WAL checkpoint) and wipes the key.
3. **Nuke order**: keyfile shredded first, then every file overwritten with
   random bytes and deleted, then pointer cleared. Never parallel with an open
   DB connection — close it first (Windows file locks).
4. **Companion API**: 127.0.0.1 only, random port, bearer token per session;
   `handshake` is the only unauthenticated route and returns no secrets.
5. **Portable state**: no writes outside the vault folder and `flow.cfg`
   beside the exe. No registry, no %APPDATA% (WebView2 profile lives in the
   vault dir — verify when M4 adds embedded tabs).
6. **Frontend types mirror Rust serde** (`rename_all = "camelCase"`) — keep
   `src/lib/api.ts` in sync with command structs.
7. Importers are **best-effort, user-correctable**: never overwrite user edits
   on re-import; re-imports key off `source_exists(site, url)`.
8. **Jobs, not modal blocking**: anything that can outlive a click (bookmark /
   game / image import, update check) runs as a job (`jobs.rs`) — the command
   grabs a `JobHandle` via `Jobs::start(kind, label)` (refuses a second job of
   the same kind), the work loop calls `set_progress(done, total, current)` and
   checks `job.cancelled()` **per item**, and the command calls `job.finish()`.
   Long loops take the vault `Mutex` **per item, never for the whole batch**, so
   the grid keeps answering while an import is mid-flight, and re-check
   `inner.opened` each iteration — Lock/Nuke closes the vault underneath a
   running job and the loop must stop, not panic. Lock / Nuke / forget-pointer
   call `jobs.cancel_all()` **before** touching the vault; Nuke also
   `jobs.clear()` because job history contains URLs and paths.
9. **Views stay separate**: GamesView and MediaView share only components where
   it genuinely helps (`TileGrid`, `ItemEditor`, which hides the pipeline fields
   for media). Pipeline filters (seen/tracking/…) and the update/dupe panels
   exist only in the Games view; site filters only in the Media view; the
   backend helpers take `kind` (`list_items(…, kind, site)`, `top_tags(…, kind)`,
   `counts_by_site(…)`) so each view's sidebar counts are always scoped.
10. **Site names come from `HOST_SUFFIXES`, never from a host label.** Early
    field reports got site "i" (from `i.4cdn.org`) and "wimg". Unknown hosts
    fall back to the *registrable domain*, and booru-CDN file links carrying a
    numeric query id (`?11353035`) route through the site API (`Mode::BooruId`)
    — pulling bytes from a CDN with the CDN as referer is what hotlink
    protection kills. `JobInfo.finished_at` exists so the ActivityBar can
    expire finished rows; don't remove it.
11. **One politeness ledger (`queue::SiteQueue`, #15).** Every network fetch —
    bookmark enrich, re-fetch, update checks, and the future M3 downloader —
    claims its slot with `state.queue.reserve(site, rate_ms, cancel).await`
    instead of tracking a private "last request per site" and sleeping inline.
    That guarantees spacing *across* concurrent jobs (two jobs hammering one
    booru was the old bug), keeps different sites progressing in parallel, and
    bails within ~100 ms of Stop. A cancelled wait leaves the lane advanced by
    at most one slot — harmless politeness, not rolled back. Lock/Nuke/
    forget-pointer also `queue.clear()`.
12. **Match suggestions are proposals, never actions (#14).** `match_suggestions`
    rows come from `ingest::find_matches`: DLsite full-text search is a POST to
    `www.dlsite.com/maniax/fs` (no public API; result anchors carry
    `product_id/RJ…` + `title="…"`), f95's XenForo `/search/search` is login-
    gated (403 without the saved cookie — say so, don't retry). Scoring is
    char-bigram Dice over bracket-stripped titles: floor 0.6, top 3 per game.
    Attaching = `add_source` + re-enrich, decided by the user in the Online
    matches panel. If live markup drifts, re-check with the ignored
    `match_live_search_smoke` test (`-- --ignored match_live`).
13. **Videos: link by default, BLOB only on explicit opt-in (D17/D18).** A
    stored video is one `media` row with kind='video' (hash-deduped), capped at
    512 MiB — enforced pre-download and again mid-stream. First frames come
    from the OS: `IShellItem.BindToHandler(BHID_ThumbnailHandler)` →
    `IThumbnailProvider.GetThumbnail` → `GetDIBits` into RGBA (the `windows`
    crate, gated `cfg(windows)`); never shell out to ffmpeg and never store
    raw frames. Shell handlers are COM — `first_frame` CoInitializes per call
    and tears down only what it initialized. A video item's thumbnail must
    never overwrite an existing site cover tile (only set when thumb is absent).
14. **Every networked path builds its client via `http::client_for(state, secs)`
    (#24).** The vault `meta` key `proxy_url` (plain KV — no schema bump) is
    read at client-build time, so a proxy change takes effect on the next
    job/command without a restart. Schemes accepted: http/https/socks4/
    socks5/socks5h (`set_proxy` validates before saving; the proxy travels
    with the vault, not the machine). reqwest's `socks` feature is what makes
    `socks5://127.0.0.1:9150` (Tor Browser) work. Never construct a bare
    `reqwest::Client` in a command path — that silently bypasses the proxy.
    `http::wiring_tests` proves the route with a local stub proxy (no network).
15. **Extraction (#17) writes only inside the destination folder.** Entry
    names are pre-screened (`safe_join`: no `..`, no root, no drive prefix)
    *before* any byte is written, cancel/partial runs delete the folder they
    created, and the archive itself is never modified. The extracted folder
    is attached as a `local` source; `local_source_path` prefers a
    folder/exe over a loose zip so the launcher sees the playable copy.
16. **One vault, one job ledger, one politeness ledger (#19).** The shareable
    `Core { inner, jobs, queue }` (Deref'd by `AppState`) is handed to both
    the tauri commands and the companion HTTP API, so a browser clip runs
    through the *exact* same `ingest::clip` path, the same single-vault lock,
    and the same site rate lanes as the `+ Clip URL` button. Never spin a
    second vault/job/queue inside a companion handler — take the shared `Core`
    clone. The companion token is per-run (rotates each launch); the
    extension's host_permissions are `127.0.0.1`/`localhost` only.
17. **Mods are thin launch recipes (D10, #25).** `mods.rs::recipe` only
    resolves what to *spawn* — the app never injects, wraps or patches a
    game, and never runs anything that isn't a user-registered `.exe` path.
    Kind semantics are fixed: `mtool` = tool exe + game exe as its one
    argument, `cheat` = start the tool (attaching stays the tool's job),
    `other` = stored args with `{game}` expansion. DLL deploy (`apply`) is
    the one file-writing path and it is guarded: entry names pre-screened
    (no escapes, no directory overwrites), originals moved to `*.flowbak`
    (never overwrite an existing flowbak), the manifest (paths + sha256)
    recorded on the row, mid-run failures roll back, `revert` restores, and
    an applied row can't be deleted until reverted. It lives in the same
    `mods` table (v7 columns) — no forked state.
18. **One lock path, one nuke path, armed before anything else (#23).** The
    panic hotkey handler calls `lock_core` — the exact Lock-button path
    (cancel jobs → clear lanes → drop conn+key → locked=true). The default
    key is registered right after the app is built, before vault/network code
    can fail; a stored custom key is added on unlock, validated by parsing
    before it is ever saved, and the default always stays armed. Hold ≥
    `PANIC_HOLD_MS` escalates via `nuke_core` — the same function the typed
    NUKE command uses, never a re-implementation — then scrubs and hides.
    Tap vs hold is a press/release state machine (`panic_hold` + `nuked`
    flags, OS auto-repeat must not fork a second episode). Escalation must
    *re-verify the physical key state* at the deadline (`GetAsyncKeyState`
    on the modifiers): global-hotkey synthesizes Windows Release events
    from a poll thread, so a dropped Release must never nuke by itself.
    Every episode bumps `panic_gen` and a watcher escalates only while it
    still owns the live generation — stale watchers cannot fire.
    `tools\panic-tap-test.ps1` / `panic-hold-test.ps1` / `panic-stale-test.ps1`
    prove the tap, hold and stale-watcher paths live against the release exe.
    The drill (`run_nuke_drill`) goes through `goon_vault::nuke` — the real
    path, not a mock — so a green drill means the real nuke works.
19. **Decoy chrome is exe-local plaintext that never holds a secret
    (#27).** `chrome.cfg` beside the exe = one sanitized line (control chars
    stripped, ≤64, re-checked on every read — `sanitize_chrome`). It may be
    read while locked (dressing the *locked* window is the point), so it must
    never gain anything but a display title. Renaming the exe/folder must
    keep working: everything resolves relative to `exe_dir`.
20. **A launch recipe may only use an interface the tool really has (#33).**
    Probe, don't assume: NW.js `MToolClient` builds ship `nw.dll` +
    `package.json` next to `MTool.exe` and have no `argv` handling, so passing
    the game as an argument is a promise that cannot be kept. The recipe layer
    is therefore *game-side launcher → (NW.js) open tool + clipboard hand-off →
    native builds get the argument*, and the note says which branch ran. The
    app never writes into a third-party tool's own state (`gameLib/*.gljson`)
    to fake a capability: reverse-engineered side files break on the tool's
    next update and hide the real workflow from the user.
21. **Never resolve identity from a hash you cannot verify (#39).** A
    file-link → post upgrade is accepted only when the site's answer carries
    the same md5 *and* the same byte size as what was downloaded. Anything
    less stays "stored as-is, no post could be identified", which the user can
    see and fix — a confidently wrong source link is worse than an admitted
    empty one.
22. **A capability that failed to register must be visible, not assumed
    (#41).** The panic hotkey is the app's safety path; measured in the field,
    a second instance silently failed to register it (`HotKey already
    registered`) while the user believed they were protected. So
    `register_panic_keys` records the failure in `AppState::panic_fault`,
    `panic_status`/`panic_info` report `armed` + `fault`, and the library top
    bar shows a red *panic key NOT armed* pill plus an explainer strip. Pair
    this with single-instance (`tauri-plugin-single-instance`, first plugin in
    the chain): a second launch **reveals** the running instance — including a
    window hidden by a panic tap, which is the only way back if you forgot the
    hotkey — instead of forking a second app that could never own the key.
    Closing the window does quit the process (verified live,
    `tools/close-exit-test.ps1`); there is no tray icon on purpose, because
    permanent chrome contradicts DESIGN §9 deniability.

## 5. Testing

```powershell
. tools\dev-env.ps1
cmd /c "cargo test --workspace > tools\cargo-test.log 2>&1"
```

- `vault` — keyfile roundtrip, wrong-passphrase rejection (AEAD + SQLite
  header checks), passphrase change keeps data key, nuke removes files.
- `locator` — pointer roundtrip + resolve against a real temp vault.
- `goon-core` — schema idempotency, media put/replace/dedup.
- `importer` — version/title parsing, engine fingerprints (Ren'Py, RPGM w/
  System.json title, non-game rejection).
- `bookmarks` — Netscape parser (entity decoding, including entity-escaped
  `file:///…` links), and a real-vault roundtrip proving local `file:///…mp4`
  / `.png` bookmarks and direct booru CDN links import as **media** (the M1
  field report said they landed in "unrecognized").
- `ingest` — classify() for every supported shape (post pages, `…/file.gif?267940`
  CDN links, `file:///C:/…`, UNC, mirrors), `local_path` Windows path handling,
  and the HTML parsers (e-hentai gate vs gallery, DLsite og:*, f95 og:title →
  title + version + engine tags). All parse-only: no network in tests.
- `jobs` — one job per kind, progress visible in the snapshot, `cancel` /
  `cancel_all` reach the work loop, finished list bounded.
- `bookmarks` (job path) — a pre-cancelled import stops early and reports
  `stopped: true`; the finished job's snapshot shows the full total.

When M3 site-packs land: save HTML/JSON **fixtures** per site and test
parsers against them (site redesigns must become failing tests, not silent
breakage). Network tests stay opt-in; the enrichment path is exercised
manually.

## 6. Adding things (patterns)

- **New command**: add to `lib.rs` (mirror the scoped `inner.opened` lock
  pattern — never hold the `MutexGuard` across `.await`), register in
  `generate_handler!`, mirror the type in `api.ts`.
- **New site support**: add it in `ingest::classify` (host → `SiteRef` with a
  `Mode`) and, if it has an API or scrapeable page, `ingest::fetch_meta`
  (return a `Meta`: title / store_bytes / thumb_bytes / tags / version / note).
  Also list it in `ingest::KNOWN_SITES` so the Sources panel can enable/disable
  it, set its cookie and its `rate_ms`. Bookmark import and "+ Clip URL" both
  go through this one path — that's deliberate, so the M4 extension needs no new
  logic. DESIGN §6 keeps the split: Rust ingestors for API-backed sites,
  declarative site-packs (M3) for selector-based ones.
- **Media policy**: images get stored in the vault (`put_full_image` +
  `put_thumbnail`); **videos stay links** (note on the item), because a 400 MB
  BLOB would make the one-action nuke slow and the DB unwieldy.
- **New site support, gated**: check `db::site_enabled(conn, site, !is_mirror_site(site))`
  before any fetch; disabled sites must produce a visible "site off" line in the
  report, never a silent skip.
- **Async commands + DB**: do all DB writes in sync scopes before/after the
  network phase (see `bookmarks::import_bookmarks` two-phase flow).
- **Long-running commands**: mark the tauri command `async fn` (runs off the
  main thread), start a job, pass `&JobHandle` into the importer, and
  `job.finish(note)` on both paths. Reports carry `stopped: bool` so the UI can
  say "stopped early — what came in is kept".
- **Progress: polling, not events**: the frontend (`lib/activity.svelte.ts`)
  polls `list_jobs()` — 800 ms while a job is live, ~4 s idle — instead of
  receiving tauri events. Fewer moving parts, and a late-loading webview still
  sees what happened. Job *state changes* fan out through `onActivityChange()`,
  which is what makes the grid re-read while items are still arriving.
  Buttons consult `jobState(kind)`: only a second job **of the same kind** is
  disabled — a running bookmark import must never grey out Lock, clip, or the
  game importers.
  **One writer per job.** `JobHandle::set_progress` overwrites `done`/`total`
  unconditionally, so a nested worker that reuses the parent's handle destroys the
  parent's counts (#49: batch re-enrich showed `0/1` forever because
  `ingest::reenrich_item` set progress per *source*). The job's owner is the only
  caller of `set_progress`; anything working *inside* the current item takes
  `job.child()`, which writes `sub_done`/`sub_total`/`sub_current` instead and
  shares the cancel flag. The owner's next `set_progress` clears the nested detail
  (it belonged to the previous item), and a nested `set_note` is ignored so a
  worker cannot rewrite the job's summary line.
- **Thumbnails**: always through `goon_core::media::put_thumbnail` (hash,
  dedup, size cap in one place).
- **Probe the site, then write the parser.** Every ingest bug so far came from
  assuming a page shape. Current measurements (2026-11, browser UA unless
  stated; `tools/` scripts + the issue bodies carry the transcripts):
  - `wimg.rule34.xxx` (and rule34's CDN generally) answers **403 to any
    non-browser User-Agent** — `TheGoonFlow/0.1 (…)` included. A browser UA gets
    the file; the `Referer` makes no difference there. See #50.
  - rule34's `dapi` (`?page=dapi&s=post…&id=`) answers a Cloudflare **CAPTCHA**
    page from a normal machine, so the md5/post lookups (#39) cannot depend on
    it — use the HTML post page and keep verifying md5+filesize (invariant 21).
  - rule34 CDN file URLs carry the **post id in the query**
    (`…/7783/<md5>.png?8886918`) — free identity to verify against.
  - **f95zone without a login cookie** returns the thread with a "register to …"
    wall and **zero** post images. With a saved cookie the page grows
    (~290 KB → ~449 KB) and the art appears, **lazily**: `<img
    class="bbImage lazyload" src="data:image/svg…" data-src=
    "https://attachments.f95zone.to/…">` — 20–26 of them. Never use `og:image`
    there, it is the site logo. See #46.
  - Those attachment URLs answer `content-type: image/avif` even when the path
    ends `.jpg` → sniff the bytes, don't trust the extension.
  - **e-hentai `/s/<token>/<gid>-<n>`** (slideshow page) is a ~4.7 KB JS shell:
    it contains no `gdtm` grid, no `og:image`, no `<img id="image">` — the art is
    injected by script. The gallery page `/g/<gid>/` is the one with the
    thumbnails, and the gid is already in the URL. See #46.
  - Cookies are stored and replayed verbatim as the `Cookie` header, and that
    works: a hand-pasted f95 cookie (`xf_session`, `xf_user`, `xf_csrf`,
    `cf_clearance`) unlocked the page above. When a source yields nothing, check
    the *parser* before blaming the cookie (#38, #46).

## 7. Release-ish notes for contributors

- MIT licensed (DESIGN D5).
- Keep `cargo test --workspace` green and `npm run build` warning-free
  (svelte a11y warnings count).
- Commit style: small, milestone-scoped (`M0: …`, `M1: gelbooru ingestor …`).
- Never commit vault material: `.gitignore` covers `flow.cfg`, vault dirs,
  `target/`, `node_modules/`, build logs under `tools/*.log`.

## 8. Portable release build

```powershell
. tools\dev-env.ps1
cd apps\desktop
npm run tauri build
```

Outputs:

- `target/release/GoonFlow.exe` — the all-in-one executable (frontend +
  everything statically linked; only requirement is the WebView2 runtime,
  preinstalled on Win 10/11). Copy it anywhere — the pointer file (`flow.cfg`)
  and any vault you create live beside it.
- `target/release/bundle/msi/` and `.../nsis/` — installers for people who
  want one (we recommend just carrying the exe for the portable story).

Release builds are slow (vendored OpenSSL + full optimization). Unsigned —
SmartScreen will warn; that's expected until there's a signing identity.
