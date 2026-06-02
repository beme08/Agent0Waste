# Layer 2: Accounting

**Goal:** Measure token consumption across AI tools.

Provides:
- Per-tool usage
- Per-model usage
- Session history
- Estimated cost
- Trends over time

## Proposed Commands

```bash
agent0waste track start
agent0waste track stop
agent0waste track status
agent0waste history
```

### Alternative UX ideas (pending validation)
```bash
agent0waste track claude
agent0waste track goose
```

> **Note:** These are speculative. Final command design will be validated by user feedback after v0.1.

## Proposed Schema

Minimal entities only:

- **sessions** — individual tracked runs (start time, end time, tool, model)
- **token_usage** — token counts per session (input, output, total)
- **tools** — registry of known agent CLIs (Hermes, Claude Code, Goose, etc.)
- **models** — model names + rough pricing metadata

## Open Questions

- Manual start/stop vs automatic wrapping?
- How should local models (Ollama, MLX, etc.) be accounted for?
- How often should pricing tables update?
- SQLite only, or also exportable JSON/CSV?
- What data can be collected without proxies or heavy instrumentation?
- Should we track context compression / repeated prompt savings?