# Agent0Waste

**Local-first waste scanner for AI agent CLIs** (Hermes, Claude Code, OpenCode, etc.)

Shows exactly what is wasting RAM, disk space, and tokens on your machine — with **zero data leaving your device**.

> v1 Status: macOS only

## Features (v1)

- 100% local (no network calls)
- Permission-first design
- Real detection of:
  - Tool bloat (expensive tools enabled by default)
  - Dead/abandoned profiles
  - Noisy or redundant cron jobs
  - Memory layer usage
  - Model awareness (local vs API + rough cost signals)
- Clean one-shot terminal report

**Platform support (v1)**: macOS only

## Installation

```bash
git clone https://github.com/beme08/Agent0Waste.git
cd Agent0Waste
cargo install --path .
```

## Usage

```bash
# One-shot scan (recommended)
agent0waste scan

# Show waste report with suggestions
agent0waste report
```

## Why this exists

Most local AI agent setups slowly accumulate waste:

- Too many expensive tools enabled by default
- Abandoned profiles and configs
- Cron jobs that keep expensive models or toolsets alive
- No visibility into which model + provider is actually being used

Agent0Waste gives you that visibility without ever phoning home.

## Roadmap

- Real token usage parsing from session logs
- Cross-platform support (Linux)
- Safe auto-cleanup mode (`--fix`)
- Support for Claude Code and OpenCode state
- Cost estimation per model

## Philosophy

Privacy-first. Local-only. No telemetry. No accounts.

If it can't run completely offline with zero data leaving your machine, it doesn't belong in v1.