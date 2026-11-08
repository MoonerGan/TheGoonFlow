# The Goon Flow — User Guide

A private, encrypted library for the adult games and imageboard content you
already browse and own. Everything stays on your machine. Nothing phones home.

**Current state: M0** — encrypted vault, portable folder, importers, library
grid with editing, nuke. URL clipping, update tracking, and downloads are on
the roadmap (see [PLAN.md](../PLAN.md)).

---

## 1. First run: create your vault

The **vault** is one folder that holds everything: the encrypted database,
thumbnails, imported images. The app itself holds *nothing* except a tiny
non-sensitive pointer file (`flow.cfg`) that says which drive the vault is on.

1. Launch the app.
2. Choose the vault folder:
   - Default: beside the app (`goonflow-vault\`) — fine on a fixed PC.
   - **Recommended for portability/deniability: a USB or external drive.** The
     app finds it again even if Windows changes the drive letter (it matches
     the volume label).
3. Pick a **passphrase**. Strong and memorable — there is no recovery, and the
   app never sees it over the network because there is no network.
4. Done. The vault is encrypted with SQLCipher; the passphrase is stretched
   with Argon2id (deliberately slow — unlock takes a second, by design).

### Running from USB
Put the app executable *and* choose a vault folder on the same stick (or any
reachable drive). Carry the stick; plug into any Windows 10/11 machine; the app
asks for the passphrase and nothing else. It writes nothing outside its folder.

## 2. Lock, unlock, and the nuke

- **Lock** (top-bar) closes the database and wipes the key from memory.
  Reopen with the passphrase.
- **Nuke** (bottom of the lock screen, and on the setup/reconnect screens):
  crypto-erase. The key is shredded first, then every vault file is
  overwritten with random bytes and deleted. Type `NUKE` to confirm — it is
  **irreversible**. Use it if you forgot the passphrase or want a clean start.
- **Forget pointer** (reconnect screen): deletes only the app's `flow.cfg`
  pointer — touches no vault files. The escape hatch when the pointer points
  at a moved/deleted folder and you just want the app working again.
- Forgotten passphrase ≠ lost content forever only via backups: without it the
  vault is a brick. That's the point.

## 3. Import what you already have

Five import buttons in the top bar. Everything imported is just a normal
library entry — click any card to correct it. The sidebar splits the library
into **Games** and **Media** (galleries/posts) views, filterable by status.

### + Game folder
Point it at the folder that contains your game folders (or at a single game
folder). Nested library layouts (`root/by-engine/Game A/`, `root/JP/deep/…`)
are searched up to 4 levels deep. The Goon Flow sniffs each one:

| Engine | How it's recognized | Bonus metadata |
|---|---|---|
| RPG Maker MV/MZ | `game.rmmzproject` / `game.rmvproject` | real title from `System.json`, cover from title images |
| RPG Maker XP/VX/Ace | `Game.exe` + `Game.ini` | title from `Game.ini`, cover from title graphics |
| Ren'Py | `game\` folder + `.rpy` scripts | window icon as thumbnail |
| Unity | `*_Data` folder | title from folder name |
| GameMaker | `data.win` | — |
| HTML5 / NW.js | `package.json` + `index.html` | title from `package.json` |
| Generic Windows | top-level `.exe` | — |

Versions are parsed from folder names like `My Game [v0.12] [renpy]` →
title `My Game`, version `0.12`, engine tag. The games themselves are **not
copied into the vault** — the vault stores the path (as a `local` source) so
you can launch/move them as usual. Re-running an import skips known folders,
so it's safe to point at the same folder after adding games. If anything
can't be stored, the report strip gains a **details** list naming the folder
and the reason.

### + Game zip
A repacked game that still lives in a `.zip` (common with repack sites) can be
added without extracting: the zip's own contents are fingerprinted for engine
files and the real title/cover pulled out of the archive. The vault stores a
pointer to the zip where it is — it is never moved or copied.

### + Game .exe
Already know the exact game? Pick its `.exe` and its containing folder is
imported as one game entry. Handy for one-offs without browsing a whole tree.

### + Images
Pick image files (PNG/JPG/WebP/GIF/BMP). They are **stored inside the encrypted
vault** (originals + thumbnail) as a single Gallery item. A delete in the app
removes the vault copy only.

### + Bookmarks
Import a browser bookmarks export (Firefox: *Bookmarks → Export Bookmarks to
HTML*; Chrome: *Bookmarks Manager → Export*). Each bookmark is classified by
site — gelbooru, rule34, e621, e-hentai/exhentai, DLsite, f95zone, and other
known boorus — and becomes a tracked item with its source URL.

**Enrich online** (checkbox, on by default): when checked, the app politely
(one request at a time, ~0.7 s apart) fetches thumbnails, full images, and
tags for booru posts (gelbooru/rule34/e621 APIs) and cover art for DLsite
products. e-hentai and f95zone import title+URL only until session cookies
land in M2. Unchecked = pure offline metadata import. This only decorates
bookmarks you import — it is **not** the browser integration (that's the
companion extension in M4).

> Note: "enrich online" contacts those sites directly from this machine — same
> as browsing them. If that matters to you, uncheck it or run behind your
> usual proxy.

The grey report strip under the toolbar shows what happened (counts per site,
thumbnails fetched, duplicates). **Anything that fails now says so under a
`details` disclosure** — which URL failed, why (deleted post, age gate, HTTP
status, network error, unrecognized site), so a failure never means silently
scanning by hand. Imported items stick even if their online art failed to
fetch.

## 4. The library

- Grid of cards (thumbnail + title + kind/version/status). Click for the
  editor.
- **Editor:** fix the title, set circle/developer, version, pipeline
  **status** (`seen / tracking / wishlist / downloaded / played`), and the
  **✓ Complete** personal marker (green chip — it means *"I finished this"*,
  independent of status, and it survives game updates).
- **Remove from vault:** deletes the entry (and vault-stored images) — your
  game folders on disk are never touched.
- Sidebar filters by status; counts are live.
- The `:port` pill shows the companion API port (used by the future browser
  extension; it listens on 127.0.0.1 only).

## 5. Privacy notes

- No telemetry, no accounts, no cloud — the app can run fully offline.
- The only network traffic is the optional bookmark "enrich online" fetch and
  (from M1+) features you explicitly trigger.
- At rest, everything meaningful is encrypted (DB + images). The folder name
  is up to you — rename the vault folder/app exe to taste; the pointer file
  contains no secrets.
- `flow.cfg` (beside the exe) reveals only *where* a vault is. Nuke clears it.

## 6. Troubleshooting

| Symptom | Fix |
|---|---|
| "Reconnect your vault" screen | Plug in the drive; if the folder moved, browse to it once — the pointer updates. |
| "that folder is not a goonflow vault" | You picked the wrong folder (e.g. the parent). Pick the one containing `vault.marker`. |
| Wrong passphrase loop | No recovery — that's crypto, not a bug. Restore from a backup or nuke & restart. |
| Import found 0 games | The scan goes 4 levels deep; the game folder itself must contain the engine files. If the game is inside a .zip, use *+ Game zip*. Selecting the game's folder directly always works. |
| White/blank window at launch | WebView2 runtime missing — install "Microsoft Edge WebView2 Runtime". |
