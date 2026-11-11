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
hit the real sites. The f95 ones need your own cookie in the environment, never
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
| `tools\smoke-release.ps1` | release exe starts, companion answers on `127.0.0.1`, `/v1/status` handshake, window is findable |
| `tools\verify-artifact.ps1` | the sha256 of the exe + the git tip it came from, then the smoke and the three checks below, so an artifact can be re-verified as a unit |
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
9. Sources rates (#28): enable a site that was never touched (e.g. e621) → its ms
   box shows that site's default (1200), not a shared 700; e-hentai stays 10500.
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
17. Media memory (#44): hold `→` through a hundred images and watch the GoonFlow
    working set in Task Manager - it should plateau near 200 MB, not climb; press the
    panic key mid-load, tap again, re-unlock → the viewer refetches and shows nothing
    from the previous vault session.
18. f95 and e-hentai art (#46 #50): with the f95 cookie set under *Sources*, *Re-fetch
    metadata* on a thread stores the post-#1 cover and the note names the attachment
    count (and says so if the cover is AVIF and cannot be thumbnailed); an
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

## 4. House rules that prevent whole classes of damage

- **Never edit non-ASCII docs/UI text through PowerShell.** This machine's ANSI
  codepage is Shift_JIS, so `Get-Content -Raw` / `Set-Content` mangles BOM-less
  UTF-8 (em dashes become ` E`, newlines get swallowed). Use a node script or the
  editor's tools; audit with `node tools\char-audit.js`, repair with
  `node tools\restore-nonascii.js`.
- Nothing vault-shaped may be committed: `flow.cfg`, `chrome.cfg`, `*.db`,
  `tools\*.log` are gitignored — keep it that way, and never print cookie or
  passphrase values into logs or issue comments.
- `gh` needs `C:\Program Files\Git\cmd` on PATH; `npx` only works through
  `cmd /c` (the `.ps1` shim will not load); `cargo` only after `tools\dev-env.ps1`.
- A finished increment means: tests green → commit → push → comment on the issue
  with what was measured → move the board card.
