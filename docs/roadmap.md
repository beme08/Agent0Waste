# Agent0Waste — Roadmap

## Vision

Make local LLM token waste **visible, measurable, and reducible** — without
asking users to install a daemon, change their model provider, or trust a
network service. Every line of code in this project is in service of that
sentence.

The project is **single-developer, macOS-first, Hermes-first**. Versions
exist to mark "I trust this release on my own machine" not to advertise.
We follow semver because the CLI is installable.

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

### v0.2.0-beta — Accounting scaffold (shipped 2026-06-02)

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
- **Token parsing was deferred to v0.2.1** — the wrapper recorded
  time, exit, command. Token extraction needed real tool output
  patterns, which we didn't have yet.

**Status:** superseded by v0.2.1. The v0.2.0-beta tag is kept for
history; the `cost` report was $0 for every session because no
tokens were being parsed.

**Non-features:** auto-detection of tokens from logs, real-time
dashboards, multi-user.

### v0.2.1 — Real token data from Hermes (shipped 2026-06-02)

> "Make the cost report non-zero."

- Discovered `~/.hermes/state.db` is a SQLite database with full
  token data per session (input, output, cache read, cache write,
  reasoning tokens) — much richer than `logs/*.log` or
  `sessions/*.jsonl`, which have no token counts
- `hermes_state.rs` reads `state.db` via `rusqlite` (bundled, no
  system SQLite required)
- `agent0waste cost --from-hermes` (default on) merges Hermes
  sessions into the cost report
- `agent0waste cost --include-local` adds `run --` records too
- 6-column cost table: key, cost_usd, sessions, input_tok,
  output_tok, cache_tok

**Exit criteria:** met — 2340+ sessions read from a real 908MB
state.db, $80+ tracked, 194M cache tokens surfaced.

### v0.3.0 — Heuristics (shipped 2026-06-02)

> "Tell me what I'm about to regret."

- **H1 cache_bloat** — `cache_read_tokens / input_tokens` ratio
  ≥ 3x in a (model, source) group with ≥ 3 sessions and ≥ 1000
  cache tokens. High severity at 8x+. Suggests the model is
  re-reading the same context window over and over.
- **H2 prompt_growth** — recent 3-day avg vs prior 3-day avg of
  input tokens, when 6+ distinct days are present, 1.5x+ growth.
  High at 2.5x+. Suggests prompts are getting bigger (memory bloat
  or accumulating context).
- **H3 auto_routing** — ≥ 5 sessions with `model = "auto"` (Hermes
  picks). Info-only. Suggests pinning a model for predictable cost.
- **H4 model_instability** — same source used 3+ distinct models
  in the window. Info-only. Suggests A/B testing or auto-rotation.

**Exit criteria:** met — 4 heuristics, 37 tests, 19 findings on
real Hermes data (14 cache_bloat, 3 prompt_growth, 1 auto_routing,
2 model_instability).

### v0.3.1 — Pricing UX + cap fix + heuristic refinement (shipped 2026-06-02)

> "Make pricing editable; make the cap big enough; make findings less noisy."

- `pricing list` — show all known models with rates
- `pricing path` — path to override file (creates dir + template
  on first run)
- `pricing add <model> <in> <out>` — append a rate to the override
  (auto-quotes TOML keys containing `.`, `:`, `/`, or whitespace)
- `pricing unset <model>` — remove an override entry by header
- `pricing check` — parses override TOML, catches negative rates,
  flags entries that shadow a default (with both rates shown so
  the user can verify the override is intentional)
- `cost --missing` — list unpriced models with session count +
  token totals + sources, plus a TOML snippet the user can append
- Default pricing table expanded from ~25 to ~44 models (added
  kimi, qwen, stepfun, mimo, owl-alpha, deepseek, gpt-4.1, gpt-5,
  o1/o3-mini, claude-3-7-sonnet, claude-sonnet-4, claude-opus-4,
  gemini-2.0-pro, llama-3.3-70b, mistral-small, grok-3-mini,
  xiaomi/mimo-v2.5-pro)
- Sessions cap raised 500 → 2000; `AGENT0WASTE_SESSIONS_CAP` env
  var override (0 = no cap); drop warning printed to stderr when
  FIFO evicts
- Heuristic thresholds tuned to reduce noise (H1 ≥ 3 sessions, H2
  ≥ 6 distinct days, H3 ≥ 5 auto sessions, H4 ≥ 3 distinct models)

**Exit criteria:** met — 43 tests pass, real data still produces
actionable findings (numbers unchanged from v0.3.0), pricing
override is now round-trippable.

### v0.4.0 — Interception (medium)

> "Block the call before it costs the token."

- Optional opt-in: configure Hermes to consult Agent0Waste before
  every LLM call
- Decision: allow, throttle (add a `<think>` cooling period), or
  prompt the user
- Heuristics from v0.3 become the decision rules
- Disabled by default; explicit `agent0waste intercept enable`
- macOS-only initially

**Fail-open vs fail-closed (design choice):**
- **Fail-open (default for v0.4.0):** if Agent0Waste is unreachable,
  the LLM call proceeds. Rationale: the wrapper is a guardrail, not
  a gate. Losing the guardrail should not break the user's
  workflow.
- **Fail-closed (opt-in, `agent0waste intercept strict`):** if
  Agent0Waste is unreachable, the LLM call is blocked and the user
  is told to retry or disable strict mode. Rationale: some users
  may prefer the system to stop spending when the watchdog is
  blind.
- The choice is per-install, set at `intercept enable` time.
- v0.4.0 ships fail-open only; fail-closed is a v0.4.1 or v0.5.0
  addition.

**Exit criteria:** end-to-end demo: same prompt 5x in a row, the
5th is intercepted with a "looks like a cache hit" message.

### v0.5.0 — Cross-platform + Claude Code adapter (medium)

> "Same CLI, more platforms, more agents."

- Replace `macos.rs` scanner with `platform/macos.rs` and
  `platform/linux.rs` and `platform/windows.rs` behind a feature flag
- Add a Claude Code adapter that reads from
  `~/.config/Claude/.../sessions/*.jsonl` (or wherever Claude Code
  stores its data) and produces SessionRecord-shaped data
- Heuristics and pricing apply unchanged — adapters are
  read-only data sources
- Run wrapper is already cross-platform (no platform code)
- Crates.io publish becomes possible once Linux/Windows paths are
  in place (the current IP block lifts naturally with the first
  successful publish)

**Exit criteria:** `cargo install agent0waste` works on Ubuntu
24.04 and Windows 11 in clean containers; `agent0waste cost` reads
both Hermes and Claude Code data without flag changes.

### v0.6.0 — Cache pricing + better heuristics (small)

> "Charge the cache what it actually costs."

- Add `cache_input` and `cache_output` rate columns to the pricing
  table (Anthropic charges 10% of input for cache reads; xAI varies)
- Heuristic: detect when a model is run on a free tier that *also*
  has a paid variant the user has paid for — flag the downgrade
- Heuristic: detect when session duration grows > 2x week-over-week
  on the same (model, source) — flag for review
- Heuristic: detect first-of-day "warmup" sessions that are
  unusually large vs mid-day sessions — flag for review
- `agent0waste intercept` daemon mode (or process-attached
  listening socket) becomes possible

**Exit criteria:** cache tokens are priced; 7+ heuristics; one
heuristic available in `agent0waste intercept` (Layer 4).

### v1.0.0 — Stable (long)

> "I trust this release on my own machine and I'm not changing it
> for at least a year."

- All four layers working (Scanning, Accounting, Heuristics, Interception)
- macOS + Linux + Windows
- Crates.io install in one command
- Semver guarantees on the CLI surface
- `agent0waste --help` is documentation
- `agent0waste validate` exists and passes against the real config
- Fail-closed interception ships (see v0.4.0)
- Pricing table covers every model any user has actually run

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
