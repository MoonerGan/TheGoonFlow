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

Expected: only Windows system DLLs. Measured on `m14` (2026-11-11, AVIF
on): **31 distinct dependent DLLs, every one a Windows system DLL, no
`dav1d*.dll`**. A dav1d entry there means the build picked up a shared library
and the exe is no longer carryable.

**Measured cost** (2026-11, release build, so nobody re-measures it):

- `target\release\GoonFlow.exe` grows **+2 273 792 bytes** (m13 → m14:
  21 108 224 → 23 382 016);
- `cargo tree -p goon-core`: **57 → 85 packages** (the toml/system-deps half is
  build-time only);
- dav1d decode alone: **2.5 ms** for a 706 B / 360x240 AVIF, **11.6 ms** for a
  50 101 B / 1600x1067 one;
- the whole `put_thumbnail` path on those files: **~7-21 ms**, output ~**14 KB**;
- what it bought, end to end: the live f95 probe's cover is 59 478 bytes of AVIF
  and now yields a thumbnail instead of a "cannot decode" note (TESTING §1).

**Licences for the new code.** DESIGN D5 keeps the project MIT; there is no
NOTICE file in the repo yet, so README's *Privacy & legal* section names dav1d
(BSD-2-Clause), dav1d-sys (MIT) and mp4parse (MPL-2.0) as the statically linked
parts. When a NOTICE lands, these are the lines it needs.
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

## 3. Data model cheat sheet (schema v10, `goon-core/src/db.rs`)

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
   entries**; the `raw` column exists in the table but **no code writes it**, so the
   selector-rot snapshot recovery is planned (M3), not available.
  that unique key, `merge_items` moves sources with `UPDATE OR IGNORE` (a copy
  would collide with the keeper's own rows). `delete_source(item_id, row_id)`
  detaches one link from the item editor.
- `tags`/`tag_types`/`item_tags` — types 1 `user_status`, 2 `site_flag`,
  3 `content`, 4 `extension` (v10, #76: what the file calls itself). A tag row
  is *(site, name)* with `ns` and `fold` derived from the name (v10): e-hentai's
  `female:big_breasts` and gelbooru's `big_breasts` are two rows whose `fold` is
  the same, which is what one bare token matches. `site = ''` means "the user
  typed it / provenance unknown" — the backfill never guesses a site.
  `tag_get_or_create_site` is the write path (`tag_get_or_create` is the `''`
  case); `add_item_tag` / `remove_item_tag` / `top_tags` are the read+write paths
  the UI uses.
- `media` — thumbnails (`kind='thumb'`, one per item, replaced on update) and
  originals (`kind='full'`, `kind='video'`), content-hash deduped BLOBs. Keeping
  them in the DB keeps the wipe story to "shred one directory". Media dupe suggestions
  (`pairing_suggestions(conn, Some("media"))`) pair *different items sharing
  one stored hash* — titles are useless for `1743761874698456.jpg`.
  v10 adds the **facts** the filter's meta predicates read: `bytes` (written by
  every writer, backfilled as `length(data)` — measured exact under SQLCipher,
  TESTING item 39), `duration_ms` (NULL until the facts probe measures it),
  `animated` (read from the container: gif frame count, PNG `acTL`, WebP
  `ANMF`) and `probed_at` — the marker that says a container has already been
  asked about this row, including when it answered nothing. A thumb row gets
  `bytes` but never `animated`: a re-encoded tile says nothing about the
  original. `db::refine_stored_facts(conn, limit)` is the bounded, resumable
  pass that fills the facts for rows written before v10; it is a **job** (stage
  S7), never part of the migration, because it reads every original once and
  unlock must not block (D15). Measured: 0.014 ms/row for a tiny file, 0.19 ms
  for a 200 KB one (the BLOB read dominates) — 1 000 rows in 185 ms.
- `sources` v10 fact columns — `bytes`, `duration_ms`, `width`, `height`,
  `probed_at`: the same facts for a file the vault does **not** hold (a
  `file:///` path stat'ed and cached, or a link measured by a ranged probe).
  Nothing writes them yet (stage S7); NULL means unprobed and never matches.
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
- `source_images` (v9, #53) — the pictures a source page **offered**, one row per
  url: `item_id`, `url` (unique per item), `found_at`, `stored_at`. They are
  candidates, never downloads: the fetch remembers urls, the user stores one at a
  time (`store_picture` sets `stored_at`). A site's art host gives out a
  per-request url (e-hentai's `keystamp`, f95's signed attachment link), so the
  **page or attachment link** is the thing worth keeping and re-resolving.
- **Game dupe suggestions** are `pairing_key(title)` equality (brackets and
  punctuation stripped, so `Summer Witch` matches `Summer Witch [v0.12] [renpy]`)
  with two guards from #47: `title_is_placeholder` skips titles that name the
  *tool* rather than the game (`mmnz-game` — what RPG Maker's `System.json`
  `gameTitle` and repack installers leave behind — plus `game`, `demo`, `sample`,
  `untitled`, `new folder`), and a bucket emits cross-site pairs before same-site
  ones and stops at 12 rows, so one shared title cannot produce N·(N-1)/2 noise.
  If you really do have more than a dozen games under one title, the rest are
  paired by hand from the item editor.
- **The query box's grammar** is `goon-core/src/query.rs` (#76 stage S2): a pure
  parser (`parse(text, Scope::Media|Games)` → `Query` of `Term`s, each keeping the
  exact text it came from, which is what the chip row shows) plus a planner
  (`Query::plan()` → `Plan { sql, params }` — one `WHERE` body of AND-ed
  fragments with `?` marks and the values for them, never a string-built query).
  Keywords are a **closed, code-owned set** (`KEYWORDS`), reserved whatever the
  value is, because that is the anti-import guarantee: a tag whose name really
  starts with a keyword is reached with `tag:"ext:something"`. Closed value sets
  are `type:` (video/image/gif), `has:` (thumb/tags), `stored:` (vault/disk/link)
  and `status:` (the five `model::Status` stages); `source:` and `ext:` take any
  slug, since their vocabulary is the vault's (invariant 10). Three things the
  SQL has to get right: timestamps are TEXT epoch seconds so date predicates
  compare `CAST(created_at AS INTEGER)` against an epoch the **parser** computed
  (`'999999999' > '1000000000'` is true as text); a bare `added:2026-11-01` is the
  UTC day, not one second; and every fact predicate reads `media.kind IN
  ('full','video')`, never a `thumb` row, so a 360 px tile is never what
  `width>1200` measured (invariant 27). `ingest::VIDEO_EXTS` is this module's
  `VIDEO_EXTS` — one list, pinned by a test, or `type:video` would miss what the
  importer accepted.
- **Migrations**: `init_schema` seeds at v1 then runs ordered, idempotent
  `if v < N` steps up to `SCHEMA_VERSION` (v10). v3 renames hosts-labeled sites
  from early field reports (`i` → `4chan`, `wimg` → `rule34`, `booru` →
  `allthefallen`); v4 adds `items.launch_path`; v5 `match_suggestions`; v6/v7
  the `mods` table and its deploy columns; v8 `pair_dismissals`; v9
  `source_images`; v10 (#76) rebuilds `tags` with provenance, adds the media and
  source fact columns (plus `media.probed_at`, the marker of which rows a
  container has answered for), seeds the `extension` tag type and creates the
  planner's indexes. v10 deliberately never reads a stored file: the facts that
  need the bytes come from `refine_stored_facts` as a background job (stage S7),
  so upgrading stays instant whatever the library size. The v10 tag rebuild is
  the one structural migration: it reuses every
  old tag id so `item_tags` never moves, and it runs with `PRAGMA foreign_keys`
  off for exactly the `DROP`/`RENAME` swap (a `DROP TABLE tags` would otherwise
  cascade every tag link away), followed by `PRAGMA foreign_key_check` to prove
  it did not. Any new schema change is a new numbered step — never edit the
  `CREATE TABLE` batch for existing columns.

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
5. **Portable state**: content stays in the vault folder; the only other write is
   `flow.cfg` beside the exe. No registry, no `%APPDATA%`. **The WebView2 profile is the
   exception** and it is not in the vault dir: tauri 2.12.5 defaults `data_directory` to
   `path().app_local_data_dir()` (`manager/webview.rs:561-569`), so this build writes
   `%LOCALAPPDATA%\<identifier>\EBWebView` - measured 75 MB / 326 files after a normal
   run, and the smoke test finds the window as `dev.goonflow.desktop-siw`. Moving it
   beside the exe is the open decision (#61); until then anything that claims otherwise
   in these docs is wrong.
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
    bookmark enrich, **clips** (#57), re-fetch, update checks, match search, the cookie
    probe, and the future M3 downloader -
    claims its slot with `state.queue.reserve(site, rate_ms, cancel).await`
    One slot covers a whole clip, not one request per clip (an f95 clip fetches its page
    plus up to three cover candidates) - the same compromise as enrich, and closing it
    means counting requests inside the fetchers.
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
    clone. The companion token is per-run (rotates each launch). Every request the
    extension *makes* goes to `127.0.0.1`/`localhost`; its `host_permissions` also
    name the three cookie hand-off hosts (`f95zone.to`, `e-hentai.org`,
    `exhentai.org`) so the popup can read that browser's own jar on one deliberate
    click — no code in the extension sends anything to those hosts.
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

23. **The release exe must carry its own UI — measured, not assumed.** tauri's own
    build script sets `dev = !custom_protocol` (tauri-2.12.5 `build.rs:253`), and a
    window built in `dev` mode loads `build.devUrl` (`http://localhost:1420`) instead
    of the embedded assets. A `cargo build --release` of an app that never declares the
    `custom-protocol` feature therefore starts, gets its window, answers the companion
    API and still reacts to the panic key, while the screen shows WebView2's "Hmmm...
    can't reach this page / ERR_CONNECTION_REFUSED" — it only looks alive while a vite
    dev server happens to listen. So `apps/desktop/src-tauri/Cargo.toml` declares
    `default = ["custom-protocol"]`; `tools\run-release.ps1` builds the frontend before
    cargo (cargo never runs tauri's `beforeBuildCommand`) and touches
    `apps\desktop\dist` so cargo re-runs its codegen and re-embeds those bytes; and
    `tools\webview-embedded-test.ps1` (= `verify-artifact.ps1` step 0) opens a WebView2
    DevTools port, fails the artifact unless the page url is `http://tauri.localhost/`
    with the app's title, and fails it again if the hashed asset names in
    `dist\index.html` are not inside the binary.

24. **A message about something that just happened is a toast; only state stays in
    the panel (#62 and #38, 2026-11-15 review).** The two bugs the report named were
    one structural mistake: a sentence sharing layout space with the thing it
    described. In the viewer the frame note was a flex sibling of the picture, so a
    stage showing a 6000 px frame spent its right third on the words "this build has
    no decoder for it" — and it showed for *every* picture, because that branch
    tested `shown`, the frame's metadata row, which is always present. On Sources the
    cookie verdict was a flex sibling of the paste box and its three buttons, so the
    box shrank to a stub and the text painted across Set/Check/Clear. Rules:

      * one-shot results (an image stored, a cookie forgotten, a merge, a scrub
        report, a refused launch, a frame this build cannot decode or a video too big
        for the player) go through `lib/toast.svelte.ts` — `info` / `warn` / `bad`,
        and `sticky()` for a result with numbers worth reading. They never live in a
        `*Msg` state var rendered as `<p class="report-inline">`.
      * persistent **state** stays inline and un-dismissed: a site switched off, the
        panic key not armed, a per-site cookie verdict, a value a form refuses
        (`ClipDialog`'s `error`, `ItemEditor`'s field validation). State must still
        be readable a minute later, a toast does not have to be.
      * `endVaultSession()` calls `clearToasts()` next to `clearMediaCache()`: a
        toast can name an item, a site or a path, so it dies with the session like
        every other decrypted string. Nothing here is persisted anywhere.
25. **A file request carries the referer of the origin that **names the file**,
    never the app's site key (#64, measured 2026-11-25).**
`store_video` used to send `REFERER: https://{site}` where `site` is
    the **key** the `sources` row stores (`rule34`), which is not a hostname. Replay
    with this app's own client (`http::live_probe`):

      ```text
      referer https://rule34                403 text/html 77 684 B  (Rule34.xxx CAPTCHA)
      referer https://ahri2mp4.rule34.xxx   200 video/mp4 59 083 373 B
      referer https://rule34.xxx            200 video/mp4 59 083 373 B
      referer (none)                        200 video/mp4 59 083 373 B
      ```

    rule34's hotlink check answers the site-key shape with `302 ->
    hl.rule34.xxx/public/hotlink.php`, and following that hop lands on a
    CAPTCHA page whose status is 403 - the `http 403 Forbidden` the report
    named, for a file that plays in a browser. Note this does **not** reverse
    item 10 for images (`wimg.rule34.xxx` answers `200 image/jpeg` to both the
    CDN and the site referer); hosts validate differently per route, so re-measure
    rather than generalise. Two rules follow: a download error must name the host
    that answered, the status, and where a redirect took the request (never a bare
`http {}`); and a `200` whose `Content-Type` is HTML is a web page, not a
    file, so it is refused instead of being stored as a video row.

26. **Two size ceilings, and the message always says which one it applied
    (#72, measured 2026-11-25).** There are two different budgets and they were
    conflated: `MAX_COVER_BYTES` (16 MiB) is the budget for **game cover art**
    scraped off a post page, and `media::MAX_MEDIA_BYTES` (96 MiB, deliberately
    equal to `INLINE_MAX` in `apps/desktop/src-tauri/src/lib.rs`) is the budget for
    **the artwork itself** - a booru original, a direct file link, a gallery page,
    a file the user pointed at. Clipping a rule34/hypnohub gif of 19 185 473 bytes
    hit the cover budget and was refused, so the item kept no bytes *and* no tile.
    Anything that stores media bytes takes the ceiling as an argument, chosen by
    what the bytes **are**, and the refusal states the size, the limit and the way
    out (`+ Images` has no such ceiling) in MB, never a bare byte count. A ceiling
    may never silently truncate. If #64's range delivery changes what the delivery
    path can carry, `INLINE_MAX` and `MAX_MEDIA_BYTES` move together — storing more
    than the viewer can be shown is not allowed.

27. **An unprobed fact is NULL and never matches (#76, schema v10).** Every
    computed media/source fact column — `media.bytes` / `duration_ms` /
    `animated`, `sources.bytes` / `duration_ms` / `width` / `height` — means
    *measured* when it holds a value and *never measured* when it is NULL, and
    NULL never satisfies a filter predicate (`size>10mb`, `length<1:30`,
    `type:gif` all miss an un-probed item). Nothing may invent a value to make a
    query return rows:
    `bytes` is written by every media writer and backfilled as `length(data)`
    (measured exact through SQLCipher, TESTING item 39); `animated` is read from
    the container's own chunks (gif frame count, PNG `acTL`, WebP `ANIM`/`ANMF`),
    never from the file name or from a thumbnail — a re-encoded tile says nothing
    about the original, so thumb rows keep `animated` NULL; durations and the
    dimensions of a link or a disk file exist only once the facts probe measured
    them, stamped in `sources.probed_at`. `media.probed_at` is the same marker
    for the vault's own rows — and the facts job's resume point, so a row whose
    container answered nothing is not read again. A merge or a re-store must
    carry the
    facts it already had — a row may never become un-probed. And the UI says
    *not measured* (`?`), never a pixel size, for the same reason (#65).

28. **The query string is session state, never persisted (#76).** What the user
    types into the filter box lives in the view's state for the length of the
    session and nowhere else: `goon-core::query` holds no connection and writes
    nothing, the `Plan` is rebuilt per request, and `endVaultSession` drops the
    view with the vault. It must not gain a home — not in `meta`, not in a
    settings file, not as a "last search" or a default filter — because a query
    is what the user was looking for while the vault was decrypted, and plan §5
    is explicit that nothing decrypted survives a lock. Preset chips are not
    saved state either: they insert grammar text (`-has:tags`, `stored:disk`)
    into the box, so what is active stays visible in the query. A real
    saved-search feature is a separate card with its own storage story, not a
    one-line addition here.

29. **A check that cannot fail is worse than no check** (found 2026-11-26 while
    reviewing #76). `tools/char-audit.js` read only its file arguments, so
    `node tools\char-audit.js` with no arguments audited **nothing** and printed
    `CLEAN`. Several commit messages, including ones written by this repo's
    reviewer, quoted that `CLEAN` as evidence. The tool now audits every
    git-tracked text file by default, reports how many files it examined, treats a
    BOM or U+FFFD as a finding on its own (the signature of a text round-trip
    through PowerShell), and exits non-zero when anything needs inspection. The
    general rule: when quoting a green check, know what the command actually
    looked at — a pass line with no coverage count is a red flag, and audit tools
    must state their coverage.

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

Frontend (Svelte 5 — there is no JS test runner in this repo):

```powershell
Set-Location apps\desktop
& "$env:ProgramFiles\nodejs\node.exe" ..\..\tools\view-zoom-check.js  # viewer zoom arithmetic
& "$env:ProgramFiles\nodejs\npm.cmd" run build                        # vite: types + bundle
& "$env:ProgramFiles\nodejs\npm.cmd" exec --yes svelte-check -- --threshold warning
```

- `tools\view-zoom-check.js` imports `apps/desktop/src/lib/view-zoom.ts` directly
  (node ≥23 strips the types), so the numbers the viewer prints are asserted rather
  than eyeballed. Anything in the UI that can produce `NaN` — a scale, a percentage,
  a byte count — belongs in a pure module with a check like this one: the reported
  `NaN%` came from `mediaEl` written without its `()`, i.e. dividing by a function
  reference (`undefined`).
- `svelte-check` is **not** a devDependency (nothing is vendored for the frontend), so
  it runs through `npm exec --yes`. It must print 0 errors **and** 0 warnings: that is
  what catches a `currentTarget` typed as `Element`, a possibly-null ref, and the
  leftover of a deleted `*Msg` variable.
- No browser is installed on this box, but **the release exe is one**: the WebView2
  runtime answers the DevTools protocol on a loopback port, so a real session can be
  measured. `tools\viewer-layout-probe.js` (run it through
  `tools\webview-layout-test.ps1`) starts a *copy* of the exe in
  `%TEMP%\goon-viewer-probe` with its own throwaway vault, imports generated PNGs of
  exact pixel size, clicks the real card and then the real **Fit** / **1:1** buttons, and
  prints the box the stage really got, what the app claimed, and what the engine drew.
  It is how the 2026-11-22 Fit / 1:1 report was found: the arithmetic test and the CSS
  contract were both green while the live app printed `0×0 px, shown at ?%` and disabled
  1:1. It asserts the app's own words — contained at Fit, 1:1 drawing the frame's own
  pixel count, the whole picture inside the stage's scroll area, and no `?` or `0×0` in
  the readout or the info line. The copy has no `flow.cfg`, so no real vault is ever
  attached, and like the other live tools it stops running instances first because the
  single-instance plugin would otherwise hand off.
- Generated fixtures pass for arithmetic the engine agrees with, which is not the same
  as the library. `tools\viewer-real-media-probe.js` is the probe for **the material
  users actually keep**: it clips the links in
  `tools\viewer-real-media-cases.json` (a booru file link, an animated gif, a
  cookie-gated gallery page, an mp4 it imports and then stores) and measures every
  item at two window sizes in a throwaway vault
  (`%TEMP%\goon-viewer-probe-real`). That cases file is **gitignored on purpose** —
  the links come out of someone's real library and this repository is public; copy
  `tools\viewer-real-media-cases.example.json` to it. Cookie-gated hosts read their
  cookie from `GOON_F95_COOKIE` / `GOON_EH_COOKIE`, which are never printed or
  committed. What the probe asserts: the info line
  describes the frame that is on screen (not the tile that stood in for it), that **Fit
  fills the box whatever the picture's size**, that 1:1 draws the frame's own pixels,
  that ± and 1:1 are usable for a video and a gif, and that nothing spills out of the
  stage at Fit. That is how the 2026-11-23 report was pinned: the generated PNGs were
  still green while a real 1200×850 jpeg was drawn at 77% of the box, a 176×256 gif at
  23%, and a stored clip was laid out past the bottom of the stage. `--reuse` re-measures
  the vault an earlier run filled.

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
  assuming a page shape. The measurements below are what the code depends on; the
  ones marked *re-measured* were re-checked on 2026-11-14 with **this app's own
  UA** (`TheGoonFlow/0.1 (personal library; user-initiated clip/import)`), because
  a marker that only holds under a browser UA is not a marker this app has. The
  ignored tests `ingest::tests::eh_live_clip_rehearsal`,
  `ingest::tests::eh_cookie_verdicts_live` and `http::live_probe` re-run them
  against the live sites.
  - **rule34 / gelbooru: the metadata api is the bot wall, the file host is not**
    (*re-measured*; this reverses what #50 first assumed). The danbooru api
    (`index.php?page=dapi…`) **and** the post html page answer `403 text/html`
    with `cf-mitigated: challenge` to the honest UA, to a browser UA and to no UA
    at all. The CDN answers that same post's file at `200 image/jpeg 678 634 B`
    to all three. So a link that **names a file** is classified
    `Mode::DirectMedia` and never sent to the api, even when a post id can be
    read out of the query (`…jpeg?3411909`) — the id is kept so the note can name
    the post it came from, and the note says plainly that the tags could not be
    read. A link that names no file is a post page, and that still goes to the
    api. **e621 is the exception**: its api and its file host both answer 200, so
    its post pages keep the api path and keep their tags.
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
  - **e-hentai's gallery page (2026) names no `<img>` for its previews at all**
    (*re-measured*): the cover is `<div id="gd1"><div style="width:250px;
    height:131px; background:transparent url(https://ehgt.org/…webp) 0 0
    no-repeat">` and the 20 grid tiles are boxes in **one** 4000x300 sheet on a
    per-gallery `*.hath.network` host, each written as
    `url(<sheet>) -200px 0 no-repeat`. So a preview is (sheet, x, y, w, h) and the
    only honest cover is a **crop** (`media::crop_tile`, which refuses a box that
    does not fit the sheet instead of clamping it). `gdtm` and `og:image` are gone
    from the page.
  - **`/g/<gid>/` without the gallery key is a bare apache 404 (236 B)**
    (*re-measured*) — that is exactly the `http 404 Not Found` #46 was filed with.
    The key is read **out of the page that links the gallery**
    (`eh_gallery_link(html, root)`), never guessed, and that is also how a `/s/`
    link finds its gallery.
  - **The `/s/<imgkey>/<gid>-<n>` slideshow page is worth fetching**
    (*re-measured*): `200`, 4 534 B, and it carries
    `<img id="img" src="https://<rand>.hath.network/h/<hash>-…-1200-630-jpg/keystamp=…;fileindex=…;xres=org/12349451_0.jpg">`
    — the **full art of that one page**, which answered `200 image/jpeg
    177 497 B` with our UA, referer or not, cookie or not. The host and the
    `keystamp` are handed out per request, so the *page link* is what gets
    remembered as a candidate and `store_picture` re-resolves the page for its
    bytes. (`api.e-hentai.org/api.php` with `{"method":"showpage",…}` was probed
    and answered a 32 B error — not needed.)
  - **Cookie probes are one page fetch on the polite lane**, and the marker must be
    a route that answers a guest and a member *differently*: f95's home page
    (`>Log in<` / `>Register<` = guest), e-hentai's `favorites.php` (guest `302 →
    bounce_login.php?b=d&bt=1-6`, member `200` 11 496 B, stable over three runs).
    exhentai has **no honest marker for this client**: `favorites.php` gives the
    `302 → forums remoteapi` hop to a guest and `200` with **0 bytes** to a real
    cookie, a made-up `ipb_pass_hash` and a real member id with that bad hash
    alike — so its verdict is `Live::Inconclusive`, and the UI shows "the site
    could not tell" instead of a green light. A route that answers the same thing
    to everybody is worse than none: `https://e-hentai.org/g/` is a 404 for guest
    and member both, which is how a good cookie got accused of being stale.
  - Cookies are stored and replayed verbatim as the `Cookie` header, and that
    works: a hand-pasted f95 cookie (`xf_session`, `xf_user`, `xf_csrf`,
    `cf_clearance`) unlocked the page above. When a source yields nothing, check
    the *parser* before blaming the cookie (#38, #46).

- **Never put a sentence in the same flex line as the controls it describes.** A long
  text node has an enormous max-content width, so it wins the space argument: the
  items that are allowed to shrink (`min-width: 0`) collapse to a stub and the text
  overlaps whatever is left. That is exactly how #38's "tiny text box, buttons
  vertical, text over the buttons" looked after the first `flex-wrap: wrap` attempt —
  wrapping only decided *how* the wreckage was stacked. `app.css` now gives the
  Sources site row real tracks: `.site-row` is a grid
  (`190px max-content max-content`), `.site-cookie` and `.cookie-state` each take
  `grid-column: 1 / -1`, the paste input has `min-width: 16rem` (10 rem under 900 px
  wide), and the buttons are `flex: 0 0 auto; white-space: nowrap`. The verdict is
  also the box's `title`, so it can be read as a tooltip instead of a row. Same rule
  in the viewer: `.viewer-note` is `position: absolute` over the stage, so no caption
  buys its width out of the picture.
- **Zoom and other on-screen arithmetic is a pure module.** `lib/view-zoom.ts` owns
  fit / scale / ceiling / percent and `tools\view-zoom-check.js` asserts it, because a
  component cannot be tested. Model (rewritten 2026-11-22, measured by
  `tools\viewer-layout-probe.js`; fit re-measured 2026-11-23 by
  `tools\viewer-real-media-probe.js`): `scale` is *screen pixels per picture pixel*, so
  **1:1 is scale 1 whatever the picture is** and the readout is `round(scale × 100)`;
  `fit = min(box.w/natural.w, box.h/natural.h)` is only the scale the **Fit** button
  picks, and picking it is a mode (`fitMode`) that any manual zoom leaves.
  - **Fit fills the stage in whichever direction that needs: it shrinks a frame bigger
    than the window and enlarges one smaller than it.** The `min(..., 1)` cap was the
    2026-11-23 bug: a 1200×850 jpeg in a 1888×1105 stage was laid out at 1200×850
    (77% of the box) and a 176×256 gif at 176×256 (23%), with Fit and 1:1 both lit and
    pressing Fit doing nothing — which is most of an imageboard library. Fit is still
    bounded by the box, so it can never lay out more pixels than the window has; the
    bar prints what it used (`278%`) and says `Fit is enlarging it to fill the window`.
    `fitIsOneToOne` is the only case where Fit and 1:1 coincide, and it needs the stage
    to be exactly the picture's own size.
  - **The frame's own pixels come from the thing that is actually on screen.** For a
    still that is the decoded `ImageBitmap`; for a gif the `<img>`'s `naturalWidth`; for
    a video the `<video>`'s `loadedmetadata` (`videoWidth`/`videoHeight`). A stand-in
    tile may not claim a size for a media kind it is not: measured live, the tile that
    stood in for a 25 MB clip set `natural` to 360×268 and nothing took it back, so the
    bar described a 1280×954 picture as `360×268 px, shown at 100%` while Fit and 1:1
    had nothing to measure against. `resetView()` clears the size on every navigation.
  - `box` is measured from the **stage's border box less its padding**, never from the
    media box. The media box used to be a flex item sized by its own content, so a
    zoomed picture raised the stage's scrollbars, which shrank the box, which changed
    the `fit` the 1:1 button had just used: measured 1:1 on a 2400 px frame drew
    2367 px while the bar read 99%.
  - `scale` is applied as the media's own `width`/`height` in CSS px, **not** as
    `transform: scale()`. A centred transform puts part of a zoomed picture outside the
    scroll container, where it cannot be scrolled to (measured: scroll width 1868 for a
    picture drawn 2367). `.frame-box` is pinned to the stage
    (`position: absolute; inset: 1rem`, `inset: 0` in fullscreen) and is a grid; the
    media takes `margin: auto`, which centres it while it fits and start-aligns it when
    it does not, so the whole frame stays reachable. The grid's single track must be a
    **definite** size (`grid-template-rows/columns: 100%`): with `auto` tracks,
    `max-height: 100%` on the child resolves against a track sized by that same child,
    so the cap is vacuous — measured 2026-11-23, a stored 1280×954 clip was laid out
    1280×954 inside a box that had 712 px of height, and Fit could not touch it.
  - `scaleCeiling` caps zoom *past* 1:1 at `MAX_DRAWN_PX` (the WebView2 texture limit),
    but 1:1 itself is never clamped away — a button that silently refuses to reach 1:1
    is worse than one that stops early.
  - Controls: **Fit** (`0`) and **1:1** (`a`) carry `class:active` / `aria-pressed`, the
    ± buttons are disabled until the frame's own pixel size is known, and the
    percentage is a separate non-clickable readout — never the label of a button. A
    size that is not known prints `?` and says so on hover; it must never print
    `0×0 px` or a fabricated 100%. (The size was read *after*
    `transferFromImageBitmap` for a while, and a detached `ImageBitmap` reports `0×0`:
    that is what made 1:1 disabled itself, the bar read `zoom 100%` for a picture drawn
    at 45%, and the info line said `0×0 px, shown at ?%`.)

## 7. Release-ish notes for contributors

- MIT licensed (DESIGN D5).
- Keep `cargo test --workspace` green (via `tools\run-test.ps1`), clippy clean of
  warnings (`tools\run-clippy.ps1` must report **0**; if you add a warning on
  purpose, `#[allow(...)]` with a comment saying why, exactly as
  `list_items` in `src-tauri/src/lib.rs` does for the frontend wire shape), and
  `npm run build` warning-free (svelte a11y warnings count).
- Commit style: small, milestone-scoped (`M0: …`, `M1: gelbooru ingestor …`).
- Never commit vault material: `.gitignore` covers `flow.cfg`, vault dirs,
  `target/`, `node_modules/`, build logs under `tools/*.log`.

## 8. Portable release build

```powershell
. tools\dev-env.ps1
cd apps\desktop
npm run tauri build
```

or the way artifacts are actually cut and signed off here:

```powershell
powershell -ExecutionPolicy Bypass -File tools\run-release.ps1 -tag m20
powershell -ExecutionPolicy Bypass -File tools\verify-artifact.ps1
```

`run-release.ps1` runs the frontend build first, because `cargo build --release` never runs
tauri's `beforeBuildCommand`: without that step the exe embeds whatever `apps\desktop\dist`
happened to hold. It then touches `dist` so cargo re-runs tauri's codegen — the build script
watches the dist **directory** only (tauri-build-2.7.4 `codegen/context.rs:96`), not the files
inside it — and `verify-artifact.ps1` step 0 proves the result: the window must come from
`http://tauri.localhost/` and the binary must contain the hashed asset names that
`dist\index.html` references right now. Plain `cargo build --release` is fine too, but only
with `custom-protocol` on (invariant 23 in §4) — otherwise the exe loads the dev URL and shows
WebView2's "can't reach this page".

Outputs:

- `target/release/GoonFlow.exe` — the all-in-one executable (frontend +
  everything statically linked; only requirement is the WebView2 runtime,
  preinstalled on Win 10/11). Copy it anywhere — the pointer file (`flow.cfg`)
  and any vault you create live beside it.
- `target/release/bundle/msi/` and `.../nsis/` — installers for people who
  want one (we recommend just carrying the exe for the portable story).

Release builds are slow (vendored OpenSSL + full optimization). Unsigned —
SmartScreen will warn; that's expected until there's a signing identity.
