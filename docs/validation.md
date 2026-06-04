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
