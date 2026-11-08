# The Goon Flow

A local-first, open source tracker + manager for adult games and imageboard content.
Browse freely, clip anything, keep one organized flow across sites — encrypted at rest,
nukeable in one action, zero cloud, zero telemetry.

**Status:** M0–M3 shipped, M4 backlog open, M5 hardening underway — the
[project board](https://github.com/MoonerGan/TheGoonFlow/projects) and
[PLAN.md §13](PLAN.md) carry the live state. Everything below already works.

- **Users:** start at [docs/USER-GUIDE.md](docs/USER-GUIDE.md)
- **Developers:** see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)

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

```sh
cargo test --workspace                    # ~81 tests: db, vault, extractors, jobs…
cd apps/desktop && npx svelte-check       # frontend types
```

Plus committed live checks against the release exe: `tools/smoke-release.ps1`
(handshake), `tools/panic-tap-test.ps1` / `panic-hold-test.ps1` /
`panic-stale-test.ps1` (the panic hotkey state machine, safe-by-construction).

## Privacy & legal

No telemetry, no cloud calls, works fully offline. Metadata and links you choose to
store stay on your disk, encrypted. You are responsible for what you download and for
your local laws; this tool stores what you tell it and nothing else.

MIT licensed — see [PLAN.md](PLAN.md) §2 (D5) for the rationale.
