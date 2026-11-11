# The Goon Flow — Design

> Durable design record: what this app is, the decisions behind it, and how it
> is built. **Live status does not live here** — what is shipped, in review or
> planned next is on the
> [GitHub project board](https://github.com/MoonerGan/TheGoonFlow/projects/1)
> and in the [issues](https://github.com/MoonerGan/TheGoonFlow/issues); the
> build/test recipe is [docs/DEVELOPMENT.md](DEVELOPMENT.md) and the manual
> field-test checklist is [docs/TESTING.md](TESTING.md).
>
> Section numbers are stable and cited from code comments (`DESIGN §4`, `D15`).

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
| D2 | Browser integration | **MV3 extension now** | Extension for daily browser work (shipped, #19). The *embedded WebView2 tabs in-app* half is unbuilt (#20) and drags the profile-location problem with it (#61). |
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
| D19 | Panic semantics, visible | **tap = hide + stop jobs, hold 3 s = crypto-erase, close = exit** | Field round 2 (#41): an app that "disappeared" was really a hidden instance still holding the vault. So the tap path must be reversible by tapping again; relaunching hands off to the running instance and reveals its window (single-instance plugin, **no tray icon** — permanent chrome fights §9 deniability); closing the window really exits; and when another program owns Ctrl+Alt+L the top bar shows a red *panic key NOT armed* pill. This is invariant 22 made concrete: a capability that failed to register must be visible. |
| D20 | Browser pairing | **paste port + token, loopback only** | `companion.rs` binds `127.0.0.1` on a random port with a per-session bearer token; the user pastes both into the extension from *Sources → Browser pairing*. No auto-connect, no Native Messaging, strict CORS on the extension origin. |
| D21 | Decoy chrome is plaintext-only | **side files never hold vault material** | `chrome.cfg` (decoy window title, etc.) and the exe name may be boring; `flow.cfg` holds only volume label + relative path. All content stays in the vault, so a rename or a decoy title never reveals anything and a nuke never has to chase plaintext. |
| D22 | AVIF thumbnails | **decode them: `image`'s `avif-native` with a statically linked dav1d** | #52: f95zone answers `image/avif` for its attachment art, so the vault held a picture the grid could not draw. Measured cost on the shipped artifact: **21 108 224 → 23 382 016 bytes (+2 273 792, m13 → m14)**; decode 2.5 ms (706 B/360×240) and 11.6 ms (50 101 B/1600×1067), thumbnail 7–21 ms / ~14 KB out. Static link keeps D4's single-file artifact true — proved by `dumpbin /dependents` showing no `dav1d*.dll`. Rejected: asking sources for a sized variant (e621/yande.re have **zero** AVIF originals and f95 has only one original per attachment) and making thumbnails in the webview (breaks every pageless job path per D15, needs a Blob per the no-Blob rule, and Chromium has no HEIF). HEIF/HEIC stays undecodable in `image` 0.25.10 and the app must keep saying so. |

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
- One folder = everything except the web engine's own profile: DB (SQLCipher) + WAL,
  thumbnails (DB BLOBs), logs. **Not** the WebView2 profile: Tauri defaults it to
  `%LOCALAPPDATA%\<identifier>\EBWebView` (measured: 75 MB, 326 files) and nothing here
  sets `data_directory` - #61 is the open decision.
- Vault root chosen on first run (default beside exe; browse to USB/partition).
- App dir holds only `flow.cfg` pointer: volume label + relative path (no sensitive data). Locked/absent vault → neutral "insert drive / pick vault" prompt (deniability: nothing revealed).
- Setup allows renaming exe/folder; locked state renders as a boring utility window.

### 4.2 Encryption & nuke
- Master key: Argon2id(passphrase) → wraps random data key (key hierarchy so passphrase changes don't re-encrypt everything).
- DB via SQLCipher. Downloaded content in XChaCha20-Poly1305 chunked containers (plain-folder mode possible with shred fallback — decide at M3 by perf).
- **Nuke:** global hotkey - tap = instant lock; hold 3 s = crypto-erase: destroy key
  material → overwrite DB+WAL+keyfile+`flow.cfg`, stop every job, clear the rate lanes,
  wipe job history, then scrub the web session and Explorer's thumbnail caches. The
  extension's own browser storage is out of reach from here: its saved port+token survive
  a nuke, and removing them is the manual step in `extensions/goonflow-clip/README.md`.
- Scrubber (`lib.rs::do_scrub`): clears the WebView2 browsing data and deletes
  `%LOCALAPPDATA%\Microsoft\Windows\Explorer\thumbcache_*.db` - files Explorer is holding
  are counted (`thumbcache_locked`) and reported, never forced. Runs on Lock when
  *scrub on every lock* is ticked, and always on hold-to-erase. A sweep of Jump Lists,
  Recent and Prefetch is **not built**.

---

## 5. Data model

**Schema of record: `crates/goon-core/src/db.rs::init_schema`.** The live tables are
`meta`, `items`, `sources`, `tag_types`, `tags`, `item_tags`, `media`, `site_settings`,
`updates`, `match_suggestions`, `mods` and `pair_dismissals`. Anything marked *planned*
below is design intent, not schema.

- `items` — ULID, kind (`Game` | `Gallery` | `Post`), normalized + original title,
  circle/dev, pipeline `status` (`Seen → Tracking → Wishlist → Downloaded → Played`),
  DNF flag, notes (the user's own field - no fetch ever writes into it), rating,
  created/updated, and **one `version` column**.
  - Note: "Complete" is **not** here — it's a tag (D9). A completed game still re-surfaces on updates.
- `sources` — many→one with items. Per-site identity: url, source id, fetched_at,
  dead-mirror flag. A `raw` column exists in the table but **no code ever writes it**, so
  the "raw JSON/HTML snapshot (selector-rot recovery)" half is *planned*, not built.
  - Auto-pairing across sites (DLsite origin ↔ f95 thread ↔ repack mirrors) via circle+title fuzzy match; manual merge/unmerge UI.
- `versions` — *planned as a table*; today a version is one column on `items` and update
  detections go to `updates` (which is what the update feed reads). Changelog watches: unbuilt.
- `tags` + `tag_types` — tag kinds with display attributes:
  - `user_status` — personal state: **Complete (green, high priority)**, Wishlist, DNF, Replay… shown as colored chips on cards, sorted first.
  - `site_flag` — developer/site facts imported from sources (f95 `[Completed]` prefix, engine tags like Ren'Py/RPGM/Unity).
  - `content` — normal tags. `tag_aliases` (booru tags ↔ DLsite genres ↔ f95 prefixes in
    one facet, doubling as the blocklist) is *planned*: nothing translates between site
    vocabularies today, and the blocklist is a plain tag filter.
- `media` — thumbnails as encrypted BLOBs in DB, content-hash keyed (dedup + trivial wipe). Previews/local paths for downloaded galleries.
- `downloads` — *planned* (M3): no download-queue table and nothing downloads by itself.
  Jobs live in memory (`jobs.rs`); a video is stored in the vault only on request.
- `mods` exists (table + deploy/rollback in `mods.rs`); `tools` and `attachments` as tables
  are *planned* (§7). Related gap, same shape: an f95 clip finds 15 images and stores one
  cover, and a `+Images` gallery stores every original but `media::best_for` reads one back
  (#53, #62).
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

## 10. Roadmap (status lives on the board)

The original milestone map, kept so the shape of the build is legible. Every
milestone below M6 shipped; what happens next is decided on the board, not here.

| Stage | Scope | State |
|---|---|---|
| M0 | Portable vault (SQLCipher + key hierarchy + nuke), locator pointer, schema, importer (games/images/bookmarks), first UI | shipped |
| M1 | Imageboards: clip pipeline, thumbnails, booru full-res, tag system | shipped |
| M2 | DLsite + f95zone ingest, cross-site pairing (suggest, never auto-merge) + merge UI, versions/update feed, repack mirrors off by default, per-site enable/cookie/rate under *Sources* | shipped |
| M2.5 | D14/D15: separate **Games** and **Media** views, shared `TileGrid`/`ItemEditor` only; observable job queue with per-item locking and clean cancel | shipped |
| M2.6 | Trust & cleanup round: site naming + idempotent migrations, booru CDN links through the site API, `+ Video` by link, dupe suggestions, batch delete/re-fetch, in-app confirm dialogs, deep game-folder import fixes | shipped |
| M3 | Downloader: DLsite library sync, booru full-res storage, mirror picker, archive extract, local game folder linking + launch, container-encryption perf decision | shipped in pieces (#15 #17 #16 #24 #25), full-res storage per site is open work |
| M4 | MV3 extension (Chrome + Firefox) with per-site clip extractors, token pairing | shipped (#19); cookie hand-off is #38. **Embedded WebView2 tabs: not started** (#20) |
| M5 | Panic hotkey + crypto-erase, scrubber, deniability (rename, decoy chrome), proxy/Tor, mods/MTool layer, packaging | shipped (#23 #27 #24 #25) |
| M6+ | Artifact radar (D11, experimental), site-pack ecosystem hardening, Stash export bridge | backlog |

Build order note: extension codegen depends on site-packs (§6).

---
## 11. Risks & mitigations

- **No official APIs** (f95/DLsite) → selector/endpoint rot → fixture tests + site-packs.
  The raw-response snapshots this line relied on are not written (#59).
- **e-hentai** rate/IP limits → strict limiter, manual gates, APIDB auth.
- **Repack site link rot** (kimochi/ryuu/otomi) → mirrors not primaries; dead-mirror flags.
- **Legal:** metadata/link tracking fine; downloading repacks of paid works is user-jurisdiction responsibility → README disclaimer mirroring f95 community tools.
- **WebView2 on USB** — profile dir must live in vault; verify no fallback to `%LOCALAPPDATA%`.
- **SQLCipher perf** on thumbnail BLOB grids → BLOB storage + content-hash dedup + thumbnail size caps; revisit at M3.
- **Tag safety:** blocklist capability wired into tag system from M1 (e926-style safe filtering possible).

## 12. Open questions (park for implementation time)

1. Plain download folder + shred vs encrypted containers (perf vs deniability) →
   still open. Images/video thumbs already live as vault BLOBs; the archive
   download pipeline is where the choice actually bites.
2. ~~Extension pairing UX~~ → decided (D20): paste port + token into the
   extension, loopback-only companion API with a per-session bearer token. No
   Native Messaging shim; nothing may auto-connect.
3. ~~UI framework details~~ → decided: Svelte 5 with hand-written dark CSS, no
   component kit. svelte-check must stay at 0 errors **and** 0 warnings, a11y
   warnings included.
4. ~~f95 account cookie storage~~ → decided in M2: cookies live in `site_settings`
   **inside the encrypted vault** (so a nuke takes them with it), pasted by the
   user under *Sources*. #38 closed the other half: the paste is checked **before**
   it is stored (`cookies.rs` wants `xf_session` on f95 and the `ipb_member_id` +
   `ipb_pass_hash` pair on e-hentai / Exhentai, and refuses a site that never asked
   for a cookie), one page fetch answers whether the site recognises the session,
   the verdict is recorded in `meta` (`cookie_ok:<site>`, `cookie_checked_at:<site>`)
   so "never checked" stays visibly unknown rather than green, and any later fetch
   that hits the login wall marks the cookie stale. The browser extension can hand
   over that browser's own sign-in cookie, by one deliberate click; the app never
   reads a cookie store by itself.
5. Stack tutorial (Tauri/Rust/Svelte) for the user — still owed; `docs/DEVELOPMENT.md`
   is the closest thing to it today.

Anything that used to be an open question but has a decision behind it is a row in
§2 or an issue on the board; do not park things here.

