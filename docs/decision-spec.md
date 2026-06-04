# Agent0Waste Decision Spec

**Status:** v0.4.2 (informal model; will be tightened as trace mode
and sandbox-exec land)
**Audience:** maintainers implementing trace mode, sandbox-exec,
strict mode, and any future enforcement layer
**Length:** 1 page

## 1. Purpose

Define the **decision pipeline** that determines whether a shimmed
command is `allow`ed, `prompt`ed, or `throttle`d. The spec is the
contract between the heuristic engine (in `intercept check`) and any
downstream consumer (the shim, the trace renderer, sandbox-exec,
strict mode). All v0.4.2+ features that touch the decision path
MUST conform to this spec.

## 2. Inputs

| Input | Source | Type |
|-------|--------|------|
| `command` | the shim's argv (e.g. `hermes chat "..."`) | string |
| `state.db` | `~/.hermes/state.db` (read-only SQLite) | path |
| `config` | `~/.config/agent0waste/intercept.toml` | TOML |
| `since_days` | from `--since` flag (default 14) | integer |
| `system_time` | for cache TTL comparison | wall clock |

## 3. Pipeline

```
intercept check --command <cmd> [<model>] [<tokens>] [<source>] [<since>]
   │
   ▼
[1] load_hermes_sessions(since_days, state.db)
       │
       ├─ read error (file missing, perm denied, parse error)
       │     → return Decision::Allow   (fail-open)
       │
       ▼
[2] cache_lookup(command, state.db.mtime)
       │
       ├─ HIT  → return cached Decision
       │
       ├─ MISS → continue
       │
       ▼
[3] for each rule in config.rules (in declaration order):
       heuristic_check(rule, sessions)
       │
       └─ if heuristic fires → return rule.action as Decision
                                 (allow / prompt / throttle)
   │
   ▼ (no heuristic fired)
[4] return Decision::Allow   (default)
   │
   ▼
[5] cache_store(command, state.db.mtime, decision)
       │
       └─ error is ignored (write failure → next call is a miss)
   │
   ▼
return Decision as JSON + exit code
```

Steps [1]–[5] are atomic from the caller's perspective. The caller
gets a single `Decision` and never sees a partial result.

## 4. Cache

| Property | Value |
|----------|-------|
| Storage | `~/.local/share/agent0waste/heuristic-cache.json` |
| Format | `{ "entries": { "<command>": { state_db_mtime_unix, expires_at_unix, decision_json } } }` |
| Key | `command` (the `--command` hint) |
| Invalidation | 2-key: `state.db.mtime` must match AND `now < expires_at` |
| Default TTL | 30s (per-rule `cache_ttl_s` field; 0 = disabled) |
| Corruption | invalid JSON, partial JSON, or unreadable file → empty cache; next call rebuilds. Never blocks command execution. |
| Write failure | ignored; next call is a miss |
| Disable | `--no-cache` flag on `intercept check` |

The cache is a **latency optimization only**. It MUST NOT change the
decision outcome for a given (command, state.db.mtime) pair. This
invariant is validated by the 3x back-to-back check (cold, warm,
`--no-cache`) in `docs/validation.md` §1.

## 5. Decisions

| Decision | JSON | Exit code | Shim behavior |
|----------|------|-----------|---------------|
| `Allow` | `{"decision":"allow"}` | 0 | exec real binary immediately |
| `Prompt` | `{"decision":"prompt","reason":"…","hint":"…"}` | 65 | prompt user y/N, exec on Y, exit 1 on N |
| `Throttle` | `{"decision":"throttle","cooldown_s":N,"reason":"…","hint":"…"}` | 64 | announce, sleep `cooldown_s`, re-check, exec anyway |
| error | (no JSON; stderr message) | any other | **fail-open**: exec real binary |

The re-check after a throttle sleep exists only to give the
heuristic a chance to update. The shim runs the real binary
**regardless** of the re-check outcome. The throttle is a guardrail
(not a gate): the goal is to slow the user down, not to block them.

## 6. Modes

| Mode | Behavior on error | Status |
|------|-------------------|--------|
| `fail-open` (default) | any error → `Allow` | shipped (v0.4.1) |
| `strict` | any error → deny (no real binary exec) | **NOT SHIPPED**; spec only, target v0.5.0 |

The `mode` field in config is a single switch. Per-command
overrides are out of scope (would be a separate `[[overrides]]`
table). Strict mode is opt-in and team-oriented.

## 7. Failure modes

| Failure | Detection point | Behavior |
|---------|-----------------|----------|
| `state.db` missing | [1] read returns "file not found" | fail-open → `Allow` |
| `state.db` unreadable (perm) | [1] read returns perm error | fail-open → `Allow` |
| `state.db` corrupted | [1] SQLite open returns error | fail-open → `Allow` |
| `state.db` too slow (>500ms) | shim's per-call timer | shim times out, fail-open → `Allow` |
| Cache file missing | [2] read returns "file not found" | empty cache, [3] runs |
| Cache file invalid JSON | [2] parse returns error | empty cache, [3] runs |
| Cache file perm denied | [2] read returns error | empty cache, [3] runs |
| Cache write fails | [5] write returns error | ignored; next call is a miss |
| Config file missing | [3] config load returns default | use default rules |
| Config rule malformed | [3] per-rule fallback | that rule → default action |
| `intercept check` binary missing | shim `exec` | shim falls through to direct exec of real binary |
| Shim timeout | shim's nohup'd timer | shim sees non-action rc, fail-open |

**The decision path never returns a "deny" in fail-open mode.** Every
failure maps to `Allow` so the shim's default execution path
(unwrapped real binary) is the safe fallback. This is the
usability-first contract for v0.4.x.

## 8. Heuristics (v0.4.1)

| Rule | Default action | Default cooldown | Default cache_ttl_s | Trigger (current) |
|------|----------------|------------------|--------------------|---------------------|
| `cache_bloat` | throttle | 30s | 30s | `cache_read_tokens / input_tokens > 10` for any session in window |
| `prompt_growth` | throttle | 60s | 30s | growing prompt size session-over-session |
| `auto_routing` | allow (info-only) | 0s | 30s | model switching detected (informational, not a guardrail) |
| `model_instability` | allow (info-only) | 0s | 30s | high error rate (informational) |

Detailed heuristic logic is in `src/intercept.rs::pick_decision`.
This spec defines the **contract** (what each rule can do and when);
the implementation is in code.

The `auto_routing` and `model_instability` defaults are `allow` by
design — the user can opt in to stricter behavior via config.
Rationale: don't be paternalistic about model-switching or A/B
testing. See `docs/v0.4-design.md` "Why are H3 and H4 'allow' by
default?".

## 9. Not in this spec (future)

These features are NOT covered by this spec and will get their own
specs when implemented:

- **sandbox-exec** (v0.4.3, issue #5) — wraps the real binary in
  `sandbox-exec` for hard isolation. Decision spec is unchanged;
  sandbox-exec is an *execution wrapper*, not a decision layer.
- **strict mode** (v0.5.0, issue #4) — flips all `fail-open` rows
  in §7 to `deny`. Adds a `Decision::Deny` variant. The shim
  would need a new exit code (e.g., 66) and a new path that does
  NOT exec the real binary on deny.
- **local LLM proxy** (v0.6.0, issue #6) — intercepts at the LLM
  call level, not the CLI level. Different inputs, different
  decision layer. Will reuse the cache design.
- **per-rule `cache_ttl_s` plumbing** (v0.4.3+) — `pick_decision`
  must surface which rule fired so the cache lookup can use the
  right TTL. Today the cache uses a fixed 30s for all rules (the
  default for every shipped rule).

## 10. Open questions (for the maintainer)

1. **Should the cache be a separate process (daemon) or a file?**
   Today it's a file. A daemon would let multiple shim invocations
   share cache state without race conditions on write. Trade-off:
   ~150ms saved vs the operational cost of a daemon. v0.4.x: file.
2. **Should `since_days` be per-rule?** Today it's global (14 days
   default). A heuristic like `model_instability` might want a
   shorter window than `cache_bloat`. Not in scope for v0.4.x.
3. **Should the spec include a Rust trait for `DecisionEngine`?**
   Today the engine is `pick_decision` in `src/intercept.rs`. A
   trait would make sandbox-exec and strict mode pluggable
   (different engines for different modes). Not in scope for
   v0.4.x; revisit when v0.5.0 lands.

## 11. Conformance

Any new feature that touches the decision path MUST:

1. Not change the decision outcome for a given
   (command, state.db.mtime, config) tuple.
2. Map every new failure mode to a row in §7 with a defined
   behavior.
3. Not introduce a "deny" in fail-open mode.
4. Pass the 3x back-to-back check (cold, warm, `--no-cache` returns
   the same decision).
5. Add or update a row in §8 if it adds a new heuristic.

Trace mode (v0.4.2) is the canonical renderer of this spec.
Sandbox-exec (v0.4.3) is the canonical enforcement layer. Strict
mode (v0.5.0) is the alternate mode that flips the error
mappings.
