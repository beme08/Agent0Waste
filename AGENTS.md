# AGENTS.md

Inherits the 12 global rules from `~/.codex/AGENTS.md`. This file adds
project-specific rules; the global rules still apply. See
https://developers.openai.com/codex/guides/agents-md for discovery and
override mechanics (we do not re-document them here).

## Build / lint gate

```bash
cargo build                                   # debug, default features
cargo build --release                         # release
cargo build --release --features bench        # Layer 6 (vLLM/SGLang) bench
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

- Default build is intentionally lean. HTTP / ML / runtime deps live
  behind `--features bench`. Do not promote them to default.
- macOS-only. Do not add Linux/Windows `#[cfg]` branches without a
  matching entry in `docs/roadmap.md`.

## Project layout

- `src/main.rs` — CLI entry (`clap` derive); subcommands live in `src/<cmd>.rs`.
- `src/permission.rs` — permission/cache gate. New file readers must go through it.
- `src/bench.rs` — Layer 6 inference bench; only compiled with `--features bench`.
- `src/cache.rs`, `src/hermes_state.rs`, `src/sessions.rs` — local state readers
  for `~/.hermes`, `~/.claude`, etc. Never exfiltrate.
- `docs/` — design notes, decisions, recipes. Update when public behavior changes.
- `tests/fixtures/` — canned vLLM/SGLang metrics for integration tests.
- `agent0waste-demo/` is a **separate codebase** with its own `AGENTS.md`
  (HyperFrames composition). Do not edit it from this repo.

## Privacy & permissions

- Local-first, network-off by default. See `SECURITY.md` and
  `docs/decision-spec.md` for the contract.
- New parsers must not log full file contents; redact paths and secrets.
- No new external services, telemetry, or update channels.

## Versioning & docs

- Bump `version` in `Cargo.toml` and add a `docs/vX.Y-design.md` note for
  any user-visible change.
- Update `README.md` when CLI surface or supported agents change.
- Keep `docs/roadmap.md` honest: move items out when shipped.

## Commits & branches

- Branch prefix: `codex/` (e.g., `codex/v0.6.0-layer-6`).
- `Cargo.lock` is committed — leave it that way. Never commit `target/`.
- Per global rule 7: show `git status` before any commit; no force-push,
  no history rewrite, no remote changes without explicit confirmation.
