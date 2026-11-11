# The Goon Flow

A local-first, open source tracker + manager for adult games and imageboard content.
Browse freely, clip anything, keep one organized flow across sites — encrypted at rest,
nukeable in one action, zero cloud, zero telemetry.

**Status:** M0–M5 shipped and in daily use; what lands next lives on the
[project board](https://github.com/MoonerGan/TheGoonFlow/projects) with the
manual checks in [docs/TESTING.md](docs/TESTING.md). Everything below already works.

- **Users:** start at [docs/USER-GUIDE.md](docs/USER-GUIDE.md)
- **Developers:** see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)
- **Why it is built this way:** [docs/DESIGN.md](docs/DESIGN.md) — decisions D1–D22

## What it does today

- **Portable encrypted vault** — one folder you place (USB / external drive / beside
  the exe), found by volume label. Argon2id → wrapped data key → SQLCipher. Wrong
  passphrase fails closed. Nuke crypto-erases key material then shreds every file.
- **Games** — track by folder, zip or exe; engine fingerprinting; zip → game folder
  extraction (zip-slip guarded); one-click launch with launcher resolution; an update
  feed (f95 / DLsite) with an "update wipes your mods" warning; duplicate pairing and
  online-match suggestions.
- **Mods & tools (launch recipes)** — MTool / Cheat Engine / any patcher per game;
  DLL/patch mods Apply into the game folder with `*.flowbak` backups and one-click
  Revert. The app never injects or patches anything itself.
- **Media** — clip a URL (gelbooru / rule34 / e621 / e-hentai / exhentai / 4cdn…),
  or import image batches and bookmark exports; thumbnails generated locally; a full
  viewer with arrows, zoom and slideshow; videos stay links by default.
- **Browser extension (MV3)** — right-click "clip to the flow" into a token-authed
  companion API on 127.0.0.1. Traffic never leaves the machine.
- **Hardening** — global panic hotkey (`Ctrl+Alt+L`: press = lock + hide; hold 3 s =
  crypto-erase), crypto-erase *drill* against a throwaway vault, OS-side scrubber
  (web session + Explorer thumbnail caches), proxy/Tor for all fetches, and
  deniability: rename the exe freely, set a decoy window title, brandless locked
  screens.

## Building

Prerequisites: Rust (MSVC toolchain), Node 18+, Tauri 2 system deps (WebView2, VS
Build Tools C++ workload).

```sh
npm install          # in apps/desktop
npm run tauri dev    # hot-reload dev app
npm run tauri build  # single standalone release exe + installers
```

The field-test artifact is the standalone `target/release/<name>.exe` — frontend
embedded, nothing installed, safe to run from a USB stick.

## Testing

```powershell
# cargo on Windows needs the MSVC + dav1d environment; these wrappers source
# tools\dev-env.ps1 for you and leave a log + summary file behind as evidence
powershell -ExecutionPolicy Bypass -File tools\run-test.ps1 -tag mNN
powershell -ExecutionPolicy Bypass -File tools\run-clippy.ps1 -tag cN
cd apps/desktop; npx svelte-check          # frontend types
```

Do **not** type `cargo test --workspace` in a bare shell on Windows: SQLCipher +
OpenSSL need the MSVC tools and, since #52, `dav1d-sys` needs the static dav1d
that `dev-env.ps1` points at. Without it cargo dies inside a C build script with
a pkg-config stack trace that says nothing useful. The wrappers above are the
entry point; `dev-env.ps1` prints `dav1d=static` (or `dav1d=missing` and the
three fixes) on its first line.

Plus committed live checks against the release exe: `tools/verify-artifact.ps1`
runs them as a set — `smoke-release.ps1` (handshake on the real vault),
`close-exit-test.ps1` (close exits, single-instance hand-off) and
`panic-tap-test.ps1` / `panic-hold-test.ps1` / `panic-stale-test.ps1` (the panic
hotkey state machine, safe-by-construction).

## Privacy & legal

No telemetry, no cloud calls, works fully offline. Metadata and links you choose to
store stay on your disk, encrypted. You are responsible for what you download and for
your local laws; this tool stores what you tell it and nothing else.

MIT licensed — see [docs/DESIGN.md](docs/DESIGN.md) §2 (D5) for the rationale.

**Third-party code inside the exe.** The release binary statically links **dav1d**
(BSD-2-Clause, VideoLAN) as its AVIF decoder, plus the Rust bindings `dav1d-sys`
(MIT) and `mp4parse` (MPL-2.0). Static linking is a deliberate choice — the
artifact stays a single file with no DLLs beside it — so the attribution lives
here instead of in a redistributable you could inspect. `cargo tree` names every
other crate and its license; the license texts ship in each project's repository.
