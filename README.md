# Petrel

*Working codename — a fast, local-first desktop email client.*

Petrel is a personal project building an email client around four commitments: **fast**
(sub-100ms interactions, keyboard-first), **open** (IMAP today; Gmail and Microsoft 365
via their native APIs; JMAP planned), **private** (your mail syncs directly between this
device and your providers — no vendor cloud in the path, remote content blocked by
default), and **durable** (a documented local store of ordinary files plus SQLite that
outlives any app).

**Status: pre-alpha, milestone M0** — engine foundations and feasibility spikes. Nothing
here is usable as a mail client yet.

## Stack

Rust engine (protocols, storage, full-text search, sanitization) behind a typed IPC seam;
Tauri 2 shell with a React/TypeScript UI. The engine is headless and UI-agnostic.

```
crates/petrel-engine       storage, search, actions, the engine API
crates/petrel-providers    provider backends (IMAP/SMTP; Gmail API; Microsoft Graph)
crates/petrel-mime         parsing, sanitization profile, message building
crates/petrel-autoconfig   account autodiscovery
crates/petrel-testkit      synthetic mailboxes, fault injection, corpora
apps/desktop               Tauri shell + UI
```

## Build

```sh
cargo test                      # engine crates (default members)
cargo test --release -p petrel-engine --test store_spike -- --ignored --nocapture
                                # storage/search benchmark (100k synthetic messages)

# Run the app (M0 demo: 10k synthetic messages + live engine search):
pnpm install
pnpm --dir apps/desktop/ui build
cargo petrel
```

`cargo petrel` is an alias for `cargo run --release -p petrel-desktop --features
custom-protocol`. The `custom-protocol` feature is what puts Tauri in production mode —
without it the webview navigates to the Vite dev server and you get a blank window.

Requires Rust 1.96+ (pinned via `rust-toolchain.toml`). The desktop app additionally needs
Node 22+ / pnpm for the UI.

## Releases

Distributed as build artifacts on the GitHub Releases tab. Pre-alpha artifacts are
unsigned: on macOS use right-click → Open on first launch.

## License

Not yet chosen — all rights reserved until a license lands with the first public release.
