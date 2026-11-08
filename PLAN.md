# The Goon Flow — Project Plan

> Living design document. Last updated: 2026-11-07. Status: **planned, not yet scaffolded**.
> Next action: get user approval → scaffold M0 in this folder.

---

## 1. What & why

A personal, local-first wrapper for tracking, storing, and managing adult game browsing
(f95zone.to, otomi-games.com, kimochi.info, ryuugames.com — most games originate on DLsite.com)
and imageboard browsing (e-hentai.org, rule34.xxx, gelbooru.com).

**Hard requirements (from user):**
- Thumbnails / rich library UI
- Wipe-clean capability (one-shot nuke)
- Anonymity (no cloud, no telemetry, deniability)
- Open source (**MIT**)
- Lightweight Windows desktop app + browser integration (both extension AND embedded tabs)

**Verdict from research:** no existing OSS tool covers games + boorus + wipe/anonymity in one.
Closest pieces worth stealing ideas from (see §3). Build custom.

---

## 2. Decisions log

| # | Decision | Choice | Notes |
|---|---|---|---|
| D1 | Custom vs adapt existing | **Custom** | Nothing unifies game sites + boorus + crypto-wipe. |
| D2 | Browser integration | **Both** | MV3 extension for daily browser + embedded WebView2 tabs in-app. |
| D3 | Scope | **Track + download** | Metadata/thumbnails AND manage downloads (DLsite, booru full-res, mirrors). |
| D4 | Stack | **Tauri 2 + Rust backend, Svelte 5 frontend** | Tiny, portable-folder-friendly, SQLCipher/age in Rust. |
| D5 | License | **MIT** | Least conspicuous code license. |
| D6 | Data location | **User-configured vault root** | Default `goonflow-vault\` beside exe; may be USB or any drive. App dir keeps only a plaintext pointer file (volume label + relative path). Resolve by volume label to survive drive-letter changes; re-prompt on failure. |
| D7 | Sources | **User-configurable registry + declarative site-packs** | Enable/disable per source; credentials & rate limits per source; new sites easy (§6). |
| D8 | Default sources ON | gelbooru, rule34, e621, e-hentai, dlsite, f95zone | **OFF by default:** otomi, kimochi, ryuugames (repack mirrors). |
| D9 | "Complete" | **Tag type, not pipeline status** | Personal "I completed this game" tag: high priority, green. Site/dev flags (f95 `[Completed]` prefix, DLsite "finished") are a *separate* tag type. Tag types carry display attributes (§5). |
| D10 | Modding tools | **Thin launch recipes + Mod manifests** | MTool/Cheat Engine integration without reinventing them; DLL mod apply/revert with update-clobber warning (§7). |
| D11 | Game appdata writes | **Never intercept; experimental "artifact radar"** | Enumeration only: engine-aware artifact discovery, save backup into vault, nuke-wizard checklist. Opt-in, per-game opt-out. Feature deferred down the line (§8). |
| D12 | Plugin strategy | **No dynamic plugin DLLs** | In-repo Rust modules + site-pack configs (TOML) generate both app extractors and extension content scripts. |
| D13 | Import existing media | **Part of M0** | (a) Games: pick a folder → per-engine fingerprinting (RPGM `System.json`/`Game.ini`, Unity `*_Data`/`boot.config`, NW `package.json`, Ren'Py layout, GameMaker `data.win`), version parsed from `[v1.23]`-style folder names, cover art extracted, local sources tracked as `site='local'`. (b) Images: batch file import → one Gallery item, originals as encrypted `full` BLOBs + thumbnail. (c) Bookmarks: Netscape HTML (browser export) parsed, host-classified, items+sources created; optional online enrichment fetches thumbnails+tags for gelbooru/rule34/e621 JSON APIs and DLsite `og:image` (rate-limited, user-initiated). All best-effort; every field correctable in the item editor. |
| D14 | View separation | **Games and Media are separate views; share only what fits** | Field report: "what sense does it make that I can filter by seen/tracking/wishlist for an image?" Games own the pipeline (status filters, version, updates, dupes); Media owns provenance (source-site + tag filters). Shared components strictly as-needed: `TileGrid`, `ItemEditor` (hides pipeline fields for media), Clip/source/nuke logic. |
| D15 | Long work | **Jobs, never modal blocking** | Field report: a slow bookmark import greyed out every unrelated button. Imports/update checks run as observable jobs (progress, current item, Stop); vault lock taken per item so the grid fills live; Lock/Nuke cancels all jobs first. UI polls `list_jobs()` rather than tauri events (fewer moving parts, late webview still sees history). |
| D16 | Completion semantics | **"Complete" is a game-only concept** | Field report round 2: "Images / video get a 'Complete' check mark which has nothing to do with them." The editor renders the Complete marker (and status/circle/version pipeline fields) only for `kind=game`; media items never expose it. Extends D9. |
| D17 | Video storage | **Track by link; vault-stored video is opt-in later** | Field report: "+ Images" silently dropped .mp4 picks. Added `+ Video`: each picked video becomes a media item tracked by `file:///` path (no vault BLOB — nuke story + DB size). Copying videos *into* the encrypted vault + first-frame thumbs is #16 (M3). |
| D18 | Video first-frame decoder | **Windows Shell thumbnail provider (Media Foundation)** — no FFmpeg, no pure-Rust gamble | #16's "ffmpeg vs pure-Rust" question, answered a third way: the OS already decodes everything this library meets (H.264/HEVC MP4, VP8/VP9 WebM via `IShellItem → BHID_ThumbnailHandler → GetThumbnail → GetDIBits`). Zero bundled bits, works offline, and Nuke/backup behavior is unchanged (thumb lands in the DB like any other). Missing codec → no thumbnail, item still stores/plays; verified live on an h264 mp4 (360x203 frame). Vault video storage stays opt-in: `media(kind='video')` BLOB, 512 MiB cap, size warning dialog, Remove restores link-only. |

---

## 3. Existing-solution research (2026-11)

**Adult game managers**
- **Playnite** (MIT, C#) + erri120/Playnite.Extensions — DLsite + F95zone + Fanza metadata providers. Closest existing game half.
- **ToolwLib** (OSS, 2026) / XLibrary / F95 Game Manager — f95-centric library + update tracker + launcher.
- **dlsite-manager** (AcrylicShrimp, OSS) — DLsite purchase sync, downloads, tags.
- **F95API / YAM / f95-game-updater** (OSS) — f95 scrapers/updaters, no library UI.

**Media organizers**
- **Stash** (AGPL, Go) — the reference for thumbnails/tags/scrapers over local files; not a site tracker; no games; no wipe story.
- porn-vault, YABS — stale Stash-alikes.

**Booru / e-hentai clients**
- **StreamBooru**, Booru Studio — multi-booru browse + bulk download (Danbooru-family APIs are clean).
- **JHenTai** (Flutter), **E-Viewer** (GPL) — e-hentai/exhentai clients w/ downloads.
- **gallery-dl** (OSS) — universal downloader reference for all booru/EH sources.

**Conclusion:** Stash ≈ 80% of the media half, Playnite ≈ 70% of the game half, neither the
cross-site tracker nor the anonymity/wipe requirements. Custom build justified. Borrow:
Stash's scraper-YAML concept (→ site-packs), Playnite's portable model, f95 version-parsing tools.

---

## 4. Architecture overview

Cargo workspace monorepo:

```
TheGoonFlow/
├── apps/
│   ├── desktop/          # Tauri 2 app: Rust core + Svelte 5 UI
│   └── extension/        # MV3 extension (Chrome + Firefox from one TS codebase)
├── crates/
│   ├── goon-core/        # domain model, SQLite schema, matching/dedup, tag system
│   ├── vault/            # SQLCipher, key hierarchy (Argon2id), shred/crypto-erase
│   ├── ingest/           # Ingestor trait + modules: booru, ehentai, dlsite, f95, repacks
│   ├── sitepack/         # declarative site-pack parser (used by app + extension codegen)
│   └── downloader/       # resumable jobs, mirror picker, archive extract (zip/7z/.001)
└── docs/
```

- **Local-first.** Single process owns DB, thumbnail cache, downloads. No cloud, no telemetry, works offline.
- **Companion API:** app listens on `127.0.0.1` only, random port, per-session bearer token, strict CORS (extension ID). Extension talks to it; embedded webviews use IPC directly.

### 4.1 Vault & portability
- One folder = everything: DB (SQLCipher) + WAL, thumbnails (DB BLOBs), encrypted download containers, WebView2 profile, logs.
- Vault root chosen on first run (default beside exe; browse to USB/partition).
- App dir holds only `flow.cfg` pointer: volume label + relative path (no sensitive data). Locked/absent vault → neutral "insert drive / pick vault" prompt (deniability: nothing revealed).
- Setup allows renaming exe/folder; locked state renders as a boring utility window.

### 4.2 Encryption & nuke
- Master key: Argon2id(passphrase) → wraps random data key (key hierarchy so passphrase changes don't re-encrypt everything).
- DB via SQLCipher. Downloaded content in XChaCha20-Poly1305 chunked containers (plain-folder mode possible with shred fallback — decide at M3 by perf).
- **Nuke:** global hotkey — tap = instant lock; hold 3 s = crypto-erase: destroy key material → overwrite DB+WAL+keyfile+webview profile → push wipe to extension storage. <2 s, no admin rights.
- Scrubber: best-effort sweep of Windows artifacts (Jump Lists, Recent, Prefetch for tracked game exes) + docs checklist.

---

## 5. Data model

- `items` — ULID, kind (`Game` | `Gallery` | `Post`), normalized + original title, circle/dev, pipeline `status` (`Seen → Tracking → Wishlist → Downloaded → Played`), DNF flag, notes, rating, created/updated.
  - Note: "Complete" is **not** here — it's a tag (D9). A completed game still re-surfaces on updates.
- `sources` — many→one with items. Per-site identity: url, source id, fetched_at, **raw JSON/HTML snapshot** (selector-rot recovery), dead-mirror flag.
  - Auto-pairing across sites (DLsite origin ↔ f95 thread ↔ repack mirrors) via circle+title fuzzy match; manual merge/unmerge UI.
- `versions` — parsed version strings (f95 `[v0.12]` titles, DLsite updated_date), changelog watches → "updated" feed.
- `tags` + `tag_types` — tag kinds with display attributes:
  - `user_status` — personal state: **Complete (green, high priority)**, Wishlist, DNF, Replay… shown as colored chips on cards, sorted first.
  - `site_flag` — developer/site facts imported from sources (f95 `[Completed]` prefix, engine tags like Ren'Py/RPGM/Unity).
  - `content` — normal tags; `tag_aliases` translate booru tags ↔ DLsite genres ↔ f95 prefixes into one facet. Also serves as the blocklist mechanism.
- `media` — thumbnails as encrypted BLOBs in DB, content-hash keyed (dedup + trivial wipe). Previews/local paths for downloaded galleries.
- `downloads` — job queue: mirrors, resume state, extraction, final path linkback.
- `tools`, `mods`, `attachments` — see §7.
- `site_settings` — per-source enable/credentials/rate limit/proxy override.

---

## 6. Ingest layer & site-packs

`trait Ingestor { identify(url) -> SiteRef; fetch(url); search(q, page); }` — reqwest with per-source rate limiters, referer headers (hotlinked thumbnails), global + per-source proxy (HTTP/SOCKS5 → Tor-able).

**As built (M1/M2):** `apps/desktop/src-tauri/src/ingest.rs` — `classify(url) -> SiteRef{site, kind, remote_id, Mode}` + `fetch_meta(...) -> Meta{title, store_bytes, thumb_bytes, tags, version, note}` + `clip()` (dedup on `sources(site,url)`; the link is stored even when the fetch fails, with the reason as a note). Trait objects deferred until site-packs make the shape real. Policy: images are stored in the vault, **videos stay links**; `file:///…` URLs and direct CDN file URLs (`…/file.gif?267940`) classify as media (`local` / host slug), which is also how bookmark imports resolve them. Per-site enable/cookie/rate come from `site_settings`.

**First-class modules (defaults per D8):**
- **booru family:** gelbooru.com public API, rule34.xxx danbooru-style API, e621 (UA required). Clean JSON — ship first (M1). e926 filter wiring available via tag blocklist.
- **e-hentai:** API3 + APIDB cookie auth; hard 10 s rate limit; 114/124 gate pages → manual pass-through UI.
- **dlsite:** product page metadata (no login), library sync w/ account cookie, direct downloads.
- **f95zone:** XenForo HTML scrape (thread prefixes → `site_flag` tags incl. `[Completed]` and engine), version parse; account cookie unlocks two-way tracker sync; downloads via seed hosts (buzzheavier, pixeldrain…) → mirror picker.
- **repacks (off by default):** otomi/kimochi/ryuugames as DLsite mirrors; dead-mirror tolerance.

**Site-packs (D12):** TOML/JSON declaring engine (`booru-family`, `xenforo`, `dlsite-mirror`) + endpoint templates + CSS selectors. A new booru = ~6-line pack. Packs drive app extractors **and** code-gen the extension content scripts — one definition, both surfaces. In-repo Rust modules for anything deeper.

**Resilience:** extractor tests run against saved HTML fixtures → site redesigns = failing tests, not silent breakage.

---

## 7. Games as software: launchers, tools, mods

- Launch actions per game: open folder, launch exe, engine-aware hints (JoiPlay etc. out of scope, user's own launcher still works).
- **`tools`** — registered executables w/ launch templates:
  - **MTool:** per-game "Launch with MTool" (MTool opened with game exe — exactly today's workflow, no added friction); optional `.megamix` file-association install step. Thin recipes, not reimplementations.
  - **Cheat Engine:** attach/launch action.
- **`mods`** — DLL/patch mods from f95 threads or local zips. Apply = deploy + back up originals `*.flowbak` + record manifest/hashes. Revert = restore.
  - **Update-clobber guard:** version watcher detects new build → warn "update wipes Mods" → one-click revert → update → re-apply.
- **`attachments`** — misc community files (patches, saves, OSTs) on items: vault-stored w/ source URL, open/extract-in-place.

---

## 8. Artifact radar (experimental, deferred — D11)

**Never intercept** game writes (conflicts with MTool injection, engines unpredictable, kernel territory kills portability).

Enumeration only:
1. Per-engine artifact patterns (Ren'Py → `%APPDATA%\RenPy\*`, Unity → `AppData\LocalLow\<co>\<game>`, RPGM → game folder…) + user-declared watched paths.
2. Scan indexes found artifacts as item metadata.
3. Opt-in payoffs: save backup into vault (tiny); nuke/scrubber wizard shows artifact checklist grouped by game, per-game opt-out for keeping saves.
Ships behind an experimental flag; never auto-deletes. Target: post-M5.

---

## 9. UI (Svelte 5)

- Virtualized thumbnail grid + facets: source, status, tag types (Complete chip green/high-priority), artist/circle, date.
- **Flow view** — unified recent feed across all sources.
- Game detail: sources/mirrors list w/ pairing UI, version history + changelog, mods/tools, attachments, download state.
- Booru post viewer; download queue; Settings: vault, sources, proxy, tags (types/colors), wipe + hotkeys.
- Locked state: passphrase unlock; neutral-looking window chrome.

---

## 10. Milestones

| # | Deliverable | Acceptance |
|---|---|---|
| **M0** ✅ | Tauri 2 + Svelte 5 + Cargo scaffold; portable vault root + `flow.cfg` pointer + volume-label resolution; SQLCipher vault + unlock screen; companion API skeleton; **importers (D13): game-folder scan w/ engine fingerprinting, image batch import, bookmark HTML import w/ optional online enrichment; library grid with thumbnail display + item editor** | App runs from USB, encrypted shell works, existing media lands in the flow |
| **M1** ✅ | Booru ingestors (gelbooru, rule34, e621) + URL clip + thumbnail BLOB cache + tag grid + tag types w/ Complete chip. Also landed here: one shared ingest path (`ingest.rs`) covering `file:///…` and direct media URLs, so bookmarks to local media resolve as media (field report) | Usable daily |
| **M2** ✅ | DLsite + f95zone ingest; cross-site pairing (suggest, never auto-merge) + merge UI; versions/update feed + "check updates"; repack mirrors (off default) + per-site enable/cookie/rate under *Sources*; e-hentai gate pass-through. **Not in this pass:** e-hentai cookie-gated *enrichment* is wired but untested against the live site, and `/api/clip` still returns 501 (needs `Arc<AppState>` in the companion — M4) | Game half works |
| **M2.5** ✅ | Field-test follow-up (D14/D15): separate **Games** and **Media** views (pipeline filters gone from media; site/tag filters gone from games), shared `TileGrid`/`ItemEditor` only; **job queue** — imports/updates run background, observable (per-item progress + current URL + Stop), per-item vault locking so the grid fills live and Lock/Nuke cleanly cancels everything; bigger default window | Nothing blocks anything; imports are watchable |
| **M2.6** ✅ | Trust & cleanup (field report round 2/3, GitHub #1–#13 + #26): `4cdn`→**4chan** site naming + idempotent schema v3 migration for existing vaults; booru-CDN file links (`wimg.rule34.xxx//…?11353035`) route through the site API via the numeric query id (that was the missing-thumbnail cause), direct fetches send site-referer + normalized `//` paths; Complete/status hidden for media (D16); `+ Video` tracked-by-link import (D17); content-hash **dupe suggestions in Media**; search boxes both views; multi-select + batch `delete_items`; per-item source add/remove + **Re-fetch metadata** (`reenrich_item`); dismissable report strips + expiring finished-job rows; in-app `ConfirmDialog` replaces `confirm()`; enrich phase round-robins per site; **game-folder import deep-fix (#26)**: Ren'Py release layouts (`game/` with compiled `.rpa`, no `renpy/`) recognized in folder + zip fingerprints, and an empty scan returns a full explanation instead of silent zeros | Everything the round-2/3 reports flagged is fixed or ticketed |
| **M3** | Downloader: DLsite library sync + downloads, booru full-res, seed-host mirror picker, archive extract, local game folder linking + launch; container-encryption perf decision | Full pipeline |
| **M4** | MV3 extension (Chrome+Firefox) w/ per-site clip extractors from site-packs; embedded WebView2 tabs + clip bar + metadata-only mode; token pairing | The "wrapper" feel |
| **M5** | Nuke + panic hotkey + crypto-erase; scrubber; deniability (renaming, decoy chrome); proxy/Tor; mods/MTool/CE layer; settings polish; portable packaging | Hardened 1.0 |
| **M6+** | Artifact radar (experimental); site-pack ecosystem hardening; maybe stash export bridge | Backlog |

Build order note: M4 extension codegen depends on site-packs (§6), which land in M1–M2.

---

## 11. Risks & mitigations

- **No official APIs** (f95/DLsite) → selector/endpoint rot → fixture tests + raw snapshots + site-packs.
- **e-hentai** rate/IP limits → strict limiter, manual gates, APIDB auth.
- **Repack site link rot** (kimochi/ryuu/otomi) → mirrors not primaries; dead-mirror flags.
- **Legal:** metadata/link tracking fine; downloading repacks of paid works is user-jurisdiction responsibility → README disclaimer mirroring f95 community tools.
- **WebView2 on USB** — profile dir must live in vault; verify no fallback to `%LOCALAPPDATA%`.
- **SQLCipher perf** on thumbnail BLOB grids → BLOB storage + content-hash dedup + thumbnail size caps; revisit at M3.
- **Tag safety:** blocklist capability wired into tag system from M1 (e926-style safe filtering possible).

## 12. Open questions (park for implementation time)

1. Plain download folder + shred vs encrypted containers (perf vs deniability) → decide at M3.
2. Extension pairing UX: paste token vs Native Messaging shim → decide at M4.
3. UI framework details: Svelte 5 + which component kit (skeleton? plain CSS) → decide at M0.
4. ~~f95 account cookie storage~~ → decided in M2: cookies live in `site_settings` **inside the encrypted vault** (so a nuke takes them with it), pasted by the user under *Sources*. Still open: whether tracker sync is bidirectional by default.
5. User asked for a stack tutorial (Tauri/Rust/Svelte) later — provide when M0 is scaffolded.

---

## 13. Session handoff notes

- User approves milestones before implementation; M1 + M2 approved and built.
- When resuming: read this file top-to-bottom, check §10 for next milestone, then §12 for pending decisions.
- Repo root: `C:\Source\TheGoonFlow`. Build/test recipe in `docs/DEVELOPMENT.md`
  (import `tools\dev-env.ps1` before cargo; SQLCipher/OpenSSL need MSVC + Strawberry Perl + NASM).
- Shipped artifact for testing: `target\release\GoonFlow.exe` (single file, ~19 MB,
  frontend embedded; only WebView2 runtime required). Debug builds of the UI are a
  trap — they point at the Vite dev server and show "connection refused" without it.
- Schema is at **v3** (`site_settings`, `updates` at v2; v3 renames mislabeled CDN
  sites like `i`/`wimg` to their real names). `init_schema` is idempotent and runs
  versioned migrations, so unlocking an older vault upgrades it in place.
- Deliberate gaps left for M3/M4: companion `/api/clip` (needs the shared state
  refactor), downloads, selector site-packs + raw HTML snapshots for rot recovery,
  and a fixture suite for the HTML parsers (currently string-literal fixtures).
- M2.5 landed (D14/D15): `jobs.rs` job registry (`list_jobs`/`cancel_job`
  commands, polled by `src/lib/activity.svelte.ts`); LibraryView is now a tab
  shell over `GamesView` / `MediaView` / `SourcesPanel` with `ActivityBar`
  (progress + Stop) and shared `TileGrid`/`ItemEditor`/`ClipDialog`; import
  commands are async and take the vault lock per item; Lock/Nuke/forget-pointer
  cancel every job first, Nuke also wipes job history. Window default raised to
  1380×860 (min 1100×640).
- M2.6 landed (GitHub issues #1–#13, trust & cleanup): `HOST_SUFFIXES` +
  `site_for_host` in `ingest.rs` (4cdn → 4chan; booru-CDN file URLs with a
  numeric query id route through the booru API — the real cause of missing
  rule34 thumbnails); `apply_meta` + `reenrich_item` shared by clip/bookmark
  enrich and the new per-item **Re-fetch metadata**; `add_source`/
  `remove_source`/`delete_items` (batch) commands; `import_video_files`
  (file:/// links, no BLOB); `db::pairing_suggestions(conn, kind)` — games by
  title key, **media by shared stored-image hash**; bookmark enrich round-
  robins across sites; schema v3. Frontend: search boxes (debounced), multi-
  select + batch delete in both grids, `ConfirmDialog` (no more native
  confirm), `ReportBar` (dismissible strips), ActivityBar expires finished
  job rows after ~12 s. 40 workspace tests green.
- Repo is now on GitHub (`MoonerGan/TheGoonFlow`); milestones + the whole
  backlog live in GitHub Issues/Project (project 1). M2.6 items are P0;
  M3–M5 sit in Backlog. Field reports continue as issues.
- M3 progress ahead of schedule: **#15 (async per-source request queue) is
  done early** — `queue::SiteQueue` (per-site `reserve()` lanes, cancellable,
  cleared on Lock/Nuke) now backs bookmark enrich, re-fetch, and update
  checks; the M3 downloader (#17) must reuse it. See DEVELOPMENT §4 inv 11.
- **#18 (game launching) is done early too** — `launcher.rs` resolves a game's
  local source to a concrete `.exe` (engine-aware, folder-name preference,
  helper-exe filtering), honours a per-game `launch_path` override (schema v4,
  set from the editor's Launch block), and spawns it with its folder as cwd.
  Loose `.zip` sources report "extract first" (extraction is the #17 downloader's
  job). `preview_launch` never spawns so the UI can show what it *would* run.
- **#14 (automated metadata for local games) landed next** — schema v5
  `match_suggestions`; `ingest::find_matches` searches DLsite (POST /fs,
  verified live) and f95 (login-cookie only), scores titles with bracket-strip +
  bigram-Dice (floor 0.6, top 3), and the Games view *Online matches* panel
  attaches or dismisses proposals. Never auto-merge (issue requirement).
- **#16 (vault video storage + first-frame thumbs) landed** — D18: OS Shell
  thumbnail provider, no ffmpeg dep (verified live on an h264 mp4).
  `video.rs` store/remove/info commands (512 MiB cap, streaming download via
  the queue lanes), `videothumb.rs` (IShellItem→IThumbnailProvider→GetDIBits),
  importer tries a first-frame tile for every `+ Video` import, editor has the
  opt-in Store/Remove block. `tools\smoke-release.ps1` now smoke-tests the
  release exe's companion handshake automatically.
- **#24 (proxy / Tor) landed next (M5 pulled forward)** — all traffic now
  builds through `http::client_for` (bookmarks, clip, enrich, updates, match
  search, video download); vault `meta.proxy_url` (no schema bump) is read
  per-build, `set_proxy` validates schemes (http/https/socks4/socks5/socks5h),
  reqwest's `socks` feature makes `socks5://127.0.0.1:9150` work with Tor
  Browser. Sources panel has the Proxy block. A wiring test with a local stub
  proxy proves the route without needing network.
- **#17 started (increment 1: extraction + folder linking)** — `extract.rs`:
  a game whose only local source is a `.zip` gains *Extract zip* in the editor:
  unpacks next to the archive (`D:\games\Foo.zip` → `D:\games\Foo`), single
  wrapper folders are unwrapped, job-cancellable, zip-slip entries abort the
  whole run with nothing written, and the resulting folder is attached as a
  `local` source (the launcher now prefers folders/exes over loose zips).
  *Link folder…* attaches an existing folder directly.
  Still open in #17: DLsite library sync, booru full-res downloads, seed-host
  mirror picker.
- **#19 (MV3 browser extension) landed** — `companion.rs` `/api/clip` is now
  real: it runs `ingest::clip` through a shared `Core` (vault lock + job
  ledger + rate lanes identical to the `+ Clip URL` button), returns the clip
  report or a friendly error, and shows up in the activity strip. New
  `extensions/goonflow-clip` (MV3: context menus, port/token options, local
  handshake test, localhost-only host permissions). Sources panel gained a
  *Browser pairing* block (copy port/token). Refactor: `AppState` Deref-exposes
  a cloneable `Core`, so UI and companion never fork state. Companion auth +
  clip ladder covered by two new `#[tokio::test]`s (68 green).
- **#21 (in-app image browsing) landed** — clicking a media card now opens a
  full viewer (`Viewer.svelte`) fed by `media_view` → `media::best_for`
  (full-res from the vault, else stored video, else the thumbnail; nothing is
  ever re-downloaded). ← / → navigate the current filtered grid, + / − / 0
  zoom, space toggles a slideshow (4 s, wraps), stored videos play inline with
  a 96 MB inline cap (bigger ones show the poster + a hint), `i` jumps to the
  editor, Esc closes. New goon-core test for the full>video>thumb preference
  (69 green).
- **#25 started (increment 1: mod/tool launch recipes, D10)** — schema v6
  `mods` table (idempotent migration, cascades with the item). Per game you
  register MTool / Cheat Engine / any tool `.exe` with stored arguments;
  `mods::recipe` resolves the launch (mtool passes the game's resolved exe as
  the argument; cheat starts CE with an "attach inside" hint; other supports
  a `{game}` placeholder in args) and `launch_mod` starts it independently.
  Thin recipes — the app never injects, wraps or patches. The game editor
  gained a *Mods & tools* block (add via exe picker, ▶ launch, × remove).
  Still open in #25: DLL apply/revert with `*.flowbak` backups + manifests,
  and the update-clobber warning wired into the version watcher.
- **#25 increment 2 (DLL deploy + clobber guard)** — schema v7 adds
  `status`/`manifest` to `mods`. A `dll` kind recipe can be **Applied**:
  payload = zip / loose .dll / folder; entries pre-screened (zip-slip +
  would-overwrite-a-directory abort whole), replaced originals move to
  `<name>.flowbak` (oldest original wins), deployed files recorded as a
  JSON manifest (path + sha256) on the row; mid-run failure rolls back.
  **Revert** restores from the manifest and removes mod-added files; missing
  flowbaks (an update already ate them) become per-file notes. Deleting an
  applied mod is refused until reverted. `list_updates` now carries
  `has_mods`, and the updates panel shows *⚠ mods on this game* — the
  update-clobber warning. 4 more mods tests + status round-trip (78 green).
- **#23 started (increment 1: panic hotkey + crypto-erase drill)** — global
  hotkey (default `Ctrl+Alt+L`, always registered; a vault can store its own
  in `meta.panic_hotkey` and unlock adds it) → one press = `lock_core`
  (cancel every job, close DB, zeroize key — the same path as the Lock
  button) + hide the window; press again = show the locked window. Stored
  keys are parsed before saving so a typo can't unarm the panic path.
  *Run crypto-erase drill* builds a throwaway vault, nukes it through the
  real nuke path and verifies nothing survives; the timestamp is kept in
  the vault meta. Sources panel gained the Panic key block + drill button.
  New drill test (79 green). Still open in #23: hold-3s variant for
  crypto-erase-by-hotkey (needs a hold-detect loop) and the scrubber.
- **#23 increment 2 (scrubber)** — `scrub_now` clears the app's own WebView2
  browsing data (`clear_all_browsing_data`) and deletes Windows Explorer
  `thumbcache_*.db` files (they can hold previews of games/media on disk —
  data the vault never held but the OS cached); locked-by-Explorer files are
  counted, not forced, and nothing else in that folder is touched. A
  vault-stored `scrub_on_lock` flag makes every Lock *and* every panic
  press run the scrub automatically. Sources panel gained the Scrubber
  block (run + toggle + honest report line). Test proves only the
  thumbcache files die (80 green).
- **#23 increment 3 (hold-3s crypto-erase)** — the hotkey is now a
  press/release state machine: a tap behaves as before (lock+hide /
  reveal), but *holding* `PANIC_HOLD_MS` (3 s) fires `nuke_core` — the
  exact function the typed-NUKE command shares — then scrubs and hides
  the window mid-erase. OS auto-repeat can't fork a second episode
  (`panic_hold` flag), and the matching release after an erase is inert
  (`nuked` flag). UI + USER-GUIDE say the quiet part loudly: rehearse
  taps, never hold on the real vault. **Verified live** against the
  release exe via `tools\panic-tap-test.ps1` (injects Ctrl+Alt+L from
  another process, watches titled-window visibility: Flow VIS→HID→VIS).
  That test also drove a hardening pass: escalation re-checks the
  *physical* Ctrl+Alt state (`GetAsyncKeyState`) at the 3 s mark — a
  dropped release event can never nuke by itself — and the repeat guard
  is physical-aware too, so a lost release can't wedge the tap path.
  Plus `panic_gen`: every hold episode bumps a generation counter and a
  watcher escalates only if it still owns the *live* episode at its
  deadline — otherwise a stale watcher from an old quick tap could erase
  1 s into a fresh hold that straddles the old deadline. All three panic
  paths now have committed live checks: `panic-tap-test.ps1`,
  `panic-hold-test.ps1`, `panic-stale-test.ps1` (the last two refuse to
  run if a vault is attached beside the exe). This closes the code side
  of #23;
  the issue waits on your manual hotkey pass (tap, hold-on-drill-notes,
  custom key, scrub-on-lock) before Done.
- **#27 started (deniability inc1: decoy chrome)** — renaming the exe was
  already fine (verified live: copy as `FlowNotes.exe` → full handshake);
  now a plaintext `chrome.cfg` (one sanitized line, ≤64 chars) beside the
  exe overrides the OS window title at startup and live via
  `get_chrome`/`set_chrome` (work locked or unlocked — the file holds
  nothing sensitive), and the in-app brand follows the decoy after the
  next lock. Sources panel gained the *Decoy title* block; USER-GUIDE §2
  spells out what is dressed (title, brand, renamed exe, brandless locked
  screens) and what is not (file-properties metadata → needs custom
  resources rebuild; vault folder naming). New sanitize/round-trip test
  (81 green).
- Field-test resume point: the current `target\release\GoonFlow.exe` (built
  after #23, schema v7) runs everything below; existing `C:\GoonFlow`
  vaults migrate v3→v7 on unlock (idempotent, in place). Manual checklist
  for the In-review issues, in the order that surfaces problems fastest:
  1. Unlock the real vault → schema lands at v7, old items intact.
  2. Viewer (#21): click media cards — ←/→ through the grid, +/−/0 zoom,
     space slideshow, big stored video → poster fallback, `i` → editor.
  3. Extension (#19): chrome://extensions → Load unpacked →
     `extensions/goonflow-clip`; paste port+token from Sources → Browser
     pairing; right-click-clip a rule34/gelbooru post → lands + notifies.
  4. Extract (#17): game tracked by a loose zip → Extract zip… → folder
     linked → ▶ Launch works.
  5. Mods (#25): register MTool (mtool kind) → ▶ starts it with the game
     exe as arg; add a real f95 zip mod as dll kind → Apply → .flowbak
     originals + deployed chip → Revert → game pristine again; check the
     ⚠ chip on an update row for a modded game.
  6. Proxy (#24): set `socks5://127.0.0.1:9150` with Tor Browser running →
     Re-fetch metadata still succeeds (traffic through Tor); Clear → direct.
  Earlier notes still hold: re-clipping old rule34/wimg bare links (or
  Re-fetch metadata on them) now pulls art (v3 migration fixed labels).
