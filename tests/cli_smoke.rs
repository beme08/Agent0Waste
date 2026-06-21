// CLI smoke tests for the `agent0waste` binary.
//
// These tests spawn the real compiled binary via `std::process::Command` and
// assert on its stdout / stderr / exit code. They cover the read-only,
// no-network subcommands so the tests run quickly and have no side effects
// on `~/.hermes/`, `~/.config/agent0waste/`, or the user's data.
//
// `env!("CARGO_BIN_EXE_agent0waste")` is stable since Rust 1.43 and gives us
// the path to the binary that `cargo test` just built.

use std::process::{Command, Output};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_agent0waste")
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn {}: {}", bin(), e))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_exit_zero(out: &Output, ctx: &str) {
    assert!(
        out.status.success(),
        "{}\nexit code: {:?}\nstdout: {}\nstderr: {}",
        ctx,
        out.status.code(),
        stdout(out),
        stderr(out),
    );
}

#[test]
fn version_prints_cargo_version() {
    let out = run(&["--version"]);
    assert_exit_zero(&out, "--version should exit 0");
    let s = stdout(&out);
    assert!(
        s.contains("agent0waste") && s.contains("0.6.0"),
        "--version output should be 'agent0waste 0.6.0'-shaped, got: {:?}",
        s
    );
}

#[test]
fn help_lists_top_level_subcommands() {
    let out = run(&["--help"]);
    assert_exit_zero(&out, "--help should exit 0");
    let s = stdout(&out);
    // All eight top-level subcommands should be listed. If one is renamed
    // or removed this assertion will tell us immediately.
    for cmd in &[
        "scan",
        "history",
        "clean",
        "pricing",
        "intercept",
        "sessions",
        "run",
        "cost",
    ] {
        assert!(
            s.contains(cmd),
            "--help should list subcommand `{}`, got:\n{}",
            cmd,
            s
        );
    }
}

#[test]
fn pricing_list_ships_default_models() {
    let out = run(&["pricing", "list"]);
    assert_exit_zero(&out, "pricing list should exit 0");
    let s = stdout(&out);
    // Header assertion: this is the user-facing proof the pricing table
    // ships with the binary. If the table is empty the count would be 0.
    assert!(
        s.contains("Known Models") && s.contains("total"),
        "pricing list header missing, got:\n{}",
        s
    );
    // A few well-known default models from the built-in table.
    for model in &["gpt-4o", "claude-3-5-sonnet", "claude-sonnet-4"] {
        assert!(
            s.contains(model),
            "pricing list should include default model `{}`, got:\n{}",
            model,
            s
        );
    }
}

#[test]
fn intercept_check_returns_valid_decision_json() {
    // `intercept check` may consult ~/.hermes/state.db and the local
    // intercept.toml, so the specific decision is machine-dependent.
    // We only assert on the *shape* of the response: valid JSON with
    // the four documented keys and a recognized decision value.
    // The exit code is the decision's exit code, not 0:
    //   Allow=0, Throttle=64, Prompt=65, Deny=66 (see Decision::exit_code).
    let out = run(&[
        "intercept",
        "check",
        "--model",
        "gpt-4o",
        "--tokens",
        "100",
        "--source",
        "cli",
    ]);
    let code = out.status.code().unwrap_or(-1);
    assert!(
        matches!(code, 0 | 64 | 65 | 66),
        "intercept check exit code {} is not a valid decision code (expected 0/64/65/66).
stdout: {}
stderr: {}",
        code,
        stdout(&out),
        stderr(&out),
    );
    let s = stdout(&out);
    let json: serde_json::Value = serde_json::from_str(s.trim())
        .unwrap_or_else(|e| panic!("intercept check output is not valid JSON: {}\nbody: {}", e, s));
    let decision = json
        .get("decision")
        .and_then(|v| v.as_str())
        .unwrap_or_else(|| panic!("missing `decision` field in: {}", s));
    assert!(
        matches!(decision, "allow" | "throttle" | "prompt" | "deny"),
        "decision `{}` is not one of allow/throttle/prompt/deny",
        decision
    );
    for key in &["cooldown_s", "reason", "hint"] {
        assert!(
            json.get(key).is_some(),
            "missing `{}` field in intercept check output: {}",
            key,
            s
        );
    }
}

#[test]
fn intercept_trace_renders_six_steps() {
    // `intercept trace` prints a multi-line trace with the six steps
    // [1] load, [2] cache, [3] heuristics, [4] decision, [5] cache store,
    // [6] sandbox. Asserting on these step labels catches any dispatch
    // regression where a step is renamed or removed.
    let out = run(&["intercept", "trace", "--model", "gpt-4o", "--source", "cli"]);
    assert_exit_zero(&out, "intercept trace should exit 0");
    let s = stdout(&out);
    for label in &[
        "[1] load",
        "[2] cache",
        "[3] heuristics",
        "[4] decision",
        "[5] cache store",
        "[6] sandbox",
    ] {
        assert!(
            s.contains(label),
            "intercept trace should contain step label `{}`, got:\n{}",
            label,
            s
        );
    }
}
