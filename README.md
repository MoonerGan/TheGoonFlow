# The Goon Flow

A local-first, open source tracker + manager for adult games and imageboard content.
Browse freely, clip anything, keep one organized flow across sites — encrypted at rest,
nukeable in one action, zero cloud, zero telemetry.

**Status:** M0 complete (encrypted vault shell, portable folder design,
companion API skeleton, D13 importers, library grid + editor, nuke).
See [PLAN.md](PLAN.md) for the full design, milestones, and decision log.

- **Users:** start at [docs/USER-GUIDE.md](docs/USER-GUIDE.md)
- **Developers:** see [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)

## Feature focus (M0)

- Portable vault: everything lives in one folder you place yourself (USB, external
  drive, or beside the exe). The exe keeps only a tiny non-sensitive pointer (`flow.cfg`)
  that finds the vault by volume label.
- Vault is encrypted at rest: Argon2id → wrapped data key → SQLCipher database.
  Wrong passphrase fails closed; nuke crypto-erases key material, then shreds files.
- Companion HTTP API on 127.0.0.1 (token-authed) — the future home of the browser
  extension clip endpoint.
- Library schema (items / sources / tags with display types / thumbnails) is live and
  versioned from day one.

## Development

Prerequisites: Rust (MSVC toolchain), Node 18+, Tauri 2 system deps (WebView2, VS Build
Tools C++ workload).

```sh
cd apps/desktop
npm install
npm run tauri dev      # hot-reload dev app
npm run tauri build    # portable-style build
```

Tests:

```sh
cargo test
```

## Privacy & legal

No telemetry, no cloud calls, works fully offline. Metadata and links you choose to
store stay on your disk, encrypted. You are responsible for what you download and for
your local laws; this tool stores what you tell it and nothing else.

MIT licensed — see [PLAN.md](PLAN.md) §2 (D5) for the rationale.
