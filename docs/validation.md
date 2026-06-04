# Validation Status

## Validated
- ✓ Hermes profile detection
- ✓ Report generation
- ✓ History snapshots

## Needs Validation
- □ Token accounting
- □ Cost estimation
- □ Storage analysis
- □ Process inspection
- □ Cross-tool detection (Claude Code, Goose, OpenCode, etc.)

---

## v0.4.0-beta — Interception (validated 2026-06-03)

End-to-end dogfood of the shim install model on a real macOS machine
(Mac16,7, 900MB+ hermes `state.db`, shim dir on PATH).

**Test command:** `~/bin/dogfood-test` — a 3-line shell script that
echoes its argv. Chosen so we can exercise all four decision paths
without spending tokens on real LLM calls.

**Test binary:** `target/release/agent0waste` from
`~/beme08/Agent0Waste/`, version 0.4.0-beta.

**Config:** `~/.config/agent0waste/intercept.toml` — overridden per
test to force a specific decision.

### Checklist

| # | Path | Result | Notes |
|---|------|--------|-------|
| 1 | **allow** | PASS | `dogfood-test allow-clean` → real binary runs, `rc=0`, empty stderr. Set `cache_bloat=allow, prompt_growth=allow` to force allow. |
| 2 | **throttle** | PASS | `dogfood-test throttle-2s` → "throttle: 12 sessions…", "sleeping 2s, then re-checking…", "still throttled…", "running anyway", real binary runs. `rc=0`, elapsed=2s. |
| 3 | **prompt (Y)** | PASS | `dogfood-test prompt-y < yes-input.txt` → "prompt: …", "continue? [y/N]", "Y" accepted, real binary runs. `rc=0`. |
| 4 | **prompt (N)** | PASS | `dogfood-test prompt-n < no-input.txt` → "prompt: …", "continue? [y/N]", "N" rejected, "[agent0waste] cancelled", real binary does NOT run. `rc=1`. |
| 5 | **timeout (5s shim timeout)** | PASS | `dogfood-test timeout-test` with `cooldown_s=30` → throttle message + sleep starts, 5s timer fires, "[agent0waste: check timed out (5s); running unwrapped]", real binary runs. `rc=0`, elapsed≈5s. |
| 6 | **fail-open (binary missing)** | PASS | `chmod -x target/release/agent0waste`, run `dogfood-test failopen` → "[agent0waste]: Permission denied" on stderr, real binary still runs. `rc=0`. |
| 7 | **fail-open (state.db unreadable)** | PASS | `chmod 000 ~/.hermes/state.db`, run `dogfood-test db-unreadable` → "[agent0waste] could not read /Users/.../state.db: … unable to open database file: …; fail-open: allowing call" on stderr, real binary runs. `rc=0`. |
| 8 | **real hermes (--version)** | PASS | `hermes --version` through the shim → real Hermes output ("Hermes Agent v0.15.1 (2026.5.29)"), `rc=0`. Real hermes MD5 unchanged: `cf9f1706e77f519c93ab2d08a5ffb2ac`. |
| 9 | **real hermes (--help)** | PASS | `hermes --help` through the shim → real usage text, `rc=0`. |
| 10 | **PATH resolution** | PASS | Shim's `find_real` walks PATH excluding `SELF_DIR`, returns `/Users/.../local/bin/hermes` (verified via `bash -c 'find_real hermes'`). |
| 11 | **shim dir on PATH** | PASS | `intercept status` reports `on PATH: yes` after `export PATH="$HOME/.local/share/agent0waste/shims:$PATH"`. `which dogfood-test` and `which hermes` both resolve to `~/.local/share/agent0waste/shims/<cmd>`. |

### Bugs found during dogfood and fixed in the same release

Three real bugs surfaced during end-to-end testing. All fixed in
`main` before this validation was recorded; unit tests still 64/64
across 10 consecutive runs.

1. **Bash `&` closes stdin in the backgrounded child when the
   parent's stdin is a non-TTY (file/pipe).** The shim's child
   `intercept run` would always see EOF on stdin, so the prompt path
   always cancelled (even when the user typed "Y"). Fix: `exec 3<&0`
   at the top of the shim, redirect the child with `<&3`. Documented
   inline in `SHIM_TEMPLATE`.

2. **`intercept run` discarded the binary returned by `split_run_args`
   and used `args[0]` as the binary instead.** Result: spawn always
   failed (e.g., "failed to spawn `prompt-y-fixed`" when the arg was
   `prompt-y-fixed` and the binary was the absolute path).
   `split_run_args` returns `(cmd, args)`; `intercept run` was
   destructuring as `(_cmd, args)` then taking `args[0]` as the
   binary. Fix: use the `cmd` from `split_run_args`. The bug was
   silent in the default allow path because of the throttle's
   "running anyway" branch — the real binary was never reached, but
   `intercept run` exited 0 so the shim appeared to work.

3. **Shim's fail-open was too narrow.** Only ran the real binary on
   timeout-killed child (rc 124/143/137). On other failures (e.g.,
   agent0waste binary missing, rc 126), the shim propagated the
   error and the real binary did not run. Fix: shim now falls through
   to `exec "$REAL" "$@"` on any rc that's not 0 (real binary already
   ran) and not 1 (user cancelled prompt). Caught and fixed during
   the "fail-open (binary missing)" test.

### Design tension noted (not fixed in v0.4.0-beta)

**5s shim timeout vs 30s throttle cooldown.** When the heuristic
fires with the default `cooldown_s=30`, the throttle's 30s sleep is
cut short by the shim's 5s hard timeout. User sees the throttle
message, then 5s later the fail-open message, then the real binary
runs. The guardrail still works (the throttle is announced) but the
intended "wait 30s and re-check" UX doesn't complete. Three options
for v0.4.1:
- Make the shim timeout user-configurable per shim
- Make `intercept run` self-timeout (push the 5s budget into the
  binary, not the shim)
- Cache heuristic output (TTL 30s) so the check itself takes <100ms
  and the throttle's 30s re-check can complete

### Demo

[agent0waste-intercept-demo.gif](agent0waste-demo/renders/agent0waste-intercept-demo.gif) — 30s, shows the 4 decision paths
in sequence (allow → prompt-Y → prompt-N → throttle) followed by a
real `hermes --version` through the shim. 402KB.

### How to reproduce

```bash
# Throwaway command (3-line shell script that echoes its argv):
cat > ~/bin/dogfood-test <<'EOF'
#!/bin/sh
echo "real dogfood-test: argc=$# args=$*"
exit 0
EOF
chmod +x ~/bin/dogfood-test

# Enable interception:
agent0waste intercept enable dogfood-test

# Per-path test configs and commands are in /tmp/v040-demo.sh
# (committed as a sibling of this file).
bash /tmp/v040-demo.sh
```

### Not validated here (out of scope for v0.4.0-beta)

- Local LLM proxy (Mechanism B in `docs/v0.4-design.md`) — v0.4.1
- Fail-closed mode — v0.4.1
- macOS `sandbox-exec` wrapper for hard isolation — v0.4.1
- Cross-agent interception (Claude Code, Goose, aider) — v0.5.0
- Cache-aware heuristic re-evaluation — v0.4.1

---

## v0.4.1-beta — Heuristic cache + shim restructure (validated 2026-06-04)

Resolves issues #3 (5s shim hard timeout vs 30s throttle cooldown UX
tension) and #7 (cache-aware heuristic re-evaluation). Both shipped
together because #7 is the cleanest resolution for #3 (per the
issue's option 3, plus the shim restructure from option 2).

**Test command:** same `~/bin/dogfood-test` from v0.4.0-beta
validation.

**Test binary:** `target/release/agent0waste` from
`~/beme08/Agent0Waste/`, version 0.4.1-beta.

**Config:** `~/.config/agent0waste/intercept.toml` — overridden per
test to force a specific decision.

### New in v0.4.1

The shim template was rewritten. Two structural changes:

1. **Shim handles the action in bash.** The old shim called
   `agent0waste intercept run -- "$REAL" "$@"`, which did the check
   AND the exec. The new shim calls `agent0waste intercept check
   --command "$REAL $*"` (a quick check) and parses the rc:
   - `0` (Allow) → exec real
   - `65` (Prompt) → prompt user y/N, exec on Y, exit 1 on N
   - `64` (Throttle) → parse `cooldown_s` from JSON, sleep, re-check,
     exec real regardless of re-check outcome
   - any other rc → fail-open (exec real)

2. **Timeout fires from a `nohup`'d subshell.** The 5s hard timeout
   that used to cover the whole `intercept run` is now a nohup'd
   bash subshell that sleeps for TIMEOUT and tries to kill the
   `intercept check` child. nohup detaches the timer from the shim's
   job table, so a fast check (cache hit, <100ms) doesn't pay the
   5s cost. If the check hangs, the timer kills it and the shim
   fails open.

The heuristic cache is a separate concern: a small JSON file at
`~/.local/share/agent0waste/heuristic-cache.json` that records
`(command, state_db_mtime, expires_at) → decision_json` for each
checked command. On a hit, `intercept check` returns the cached
decision without re-reading state.db.

### Checklist

| # | Path | Result | Elapsed | Notes |
|---|------|--------|---------|-------|
| 1 | **allow** | PASS | 72ms | `cache_bloat=allow, prompt_growth=allow`. Real binary runs, rc=0. |
| 2 | **throttle (2s cooldown)** | PASS | 2.133s | Throttle msg, sleep 2s, re-check, run anyway. Full sleep completes. |
| 3 | **prompt Y** | PASS | 67ms | Stdin from `echo Y`, prompt shown, real runs. |
| 4 | **prompt N** | PASS | 63ms | Stdin from `echo N`, "cancelled", real does NOT run, rc=1. |
| 5 | **throttle (10s cooldown)** | PASS | 10.117s | The original 5s/30s bug is fixed. Sleep completes naturally. |
| 5b | **throttle (30s cooldown)** | PASS | 30.121s | Same as #5 with default cooldown. Full 30s sleep completes. |
| 6 | **fail-open (non-exec binary)** | PASS | 36ms | `chmod -x` on agent0waste, "Permission denied" on stderr, real runs. |
| 7 | **fail-open (db unreadable)** | PASS | 279ms | `chmod 000` on state.db, "could not read...; fail-open: allowing call" on stderr, real runs. |
| 8 | **real hermes (--version)** | PASS | 388ms | `hermes --version` through shim, real hermes output, MD5 `cf9f1706e77f519c93ab2d08a5ffb2ac` unchanged. |
| 9 | **cache hit (warm)** | PASS | 64ms (hit) vs 64ms (miss) | OS page cache is warm, so the win is small here. On a cold cache, the speedup would be ~150ms (state.db read skipped). |
| 10 | **mtime invalidation** | PASS | <50ms | `touch ~/.hermes/state.db` bumps mtime → cache returns miss, heuristics re-run, cache mtime updated. |
| 11 | **intercept run (legacy one-shot)** | PASS | <50ms | `agent0waste intercept run -- /bin/echo "..."` still works. The cache is wired into this path too. |

### What changed in the shim

The shim's `do_check` function (the heart of the new design):

```bash
do_check() {
    local outf errf
    outf=$(mktemp); errf=$(mktemp)
    (
        "__AGENT0WASTE_PATH__" intercept check --command "$REAL $*" <&3
    ) > "$outf" 2> "$errf" &
    local child_pid=$!
    # Background timer, fully detached from this shell's job table.
    nohup bash -c "sleep $TIMEOUT 2>/dev/null; if kill -0 $child_pid 2>/dev/null; then echo '[agent0waste: check timed out (${TIMEOUT}s); running unwrapped]' >&2; kill -KILL $child_pid 2>/dev/null || true; fi" >/dev/null 2>&1 &
    wait "$child_pid" 2>/dev/null
    local rc=$?
    cat "$errf" >&2
    cat "$outf"
    rm -f "$outf" "$errf"
    return $rc
}
```

The `nohup` is critical. A `( sleep 5 ) &` background subshell is
tracked by the parent shell's job table, and when the parent
exits, bash waits for it (and any exec'd children) to finish.
That made every shim invocation wait 5s even on a 35ms check.
`nohup` detaches the subshell from the job table, so bash doesn't
wait on it. The trade-off: we can't cancel the timer. It runs
to completion even after the child finishes. The `kill -0` inside
the timer prevents acting on a recycled PID.

### Bug found and fixed during v0.4.1 dogfood

**5s shim timer blocking fast checks.** First version of the
new shim used `( sleep $TIMEOUT; ... ) &` for the timer. A 35ms
check still cost 5s because bash's $() subshell implicitly waits
for all background jobs of the subshell to finish — including
the timer subshell's exec'd `sleep`. Switched to `nohup bash -c
"..." &` which detaches the timer from the job table. Fix
documented in the shim template's comment.

### Design tension noted (not fixed in v0.4.1)

**Per-rule `cache_ttl_s` is plumbed into the config and shown in
`intercept status` but the cache module uses a fixed 30s TTL.** The
reason: `pick_decision` returns a `Decision` but doesn't tell us
which rule fired, so the cache integration can't look up the
right rule's TTL. Tracked as a follow-up to #7: add a `rule_id`
field to `Decision` (or a `DecisionWithSource` return type from
`check()`) and plumb the per-rule TTL through. For v0.4.1, the
fixed 30s TTL is the safe default — all four default rules have
`cache_ttl_s = 30` anyway.

### Demo

The same `agent0waste-intercept-demo.gif` from v0.4.0-beta still
applies — the user-visible decision paths (allow/prompt/throttle)
are unchanged. What changed is the elapsed time: the throttle
sleep in the shim completes naturally now, and the cache hit
path skips the state.db read.

### How to reproduce

```bash
# Same setup as v0.4.0-beta validation (~/bin/dogfood-test).
# Then per-path commands are in the v040-demo.sh script.
bash /Users/.../Agent0Waste/scripts/v040-demo.sh
```

### Not validated here (out of scope for v0.4.1)

- Local LLM proxy (Mechanism B) — v0.4.2+
- Fail-closed mode — v0.4.2+ (issue #4)
- macOS `sandbox-exec` wrapper — v0.4.2+ (issue #5)
- Cross-agent interception — v0.5.0
- Per-rule `cache_ttl_s` plumbing — v0.4.2+ follow-up
- 5s → 500ms shim timeout (now safe with the cache) — v0.4.2+
