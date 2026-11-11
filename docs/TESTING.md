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
```

What the Rust suites actually cover (`DEVELOPMENT.md` §5 lists the per-crate
breakdown): vault key handling and rejection, locator resolution against a temp
vault, schema idempotency and migrations, media put/dedup, importer engine
fingerprints, the Netscape bookmark parser, `ingest::classify` for every URL
shape, the HTML parsers against fixtures, and the job registry (progress, cancel,
bounded history). Parsers are parse-only — no network in tests, by design.

Release build: `cmd /c "cargo build --release --locked"` from a shell that ran
`tools\dev-env.ps1`; the frontend is embedded by `build.rs`, so
`target\release\GoonFlow.exe` is the whole app.

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
