# Testing and verification

Design decisions are in [DESIGN.md](DESIGN.md), the build recipe and code
invariants in [DEVELOPMENT.md](DEVELOPMENT.md), and *what is finished* on the
[project board](https://github.com/MoonerGan/TheGoonFlow/projects/1). This file is
how any of it gets proven.

## 1. Automated, every increment

```powershell
powershell -ExecutionPolicy Bypass -File tools\dev-env.ps1        # MSVC + perl + nasm on PATH
powershell -ExecutionPolicy Bypass -File tools\run-test.ps1 -tag mNN   # cargo test --workspace -> tools\cargo-test-mNN.log
cmd /c "cargo fmt --all -- --check"
cmd /c "cargo clippy --workspace --all-targets"
cd apps\desktop
cmd /c "npx svelte-check --tsconfig ./tsconfig.json --output human"   # must be 0 errors AND 0 warnings
cmd /c "npm run build"
node tools\char-audit.js <every file you touched>                     # encoding guard
node tools\lost-calls.js apps\desktop\src\components\ItemEditor.svelte <rev before> HEAD
```

`lost-calls.js` exists because a dialog rework can pass every compiler and still
drop a button: it diffs the `await someApi(...)` calls and `onclick={handler}`
buttons of one file between two revisions and fails if either shrank. Renamed
handlers show up as a lost+new pair, which is the honest result - read it.

What the Rust suites actually cover (`DEVELOPMENT.md` §5 lists the per-crate
breakdown): vault key handling and rejection, locator resolution against a temp
vault, schema idempotency and migrations, media put/dedup, importer engine
fingerprints, the Netscape bookmark parser, `ingest::classify` for every URL
shape, the HTML parsers against fixtures, and the job registry (progress, cancel,
bounded history). Parsers are parse-only — no network in tests, by design.

The exceptions are `#[ignore]`d on purpose and run by hand: `tools\` drives the
real exe (below), and inside cargo `match_live_search_smoke`,
`first_frame_live_probe`, `f95_live_art_probe` (the live half of #46),
`live_f95_markers_separate_member_from_guest_under_our_ua` (the live half of #38)
and `f95_live_clip_rehearsal` (#52/#54: clips the live thread into a scratch
encrypted vault through the real `clip()` and then reads the vault rows back)
hit the real sites. `first_frame_live_probe` (#16) needs `GOON_FF_PATH` pointed at
any real video file and **skips** without it — it is the only one of the five that
depends on a file on this machine, so a whole-`--ignored` run stays readable. The
f95 ones need your own cookie in the environment, never
in the repo — and they need the dev env, because `dav1d-sys` builds the AVIF
decoder (#52):

```powershell
$env:GOON_F95_COOKIE = (Get-Content "$env:USERPROFILE\Downloads\f95cookie.txt" -Raw).Trim()
powershell -ExecutionPolicy Bypass -Command ". tools\dev-env.ps1; cargo test -p goonflow-desktop --lib -- --ignored --nocapture f95_live_art_probe"
powershell -ExecutionPolicy Bypass -Command ". tools\dev-env.ps1; cargo test -p goonflow-desktop --lib -- --ignored --nocapture f95_live_clip_rehearsal"
```

Last run, 2026-11-11 (same thread as #46, cookie refreshed): title parsed,
**15** lazy attachment images with the cookie, **0** as a guest (the login-wall
note instead), the first attachment 59 478 bytes and sniffed `image/avif`, and
the note reads *"cover stored from post #1 as image/avif; 15 attachment image(s)
on the page — they stay links, only the cover was downloaded"* — the AVIF now
decodes into a thumbnail instead of announcing a missing decoder (#52). The
#38 probe in the same run: `guest says signed-in = false, cookie says signed-in
= true`.

The rehearsal is the one that found a bug no offline test could see (#54): it
prints what the **vault** ends up holding, not what the report claimed.

```
clip  = created true stored_image true thumb true tags 0 images_found 15
vault = full 59 478 bytes mime image/avif (sniffed image/avif)
vault = thumb 33 144 bytes mime image/jpeg
grid  = 1 game row(s) for site f95, this one has_thumb
guest = created true stored_image false thumb false images_found 0
        note "f95 answered, but this page shows its art to members only …"
```

Two facts worth remembering from it: an f95 clip lands in the **Games** grid
(`kind = 'game'`), not Media — filtering the wrong kind is how a tile looks
missing — and it stores **0 tags**, because nothing in `f95_thread` reads the
thread's tag list yet.

Release build: `powershell -ExecutionPolicy Bypass -File tools\run-release.ps1
-tag mNN` (runs `tools\dev-env.ps1`, stops a live GoonFlow first, records the
exe's mtime/bytes/sha256 in `tools\last-build-summary.txt`). The frontend is
embedded by `build.rs`, so `target\release\GoonFlow.exe` is the whole app.
Then `tools\verify-artifact.ps1` runs section 2 against that exact exe.

**Only one release build at a time, and read the summary before quoting it.** The
wrapper holds `tools\last-build.lock` and aborts with exit 2 if another build
holds it; it also prints `build ran to completion: yes|NO` (derived from cargo's
own `Finished \`release\` profile` line in the log) and `exe rebuilt: yes|no|new`.
That ceremony exists because a run once reported `exit=0 sha=<previous build>`
for a build that never started — the log file was still held by the previous run,
`Out-File` threw, and the script went on hashing the old exe. If `build ran to
completion: NO`, the sha in that summary is someone else's artifact; quote
`Get-FileHash target\release\GoonFlow.exe` instead.

## 2. Live suite — real windows, real hotkeys

These drive the actual exe with Win32 calls, because the panic path and the
single-instance behaviour are the two things unit tests cannot cover.

| script | what it proves |
|---|---|
| `tools\webview-embedded-test.ps1` | the window shows the UI **inside the exe**: it opens a WebView2 DevTools port and reads the page it really loaded, so `url = http://tauri.localhost/` + the app's title pass and `http://localhost:<port>` (a vite dev server, `ERR_CONNECTION_REFUSED` on screen) fails the artifact; it also insists the hashed asset names in `dist\index.html` are inside the binary, so yesterday's UI cannot ship as today's |
| `tools\smoke-release.ps1` | release exe starts, companion answers on `127.0.0.1`, `/v1/status` handshake, window is findable |
| `tools\verify-artifact.ps1` | the sha256 of the exe + the git tip it came from, then step 0 (the row above) and the smoke and the checks below, so an artifact can be re-verified as a unit |
| `tools\close-exit-test.ps1` | closing the window really exits; panic-tap hides and leaves the process alive; a second launch hands off to the running instance and reveals its window, leaving one process (D19) |
| `tools\panic-tap-test.ps1` | tap hides the window, tap again brings it back, jobs stop |
| `tools\panic-hold-test.ps1` | holding 3 s escalates to the crypto-erase path |
| `tools\panic-stale-test.ps1` | a stale erase marker from an old run does not fire (and the watcher really notices a fresh one) |
| `tools\window-probe.ps1` | enumerates every window a running instance owns, with styles + pixel proof — how the 16×16 hand-off window was shown to paint nothing |

Safety rules for this group:

- **Hold-erase destroys a vault.** Run `panic-hold-test.ps1` /
  `panic-stale-test.ps1` against a *scratch copy* of the exe in its own folder with
  no `flow.cfg` beside it, so no real vault is attached. The scripts take `-Exe`
  and `-Dir` for that reason.
- Kill leftover instances before rebuilding (`Get-Process GoonFlow | Stop-Process
  -Force`) — a running instance holds `target\release\GoonFlow.exe` open and cargo
  fails with `failed to remove file`.
- `Ctrl+Alt+L` is the real hotkey: these scripts send real keystrokes, so do not run
  them while typing something else.

## 3. Manual field-test checklist

The order below surfaces problems fastest. Each item names the issue it covers;
the board's *In review* column is the current list.

1. Unlock the real vault → schema lands at the current version, old items intact.
2. Viewer (#21): click media cards — ←/→ through the grid, `+`/`−`/`0` zoom, space
   slideshow, big stored video falls back to its poster, `i` opens the editor.
3. Extension (#19): `chrome://extensions` → Load unpacked →
   `extensions/goonflow-clip`; paste port + token from *Sources → Browser pairing*;
   right-click-clip a rule34/gelbooru post → it lands and notifies.
4. Extract (#17): game tracked by a loose zip → *Extract zip…* → folder linked →
   ▶ Launch works.
5. Mods (#25): register MTool (`mtool` kind) → ▶ opens it; add a real f95 zip mod
   as `dll` kind → Apply → `.flowbak` originals + deployed chip → Revert → game
   pristine; check the ⚠ chip on an update row for a modded game.
6. Proxy (#24): `socks5://127.0.0.1:9150` with Tor Browser running → *Re-fetch
   metadata* still succeeds; Clear → direct.
7. Panic (#23): the automated part is §2; the finger pass is — from another app,
   tap `Ctrl+Alt+L` → window vanishes and jobs stop, tap again → locked window
   returns; run the crypto-erase drill on a throwaway vault; store a custom key and
   try both; tick scrub-on-lock and watch the counts.
8. Deniability (#27): copy the exe to a boring name → works; set a *Decoy title* in
   Sources → taskbar title changes at once, brand returns after re-lock; check
   `chrome.cfg` reads like nothing sensitive (D21).
9. Sources rates (#28): enable a site that was never touched → its ms box starts at
   that site's own default, not one shared number. `ingest::default_rate_ms` says
   **10 500** for e-hentai and Exhentai, **1 200** for the five repack mirrors (kimochi,
   otomi, ryuugames, nagadoki, hentaihaven) and **700 for everything else — every booru
   included**, so an e621 box reads 700. Setting one site's rate must not move
   another's, and enabling a site for the first time must not reset a rate you had
   already set (#28).
10. Layout (#29 #30 #32): shrink to the 1100×640 minimum → one scrollbar for the
    grid, sidebar scrolls on its own, long folder names wrap per character, and an
    item with many tags keeps its Save/Cancel reachable.
11. Dupes (#31): *Possible dupes* rows show `1/2` with each entry's own link/path;
    **Keep both** on an obvious false pair → it disappears and stays gone after a
    restart.
12. MTool (#33 #48): ▶ on an NW.js install → the tool opens *with the game*, not
    empty-handed.
13. Viewer (#34 #35 #42): open a picture, start an import → no flicker; `f` fills
    the screen, first Esc leaves fullscreen, second Esc closes the viewer.
14. File links (#39) + queues (#40) + batch (#36): import a browser bookmark
    export → bare booru image links store the picture (with a *no post could be
    identified* note where that is the case); sidebar *Sources → No tags* lists
    exactly those; select all → **Re-fetch metadata** fixes what the sites can
    identify, and Stop works mid-run.
15. Hidden instance + unarmed key (#41): panic-tap to hide, then run the exe again →
    the same window returns and Task Manager still shows one GoonFlow; close it with
    the X and confirm the process is gone; if another program owns `Ctrl+Alt+L`, the
    top bar shows the red *panic key NOT armed* pill and says why.
16. Item dialog (#43 #45): open a game → it opens read-only with collapsed sections,
    counts in the headers; *Edit* turns fields into inputs, changing Notes then *Save*
    returns to view mode showing what the vault holds; leaving with unsaved edits asks
    first; in a 560 px window nothing scrolls sideways and Save stays reachable.
    #43's second round: in Edit mode **Cancel** leaves the form and keeps the entry open
    in view mode - it must not close the dialog - and it still asks before discarding a
    field you changed. In the Sources section a 120-character url must not be laid over
    its own *Store* / detach control: the row wraps instead.
17. Media memory (#44): hold `→` through a hundred images and watch the GoonFlow
    working set in Task Manager - it should plateau near 200 MB, not climb; press the
    panic key mid-load, tap again, re-unlock → the viewer refetches and shows nothing
    from the previous vault session.
18. f95 and e-hentai art (#46 #50 #54): with the f95 cookie set under *Sources*, *Re-fetch
     metadata* on a thread stores the post-#1 cover **and** its tile, and the note names
     the attachment count; an
    `e-hentai.org/s/...` link still finds its gallery; a gated page says *members only*
    instead of "carried nothing new".
19. Batch counters (#47 #49): start an extract or a batch re-fetch and watch the job
    row - `done` climbs while files appear, and the *Possible dupes* list holds no
    `mmnz-game` pairs.
20. Cookie truth (#38): *Sources* has a cookie box only for f95, E-hentai and
    Exhentai (no box anywhere else). Paste something with no `xf_session` → the app
    refuses it, names the missing key, and the vault keeps the cookie it had. Paste
    a live one → the line under the box says the site answered as a signed-in
    member, with the time. Press **Check** after logging out in the browser → that
    line goes red and says the site answered as a guest. Clip an f95 thread while
    the cookie is dead → the clip report strip says *members only* in the site's own
    words (the reason belongs to the report, not the item: `items.notes` is your own
    notes field and no fetch writes into it) and the Sources line marks the cookie
    stale without being asked.
21. AVIF thumbnails (#52): an f95 item clipped **before #52** (m13 or older) has no
    tile while the viewer still shows the real art — the vault always held the
    picture, only the tile was missing. **Re-fetch metadata** on it → the tile
    appears, because f95 serves its cover as AVIF and dav1d now decodes it. Clip a
    *new* f95 thread link → the tile is there without asking. A HEIF/HEIC file still
    stores fine and still says out loud that this build cannot thumbnail that format.
22. Full cover, not only the tile (#54): an f95 item clipped with **m14 or m15** kept
    only the 360 px tile, so its viewer showed a soft, upscaled version of the cover.
    **Re-fetch metadata** with this build → the report strip says *image stored,
    thumbnail made* and the viewer shows the real preview-size art (59 478 bytes of
    AVIF in the measured thread). Items clipped before #52, or clipped with this
    build, already hold the original.
23. Login-walled fetches mark the cookie that failed (#55): paste an e-hentai cookie
    under *Sources* (and later make it dead, or wait for the session to expire). Clip an
    `e-hentai.org/g/…` link → the item says the gate was hit **and** the Sources line
    for e-hentai turns red by itself, with the time. Exhentai keeps its own separate
    verdict: one site dying must not mark the other stale, and a site with **no** cookie
    saved is never shown as refused (there is no cookie there to be stale). f95 has worked
    this way since #38; the EH pair did not until now.
24. Search privacy (#56): with no f95 cookie saved, *Find online matches* must not
    contact f95zone.to at all — the report carries one line, *f95: search skipped — no
    login cookie saved (Sources)*, and the run is faster because no f95 lane slot is spent
    on a request that cannot work. With a cookie, f95 suggestions appear (live probe:
    `f95 hits with cookie: 3`). A refused cookie now says so differently from having none.
25. Clips obey the politeness lane (#57): set e-hentai's rate high (its default is
    10 500 ms) and clip two gallery links in a row → the second waits roughly 10 s before
    the site is touched at all, instead of firing immediately while a re-fetch job queues
    politely behind it. Press **Lock** during that wait → no item appears and the strip
    says *the vault was locked before ehentai could answer — the link was not clipped*.
    A **first** clip to a quiet site never waits, and clipping the same link twice costs no
    lane slot (the dedup answers before the queue is asked).
26. A clip is a job now (#58 #60): *+ Clip URL* on a slow site (e-hentai at its 10 500 ms
    default) → the strip gains a `clip: <url>` line with a **Stop** button while it waits.
    Clip a second link from the browser extension during that wait → the app answers *a clip
    is still running - the activity strip shows where it got to* instead of fetching twice.
    Press **Lock** during the wait → the `clip:` line stops and no item appears (that is
    #57's lane giving up). The labels must read `clip:`, `match search:`, `video store:`,
    `extract:` - never `updates:` for something that is not an update check (#60).


 27. e-hentai with **no cookie saved at all** (#46): clip a gallery link of the shape
     `https://e-hentai.org/g/<gid>/<key>/` → the cover arrives even though the page names
     no `<img>` any more. It names a **sprite sheet plus a CSS box**, and that box is cut
     out of the sheet (measured 2026-11-14: a 250x131 cover out of an 11 422 B `ehgt.org`
     sheet, 20 tiles at 200x105/200x141 out of a 4000x300 `*.hath.network` sheet). The
     title, 16 tags and the 20-page count arrive, and Sources lists one link per page.
     Clip a `/s/<imgkey>/<gid>-<n>` slideshow link instead → the gallery's **key is read out
     of that page**, because `https://e-hentai.org/g/<gid>/` without the key is a bare
     apache 404 (236 B) now - that 404 was the whole of #46 - and the page's own art is
     stored (measured `200 image/jpeg 177 497 B` for page 1, `image/webp 76 300 B` for
     page 7, with or without a referer, with or without a cookie).
 28. A rule34/gelbooru **file** link (#50), e.g.
     `https://wimg.rule34.xxx/images/3043/a6cf….jpeg?3411909` → the picture stores, and the
     note must say the tags could not be read: that site's metadata api answers a
     Cloudflare bot check (measured `403 text/html` plus `cf-mitigated: challenge` for this
     app's honest UA, for a browser UA and for no UA at all, while the file host answers the
     same link at `200 image/jpeg 678 634 B`). A rule34 **post page** still goes to the api,
     and e621 keeps its api path - its api and its file host both answer 200.
 29. Sources, cookie controls (#38): **Set** checks a paste before storing it, **Check**
     re-asks the site exactly one page, **Clear** takes the paste back out - and the rate
     box and the on/off switch must survive that. What the sites answer decides the
     sentence: e-hentai's marker is `favorites.php` (a guest is bounced to
     `bounce_login.php?b=d&bt=1-6`, a member gets 11 496 B of page, three times in a row),
     so a good cookie is no longer accused by the `/g/` 404 the site now answers to
     *everybody*; exhentai answers `200` with **0 bytes** to a real cookie, to a made-up
     `ipb_pass_hash` and to a real member id with that bad hash alike, so its verdict says
     the site could not tell them apart and the row stays un-green instead of claiming a
     working session.
 30. Viewer, the parts that used to move on their own: open a media card whose tile is
     small and whose full copy is much larger → the filmstrip and the control bar must
     not jump when the full picture lands (the media sits in a box that already fills
     the stage). Press `a` (or **Actual**) → the picture shows at its own pixels and the
     button's tooltip names them (`this frame is 1200x630`); `0` fits it again. A video
     that exists only as a link, or as a file in a folder, says *"kept as a link or as a
     path on this machine… not played here yet"*; a video stored in the vault past 96 MB
     states its size; and neither sentence says "open it from its source link", because
     for those items there is nothing to open.

## 4. House rules that prevent whole classes of damage

- **Never edit non-ASCII docs/UI text through PowerShell.** This machine's ANSI
  codepage is Shift_JIS, so `Get-Content -Raw` / `Set-Content` mangles BOM-less
  UTF-8 (em dashes become ` E`, newlines get swallowed). Use a node script or the
  editor's tools; audit with `node tools\char-audit.js`, repair with
  `node tools\restore-nonascii.js`.
- Nothing vault-shaped may be committed. `.gitignore` covers `flow.cfg` (the plaintext
  pointer beside the exe), `chrome.cfg` (the plaintext decoy title, also beside the exe),
  `goonflow-vault/`, `*.db` / `*.db-wal` / `*.db-shm`, `*-journal`, `*.log`, `tools/*.log`
  and `tools/last-build.lock`. Check with `git check-ignore -v <path>` rather than trusting
  this sentence, and never print cookie or passphrase values into logs or issue comments.
- `gh` needs `C:\Program Files\Git\cmd` on PATH; `npx` only works through
  `cmd /c` (the `.ps1` shim will not load); `cargo` only after `tools\dev-env.ps1`.
- A finished increment means: tests green → commit → push → comment on the issue
  with what was measured → move the board card.
