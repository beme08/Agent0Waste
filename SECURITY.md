# Security

Agent0Waste is designed with a strong local-first and permission-first approach.

## Principles

- No network calls by default
- No data is ever sent anywhere
- All scanning requires explicit or cached user permission
- Only reads from known agent directories (~/.hermes, etc.)

## Current status (v1)

- Permission system implemented via `permission.rs`
- macOS only
- No secrets or credentials are read

## Reporting issues

If you find a security issue, please open an issue with the `security` label.