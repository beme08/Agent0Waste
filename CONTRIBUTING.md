# Contributing to Agent0Waste

Thank you for your interest in contributing!

## Project Goals

- Stay local-first and privacy-hardened
- Keep the tool fast and one-shot by default
- Support multiple agent CLIs over time (Hermes, Claude Code, etc.)
- Maintain high code quality and test coverage

## How to Contribute

1. **Open an issue first** for any non-trivial change
2. Fork the repo and create a feature branch
3. Make sure `cargo test` and `cargo build --release` pass
4. Submit a pull request

## Development Setup

```bash
git clone https://github.com/beme08/Agent0Waste.git
cd Agent0Waste
cargo build
cargo test
```

## Current Focus (v1)

- macOS support only
- Real data collection
- Clean reporting

Linux and Windows support are planned for **v2**.

## Code Style

- Keep it simple and readable
- Prefer one-shot execution over long-running processes
- Document any new permission requirements clearly

## Questions?

Open an issue with the `question` label.