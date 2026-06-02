# Agent0Waste Vision

Local-first waste elimination engine for AI agent CLIs.

## Layer 1: Audit
**Goal:** Find waste

- `agent0waste scan`
- `agent0waste report`
- `agent0waste history`

## Layer 2: Accounting
**Goal:** Measure waste

- `agent0waste track start`
- `agent0waste track stop`
- `agent0waste track status`
- `agent0waste history`

## Layer 3: Optimization
**Goal:** Reduce waste

- `agent0waste remember`

## Layer 4: Interception
**Goal:** Prevent waste

- `agent0waste proxy` (optional)

---

## Current Status

### Shipped (v0.1.0-alpha)
- Local profile and tool detection
- Clean one-shot report generation
- History snapshots
- Zero network calls, permission-first design

### Experimental
- None yet

### Future Work
- Layer 2: Token accounting
- Layer 3: Optimization recommendations
- Layer 4: Proxy interception (optional)