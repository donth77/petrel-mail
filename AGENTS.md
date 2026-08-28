# AGENTS.md

Guidance for agents and new humans working in this repository.

## What this is

Petrel: a local-first desktop email client. Rust engine + Tauri 2 shell + React/TS UI.
Pre-alpha (M0). The engine is the product; the UI is a thin view over it.

## Commands

```sh
cargo test                                # engine crates; must stay green
cargo fmt --all && cargo clippy --workspace --all-targets -- -D warnings
cargo test --release -p petrel-engine --test store_spike -- --ignored --nocapture
                                          # storage/search benchmark (slow, opt-in)
cargo petrel                              # run the desktop app (alias: release +
                                          # --features custom-protocol; without that
                                          # feature Tauri loads the dev server → blank window)

PETREL_SELFTEST=1 cargo petrel            # drive search from inside the webview and log
                                          # results (UI→IPC→engine smoke test)
PETREL_SPIKE_S2=1 cargo petrel            # webview isolation harness (hostile documents in
                                          # sandboxed frames; verdicts logged engine-side)
```

The shell carries a permanent diagnostic init script: it reports the loaded URL, DOM state,
JS errors, and CSP violations over IPC to stderr (`[frontend] …`, `[nav] …`). A webview is
opaque from outside — **check what URL it actually loaded before theorizing about why a page
is blank.**

## Architecture in five lines

1. `petrel-engine` owns everything trusted: protocols, TLS, OAuth tokens, storage, search,
   MIME parsing, sanitization. The UI never touches the network or secrets.
2. UI ↔ engine only via typed IPC: commands, paged queries (≤50 rows), change events.
   Never bulk data over IPC; bulk bytes go over a custom protocol with single-use tokens.
3. Message HTML is hostile input: parse → sanitize (allowlist) → render only inside a
   sandboxed, no-script, no-network frame. Fail closed to text.
4. Storage: raw message blobs on disk (content-hashed, zstd) + one SQLite (WAL). The FTS
   index is updated in the same transaction as message writes — **never write messages or
   `fts_content` outside the store API**; index drift is the one unforgivable bug class.
5. Provider differences (IMAP vs Gmail API vs Graph) live behind the backend trait in
   `petrel-providers`; engine logic must not leak provider semantics.

## Hard rules

- **No mail content, subjects, addresses, or query terms in logs** at any default level.
- Secrets live in the OS keychain only — never in SQLite, files, or test fixtures.
- TLS always; no plaintext IMAP/SMTP code paths.
- Changes to the sanitizer profile, the remote-content fetch broker, or the custom-protocol
  handler are security-sensitive: they require their regression corpora to pass and a
  clearly-flagged PR.
- Tests use synthetic data from `petrel-testkit`. Never commit real mail, real addresses,
  or recorded traffic containing either.
- Dependency additions must pass `cargo deny check` (MIT-compatible licenses only).

## Conventions

rustfmt + clippy clean (`-D warnings`) · conventional commits · every behavior change
lands with its tests · benchmarks and provider-quirk suites are part of review, not
afterthoughts.

**CSS uses logical properties, not physical ones** — `margin-inline-start`, `padding-block`,
`inset-inline-end`, `text-align: start`. Never `left`/`right`/`margin-left`. Petrel ships
left-to-right only and has no right-to-left support planned, but logical properties cost
nothing while a component is being written and are miserable to retrofit across all of them
later. Physical properties are correct only where the thing genuinely is physical — a drop
shadow's offset, a spinner's rotation.

**Triage gestures are optimistic.** The row leaves, the chip appears and the counts move before
the engine is asked; the captured prior state goes back on failure; a debounced recount reconciles.
Route new triage actions through `useTriage.run` rather than calling `api.triage` from a component,
or the gesture ends up fast in the list and slow in the sidebar. Not everything qualifies — see
[docs/09 §7c](docs/09-engineering-practices.md) for what stays pessimistic and why.

**User-facing strings are never literals in components.** They come from the Fluent bundle,
even while English is the only locale. A string committed inline is invisible to translation
and to the pseudolocale check that catches truncation.
