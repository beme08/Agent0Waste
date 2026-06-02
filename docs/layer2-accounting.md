# Layer 2: Accounting

**Goal:** Measure token consumption across AI tools.

The active design for this layer is in **[v0.2-design.md](./v0.2-design.md)** —
confirmed decisions, schema, commands, and implementation order.

This file is kept as a reference for the high-level intent and the original
open questions that the v0.2 design resolved.

## What this layer provides

- Per-tool usage
- Per-model usage
- Session history
- Estimated cost
- Trends over time

## Originally open questions (resolved in v0.2 design)

| Original question | Resolved as |
|---|---|
| Manual start/stop vs automatic wrapping? | `run -- <cmd>` wrapper |
| How should local models (Ollama, MLX) be accounted for? | Same parser; cost is $0 for local; record `is_local: true` |
| How often should pricing tables update? | Built-in defaults, optional user override file, no auto-update |
| SQLite only, or also exportable JSON/CSV? | JSON only for v0.2; sessions are JSON-per-file |
| What data can be collected without proxies? | Wrapper captures stdout/stderr + wall time + tool logs |
| Should we track context compression savings? | No — deferred to Layer 3 |
