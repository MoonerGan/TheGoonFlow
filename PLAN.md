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
- Field-test resume point: open the existing `C:\GoonFlow` vault — the v3
  migration fixes the mislabeled `i` rows on unlock, and re-clipping the old
  rule34/wimg links (or pressing Re-fetch metadata on them) now pulls art.
