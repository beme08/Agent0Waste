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

## The layers

Agent0Waste is built in layers. Each layer is independently useful and
ships in order. Layers 1–5 are local-first config / token / execution
audit. Layer 6 is the inference systems profiler — it doesn't consume
the previous layers, it sits beside them and is gated behind a cargo
feature.

| Layer | Name           | Output                                | Ships in |
|-------|----------------|---------------------------------------|----------|
| 1     | Scanning       | "What looks wasteful on this machine" | v0.1.0-alpha |
| 2     | Accounting     | "What did I actually spend"           | v0.2.0-beta |
| 3     | Heuristics     | "What's about to become wasteful"     | v0.3.0 |
| 4     | Interception   | "Block wasteful calls before they run" | v0.4.0 |
| 5     | Sandbox (opt-in) | "Restrict what the binary can touch" | v0.4.3 (experimental) |
| 6     | Systems Profiler | "How is the inference server doing?" | v0.6.0 (`--features bench`) |

Each of Layers 1–5 **consumes** the previous layer. Layer 2 reads
SessionRecords that Layer 1 would never have produced. Layer 3 reads
cost data from Layer 2 to spot trending waste. Layer 4 hooks into the
model provider to enforce what Layer 3 suggests. Layer 5 wraps the real
binary in `sandbox-exec` after the Layer 4 decision. Layer 6 sits
alongside: it talks to whatever inference server the user points it at
and reports throughput, latency, KV cache pressure, and a 0–100 waste
score.

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

### v0.4.0 — Interception (shipped 2026-06-03, tagged 2026-06-03)

> "Block the call before it costs the token."

- Optional opt-in: configure Hermes (or any wrapped command) to
  consult Agent0Waste before every LLM call
- Decision: `allow`, `throttle` (add a `<think>` cooling period),
  or `prompt` the user
- Heuristics from v0.3 become the decision rules
- Disabled by default; explicit `agent0waste intercept enable <cmd>`
- macOS-only
- Shim installs to `~/.local/share/agent0waste/shims/<cmd>`
  (UV-style) to avoid clobbering `~/.local/bin/<cmd>` (issue #1)
- **Fail-open only.** The wrapper is a guardrail, not a gate —
  losing the guardrail should not break the user's workflow. The
  fail-closed mode that was originally specced for v0.4.0 was
  deferred to v0.5.0 to give it the design attention it needed.

**Exit criteria:** met — `docs/validation.md` v0.4.0-beta section
records 11/11 dogfood paths passing on a real machine.

### v0.4.1 (heuristic cache) — Cache + shim restructure (shipped 2026-06-04, PR #8)

> "Make the check fast on repeated calls; fix the throttle/timeout UX."

Resolves issues #3 (5s shim hard timeout vs 30s throttle cooldown)
and #7 (cache-aware heuristic re-evaluation). Both shipped together
because the cache is the cleanest resolution for the timeout tension.

- **Heuristic cache** — small JSON file at
  `~/.local/share/agent0waste/heuristic-cache.json`, keyed by
  `(command, state.db mtime, TTL)`. On a hit, `intercept check`
  returns the cached decision without re-reading `state.db` (saves
  ~150ms per call). Default TTL 30s; per-rule `cache_ttl_s` is
  parsed and stored in `RuleConfig` but not yet plumbed through
  to the cache write (v0.5.1).
- **Shim restructure** — the shim now calls `intercept check` and
  parses the rc to decide allow/throttle/prompt, then exec's the
  real binary. The old `intercept run -- <cmd>` one-shot is kept
  for legacy callers. The 5s hard timeout was lowered to 500ms
  (shim uses `perl` for sub-second sleep on macOS).
- **3x back-to-back check** — cold, warm, `--no-cache` return the
  same decision (cache is latency-only, never changes outcomes).
- **Corruption behavior documented** — missing / unreadable /
  unparseable cache file → empty cache, no warning, next put
  rebuilds. A corrupt cache cannot prevent command execution.

**Exit criteria:** met — `docs/validation.md` v0.4.1-beta and
v0.4.1-rc sections, 64 unit tests pass on 10 consecutive runs.

### v0.4.2 (intercept trace) — Decision spec + trace mode (shipped 2026-06-04, commit `c86e448`)

> "Make the decision pipeline inspectable without exec'ing the real binary."

- **`docs/decision-spec.md`** — formal 1-page model of the decision
  pipeline (load → cache → heuristics → decision → cache store),
  the cache contract, the failure table (§7), and the §5/§7
  separation. All v0.4.2+ features that touch the decision path
  conform to this spec.
- **`intercept trace --command "<cmd>"`** — renders the decision
  pipeline as a human-readable trace (steps [1]–[6] with timings).
  Pure preview: never exec's the real binary, never writes the
  cache. The shim's actual `intercept check` call is unchanged.
- **`fired_rule: Option<String>`** added to the internal
  `CheckTrace`, unblocking the per-rule TTL plumbing that ships
  in v0.5.1.
- **Read-only contract** — trace is preview-only by spec; a
  follow-up fix (`af78d83`) made the no-cache-write behavior
  explicit in the source.

**Exit criteria:** met — trace output renders all 6 spec steps
on real data; `intercept check` and `intercept run` behavior
unchanged.

### v0.4.3 (Layer 5 sandbox-exec) — MacOS sandbox wrapper (shipped 2026-06-04, PR #11)

> "Restrict what the wrapped binary can touch — even if the heuristic engine is bypassed."

- **Layer 5 = execution wrapper, not a decision layer.** Layer 4
  decides *whether* to run; Layer 5 decides *what the run can
  touch*. The decision spec is unchanged.
- **Deny-default SBPL profile** at
  `~/.config/agent0waste/sandbox/<cmd>.sb`. Writes are restricted
  to an allowlist (`~/.hermes`, `~/.local`, `~/.cache`,
  `/private/tmp`, `/private/var/folders`, `/dev/null`); reads
  stay near-global (allow-listing read paths is impractical for
  Python startup imports). Network is outbound-only.
- **`intercept enable-sandbox <cmd>` / `disable-sandbox` /
  `validate-sandbox`** — opt-in via `intercept.toml`. The shim's
  `SANDBOX_ENABLED` and `SANDBOX_PROFILE` env vars are baked at
  install time.
- **Bypass-aware** — `--agent0waste-bypass` overrides the Layer 4
  policy but still wraps the real binary in `sandbox-exec`
  (bypass is a *policy* override, not an *isolation* override).
- **macOS-only.** Non-macOS hosts silently skip Layer 5.

**Exit criteria:** partial. Profile deploys, shim wraps, and the
CLI surface works. **The `validate-sandbox` smoke test has been
observed to fail with exit 70 on some macOS versions** because
the profile paths are hardcoded at template time (the v0.4.3
design's open question #1). Tracked as a v0.5.2 bug.

### v0.5.0 (fail-closed mode) — Shipped 2026-06-04 (PR #12). Manifest bumped to 0.5.0 in PR #14.

> "Make the default decision configurable: fail-open (current) or fail-closed (opt-in)."

Resolves issue #4 (v0.5.0: fail-closed mode — supersedes 'strict mode').

- **`Decision::Deny` + exit 66.** New `Decision` variant emitted
  when the user configures a rule with `action = "deny"`, or when
  `mode = "fail-closed"` and no rule fires. The shim honors
  `Deny` by **not** exec'ing the real binary (distinct from
  `Throttle` / `Prompt`, which both eventually exec).
- **`Action::Deny`** as a rule action in `intercept.toml` — users
  can opt specific heuristics into hard-no behavior.
- **`mode = "fail-closed"`** config key — when set, the default
  decision (no rule fired) flips from `Allow` to `Deny`. The flag
  is read live by `intercept check` on every call (no shim
  reinstall needed to toggle).
- **§5/§7 first-class decision** — `mode` only flips §5 (the
  decision table), never §7 (the failure table). Heuristic
  timeouts and other runtime failures stay fail-open regardless
  of mode. A timeout is a system failure, not a policy outcome.
- **`--agent0waste-bypass`** shim flag — long-form only, grep-able
  in `ps`, audit-logged to
  `~/.local/share/agent0waste/bypass.log` (silent on write
  failure; bypass proceeds anyway). Bypasses policy, not
  isolation (sandbox-exec still wraps).
- **Audit log contract pinned** — `0600` perms on first write,
  ISO 8601 UTC timestamps, one line per bypass event. No CLI
  reader; user can `cat` the log.

**Exit criteria:** met — `docs/validation.md` v0.5.0 section
records the §5/§7 separation, the bypass audit trail, and the
fail-closed regression. Manifest is at `0.5.0` post-PR #14.

### v0.5.1 (per-rule cache TTL) — Planned

> "Stop hardcoding 30s — let each rule pick its own cache TTL."

Resolves issue #10 (`v0.4.4: per-rule cache_ttl_s plumbing`,
labeled v0.4.4 in the issue tracker but the v0.4.4 work was
absorbed into v0.5.0; this ships as v0.5.1 to match the
manifest-string progression).

- **`pick_decision`** returns `fired_rule: Option<String>`. This
  is the unblocker — the cache write can now look up
  `cfg.rules[&fired_rule].cache_ttl_s` instead of using 30s for
  every rule.
- **Default fallback = 30s** when no rule fired (preserves
  current behavior for the default `Allow` path and the
  fail-closed `Deny` mode-flip).
- **`intercept trace` step [5]** shows the actual TTL used and
  the source rule (e.g., `would write TTL=2s (from cache_bloat
  rule) (trace is preview-only)`).
- **User-visible impact for the default config: zero.** All four
  default rules use `cache_ttl_s = 30`. The change matters only
  for users who ship custom rules with different TTLs.

**Status:** design locked; issue #10 open. Code change is a
small edit in `src/main.rs` (dispatch glue) plus per-rule TTL tests
in `src/intercept.rs` and `src/cache.rs`, and a `docs/validation.md`
v0.5.1 section. No schema change to the cache file.

### v0.5.2 (Layer 5 path fix) — Planned

> "Make `validate-sandbox hermes` actually pass on real macOS."

The v0.4.3 design's open question #1: the SBPL profile template
hardcodes the home-directory path at profile-generation time,
which means `sandbox-exec` rejects profiles that don't match
the literal `/Users/...` path the templating baked in. The
v0.4.3 design accepted this as a known limitation, but the
limitation means Layer 5 ships broken-by-default on any host
where `$HOME` doesn't match the templated literal.

- **Profile templating fix** in `src/sandbox.rs` — generate the
  SBPL at install time using the actual `$HOME`, not a literal
  `/Users/me` placeholder.
- **`validate-sandbox`** smoke test passes on real macOS.
- **No behavior change** for users whose shim was already
  installed with the broken profile; the fix takes effect on
  next `intercept enable-sandbox` reinstall.

**Status:** design locked in the v0.4.3 design doc. The
`validate-sandbox hermes` failure I observed during PR #14
testing is the trigger. ~1 day of code work.

### v0.6.0 (Layer 6: Systems Profiler & Benchmark) — Shipped 2026-06-17

> "Measure, don't modify."

Extends the existing five layers with a new **Layer 6** that benchmarks
OpenAI-compatible inference servers (vLLM, SGLang, or any compatible
endpoint) and emits a JSON + CSV report with an explainable 0–100
`waste_score` (lower is better).

- **Three first-class targets**: `vllm`, `sglang`, `baseline`. The
  `baseline` target is for TGI, llama.cpp server, or any other
  OpenAI-compatible endpoint; it does client-side measurements only.
- **Swept-concurrency chat-completion load** — default sweep is
  `[1, 4, 16, 32]`, configurable via `--concurrency`. Each level gets
  `--num-requests` requests, after a configurable warmup.
- **Optional Prometheus `/metrics` scraping** at 1 Hz, stops cleanly
  when the loadgen finishes. Missing series are reported as `null` —
  no fabricated "hit rate".
- **Waste score** with five axes (kv_cache_pressure,
  gpu_underutilization, tail_latency, ttft_jitter, context_oversize),
  proration over unavailable axes, monotonic in each axis
  (verified by unit tests).
- **No custom kernels. No first-class MLX / oMLX integration in
  v0.6.** Both deferred to v0.7.
- **`bench` cargo feature** gates the heavy HTTP runtime so the
  default `cargo install` binary stays lean. The `bench` subcommand
  errors out at runtime if the feature is not enabled.

**Exit criteria:** met. `cargo test --features bench` is green
(147 tests). `agent0waste bench run vllm` produces a JSON + CSV
report end-to-end against a running server. The waste score is
monotonically non-decreasing in any single axis.

**First blog acceptance check:** two primary numbers — vLLM remote
and SGLang remote — on Llama 3 8B, each clearly labelled with
hardware config. Optional `baseline` appendix. No MLX result
required for v0.6.

See [`docs/v0.6-bench-design.md`](v0.6-bench-design.md) for the full
scope and `docs/bench-recipes.md` for copy-pasteable commands.

### v0.6.1 — Cache pricing + heuristic expansion (planned, smaller)

> "Price the cache, expand the heuristics."

The cache-pricing and heuristic-expansion work that was originally
scoped for v0.6.0 is now v0.6.1. Layer 6 shipped first because the
benchmark work unblocks the v0.7 MLX work and the upstream vLLM/SGLang
findings conversation.

- **Cache pricing** — add `cache_input` and `cache_output` rate
  columns to the pricing table. Anthropic charges 10% of input for
  cache reads; xAI varies. The cost report will surface cache spend
  separately from regular token spend.
- **Heuristic expansion** — detect when a model is run on a free
  tier that *also* has a paid variant the user has paid for (flag
  the downgrade). Detect when session duration grows > 2x
  week-over-week on the same (model, source). Detect first-of-day
  "warmup" sessions that are unusually large vs mid-day sessions.

### v0.7 — Apple Silicon / MLX backend (planned)

> "First-class Apple Silicon profiling, local Mac benchmarking."

**Not shipped in v0.6.** The v0.7 scope:

- **First-class `mlx-lm` / oMLX Apple Silicon target** — a new
  `Target` impl that talks to mlx-lm's OpenAI-compatible server.
  The `Target` trait in v0.6 is shaped so this is a single-file
  change.
- **Local Mac benchmarking recipe** — copy-pasteable commands for
  benchmarking mlx-lm on the developer's own machine, with richer
  local hardware counters (Metal / `powermetrics`) where available.
- **Optional custom-kernel slot** — a future kernel backend could
  attach kernel-specific counters via the `Target` trait. Not
  required for v0.7; may land later.
- **Upstream PRs to vLLM / SGLang** with findings from v0.6 numbers.


in the same milestone.

**Cross-platform (deferred from the original v0.5.0 framing):**
`crates.io` publish becomes possible once Linux/Windows paths
are in place. The macOS-specific bits (scanner, sandbox-exec,
intercept shim) need feature flags. Realistically a v0.6.0+
slip into v1.0.0 scope.

### v1.0.0 — Stable (long)

> "I trust this release on my own machine and I'm not changing it
> for at least a year."

- All four (five) layers working — Scanning, Accounting,
  Heuristics, Interception, Sandbox
- macOS + Linux + Windows
- Crates.io install in one command
- Semver guarantees on the CLI surface
- `agent0waste --help` is documentation
- `agent0waste validate` exists and passes against the real config
- Fail-closed interception ships (see v0.5.0)
- Pricing table covers every model any user has actually run
- Local LLM proxy is the default for at least one model (the
  shim model, if nothing else)

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
- **Cross-platform (until v0.6.0+).** macOS is the only supported
  host. Linux/Windows support is on the roadmap but is not a
  v0.5.x milestone — see v0.6.0 above.

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
- **Source of truth per milestone: the per-version design doc.**
  The roadmap is the high-level synthesis. For the precise scope
  of any shipped version, read `docs/v0.<n>-design.md` (or
  `docs/decision-spec.md` for the Layer 4 contract). The
  `docs/validation.md` file records the actual e2e test
  outcomes for each release.
