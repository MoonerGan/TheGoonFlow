# The Goon Flow — Developer Guide

Milestone plan, architecture rationale, and the decision log all live in
[PLAN.md](../PLAN.md). This doc is the practical "how to build, where the code
is, what invariants matter" companion.

## 1. Prerequisites (Windows)

| Tool | Why | Install |
|---|---|---|
| Rust (MSVC toolchain) | everything native | `winget install Rustlang.Rustup` |
| VS 2022 Build Tools, C++ workload | linking + C builds (SQLCipher, OpenSSL) | `winget install Microsoft.VisualStudio.2022.BuildTools` + `--add Microsoft.VisualStudio.Workload.VCTools --includeRecommended` |
| Strawberry Perl + NASM | vendored OpenSSL build (SQLCipher backend) | `winget install StrawberryPerl.StrawberryPerl` `winget install NASM.NASM` |
| Node 18+ | frontend toolchain | `winget install OpenJS.NodeJS.LTS` |
| WebView2 runtime | Tauri webview | preinstalled on Win 11 |

### The dev-env wrapper
Plain `cargo` in a bare shell **fails for the C deps** (they need the MSVC
env, not just `cl` on PATH). Always:

```powershell
. tools\dev-env.ps1        # imports vcvars64 env + cargo/node/perl/nasm paths
cmd /c "cargo test --workspace > tools\cargo-test.log 2>&1"
```

`cmd /c "... > log 2>&1"` is deliberate: PowerShell 5.1 mangles native-command
stderr progress into `NativeCommandError` records — cargo works, but the log
capture gets noisy/abort-prone without the redirection wrapper.

First full build compiles vendored OpenSSL + SQLCipher amalgamation + Tauri —
expect several minutes. Incremental afterwards is fast.

Known harmless noise: linking emits hundreds of `LNK4099: PDB 'ossl_static.pdb'
was not found` lines from the vendored OpenSSL objects (no debug info shipped).
Ignore; it does not affect the build. Grep test runs with e.g.
`Select-String -Pattern 'test result|error'` instead of reading raw tails.

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
  goon-core/    domain model, SQLite schema (v2), media pipeline (thumbnail gen,
                sha256 dedup, BLOB storage)
apps/desktop/
  src/          Svelte 5 (runes) + Vite frontend
  src-tauri/
    src/
      lib.rs        AppState, vault lifecycle commands, all tauri commands
      companion.rs  127.0.0.1 axum API (handshake/ping/clip stub)
      ingest.rs     PLAN §6: one clip pipeline — classify(url) → fetch_meta → store
      importer.rs   D13: game-folder scan, engine fingerprinting, image import
      bookmarks.rs  D13: Netscape bookmark parse + polite per-site enrichment
    capabilities/   Tauri 2 permission sets (core + dialog)
tools/
  dev-env.ps1       MSVC/toolchain env import
  make-icon.ps1     regenerates src-tauri/icons
```

## 3. Data model cheat sheet (schema v2, `goon-core/src/db.rs`)

- `items` — one row per tracked thing. `kind`: game/gallery/post. `status`:
  pipeline stage (seen/tracking/wishlist/downloaded/played). `version` is a
  first-class column. **Completion is NOT a status** — it's a `user_status`
  tag (D9): exactly one per item, e.g. `complete` (green, priority 100).
- `sources` — many→one to items; `UNIQUE(site, url)`; `site='local'` +
  `file:///…` url for imported games **and local media bookmarks**; raw
  snapshot column reserved for M3 selector recovery. Because of that unique
  key, `merge_items` moves sources with `UPDATE OR IGNORE` (a copy would
  collide with the keeper's own rows).
- `tags`/`tag_types`/`item_tags` — types 1 `user_status`, 2 `site_flag`,
  3 `content`. `add_item_tag` / `remove_item_tag` / `top_tags` are the M1
  read+write paths; `list_items(…, tag)` filters on them.
- `media` — thumbnails (`kind='thumb'`, one per item, replaced on update) and
  originals (`kind='full'`), content-hash deduped BLOBs. Keeping them in the
  DB keeps the wipe story to "shred one directory".
- `site_settings` (v2) — per site: `enabled`, `cookie`, `rate_ms`. Read with
  `site_enabled(conn, site, default)` — the default differs per site (repack
  mirrors ship **off**, D8), so never assume `true`.
- `updates` (v2) — the change feed: `record_update` + `list_updates`;
  `update_targets` picks the game rows worth re-visiting (dlsite/f95 sources,
  non-DNF statuses).

## 4. Invariants (do not break without updating PLAN.md)

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
  logic. PLAN §6 keeps the split: Rust ingestors for API-backed sites,
  declarative site-packs (M3) for selector-based ones.
- **Media policy**: images get stored in the vault (`put_full_image` +
  `put_thumbnail`); **videos stay links** (note on the item), because a 400 MB
  BLOB would make the one-action nuke slow and the DB unwieldy.
- **New site support, gated**: check `db::site_enabled(conn, site, !is_mirror_site(site))`
  before any fetch; disabled sites must produce a visible "site off" line in the
  report, never a silent skip.
- **Async commands + DB**: do all DB writes in sync scopes before/after the
  network phase (see `bookmarks::import_bookmarks` two-phase flow).
- **Thumbnails**: always through `goon_core::media::put_thumbnail` (hash,
  dedup, size cap in one place).

## 7. Release-ish notes for contributors

- MIT licensed (PLAN D5). 
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
