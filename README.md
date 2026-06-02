# Agent0Waste

**Local-first waste scanner for AI agent CLIs** (Hermes, Claude Code, OpenCode, etc.)

Shows exactly what is wasting RAM, disk space, and tokens on your machine -- with **zero data leaving your device**.

> v0.1.0-alpha -- macOS only

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

# (clean is a no-op in v0.1 — Layer 2 ships in v0.2)
./target/release/agent0waste clean
```

Or install globally:

```bash
cargo install --path .
agent0waste scan
```

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
| 2 -- Accounting | designed (v0.2) | Per-tool / per-model / per-session token tracking + cost estimation |
| 3 -- Optimization | planned | Iteration memory, context compression across runs |
| 4 -- Interception | future | Optional real-time proxy for output filtering |

See [docs/roadmap.md](docs/roadmap.md) for details.

## Philosophy

Privacy-first. Local-only. No telemetry. No accounts.

If it can't run completely offline with zero data leaving your machine, it doesn't belong here.

## License

MIT
