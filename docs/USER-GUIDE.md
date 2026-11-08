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
- **Panic key** (global hotkey, works from any other app): `Ctrl+Alt+L` by
  default — a short press does a Lock *and* hides the window entirely; press
  again to bring the locked window back. **Hold the key down for 3 seconds
  and it escalates to a full crypto-erase** of the attached vault — exactly
  what typing `NUKE` does, but with no confirmation possible, because in a
  real panic there is no time to type. The window hides the moment the erase
  starts. Learn the difference with the **crypto-erase drill** below, and
  rehearse the tap — deliberately do this on the real vault only when you
  mean it.
  - **Lost the window?** A hidden app is still running (it holds its own exe
    file, so a rebuild or delete will fail until it is closed). Tap the panic
    key again, or simply **start the app again**: a second launch now reveals
    the existing window instead of starting a second copy.
  - **Closing the window quits the app.** It does not sit in the system tray or
    keep running in the background — the only way to be left with a running
    but invisible app is the panic hide above.
  - If the hotkey could not be registered (another program or another GoonFlow
    instance already owns that combination), the top bar shows a red
    **panic key NOT armed** pill and a strip saying why. That case is real —
    it happened during testing — and the app refuses to pretend otherwise.
- Under *Sources* you can store a custom panic key with the vault (it follows
  the drive, not the machine). The same Sources block has the
  **crypto-erase drill**: builds a throwaway vault and nukes it through the
  real nuke path, so you can verify the nuke actually destroys everything
  without gambling your real vault.
- **Nuke** (bottom of the lock screen, and on the setup/reconnect screens):
  crypto-erase. Every running import/fetch is cancelled first, then the key is
  shredded, then every vault file is
  overwritten with random bytes and deleted. Type `NUKE` to confirm — it is
  **irreversible**. Use it if you forgot the passphrase or want a clean start.
- **Forget pointer** (reconnect screen): deletes only the app's `flow.cfg`
  pointer — touches no vault files. The escape hatch when the pointer points
  at a moved/deleted folder and you just want the app working again.
- **Scrubber** (Sources tab): the vault is encrypted, but *outside* it the OS
  keeps traces — the app's web session (thumbnails it fetched) and Windows
  Explorer's thumbnail caches (previews of game/video files on any drive).
  **Scrub now** clears both; files Explorer is actively holding are reported,
  not forced. Tick *scrub on every lock* and the same cleanup runs whenever
  you Lock or hit the panic key — so "locked" really means cleared.
- Forgotten passphrase ≠ lost content forever only via backups: without it the
  vault is a brick. That's the point.
- **Making it boring (deniability):** the app is built to sit on a drive
  without announcing itself —
  - **Rename the exe and its folder freely** (`FlowNotes.exe`, anything).
    Everything resolves relative to the exe; a renamed copy was verified to
    work fully. The pointer file (`flow.cfg`) and vault never care.
  - **Decoy title** (Sources → *Decoy title*): store one boring line
    ("Flow Notes", "Sync Client"…) and the window/taskbar title changes from
    the neutral *Flow* to it, on every future start as well. It lives in a
    plaintext `chrome.cfg` beside the exe — safe to read, it contains nothing
    but the decoy itself. Once set, the in-app brand follows the decoy too
    (the forgot-to-lock case).
  - Locked and setup screens already show no branding.
  - Honest limits: Explorer's *file properties* still reveal the internal
    build name until you rebuild with custom resources; what you name the
    *vault folder* and how you mount it is yours. Deniability covers casual
    looks, not a forensic examiner with your unlocked machine.

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
the nuke story simple. Whenever Windows can decode the file, the import also
grabs a **first-frame preview** so the grid shows real tiles (no extra codecs
are installed — it uses the same player Windows already has).

**Store copy in vault** (per video item, in its editor): opt-in escape hatch —
copies that one video into the encrypted database (up to 512 MB each, with a
size warning first), so it survives even if the original file goes missing.
**Remove stored copy** puts the item back to link-only; the original file was
never touched either way. Nuke destroys stored videos with everything else.

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
is **not** the browser integration — that's the *GoonFlow Clip* extension, below.

> Note: "enrich online" contacts those sites directly from this machine — same
> as browsing them. If that matters to you, uncheck it or run behind your
> usual proxy.

The grey report strip under the toolbar shows what happened (counts per site,
thumbnails fetched, duplicates). **Anything that fails now says so under a
`details` disclosure** — which URL failed, why (deleted post, age gate, HTTP
status, network error, unrecognized site), so a failure never means silently
scanning by hand. Imported items stick even if their online art failed to
fetch.

### Browser extension (GoonFlow Clip)
Clip without leaving the page. Two builds ship with the app, because Chrome and
Firefox are not the same animal here:

- **Chrome / Edge** (`extensions/goonflow-clip`): `chrome://extensions` →
  **Developer mode** → **Load unpacked** → pick that folder.
- **Firefox** (`extensions/goonflow-clip-firefox`): **use this one, not the
  Chrome folder.** MV3 extensions run their background from a *service worker*,
  and Firefox disables service workers by default on hardened profiles
  (`dom.serviceWorkers.enabled=false`), which kills the Chrome build before it
  starts — and it does that for **every** install, temporarily loaded or
  permanently signed. `about:add-ons` → gear icon (top right) → **Load Temporary
  Add-on…** → pick that folder's `manifest.json`. Temporary add-ons are dropped
  when Firefox restarts, so re-load after each restart (or have the add-on
  signed for a permanent install). If you edit the extension's files, *remove*
  it and load it again — Firefox does not reload a manifest on the fly.

**Click the toolbar icon** — that's the whole UI: it shows whether the app is
reachable and whether the vault is locked, and **Clip this page** is one button.
**Pairing / options** in that popup opens the pairing form. Pair once per app
launch: the app listens on a random local port with a fresh token each start, so
open **Sources → Browser pairing**, *Copy port* and *Copy token*, paste both in,
and press **Test connection**. The popup then tells you what it found instead of
you guessing.

Right-click any **link, image, page, or selected URL** → *Clip … to GoonFlow*.
It travels the exact same pipeline as *+ Clip URL*: classified, politely
enriched, stored in the vault, and it appears in the activity strip. A desktop
notification tells you it landed (or why it didn't). The extension can only
talk to `127.0.0.1`/`localhost` — it has no internet access at all; only the
app reaches the content sites (and through your proxy if you set one).

## 4. The library

- Grid of cards (thumbnail + title + kind/version/status). **Games**: click opens
  the editor. **Media**: click opens the **full viewer**. Cards arrive live
  during imports.
- **Media viewer**: the real image from the encrypted vault (full-res when it
  was stored), not a re-download. Navigate with **← / →** through the current
  grid (including filters), **zoom** with `+`/`-`/`0` or the buttons, hit
  **space** for a **slideshow** (4s per item, wraps), `f` (or the ⛶ button) for
  **fullscreen** — the image fills the window, chrome hidden, `Esc` or `f` back
  out — `i` opens the item's
  editor, **Esc** closes. Stored videos play inline (very large ones show the
  poster instead); link-only videos show their first-frame poster with a hint.
  The picture you are looking at stays put while the next one loads in the
  background; importing something else never re-flickers the current frame.
- **Search box** (both views): matches title/circle as you type — the quick
  way to confirm something actually landed.
- **Select… mode** (both views): click cards to tick them, *Select all* /
  *Done* in the toolbar, then **Delete selected** removes the whole batch in
  one confirmed step, or **Re-fetch metadata** re-visits every source of every
  selected item as one cancellable job (progress + current item in the activity
  strip, Stop works, locking the vault stops it too). Great after a messy
  import.
- **Editor (games):** title, circle/developer, version, pipeline **status**
  (`seen / tracking / wishlist / downloaded / played`), and the **✓ Complete**
  personal marker (green chip — it means *"I finished this"*, independent of
  status, and it survives game updates).
- **▶ Launch (game editor):** starts the game. The app finds the launcher from
  the engine layout (Unity `<Name>.exe`, RPG Maker `Game.exe`, Ren'Py, or an
  exe named after the folder) and runs it with its own folder as the working
  directory. If auto-detect picks wrong, **Set launcher…** lets you point at
  the right `.exe` (saved per game; **Clear override** reverts). A game still
  inside a `.zip` shows **Extract zip…**: it unpacks into a folder next to
  the archive (the zip itself is never touched, the run is cancellable, and a
  malicious archive that tries to write outside the folder is refused whole)
  and links that folder, so **▶ Launch** lights up. **Link folder…** points a
  game at any folder already on disk. The game keeps running if you close the
  app.
- **Mods & tools (game editor):** register the helper tools you already use —
  **MTool**, **Cheat Engine** (started; you attach inside it), or any patcher
  with saved arguments (`{game}` expands to the game's exe). Each row gets a ▶
  button. These are *launch recipes only*: the app never injects, wraps or
  patches anything itself, and it never writes into another tool's own files.
  - **MTool builds differ**, and the recipe follows the build rather than
    pretending otherwise. Older native builds take the game's exe as an
    argument, so that's what they get. The current **NW.js MToolClient** builds
    (0.6x: `nw.dll` + `package.json` next to `MTool.exe`) have **no
    command-line target at all** — their game list is drag-and-drop — so
    passing an argument would just do nothing. For those the app (1) uses the
    **game's own MTool launcher** if the repack ships one (`StartWithTool.bat`,
    `MTool_Game.exe`, `launchGameWithMTool…`), which is the flow that actually
    starts the game under the tool; otherwise (2) it opens MTool and puts the
    game's exe path **on your clipboard** so you can drop it in, and says so in
    the message under the mod list.
  - **DLL / patch mods** go one step further: add a mod kind of *DLL / patch
    mod* pointing at its **zip, single .dll, or an unpacked folder**, then
    **Apply** deploys the files into the game folder — every file it replaces
    is first moved to `<name>.flowbak`, and the exact file list is recorded
    on the item. **Revert** restores the originals and removes what the mod
    added. A payload with entries that would escape the game folder, or that
    collide with a folder, is refused whole; a failure mid-deploy rolls
    everything back. You can't delete an applied mod before reverting it.
  - **Update-clobber warning:** when an update is recorded for a game that
    has Mods & tools registered, the updates panel flags it with
    *⚠ mods on this game* — the new build will land on top of that folder.
    Revert (or expect to re-apply) around updates.
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
- **Sources** (sidebar, both views): the *fix-my-metadata* queues, with counts —
  **Local only** (a folder/file and no online link yet: add the DLsite/f95 page
  or the post link, then Re-fetch), **No local file** (games whose folder is
  gone or was never linked) and **No tags** (bare imports and unresolved file
  links). Click one to filter the grid to exactly that queue; it stacks with
  search, tag and site filters, and **Everything** / *clear* drops it again.
- **Updates** (Games view): re-visits the DLsite pages and f95 threads of games
  you track and records version changes. A game marked Complete still surfaces
  here — completion is a tag, not a full stop.
- **Possible dupes** (both views): in Games, the same title from DLsite + f95 +
  a local folder is almost certainly one work. In Media, two entries holding
  the **exact same image bytes** (reposted across boorus, imported twice) show
  up here. Nothing merges itself; you pick which entry to keep and the other's
  sources/tags/art are folded into it.
  - Each row labels the two entries **1** and **2** and prints that entry's own
    **sites and source links/paths** underneath, so a folder import and a web
    link with the same title are tellable apart.
  - **Keep 1 / Keep 2** merge. **Keep both** means *"not a dupe — stop showing
    me this pair"*: the dismissal is stored in the vault and survives restarts
    and re-imports (nothing is deleted, both entries stay exactly as they are).
- **Online matches** (Games view): games you imported from a folder are matched
  *by title* against DLsite's and f95zone's own search (version/engine brackets
  like `[v0.12] [renpy]` are stripped first). Only confident hits (≥60%) are
  proposed, up to three per game, each with a score. **Attach + fetch** links
  that page as a source and pulls its cover/title/version; **Dismiss** throws
  the proposal away. Nothing ever links itself. f95 search runs only with your
  saved f95 cookie (Sources tab); without it, DLsite matching still works.
- **Sources** (tab): grouped, top to bottom — *Network* (proxy), *Panic &
  privacy* (panic key, crypto-erase drill, scrubber, decoy title), *Game sites*,
  *Repack mirrors*, *Imageboards and galleries*, *Local files*, *Browser
  pairing*, and a *Getting a cookie* how-to. Per site: turn it on or off, set how
  polite it should be (ms between requests), paste a **cookie** for sites that
  need a login, and see how many items each site currently accounts for. The
  rate box starts at that site's own default (e-hentai's ~10 s, boorus' 1–2 s),
  and enabling a site for the first time keeps that default instead of resetting
  it to one shared number.
  - **Getting a cookie** (f95 threads, E-hentai/Exhentai): log in in your
    browser, press **F12** → **Network** → reload → click the first request →
    under *Request Headers* copy everything after `Cookie:` (one long line, no
    quotes) and paste it into that site's box. The app keeps it inside the
    encrypted vault, never in a plain config file, and it never reads your
    browser's own cookie store — this is a copy you chose to hand over. When a
    session goes stale the fetch says so out loud (e.g. *"f95 wants a login —
    set its cookie under Sources"*) rather than importing nothing quietly.
- **Proxy** (top of the Sources tab): route *every* outbound request — clip
  fetches, enrich, update checks, match search, video downloads — through one
  proxy address, e.g. `http://127.0.0.1:7890` or a **Tor** SOCKS endpoint.
  With the Tor Browser running, `socks5://127.0.0.1:9150` hides which
  adult sites this machine visits; the setting is stored inside the encrypted
  vault (so it follows the USB drive, never this PC) and is validated before
  saving — a typo is refused, not silently trusted. **Clear** goes back to
  direct connections. The app never talks to anything except the sites you
  track; a proxy just changes *how* it reaches them.
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
| Clip stored the picture but no tags/source | The link pointed straight at a file with **no post id anywhere in it** (`img3.gelbooru.com/images/2b/34/….jpg`). The image is stored anyway and the item says *"no post could be identified"*. Where the site can look a file up by hash, the app matches it back to its post — but only when the post's md5 **and** file size both match the bytes it downloaded, so it never guesses a wrong post. Fix the rest by hand: open the item, paste the real post page into **Sources**, then **Re-fetch metadata**. Find these in bulk under **Sources → No tags** in the sidebar. |
| ▶ next to MTool started the tool but the game did not open | Recent MTool builds (NW.js `MToolClient`) accept **no** game on the command line — their list is drag-and-drop, so nothing was ever going to start. The app now uses the game's own `StartWithTool.bat` / `MTool_Game.exe` when the repack ships one, otherwise it opens MTool and puts the game's exe path **on your clipboard** for you to drop in (the message under the mod list says which happened and shows the path). |
| Extension icon does nothing / only *Pin · Manage* | Out of date load: remove the add-on and load it again (temporary Firefox add-ons never pick up manifest changes on their own). The Chrome folder also cannot work in Firefox — use `extensions/goonflow-clip-firefox`. The popup says what it sees: *Nothing on port N* = the app isn't running or its port changed (re-copy port + token), *vault is LOCKED* = unlock it first. |
| A *Possible dupes* pair is not actually a dupe | **Keep both**. It records a dismissal in the vault (both entries stay untouched) and that pair never comes back. |
| The window disappeared but the app is still using the exe (a rebuild or delete fails with *"failed to remove file GoonFlow.exe"*) | A hidden instance is still running — that is the panic-key state: a short press locks **and hides** the window. Run the app again: it reveals the existing window instead of starting a second copy (or press the panic key again), then close it with the window's X or **Lock**. |
| The panic key does nothing when pressed | Look at the top bar: a red **panic key NOT armed** pill means the combination could not be registered (another program, or an older GoonFlow instance, already owns it). The strip under the bar says exactly what conflicted; close it or set a different key under Sources → Panic & privacy. |
| An old item shows site "i" or "wimg" | Fixed automatically: on the next unlock, a schema (v3) migration renames those mislabeled sources to their real site (`4chan`, `rule34`, …). |
| White/blank window at launch | WebView2 runtime missing — install "Microsoft Edge WebView2 Runtime". |
