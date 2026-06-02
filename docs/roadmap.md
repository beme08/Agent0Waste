# Agent0Waste — Roadmap

## Vision

Make local LLM token waste **visible, measurable, and reducible** — without
asking users to install a daemon, change their model provider, or trust a
network service. Every line of code in this project is in service of that
sentence.

The project is **single-developer, macOS-first, Hermes-first**. Versions
exist to mark "I would pay for a beer if you found a bug in this release"
not to advertise. We follow semver because the CLI is installable.

---

## The four layers

Agent0Waste is built in four layers. Each layer is independently useful
and ships in order.

| Layer | Name           | Output                                | Ships in |
|-------|----------------|---------------------------------------|----------|
| 1     | Scanning       | "What looks wasteful on this machine" | v0.1.0-alpha |
| 2     | Accounting     | "What did I actually spend"           | v0.2.0-beta |
| 3     | Heuristics     | "What's about to become wasteful"     | v0.3.0 |
| 4     | Interception   | "Block wasteful calls before they run" | v0.4.0 |

Each layer **consumes** the previous layer. Layer 2 reads SessionRecords
that Layer 1 would never have produced. Layer 3 reads cost data from
Layer 2 to spot trending waste. Layer 4 hooks into the model provider
to enforce what Layer 3 suggests.

---

## Version timeline

### v0.1.0-alpha — Scanning (shipped 2026-06-02)

> "Tell me what's on this machine."

- `agent0waste scan` — reads `~/.hermes/`, counts tools, detects the
  active model, measures memory pressure
- `agent0waste history` — last 100 scans
- Per-profile tool bloat detection (3 expensive tools flagged)
- Single-binary, no daemon, no network

**Non-features:** token accounting, real cost, cross-machine sync.

### v0.2.0-beta — Accounting (this milestone)

> "Tell me what I actually spent."

- `agent0waste run -- <cmd>` — child-process wrapper, records every
  invocation as a SessionRecord
- `agent0waste sessions` — lists recorded sessions newest-first
- `agent0waste cost` — rollup by total / model / provider / day,
  with a 7-day default lookback
- Built-in pricing for ~25 common models across OpenAI, Anthropic,
  xAI, Google, Meta, Mistral
- `~/.config/agent0waste/pricing.toml` override file
- 500-session FIFO cap, JSON-per-file storage
- **Token parsing is deferred to v0.2.1** — the wrapper records
  time, exit, command. Token extraction needs real tool output
  patterns, which we don't have yet.

**Non-features:** auto-detection of tokens from logs, real-time
dashboards, multi-user.

### v0.2.1 — Token extraction (small)

> "Make the cost report non-zero."

- Read `~/.hermes/logs/*.log` for token lines
- Pattern-match common SDK formats: OpenAI, Anthropic, xAI Python SDKs
- Re-stamp existing SessionRecords (idempotent) with `input_tokens`,
  `output_tokens`, then re-apply pricing
- `agent0waste cost` starts showing real dollars

**Exit criteria:** at least one real recorded session shows a
non-zero `cost_usd` after running `agent0waste cost --recompute`.

### v0.3.0 — Heuristics (small)

> "Tell me what I'm about to regret."

- Detect repeated identical prompts (cache hit candidate)
- Detect prompt runs that grow monotonically over a week (memory
  bloat candidate)
- Detect model downgrade on a profile (cost drop, quality drop —
  flag for review)
- Surface in `agent0waste cost` as "warnings" attached to rows

**Exit criteria:** three heuristics implemented, each with a unit
test, each shown in the cost report.

### v0.4.0 — Interception (medium)

> "Block the call before it costs the token."

- Optional opt-in: configure Hermes to consult Agent0Waste before
  every LLM call
- Decision: allow, throttle (add `<think>` cooling period), or
  prompt the user
- Heuristics from v0.3 become the decision rules
- Disabled by default; explicit `agent0waste intercept enable`
- macOS-only initially

**Exit criteria:** end-to-end demo: same prompt 5x in a row, the
5th is intercepted with a "looks like a cache hit" message.

### v0.5.0 — Linux support (medium)

> "Same CLI, different paths."

- Replace `macos.rs` scanner with a `platform/macos.rs` and
  `platform/linux.rs` split behind a feature flag
- Scan: `~/.hermes/` and `~/.config/Claude/...` (path-by-path)
- Run wrapper is already cross-platform (no platform code)
- Crates.io publish becomes possible

**Exit criteria:** `cargo install agent0waste` works on Ubuntu 24.04
in a clean container.

### v0.6.0 — Windows support (small)

> "Same CLI, NTFS."

- Add `platform/windows.rs` for the scanner
- `wmic` or `Get-CimInstance` for RAM
- Mostly path-handling
- A few real users on Windows will find the bugs

**Exit criteria:** `cargo install agent0waste` works on Windows 11
in an admin PowerShell.

### v1.0.0 — Stable (long)

> "I would pay for a beer if you found a bug in this release."

- All four layers working
- macOS + Linux + Windows
- Crates.io install in one command
- Semver guarantees on the CLI surface
- `agent0waste --help` is documentation
- `agent0waste validate` exists and passes against the real config

**Non-goals even at v1.0:**
- Real-time web dashboard
- Cross-device sync
- Model-routing ("use this model for that task")
- Agent replacement
- SaaS / hosted service

---

## Non-goals (project-wide)

These are intentionally **not** in scope at any version:

- **Network calls.** Agent0Waste never phones home. Period.
- **Replacement of the agent.** We observe; we do not edit configs
  on the user's behalf (`clean` is a no-op stub through v0.2).
- **Predictive cost modeling.** Heuristics look at *what already
  happened*, not what *might* happen.
- **Cross-machine sync.** Sessions are local JSON files. The user
  can `rsync` them if they want.
- **A daemon.** The wrapper model (Layer 2) is more accurate and
  uses zero resources when not running.

---

## Tracking & cadence

- **No fixed release schedule.** Versions ship when the exit
  criteria above are met.
- **Pre-1.0 versions are pre-release.** A `0.x.y` version is "I
  use it daily" not "anyone should use it."
- **Breaking changes are allowed between minor versions** until
  v1.0.0. After v1.0, only the major version may break.
- **One committer, one decision-maker.** The roadmap above is a
  public commitment but not a contract; if reality changes, the
  roadmap updates.
