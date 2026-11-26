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
     the stage). **Fit** (or `0`) puts the whole picture inside the window and **1:1** (or
     `a`) shows it at its own pixels; whichever one you are on is the highlighted
     button, and the number beside them is the picture's scale against its own pixels
     (`6000×4000 px, shown at 20%`). Item 31 below is the version of that sequence the
     2026-11-15 report asked for. A video
     that exists only as a link, or as a file in a folder, says *"kept as a link or as a
     path on this machine… not played here yet"*; a video stored in the vault past 96 MB
     states its size; and neither sentence says "open it from its source link", because
     for those items there is nothing to open.

 31. Viewer, the numbers (#62, 2026-11-15 report: "Actual is always `NaN%`"). Open a
     large picture — a 6000×4000 PNG import is the measured case → the bar reads the
     honest scale (`20%` in a normal window), **not** `100%`, and the info line spells
     it out: `6000×4000 px, shown at 20%`. Press **1:1** (or `a`) → the readout goes to
     `100%`, the picture grows until one picture pixel is one screen pixel, and the
     **1:1** button is the highlighted one. Press **Fit** (or `0`) → back to the first
     number and **Fit** is highlighted instead. `+` / `-` keep working past 1:1 for a
     frame that needs it. Nothing in this sequence may ever print `NaN%`, including
     while the full copy is still loading behind its thumbnail. Item 35 is the same
      sequence, run by a script against the real exe.
 32. Viewer, ordinary pictures (#62, same report: "almost every image shows *this
     build has no decoder for it*"). An ordinary `.jpg` / `.png` / `.webp` / `.gif`
     frame shows **no** such sentence in the stage at all, and no toast either: the
     false note came from a branch that tested `shown`, which is the frame's metadata
     row and is always present. A frame that genuinely cannot be shown says so as a
     **toast** in the corner that disappears by itself, and the picture's space is
     never reduced by a sentence — the stage text is an overlay, so a 6000 px frame is
     still laid out across the whole stage with a note floating over it.
 33. Sources, the same row again (#38, second kick-back). At a normal window width
     every site row shows: name / **enabled** / rate box on one line, then the cookie
     paste box with **Set · Check · Clear** beside it on **one** line — never stacked —
     and the cookie verdict on **its own line below**, never across the buttons. The
     paste box never collapses below about 16 rem (≈256 px) no matter how long the
     verdict sentence is, and hovering the box shows that verdict as a tooltip. Resize
     the window narrow: the row re-wraps, the buttons stay on one line and the box
     keeps at least 10 rem. Press **Check**: the answer appears both under the box
     (it stays) and as a toast (it leaves).
 34. One toast system for the whole app (#62's "this should apply for almost all
     warnings"). Store a picture from an item's attachment list → the corner says
     `stored image/webp (240 KB)` and vanishes on its own; so do *Clear* on a cookie
     (`… cookie forgotten`), a *Keep both* on a dupe pair, an update check, a match
     search, a scrub report, a refused launch and a mod that would not start. None of
     them may leave a line of text wedged between the controls it came from. What
     **must** stay written in the panel: a site switched off, *panic key NOT armed*, a
     cookie's per-site verdict, and a value the editor refuses. Press the panic key or
     **Lock** while a toast is up → the toast is gone with it, like the media cache.

  35. Viewer, **Fit** and **1:1** measured rather than assumed (2026-11-22 report:
      "Fit still doesn't make the image actually fit to the screen size… certain images
      show both as selected even though the image resolution definitely doesn't fit").
      The cases are the ones `tools\viewer-layout-probe.js` generates, imports and
      clicks for real — **2400×1600**, **1200×3000** and **600×400** PNGs in a 1380×860
      window. The two large ones must open *contained*, with the readout at their real
      scale (`45%` and `24%` in that window) and **Fit** as the only lit button; **1:1**
      must draw them at exactly 2400×1600 and 1200×3000 (`drawn/own 1.000`, bar `100%`)
      and leave the stage scrollable to **every** part of the frame — top edge and left
      edge reachable, not clipped behind a centred zoom. **Fit** returns them to the
      opening size in one press. The 600×400 picture is *smaller* than the window, so
      **Fit** has to enlarge it: it opens at `168%` filling the stage height —
      `1006×670` drawn in the `1348×670` box that measurement had — **Fit** the only
      lit button, and the bar saying
      `600×400 px, shown at 168% · Fit is enlarging it to fill the window`. Pressing
      **1:1** draws it at exactly 600×400 — smaller than the stage, `100%`, **1:1** the
      only lit button — and the bar adds `· Fit would be 181%`, quoted against the
      `1348×723` box that measurement had (the stage had 53 px more of itself at
      that moment, and Fit is always quoted against the box as it is then), so the
      difference between the two buttons is stated rather than implied. Nothing in this
      sequence may print
      `0×0 px` (the size was read off a detached `ImageBitmap`) or `zoom 100%` (the
      transform standing in for a scale the app did not know).
   36. The same buttons on **the material a real library holds**, not generated fixtures
      (2026-11-23 report, #65: the gelbooru jpeg named there "still has the same bug… it
      also seems to recur with anything animated and some e-hentai images").
      `tools\viewer-real-media-probe.js` clips the links in
      `tools\viewer-real-media-cases.json` (gitignored, see DEVELOPMENT §5) plus a stored mp4
      into a throwaway vault and measures every item at two window sizes; run it by hand
      (cookie-gated hosts read `GOON_F95_COOKIE` / `GOON_EH_COOKIE`). What it must show, in a
      1380×860 window with a 1348×765 box:

      - **1200×850 jpeg** — opens at `90%` filling the height (`1079×765` drawn), and at
        `130%` / `1559×1105` in a 1920×1200 window. Before the fix: drawn 1200×850 in the
        big window, 77% of the box, Fit and 1:1 both lit, pressing Fit doing nothing.
      - **176×256 gif** — opens enlarged to fill the height (`299%`, `526×765` drawn,
        still animating), and **1:1** drops it to exactly 176×256. Before: 176×256 at Fit,
        36% of the box; `431%` / `759×1105` in the big window.
      - **stored mp4** — the bar prints its own `1280×954`, read from `loadedmetadata`,
        and Fit contains it inside the box (`80%`, `1026×765`). The tile that stood in for
        it while the file decrypted must never be described as the picture: before, the
        bar read `360×268 px, shown at 100%` above a clip laid out **past the bottom of
        the stage** (954 px in a box with 712 px), with Fit, 1:1 and ± doing nothing at
        all.
      - **e-hentai gallery page** (`image/webp`, 1280×1811) — shrinks to fit (`42%`,
        `540×765`), reaches exactly its own pixels at 1:1, `full-res in vault` in the
        info line.

      For every item the info line’s `N×M px` must equal the frame actually on screen,
      ± and 1:1 must be enabled for a gif and a video, and nothing may spill out of the
      stage while Fit is selected. `--reuse` re-measures a vault an earlier run filled.

      The run on 2026-11-23 covered 12 items × 2 window sizes and printed
      `VERDICT: 0 of 24 cases broken`. Six of those measurements had **no picture on the
      stage at all** — two 18–19 MB gifs and a video link, none of which the app stores — and
      the probe reports those separately instead of failing them, because the stage says
      so in words (`nothing viewable is stored for this item … Press *Store* under
      Sources`). That storage gap is #72 (the 16 MiB cover-art ceiling applied to booru
      originals) and #64 (videos), not a geometry bug: an item whose bytes were never
      stored must show the sentence, must not print a pixel size, and must leave the zoom
      buttons disabled with the readout at `?`.

   37. **A linked video file can be stored in the vault** (#64, 2026-11-25: the
      report was `http 403 Forbidden` on a rule34 `.mp4` link that plays in a
      browser). Press *Store copy in vault* on an item whose source is a direct
      file link: it must store and report its size, or, if the host really
      refuses, the message must name the host that answered, the status, and where
      a redirect took the request — never a bare `http 403 Forbidden`. To check the
      wording without a real download, point the source at a local stub that
      answers `200` with `text/html` (rule34's real answer to a wrong referer is
      exactly that, behind a `302` to `hotlink.php`): the store must refuse,
      say it was a web page rather than a video, and leave **no** `kind=video` row
      holding the CAPTCHA. The replay lives in `http::live_probe` (ignored, live):
      `store_video replay, referer https://rule34 -> 403` while the URL's own origin
      and `https://rule34.xxx` both give `200 video/mp4 59 083 373 B`.
   38. **A picture is sized against the right ceiling** (#72, 2026-11-25 report:
      ordinary booru gifs of 19 185 473 and 18 910 422 bytes were refused outright,
      leaving the item with no bytes *and* no tile). Clip a booru post whose file is
      a big gif: it must store, and the tile must exist. The regression is asserted
      in `sniffed_bytes_not_the_url_decide_what_is_stored`: the same 19 MB gif is
      accepted as the artwork (`media::MAX_MEDIA_BYTES`, 96 MB) and still refused as
      **cover art** (`MAX_COVER_BYTES`, 16 MB). A refusal must name the size, the
      limit and what to do instead (`is 18 MB, over this app's 16 MB limit for cover
      art`, `… save the file and add it with + Images`), never a raw byte count.

   39. **Schema v10: an old vault upgrades without losing one tag** (#76 stage
      S1). Unlock a field-test vault made before this change: the grid holds the
      same items, every item's tag chips are the tags it had (the rebuild reuses
      the old tag ids, so `item_tags` never moves), the schema is at v10, and
      unlock must not stall — the upgrade reads no stored file at all, so a big
      vault is as quick as a small one. The
      automated trio that pins it, measured 2026-11-26 with `cargo test
      --workspace` = **146 passed, 0 failed, 8 ignored** (goon-core 40, desktop
      100 + 8 ignored, vault 4, locator 2):

      - `v10_rebuild_keeps_links_folds_names_and_backfills_facts` builds a
        v9-shaped vault by hand (6 tags, 6 item-links, 4 media rows whose bytes
        are **real files** — a 3-frame gif, a still PNG, a 6-byte gif stub and a
        tile), runs the migration twice, and asserts all **6** links still point
        at the tags they used to; `female:big_breasts` folds to `big_breasts`
        while `foo:bar` folds to **itself** (an unknown prefix is part of the
        tag); **0** rows gained a site (the backfill never invents provenance,
        invariant 21); `f95-completed` became `site_flag`; every stored row came
        back with `bytes` equal to its own plaintext length and **every** fact
        that needs the file still NULL — the upgrade reads no file, so the
        pre-v10 `0 px` row became NULL rather than a size `width<400` would
        match. Then the facts pass runs: `refine_stored_facts(&conn, 2)` returns
        `scanned 2, dims 2, animated 1, still 1` and writes `4x4 / animated=1`
        for the gif and `8x8 / animated=0` for the PNG; the next call scans the
        1 unreadable row and leaves it NULL; the third call scans **0** rows,
        which is the resume marker working; and the tile row never gained an
        animation fact.
      - `encrypted_media_length_equals_the_plaintext_length` runs the very
        statement the backfill uses (`bytes = length(data)`) inside a **real
        SQLCipher vault** on a body of **1 048 613 bytes** (1 MiB + 37, many
        pages deep) and gets **1 048 613** back, and the BLOB round-trips byte
        for byte. Every `size>` query depends on that being true.
      - `animation_comes_from_the_container_chunks` proves `animated` is read
        from the file rather than its name: `image`'s own encoder gives
        `Some(false)` for a 1-frame gif and `Some(true)` for a 2-frame one; a PNG
        with an `acTL` chunk and a WebP with an `ANMF` chunk are `Some(true)`; an
        mp4 is **`None`** — not probed, which is not the same as "not animated".

      What a user notices before stage S7 runs the facts job: `size>` works for
      everything (byte lengths come from the migration, free), while `width>`,
      `type:gif` and `length>` only match items stored from v10 forward. That is
      the honest state, not a bug to paper over — the pass costs about the time
      it takes to read the library once (measured **185 ms** for 1 000 rows at
      200 KB, ~1 GB/s), so it runs as a job with progress, never inside unlock.

   40. **The query box is understood before it is ever run** (#76 stage S2). Each
      worked example in `docs/plan/76-plan.md` §4 parses to the AST the plan
      promises, and the same set **prepares and binds as real SQL against the
      real v10 schema**, so stage S3 has nothing left to discover. Pinned by the
       `goon-core::query` tests, measured 2026-11-26 and again after the S2
       review pass described below, with `cargo test --workspace` = **209
       passed, 0 failed, 8 ignored** (goon-core **102**, desktop **101** + 8
       ignored, vault 4, locator 2):
      ignored, vault 4, locator 2):
      - one test per worked example, all **14** (`example_01…example_14`), plus
        `every_placeholder_has_exactly_one_parameter` over **28** queries (the `?`
        count equals the parameter count, and the parenthesis depth returns to
        **0**) and `every_example_is_sql_the_schema_can_run` (the same 28
        prepared, bound and stepped against `init_schema`'s v10 tables in
        memory). The depth check earned its keep immediately: `type:video`'s
        extension arm was one parenthesis short, which SQLite would have refused
        to prepare and no AST test would have caught.
      - the colon stays overloaded on purpose: `female:big_breasts`, `foo:bar` and
        `other:variant_set` are literal tags, and so is the `<site>:<name>` form
        (`ehentai:big_breasts`, `local:clip`) — plan §9 decision 2 deliberately
        does not reserve that shorthand, so a grammar sugar added later cannot
        break a query that exists today. Meanwhile `ext:banana` is a **predicate**
        whatever the value (`ext_is_reserved_whatever_the_value_is` — reservation
        must not depend on what a vault happens to contain), and
        `tag:"ext:something"` / `tag:"type:video"` / `tag:size` are the escape
        hatch back to a literal name that contains a keyword.
      - `a_date_is_compared_as_a_number_never_as_text` pins the trap that would
        otherwise live forever: SQLite reports `'999999999' > '1000000000'` as
        **1** and `CAST('999999999' AS INTEGER) > CAST('1000000000' AS INTEGER)`
        as **0**, and every `added:`/`updated:` plan emits
        `CAST(items.created_at AS INTEGER)` with only `Value::Int` bound. A bare
        `added:2026-11-01` is the **UTC day** `[1793491200, 1793577600)`, not one
        second; `added:>=2026-11-01` binds the day's start; `added:>2026-11-01`
        binds the next day's start; `2026-02-29`, `2026-04-31` and
        `2026-11-01T25:60` are refused rather than quietly normalized.
      - facts never measure a tile: `a_fact_never_measures_a_thumbnail` demands
        `m.kind IN ('full','video')` in every `size>`/`width>`/`height>`/`ratio:`
        fragment, and only `has:thumb` may look at the tile — the lie the code
        map's review note 1 warned about.
      - an error is a value, never a crash or a guess: `size>x` answers with
        `size>100mb`, `length>1:70` explains that minutes and seconds are 0-59,
        a lone `-` says it negates something, `"summer witch` says the quote
        never closes, `tag>big_breasts` says the keyword matches by name, and
        `status:wishlist` is refused in the **media** scope with a message naming
        Games — D14 enforced in the grammar, not only in the rails.
      - one vocabulary, not two: `ingest::VIDEO_EXTS` **is**
        `goon_core::query::VIDEO_EXTS`
        (`media_ext_follows_the_grammars_video_list` in `ingest`, which iterates
        the grammar's own list rather than a copy of it), so `type:video`
        cannot miss a file the importer called a video.
      - `an_id_predicate_is_exact_and_knows_the_real_id_shape` pins the id shape
        against the code rather than the plan: ids are `new_id(prefix)` —
        `c-1f9a2b7c11d4e5a6b7c8`, hyphen included — not the ULID plan §4 sketched,
        and an id that simply does not exist is a query matching nothing, not an
        error (only `id:cl/ip`, which cannot be an id, is refused).

      The second review pass (same day) added the half the AST tests could not
      see — what the planned SQL *selects* — so `query.rs` grew a synthetic
      fixture library (`seeded_library`, `fixture_item`, `matched`) and runs the
      planned `WHERE` against the real v10 schema:

      - **a negated fact plans the contrary comparison, not `NOT (…)`**
        (`a_negated_fact_never_counts_an_unmeasured_item`, `Cmp::opposite_sql`,
        `Term::self_negates`): `NOT (bytes > ?)` is true of an item nobody ever
        measured, so `-size>1gb` becomes `bytes <= ?` and the never-probed item
        is missed by both halves. Existence and derived predicates (`has:`,
        `stored:`, `source:`, `ext:`, a tag, a date) stay plain complements,
        because there is no unknown behind them. `type:` negation is derived the
        same way: `-type:video` wants a still row, `-type:image` wants a video
        row *or* an animated one, and a gif arm may read `kind IN
        ('full','video')` — it is not tied to the container alone.
      - **the three #40 rails are the same predicate in grammar text**
        (`the_old_source_queues_are_the_same_predicate_in_grammar_text`):
        `stored:local-only` is the old `local_only` badge word for word
        (`EXISTS sources site='local'` **and** `NOT EXISTS sources
        site<>'local'`), `-stored:disk` is `no_local`, `-has:tags` is `no_tags`,
        and the plan's `WHERE` counted by `COUNT(*)` equals what
        `db::source_stats` badges — so a preset chip cannot silently widen a
        queue the user already knows. That is why `stored:` has **four** values
        (`vault | disk | local-only | link`), and
        `closed_values("stored")` is what the error text lists.
      - a value never reaches SQL text: `every_placeholder_has_exactly_one_parameter`
        over **26** queries, and `a_value_is_bound_and_never_written_into_the_sql`
        which strips the code-owned fragments (`CODE_OWNED_DIGITS`: `SELECT 1
        FROM`, `animated = 0/1`, `height > 0`) and then insists no bound value's
        own text appears anywhere in the statement — the case the placeholder
        count cannot catch. `a_quoted_phrase_cannot_reach_the_sql_text` runs
        `"); DROP TABLE items;--"` as a phrase and checks the library still has
        the rows it had.
      - the bounds are refused, not guessed: `MAX_TERMS` **50** and
        `MAX_INPUT_CHARS` **2 048** (past which SQLite answers with a prepare
        error instead of a result, so the grammar says no first), a negative
        pixel count, a colon-form length whose milliseconds would overflow
        (`length>999999999999999:00`), a five-digit year, an epoch past
        `LAST_DATE_EPOCH`, and empty `""` / `circle:` / `tag:` values. Every
        epoch computation is `checked_mul`/`checked_add`: before this, a
        12-digit year overflowed — a panic in debug, a wrapped timestamp in
        release.
      - invisible input is caught where it is grammar: control, format, bidi,
        zero-width, en/em dash and fullwidth colon are refused **inside a term**
        (an en dash is not the `-` that negates, and that mistake is invisible),
        while inside quotes the text stays verbatim data, because a scraped
        title really does contain soft hyphens and NBSP.
      - `circle:` and `rating:` are valid in **both** views
        (`circle_and_rating_are_valid_in_both_views_because_items_carry_them` —
        the columns are on the shared `items` row and an e-hentai gallery has a
        circle), and only `status:` is Games-only
        (`status_is_games_only…`). Plan §1 said "Games get `status:`/`circle:`;
        Media never do", which contradicted §4's own vocabulary table; the
        sentence is corrected, and `keyword_hint(Scope::Media)` is generated from
        `keywords()` so a hint list can never drift from the grammar.

      What it costs, measured 2026-11-26 with a throwaway harness over 20 000
      rounds (the per-keystroke path stage S5 will use): parse+plan is **0.0032 ms**
      for one term, **0.0096 ms** for three, **0.0199 ms** for seven; typing the
      longest worked example parses every one of its **77** prefixes for **0.62 ms**
      in total. The parse is noise next to the SQL it feeds, so the suggester can
      re-plan on every debounce tick without a cache.

   41. **100k items, filtered, in milliseconds** (#76 stage S2, plan §6/§8.3).
      `cargo run -p goon-core --release --example filter_scale_probe` builds a
      throwaway plaintext vault under `%TEMP%` (**100 000 items**, 10 tags each
      ≈ **1 000 000** `item_tags` rows, 20 000 distinct tags, half vaulted, 70 %
      of those probed, the rest deliberately un-probed so invariant 27 has rows
      to hide) and prints p50/p95 for both statements every filtered grid runs:
      the page (`… ORDER BY items.updated_at DESC LIMIT 300`) and the paired
      `COUNT(*)` behind "N matches · M total". Fixture ids, titles and tag names
      are synthetic (`fixture_tag_00421`), the media BLOB is an 8-byte stub, and
      the whole thing is deleted again. It also prints `EXPLAIN QUERY PLAN` for
      four shapes, which is where the one cost the grammar knowingly accepts is
      recorded: `CAST(created_at AS INTEGER)` makes an index on the TEXT stamps
      useless, so `added:` is a **scan** of `items`. Measured 2026-11-26 in
      `tools/filter-scale-20261126.log` (release build, plaintext vault, 104 MiB,
      built in 13.9 s): the slowest grid page is the dense tag
      `big_breasts` (64 092 matches) at **197 ms p50 / 199 ms p95**, and
      `big_breasts -anal` is **190 ms**; every other shape is under **80 ms**, and
      several are under 10 ms (`stored:local-only` 1 ms, `size>100mb type:image`
      2 ms, `-size>1gb type:gif` 10 ms, the date window 0 ms). The paired
      `COUNT(*)` is the honest surprise: **86 ms** for the same tag query, and up
      to **171 ms p95** for `ratio:>2 height>800`, because a `LIMIT 300` page can
      stop early on the `updated_at` order while a total cannot. Stage S3's job is
      not to make the count fast but to stop running it on every keystroke.
      The fixture is also what makes `-has:tags` measurable: one item in twelve
      carries no tags at all, and 1 in 6 knows no extension, which is the pre-v10
      gap #77 has to close before `ext:` means what it says.

      Two numbers the log records on purpose rather than leaving to be inferred.
      `ext:gif ext:mp4` matches **0 rows by construction** - an item carries one
      `extension` tag, since `db::set_item_extension_tag` writes exactly one - so
      the conjunction the plan's example shows is a query that can never be true,
      which stage S5 must handle as a hint ("did you mean `ext:gif` **or**
      `ext:mp4`?") rather than a result. And the `EXPLAIN QUERY PLAN` lines show
      `size>100mb`, `type:video` and every `added:` window start with `SCAN items`
      plus a correlated `SEARCH m USING INDEX media_hash_idx (item_id=? AND
      kind=?)`: the plan shape is item-at-a-time with an index probe per item, so
      the cost is the **`items` row count**, not the match count. A tag query is
      the opposite shape - `MULTI-INDEX OR` over `tags_fold_idx` / `tags_name_idx`
      then `item_tags_tag_idx` as a covering index - which is why it gets *slower*
      the more items match.

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
