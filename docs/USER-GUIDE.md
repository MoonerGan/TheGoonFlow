# The Goon Flow — User Guide

A private, encrypted library for the adult games and imageboard content you
already browse and own. Everything stays on your machine. Nothing phones home.

**Current state: M1/M2** — encrypted vault, portable folder, importers, separate
Games and Media library views, URL clipping, tags, update tracking, live import
progress with Stop. Downloads and the browser extension are on the roadmap (see
[PLAN.md](../PLAN.md)).

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

- **Lock** (top-bar) closes the database and wipes the key from memory. Any
  running import, clip or update check is cancelled first — nothing keeps
  fetching or writing after you lock. Reopen with the passphrase.
- **Nuke** (bottom of the lock screen, and on the setup/reconnect screens):
  crypto-erase. Every running import/fetch is cancelled first, then the key is
  shredded, then every vault file is
  overwritten with random bytes and deleted. Type `NUKE` to confirm — it is
  **irreversible**. Use it if you forgot the passphrase or want a clean start.
- **Forget pointer** (reconnect screen): deletes only the app's `flow.cfg`
  pointer — touches no vault files. The escape hatch when the pointer points
  at a moved/deleted folder and you just want the app working again.
- Forgotten passphrase ≠ lost content forever only via backups: without it the
  vault is a brick. That's the point.

## 3. Getting things in

Three tabs across the top — **Games**, **Media**, **Sources** — and each view
carries only its own actions:

- **Games** view: *+ Game folder / + Game zip / + Game .exe*, pipeline filters
  (seen / tracking / wishlist / downloaded / played), Updates, Possible dupes,
  Online matches.
- **Media** view: *+ Clip URL / + Images / + Bookmarks*, filters by **source
  site** and **tag**. No pipeline filters — a booru post has no "wishlist"
  state, so you will never be offered one.
- **Sources**: site on/off, politeness rates, login cookies.

Everything you add becomes a normal library entry — click any card to correct it.

### Nothing blocks the app (and you can watch/stop it)

Imports run in the background as **jobs**. While one runs:

- every other button stays usable — import bookmarks and edit a game at the
  same time;
- the **activity strip** under the tabs shows what is happening: item counts
  (`bookmarks 134/812`), a progress bar, and the exact URL/folder it is on
  right now — so a slow enrich phase is visible, not a frozen window;
- **Stop** on the strip halts a job at the current item; everything already
  imported is kept;
- items appear in the grid **as they are imported**, not all at the end.

Only a *second job of the same kind* is refused (two simultaneous bookmark
imports of one file would just race each other) — the button says why.

### + Clip URL (the everyday one)
Paste a single link and it is identified, stored, and decorated where possible:

| You paste | What happens |
|---|---|
| a booru post page (gelbooru, rule34, e621) | post tracked, image + tags pulled in |
| a direct file link (`…/6ce01.gif?267940`) | file grabbed for images; **videos stay a link** |
| `file:///D:/Media/clip.mp4` or `.png` | resolves as media: images are stored + thumbnailed, videos tracked by path |
| e-hentai / exhentai gallery | title + cover (if the page isn't the IP-login gate) |
| DLsite product, f95 thread | title, version, cover/engine tags |
| a repack mirror (kimochi, otomi, ryuugames…) | tracked as a DLsite mirror — **off by default**, switch it on under *Sources* |

If the link is already in the vault it is not duplicated — the app tells you
which item already holds it. If a site needs a login, the clip still stores the
link and says so; add the cookie under *Sources* and clip again to enrich it.

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

### + Video
Pick video files (MP4/WebM/MKV/MOV/AVI). Videos are **tracked by path, not
stored** — each file becomes its own media item pointing at `file:///…`, the
file itself stays exactly where it is. That keeps the encrypted vault small and
the nuke story simple. (Vault-stored video is planned as an opt-in later.)

### + Bookmarks
Import a browser bookmarks export (Firefox: *Bookmarks → Export Bookmarks to
HTML*; Chrome: *Bookmarks Manager → Export*). Each bookmark is classified the
same way *+ Clip URL* classifies one link: gelbooru, rule34, e621 and other
known boorus, e-hentai/exhentai, DLsite, f95zone, **direct image/video file
URLs**, and **`file:///` links to media already on this machine** — so a
bookmark to `file:///D:/Videos/clip.mp4` becomes a media item, not an error.
Anything else counts as *unrecognized* and is listed in the report.

**Enrich online** (checkbox, on by default): when checked, the app politely
(one request per site at a time, honouring that site's rate) fetches
thumbnails, full images, and tags for booru posts, cover art for DLsite and
repack pages, and title/version/engine tags for f95 threads. e-hentai and
login-gated sites import title+URL unless you saved a cookie for them under
*Sources*. Unchecked = pure offline import (local files are still read and
thumbnailed — they need no network). This only decorates what you import — it
is **not** the browser integration (that's the companion extension in M4).

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
  editor. Cards arrive live during imports.
- **Search box** (both views): matches title/circle as you type — the quick
  way to confirm something actually landed.
- **Select… mode** (both views): click cards to tick them, *Select all* /
  *Done* in the toolbar, then **Delete selected** removes the whole batch in
  one confirmed step. Great after a messy import.
- **Editor (games):** title, circle/developer, version, pipeline **status**
  (`seen / tracking / wishlist / downloaded / played`), and the **✓ Complete**
  personal marker (green chip — it means *"I finished this"*, independent of
  status, and it survives game updates).
- **▶ Launch (game editor):** starts the game. The app finds the launcher from
  the engine layout (Unity `<Name>.exe`, RPG Maker `Game.exe`, Ren'Py, or an
  exe named after the folder) and runs it with its own folder as the working
  directory. If auto-detect picks wrong, **Set launcher…** lets you point at
  the right `.exe` (saved per game; **Clear override** reverts). A game still
  inside a `.zip` says so — extract it first (downloads/extracting arrive in
  M3). The game keeps running if you close the app.
- **Editor (media):** title and tags only — no status/version/circle and no
  Complete marker, because none of that applies to a post or gallery.
- Both editors: add or remove **tags** (booru tags arrive on their own when you
  clip/enrich; add your own, e.g. `renpy`, `dnf`).
- **Sources inside the editor:** every source link an item has, with an × to
  detach a wrong one, and a box to **paste a new source link** — for example
  the DLsite or f95 page of a game you imported as a bare folder. Press
  **Re-fetch metadata** to visit all of the item's sources and pull in cover
  art, tags, title and version it was missing. Each source reports what it
  gave (or why it failed) right in the editor.
- **Tagged** (sidebar): the most used tags *in the current view* (game tags
  and media tags are kept apart); click one to filter, click again (or *clear*)
  to unfilter.
- **From site** (Media sidebar): filter by where the item came from — booru,
  e-hentai, `local` for your own files — with live counts.
- **Updates** (Games view): re-visits the DLsite pages and f95 threads of games
  you track and records version changes. A game marked Complete still surfaces
  here — completion is a tag, not a full stop.
- **Possible dupes** (both views): in Games, the same title from DLsite + f95 +
  a local folder is almost certainly one work. In Media, two entries holding
  the **exact same image bytes** (reposted across boorus, imported twice) show
  up here. Nothing merges itself; you pick which entry to keep and the other's
  sources/tags/art are folded into it.
- **Online matches** (Games view): games you imported from a folder are matched
  *by title* against DLsite's and f95zone's own search (version/engine brackets
  like `[v0.12] [renpy]` are stripped first). Only confident hits (≥60%) are
  proposed, up to three per game, each with a score. **Attach + fetch** links
  that page as a source and pulls its cover/title/version; **Dismiss** throws
  the proposal away. Nothing ever links itself. f95 search runs only with your
  saved f95 cookie (Sources tab); without it, DLsite matching still works.
- **Sources** (tab): turn each site on or off, set how polite it should be (ms
  between requests), paste a **cookie** for sites that need a login (e-hentai,
  f95 threads), and see how many items each site currently accounts for.
  Repack mirrors are off by default — they are re-uploads of DLsite works, and
  their links fold into the original through *Possible dupes* rather than
  staying separate entries.
- **Remove from vault:** deletes the entry (and vault-stored images) — your
  game folders on disk are never touched. Destructive actions ask in an
  in-app dialog, not a system popup.
- Report strips (the grey lines under the toolbar) have an **×** and expire
  on their own; finished jobs leave the activity strip after a few seconds.
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
| Import found 0 games | The scan goes 5 levels deep; the game folder itself must contain the engine files. A Ren'Py game is recognized even when it ships only compiled `game/*.rpa` files — if a folder still isn't recognized, the report now **tells you exactly what it saw**: folders checked, archives found (7z/rar must be extracted first; a .zip game goes through *+ Game zip*), folders that almost matched, and the picked folder's top-level contents. Selecting the game's folder directly always works. |
| Bookmark import feels slow | It's probably the *enrich online* fetch, which is deliberately polite (one request per site, several seconds apart for some sites). The activity strip shows exactly which link it's on. Hit **Stop** any time — everything imported so far is kept — and re-run with enrich off (or import again later with just the new bookmarks); duplicates are skipped automatically. |
| Clip says "…is disabled — enable it under Sources" | That site is switched off (repack mirrors ship off). Turn it on in *Sources* and clip again. |
| Clip says "can't identify that link" | It is not a media file and not a site the app knows yet. Site support grows over time; the link text itself is never lost if you keep it in your browser. |
| Local `file:///…` item has no thumbnail | The file was moved, renamed, or the drive is unplugged — the item is still tracked by path. Re-clip from the new location to store a copy. |
| Booru clip stored the link but no art | Deleted post, video post (videos stay links), or the site needs a login cookie. The reason is in the report strip's *details*. Direct file links like `wimg.rule34.xxx/images/….jpeg?12345` now resolve through the site's API — old items imported before that fix can be repaired with **Re-fetch metadata** in the item editor. |
| An old item shows site "i" or "wimg" | Fixed automatically: on the next unlock, a schema (v3) migration renames those mislabeled sources to their real site (`4chan`, `rule34`, …). |
| White/blank window at launch | WebView2 runtime missing — install "Microsoft Edge WebView2 Runtime". |
