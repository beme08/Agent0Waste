# Agent0Waste

**Local-first waste scanner for AI agent CLIs** (Hermes, Claude Code, OpenCode, etc.)

Shows exactly what is wasting RAM, disk space, and tokens on your machine -- with **zero data leaving your device**.

> v0.4.0-beta -- macOS only

<p align="center">
  <img src="agent0waste-demo/renders/agent0waste-demo.gif" alt="Agent0Waste demo -- real scan of local Hermes profiles (505 KB, 30s)" width="720" />
</p>

<p align="center">
  <em>Real scan output from <code>agent0waste scan</code> on the developer's machine. <a href="agent0waste-demo/renders/agent0waste-demo.mp4">Watch the 1920x1080 MP4</a> for full quality.</em>
</p>

## What it does

Agent0Waste scans your local AI agent setup and finds waste:

- **Tool bloat** -- expensive tools enabled by default that inflate context and slow inference
- **Dead profiles** -- abandoned profiles consuming disk and config space
- **Orphan cron jobs** -- scheduled jobs keeping expensive models alive
- **Memory layer pressure** -- accumulated memory layers eating RAM
- **Model awareness** -- detects local vs remote model + context window
- **Scan history** -- tracks efficiency across multiple runs

All detection is 100% local. No network calls. No telemetry.

## Installation

```bash
git clone https://github.com/beme08/Agent0Waste.git
cd Agent0Waste
cargo build --release
./target/release/agent0waste
```

## Usage

```bash
# Run a scan (default command)
./target/release/agent0waste scan

# Scan with model override
./target/release/agent0waste scan --model claude-sonnet-4-20250514 --provider anthropic

# Show scan history
./target/release/agent0waste history

# Layer 2: cost report from recorded + Hermes sessions
./target/release/agent0waste cost --since 7

# Layer 3: heuristic warnings
./target/release/agent0waste cost --warnings

# Layer 4: install a shim that consults heuristics before each call
./target/release/agent0waste intercept enable hermes
./target/release/agent0waste intercept status
./target/release/agent0waste intercept disable hermes

# (clean is a no-op — observation only, by design)
./target/release/agent0waste clean
```

Or install globally:

```bash
cargo install --path .
agent0waste scan
```

### Layer 4 (interception)

`agent0waste intercept enable <cmd>` installs a small bash shim in
`~/.local/share/agent0waste/shims/<cmd>` (NOT `~/.local/bin/`, which
is shared with cargo, uv, and homebrew). Add that dir to your PATH
to make interception active:

```bash
export PATH="$HOME/.local/share/agent0waste/shims:$PATH"
```

The shim records the call (Layer 2), runs heuristics on recent Hermes
sessions (Layer 3), and either `allow`s, `throttle`s, or `prompt`s
based on the rule table. Fail-open by default — if Agent0Waste is
unreachable, the call proceeds and a stderr message is logged.

The shim is a real file you can `cat` and audit; nothing in your shell
rc or `~/.hermes/config.yaml` is touched. `intercept disable <cmd>`
removes the shim. `intercept migrate <cmd>` moves a legacy
v0.4.0-alpha shim from `~/.local/bin/` to the new location.

<p align="center">
  <img src="agent0waste-demo/renders/agent0waste-intercept-demo.gif" alt="Agent0Waste interception demo — allow, prompt (Y/N), throttle paths + real hermes --version" width="720" />
</p>

<p align="center">
  <em>Real run of <code>scripts/v040-demo.sh</code> on the developer's machine. <a href="docs/validation.md#v040-beta--interception-validated-2026-06-03">See the validation record</a> for the full checklist + verbatim output.</em>
</p>

## Example output

```
Agent0Waste -- Local Token Waste Scanner

  reading config          17% [███░░░░░░░░░░░░░░░░░]
  scanning profiles       33% [██████░░░░░░░░░░░░░░]
  detecting model         50% [██████████░░░░░░░░░░]
  checking memory         66% [█████████████░░░░░░░]
  analyzing tools         83% [████████████████░░░░]
  computing waste        100% [████████████████████]
  scan complete          100% [████████████████████]

Model     : grok-4.3 (xai-oauth)  [remote]

Hermes profiles (8):
  hermes-core          tools: 13  (3 expensive)
  hermes-cto           tools:  0  (clean)
  ...

Waste detected:
  [high] tool_bloat -- 3 expensive tools enabled by default in hermes-core (web, browser, computer_use)
       -> ~20-40k tokens/mo + lower RAM

Efficiency: 85% [█████████████████░░░]
If cleaned: 98% [███████████████████░]
  -> -18k tokens/mo | +13% speed
Machine: macOS * Mac16,7
```

## Layer roadmap

| Layer | Status | What it does |
|-------|--------|--------------|
| 1 -- Audit | shipped (v0.1) | Scan for config bloat, tool waste, memory pressure, model awareness |
| 2 -- Accounting | shipped (v0.2.1) | Per-tool / per-model / per-session token tracking + cost estimation from `state.db` |
| 3 -- Heuristics | shipped (v0.3.1) | cache_bloat, prompt_growth, auto_routing, model_instability findings |
| 4 -- Interception | shipped (v0.4.0-beta) | Per-command shim that consults heuristics before each LLM call. Opt-in: `agent0waste intercept enable <cmd>`. macOS-only. |

See [docs/roadmap.md](docs/roadmap.md) for details.

## Philosophy

Privacy-first. Local-only. No telemetry. No accounts.

## License

MIT
