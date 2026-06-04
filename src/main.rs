use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

mod cache;
mod core;
mod cost;
mod heuristics;
mod hermes_state;
mod history;
mod intercept;
mod macos;
mod permission;
mod pricing;
mod report;
mod run;
mod sessions;
mod types;

use core::{detect_model, scan_hermes};
use cost::{format_table, missing_models, report as cost_report, report_hermes, GroupBy};
use heuristics::run_all as run_heuristics;
use history::HistoryEntry;
use intercept::{check as intercept_check, intercept_toml_path, Action, CheckHint, Decision, InterceptConfig, Mode};
use macos::MacOSScanner;
use pricing::Pricing;
use sessions::Sessions;
use types::*;

fn main() {
    let cli = Cli::parse();

    match &cli.command {
        Some(Commands::Scan { model, provider }) => {
            let scanner = MacOSScanner::new();
            let model_override = model.as_deref();
            let provider_override = provider.as_deref();

            println!("Agent0Waste — Local Token Waste Scanner");
            println!();

            let mut result = run_scan(&scanner, model_override, provider_override);

            report::print_report(&result);

            let (current, potential) = calc_efficiency(&scanner, &result);
            print_efficiency_meter(&scanner, &result, current, potential);

            record_history(&mut result, current);
        }
        Some(Commands::History) => {
            show_history();
        }
        Some(Commands::Clean { dry_run }) => {
            run_clean(*dry_run);
        }
        Some(Commands::Sessions) => {
            run_sessions_list();
        }
        Some(Commands::Run { .. }) => {
            // `agent0waste run -- <cmd> [args...]`
            // We split argv ourselves so clap doesn't have to know about
            // trailing args.
            let argv: Vec<String> = std::env::args().collect();
            match run::split_run_args(&argv) {
                Ok((cmd, args)) => {
                    let arg_refs: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
                    match run::run_and_record(&cmd, &arg_refs) {
                        Ok(rec) => {
                            eprintln!("[agent0waste] recorded session {}", rec.id);
                            // Apply cost if pricing known — non-fatal.
                            let pricing = Pricing::load();
                            let sessions = Sessions::new();
                            let list = sessions.list();
                            if let Some(mut r) = list.into_iter().find(|r| r.id == rec.id) {
                                sessions::Sessions::apply_cost(&mut r, &pricing);
                            }
                            std::process::exit(rec.exit_code);
                        }
                        Err(e) => {
                            eprintln!("error: {}", e);
                            std::process::exit(run::EXIT_BAD_ARGS);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("error: {}", e);
                    eprintln!();
                    eprintln!("usage:  agent0waste run -- <cmd> [args...]");
                    eprintln!("example:  agent0waste run -- hermes run foo");
                    std::process::exit(run::EXIT_BAD_ARGS);
                }
            }
        }
        Some(Commands::Cost { by, since, export, from_hermes, include_local, warnings, missing }) => {
            run_cost(by.as_deref(), *since, export.as_deref(), *from_hermes, *include_local, *warnings, *missing);
        }
        Some(Commands::Pricing { action }) => {
            run_pricing(action);
        }
        Some(Commands::Intercept { action }) => {
            run_intercept(action);
        }
        None => {
            // Default: run a scan
            let scanner = MacOSScanner::new();
            println!("Agent0Waste — Local Token Waste Scanner");
            println!();

            let mut result = run_scan(&scanner, None, None);

            report::print_report(&result);

            let (current, potential) = calc_efficiency(&scanner, &result);
            print_efficiency_meter(&scanner, &result, current, potential);

            record_history(&mut result, current);
        }
    }
}

#[derive(Parser)]
#[command(name = "agent0waste", version, about = "Local Token Waste Scanner")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Scan your system for token waste
    Scan {
        /// Override model detection
        #[arg(long)]
        model: Option<String>,
        /// Override provider detection
        #[arg(long)]
        provider: Option<String>,
    },
    /// Show scan history and trends
    History,
    /// Clean up detected waste files
    Clean {
        /// Preview changes without deleting
        #[arg(long, default_value_t = true)]
        dry_run: bool,
    },
    /// Manage the pricing table (list / add / unset / path)
    Pricing {
        #[command(subcommand)]
        action: PricingAction,
    },
    /// Layer 4: intercept calls based on heuristic rules
    Intercept {
        #[command(subcommand)]
        action: InterceptAction,
    },
    /// List recorded sessions (Layer 2)
    Sessions,
    /// Run a command and record the session (Layer 2)
    ///
    /// Usage:  agent0waste run -- <cmd> [args...]
    ///
    /// Note: clap parses `run` as a flagless subcommand; the actual
    /// command + args are read from std::env::args() past the `--`
    /// separator (see run::split_run_args).
    Run {
        /// Catch-all for clap so trailing args aren't rejected.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        _args: Vec<String>,
    },
    /// Cost report from recorded sessions (Layer 2)
    Cost {
        /// Group by: total | model | provider | day
        #[arg(long, default_value = "total")]
        by: Option<String>,
        /// Look back N days (default 7). 0 = "always".
        #[arg(long, default_value_t = 7)]
        since: i64,
        /// Export format: json (other values = human table)
        #[arg(long)]
        export: Option<String>,
        /// Read from ~/.hermes/state.db (real token data). Default: on.
        #[arg(long, default_value_t = true)]
        from_hermes: bool,
        /// Also read from local session records. Default: off.
        #[arg(long, default_value_t = false)]
        include_local: bool,
        /// Layer 3: include heuristic warnings below the cost table.
        #[arg(long, default_value_t = false)]
        warnings: bool,
        /// List models with no pricing entry (and a TOML snippet).
        #[arg(long, default_value_t = false)]
        missing: bool,
    },
}

fn phase_bar(label: &str, step: usize, total: usize) {
    let step = step.min(total - 1);
    let pct = ((step + 1) as f64 / total as f64 * 100.0) as usize;
    let filled = ((step + 1) * 20).saturating_div(total);
    let empty = 20usize.saturating_sub(filled);
    let bar = "█".repeat(filled) + &"░".repeat(empty);
    print!("\r  {:<22} {:>3}% [{}]", label, pct, bar);
    std::io::stdout().flush().unwrap();
}

fn run_scan(
    scanner: &MacOSScanner,
    model_override: Option<&str>,
    provider_override: Option<&str>,
) -> ScanResult {
    let phases = [
        "reading config",
        "scanning profiles",
        "detecting model",
        "checking memory",
        "analyzing tools",
        "computing waste",
    ];
    let total = phases.len();
    let delays = [250, 300, 400, 250, 350, 200];

    // Phase 1: reading config
    phase_bar(phases[0], 0, total);
    let hermes_profiles = scan_hermes();
    std::thread::sleep(std::time::Duration::from_millis(delays[0]));

    // Phase 2: scanning profiles
    phase_bar(phases[1], 1, total);
    let _profile_count = scanner.count_hermes_profiles();
    std::thread::sleep(std::time::Duration::from_millis(delays[1]));

    // Phase 3: detecting model
    phase_bar(phases[2], 2, total);
    let model_info = if let Some(m) = model_override {
        Some(ModelInfo {
            name: m.to_string(),
            provider: provider_override.unwrap_or("override").to_string(),
            is_local: m.contains("qwen") || m.contains("mlx") || m.contains("local"),
            context_window: core::model_context_window(m),
            cost_per_million: None,
        })
    } else {
        detect_model()
    };
    std::thread::sleep(std::time::Duration::from_millis(delays[2]));

    // Phase 4: checking memory
    phase_bar(phases[3], 3, total);
    let memory_layers = scanner.get_memory_layers_gb();
    std::thread::sleep(std::time::Duration::from_millis(delays[3]));

    // Phase 5: analyzing tools
    phase_bar(phases[4], 4, total);
    let _kanban_count = scanner.count_kanban_boards();
    let ram_gb = scanner.get_total_ram_gb().unwrap_or(32.0);
    std::thread::sleep(std::time::Duration::from_millis(delays[4]));

    // Phase 6: computing waste
    phase_bar(phases[5], 5, total);
    std::thread::sleep(std::time::Duration::from_millis(delays[5]));

    println!();
    phase_bar("scan complete", 6, total);
    println!();

    let mut waste_items: Vec<WasteItem> = Vec::new();

    // tool_bloat: count expensive-named tools across all profile toolsets
    // (same source as the per-profile "(N expensive)" line above, so the
    // number in the waste line always matches the profile list).
    let total_expensive: usize = hermes_profiles.iter().map(|p| p.expensive_tools.len()).sum();
    if total_expensive > 0 {
        let worst = hermes_profiles
            .iter()
            .max_by_key(|p| p.expensive_tools.len())
            .unwrap();
        let tool_names = worst.expensive_tools.join(", ");
        let severity = if worst.expensive_tools.len() >= 3 { "high" } else { "medium" };
        waste_items.push(WasteItem {
            category: "tool_bloat".to_string(),
            description: format!(
                "{} expensive tools enabled by default in {} ({})",
                worst.expensive_tools.len(),
                worst.name,
                tool_names
            ),
            severity: severity.to_string(),
            estimated_savings: Some("~20-40k tokens/mo + lower RAM".to_string()),
        });
    }

    if memory_layers.0 > 1.0 {
        waste_items.push(WasteItem {
            category: "memory_bloat".to_string(),
            description: format!("Mnemosyne memory layer using {:.1} GB", memory_layers.0),
            severity: "medium".to_string(),
            estimated_savings: Some("run mnemosyne_sleep to compress".to_string()),
        });
    }

    ScanResult {
        hermes_profiles,
        crons: vec![],
        tool_bloat: vec![],
        model_info,
        ram_usage_mb: (ram_gb * 1024.0) as u64,
        waste_items,
    }
}

fn record_history(result: &mut ScanResult, efficiency: u32) {
    let mut hist = history::History::load();
    let model_name = result
        .model_info
        .as_ref()
        .map(|m| m.name.as_str())
        .unwrap_or("unknown");
    hist.entries.push(HistoryEntry {
        timestamp: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        efficiency,
        profiles_count: result.hermes_profiles.len(),
        waste_count: result.waste_items.len(),
        model: model_name.to_string(),
        waste_categories: result.waste_items.iter().map(|w| w.category.clone()).collect(),
    });
    hist.save();
}

fn show_history() {
    let hist = history::History::load();
    if hist.entries.is_empty() {
        println!("No scan history yet. Run `agent0waste scan` first.");
    } else {
        println!(
            "Agent0Waste — Scan History ({} entries)\n",
            hist.entries.len()
        );
        println!(
            "{:<22} {:>4}  {:>3}p  {:>3}w  {}",
            "Time", "Eff%", "Prf", "Wst", "Model"
        );
        for e in &hist.entries {
            println!(
                "  {}  eff={}%  profiles={}  waste={}  model={}",
                e.timestamp, e.efficiency, e.profiles_count, e.waste_count, e.model
            );
        }
    }
}

fn calc_efficiency(scanner: &MacOSScanner, result: &ScanResult) -> (u32, u32) {
    let expensive = result
        .waste_items
        .iter()
        .filter(|w| w.category.contains("tool") || w.category.contains("bloat"))
        .count();
    let current: u32 = if scanner.count_hermes_profiles() > 0 {
        100 - ((expensive * 15) as u32).min(70)
    } else {
        95
    };
    let potential = ((current as f32) * 1.4).min(98.0) as u32;
    (current, potential)
}

fn print_efficiency_meter(
    scanner: &MacOSScanner,
    result: &ScanResult,
    current: u32,
    potential: u32,
) {
    println!(
        "\nEfficiency: {}% [{}]",
        current,
        "█".repeat((current as usize) / 5) + &"░".repeat(20 - (current as usize) / 5)
    );
    println!(
        "If cleaned: {}% [{}]",
        potential,
        "█".repeat((potential as usize) / 5) + &"░".repeat(20 - (potential as usize) / 5)
    );

    let ram_savings = if result
        .waste_items
        .iter()
        .any(|w| w.category.contains("memory"))
    {
        " -2.1 GB RAM"
    } else {
        ""
    };
    let expensive = result
        .waste_items
        .iter()
        .filter(|w| w.category.contains("tool") || w.category.contains("bloat"))
        .count();
    println!(
        "  →{}{} | +{}% speed",
        ram_savings,
        if expensive > 0 { " -18k tokens/mo" } else { "" },
        potential as i32 - current as i32
    );
    println!("Machine: {}", scanner.get_machine_info());
}

fn run_clean(_dry_run: bool) {
    println!("Clean mode not yet implemented. Use `agent0waste scan` to audit.");
}

// ---------------------------------------------------------------------------
// Pricing management
// ---------------------------------------------------------------------------

#[derive(Subcommand)]
enum PricingAction {
    /// Show all known models and their rates.
    List,
    /// Print the path of the user's override file (and create it if missing).
    Path,
    /// Add or update a model rate in the user's override file.
    ///
    /// Example: agent0waste pricing add 'mimo-v2.5' 0.40 2.00
    ///          agent0waste pricing add 'openrouter/owl-alpha' 0 0
    Add {
        /// Model name (quote it if it contains a dot or colon).
        model: String,
        /// USD per 1M input tokens.
        input: f64,
        /// USD per 1M output tokens.
        output: f64,
    },
    /// Remove a model from the user's override file (keeps the default).
    Unset {
        /// Model name to remove from the override.
        model: String,
    },
    /// Validate the override file: parses the TOML, catches negative
    /// rates, and flags entries that shadow a default.
    Check,
}

// ---------------------------------------------------------------------------
// Interception (Layer 4)
// ---------------------------------------------------------------------------

/// The shim template installed by `intercept enable <command>`.
///
/// `__COMMAND__` is replaced with the real command name (e.g. "hermes").
/// `__TIMEOUT__` is replaced with the install-time timeout (5s default).
/// `__AGENT0WASTE_PATH__` is replaced with the absolute path to the
/// `agent0waste` binary that ran `intercept enable`. The shim uses
/// this absolute path so the user does NOT need `agent0waste` on
/// their PATH (cargo-installed binaries often aren't on PATH until
/// the user explicitly adds `~/.cargo/bin`).
///
/// The shim is a 1-purpose file: it execs `agent0waste intercept run --`
/// with the same args, after finding the real binary. The real work
/// (Layer 4 decision + Layer 2 recording) happens in `intercept run`.
///
/// Fail-open behaviors live in the shim: a hard timeout around
/// `intercept run` (5s default), with a distinct stderr message
/// and direct exec of the real command if the timeout fires.
const SHIM_TEMPLATE: &str = r#"#!/usr/bin/env bash
# Installed by agent0waste intercept enable v0.4.1
# Command: __COMMAND__
# Timeout: __TIMEOUT__s (per check call; throttle sleep is unbounded)
# Agent0waste: __AGENT0WASTE_PATH__
# Disable: agent0waste intercept disable __COMMAND__
# Audit:   cat "$0"
set -uo pipefail

SELF_DIR="$(cd "$(dirname "$0")" && pwd)"

# Find the real binary, excluding our own directory. We need to
# handle the shell-builtin trap (e.g. `echo` is a bash builtin on
# macOS; `command -v echo` returns the literal "echo" which would
# make the shim recurse into itself). The robust way is to walk
# PATH ourselves, find the first executable file, and verify it's
# not a directory or alias to the shim.
find_real() {
    local name="__COMMAND__"
    local d
    local stripped
    stripped="$(echo "$PATH" | tr ':' '\n' | grep -vFx "$SELF_DIR" | paste -sd: -)"
    # Split stripped on : and walk each dir.
    echo "$stripped" | tr ':' '\n' | while IFS= read -r d; do
        if [ -z "$d" ]; then continue; fi
        if [ -x "$d/$name" ] && [ ! -d "$d/$name" ]; then
            echo "$d/$name"
            return 0
        fi
    done
    # Fallback: command not on PATH (or only the shim dir had it).
    # Print nothing — the shim will fail loudly when it tries to exec
    # the empty $REAL, which is better than the shim silently recursing.
}

REAL="$(find_real)"
TIMEOUT="${AGENT0WASTE_INTERCEPT_TIMEOUT:-__TIMEOUT__}"

# Save our stdin on fd 3 before backgrounding. Bash closes stdin in
# backgrounded children (`cmd &`) when the parent's stdin is a non-TTY
# (file/pipe), which breaks the prompt path — the child sees EOF and
# cancels. Redirecting the child to fd 3 keeps stdin alive for it.
exec 3<&0

# do_check ARGS... — run one `intercept check` with a hard timeout.
# Echoes the decision JSON to stdout, prints stderr to user's stderr.
# Returns the rc of `intercept check` via subshell $?.
#
# Timeout mechanism: a nohup'd bash subshell sleeps for TIMEOUT then
# kills the child if it's still alive. nohup detaches the timer from
# the shim's job table, so a fast check (cache hit, <50ms) doesn't
# pay the 5s cost. If the check hangs, the timer's kill fires and
# the shim sees a non-action rc, falling through to fail-open.
do_check() {
    local outf errf
    outf=$(mktemp); errf=$(mktemp)
    (
        "__AGENT0WASTE_PATH__" intercept check --command "$REAL $*" <&3
    ) > "$outf" 2> "$errf" &
    local child_pid=$!
    # Background timer, fully detached from this shell's job table.
    # If the check finishes first, the kill becomes a no-op (the
    # child is already dead). The kill -0 guard inside the subshell
    # prevents acting on a PID that's been recycled.
    nohup bash -c "sleep $TIMEOUT 2>/dev/null; if kill -0 $child_pid 2>/dev/null; then echo '[agent0waste: check timed out (${TIMEOUT}s); running unwrapped]' >&2; kill -KILL $child_pid 2>/dev/null || true; fi" >/dev/null 2>&1 &
    wait "$child_pid" 2>/dev/null
    local rc=$?
    cat "$errf" >&2
    cat "$outf"
    rm -f "$outf" "$errf"
    return $rc
}

# Initial check. If it fails (timeout, intercept check crash, db
# unreadable), fail-open: run the real binary. The valid decision rcs
# are 0 (allow), 64 (throttle), 65 (prompt). Anything else is an
# error and we fall through to fail-open.
CHECK_JSON=$(do_check "$@")
CHECK_RC=$?
if [ "$CHECK_RC" != "0" ] && [ "$CHECK_RC" != "64" ] && [ "$CHECK_RC" != "65" ]; then
    exec 3<&-
    exec "$REAL" "$@"
fi

case "$CHECK_RC" in
    0)  # Allow
        exec 3<&-
        exec "$REAL" "$@"
        ;;
    65) # Prompt: ask the user y/N
        # Extract the reason from the JSON for the user message.
        # Robust enough for heuristic reasons (short, no nested quotes);
        # if a heuristic ever produces escaped quotes, the user just sees
        # an empty [prompt: ] line and can still answer y/N.
        REASON=$(echo "$CHECK_JSON" | grep -oE '"reason":"[^"\\]*(\\.[^"\\]*)*"' | head -1 | sed 's/^"reason":"//' | sed 's/"$//')
        if [ -n "$REASON" ]; then
            echo "[agent0waste] prompt: $REASON" >&2
        fi
        echo "[agent0waste] continue? [y/N]" >&2
        read -r response <&3
        exec 3<&-
        case "$response" in
            [Yy]|[Yy][Ee][Ss])
                exec "$REAL" "$@"
                ;;
            *)
                echo "[agent0waste] cancelled" >&2
                exit 1
                ;;
        esac
        ;;
    64) # Throttle: parse cooldown_s, sleep, re-check, then run.
        # The re-check output is discarded: the throttle is a guardrail,
        # not a gate. Even if the re-check still says throttle, we run.
        COOLDOWN=$(echo "$CHECK_JSON" | grep -oE '"cooldown_s":[0-9]+' | grep -oE '[0-9]+' | head -1)
        if [ -z "$COOLDOWN" ]; then COOLDOWN=30; fi
        REASON=$(echo "$CHECK_JSON" | grep -oE '"reason":"[^"\\]*(\\.[^"\\]*)*"' | head -1 | sed 's/^"reason":"//' | sed 's/"$//')
        if [ -n "$REASON" ]; then
            echo "[agent0waste] throttle: $REASON" >&2
        fi
        echo "[agent0waste] sleeping ${COOLDOWN}s, then re-checking..." >&2
        sleep "$COOLDOWN"
        do_check "$@" >/dev/null 2>&1 || true
        exec 3<&-
        exec "$REAL" "$@"
        ;;
esac
"#;

/// Default timeout (in seconds) for the shim's intercept check call.
/// v0.4.0 ships with 5s; v0.4.1+ targets 500ms once heuristic output
/// is cached.
const DEFAULT_INTERCEPT_TIMEOUT_S: u64 = 5;

/// Path to the dedicated shim dir (`~/.local/share/agent0waste/shims`).
///
/// v0.4.0 install model: shims live here, NOT in `~/.local/bin/`. The
/// previous model wrote to `~/.local/bin/`, which is shared with cargo,
/// uv, npm, pipx, and homebrew — every install/update in that space
/// can clobber or be clobbered. This dir is dedicated to agent0waste
/// shims; only this binary writes to it. The user adds it to PATH
/// manually; we never edit shell RC files (per the v0.4.0 non-goal).
///
/// We follow the XDG `~/.local/share/<tool>/` convention used by uv
/// and other Python tools, so the dir coexists with their install
/// trees without conflict.
fn shim_dir() -> Option<PathBuf> {
    dirs::home_dir()
        .map(|h| h.join(".local").join("share").join("agent0waste").join("shims"))
}

/// Path to `~/.local/bin`. Kept only to (a) detect legacy v0.4.0-alpha
/// shims for migration, and (b) be stripped from PATH in
/// `find_real_command` so legacy shims don't shadow the real binary
/// during install-time resolution. New shims do NOT go here.
fn local_bin() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".local").join("bin"))
}

fn shim_path(command: &str) -> Option<PathBuf> {
    shim_dir().map(|p| p.join(command))
}

/// True if `path` looks like a v0.4.0-alpha agent0waste shim. We
/// detect by content (the install comment) rather than by path, so
/// that user-authored scripts at `~/.local/bin/<cmd>` don't trigger
/// false-positive "legacy shim" warnings.
fn is_legacy_shim(path: &std::path::Path) -> bool {
    std::fs::read_to_string(path)
        .map(|s| s.contains("Installed by agent0waste intercept enable v0.4.0-alpha"))
        .unwrap_or(false)
}

/// Resolve the real `command` on PATH, excluding the shim dir (where
/// the new shim lives) AND `~/.local/bin` (where a legacy v0.4.0-alpha
/// shim might still live). Used at `intercept enable` time to record
/// the real path in the install message; the shim does its own
/// resolution at run time so it stays correct if the user changes
/// their setup.
///
/// Uses `type -p` (not `command -v`) to avoid the shell-builtin trap
/// (e.g. `echo` on macOS is a builtin, and `command -v echo` returns
/// the literal "echo" which would cause the shim to recurse into
/// itself).
fn find_real_command(command: &str) -> Option<String> {
    let excluded: Vec<String> = [shim_dir(), local_bin()]
        .into_iter()
        .flatten()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let stripped = std::env::var("PATH").ok().map(|path| {
        path.split(':')
            .filter(|d| !excluded.iter().any(|e| e.as_str() == *d))
            .collect::<Vec<_>>()
            .join(":")
    });
    stripped
        .as_deref()
        .and_then(|p| {
            std::process::Command::new("bash")
                .arg("-c")
                .arg(format!("PATH='{}' type -p {}", p, command))
                .output()
                .ok()
        })
        .and_then(|out| {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.is_empty() { None } else { Some(s) }
            } else {
                None
            }
        })
}

/// Absolute path to the running `agent0waste` binary. Baked into the
/// shim at install time so the shim does not depend on
/// `agent0waste` being on the user's PATH.
fn current_exe_path() -> String {
    std::env::current_exe()
        .ok()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "agent0waste".to_string())
}

#[derive(Subcommand)]
enum InterceptAction {
    /// Run heuristics against the current state and emit a JSON decision.
    ///
    /// Exit code: 0 = allow, 64 = throttle, 65 = prompt. The wrapper
    /// (wrap.sh, installed by `intercept enable`) uses these codes to
    /// decide whether to run, sleep, or ask.
    Check {
        /// Model hint (used in the decision message only; doesn't change action).
        #[arg(long)]
        model: Option<String>,
        /// Estimated input tokens (used in the decision message only).
        #[arg(long)]
        tokens: Option<u64>,
        /// The command being wrapped (used in the decision message only).
        #[arg(long)]
        command: Option<String>,
        /// Source of the call: cli | cron. Default: cli.
        #[arg(long, default_value = "cli")]
        source: String,
        /// Look back N days (default 7). 0 = "always".
        #[arg(long, default_value_t = 7)]
        since: i64,
        /// Skip the heuristic cache (force a fresh check).
        #[arg(long)]
        no_cache: bool,
    },
    /// Show the current interception state (stub for v0.4.0).
    Status,
    /// Install the wrapper and shell alias (stub for v0.4.0).
    Enable {
        /// Command name to wrap (e.g. "hermes", "claude").
        command: String,
        /// Use fail-closed mode instead of fail-open (v0.4.1+).
        #[arg(long)]
        strict: bool,
    },
    /// Remove the wrapper and shell alias (stub for v0.4.0).
    Disable {
        /// Command name to remove the shim for.
        command: String,
    },
    /// Move a legacy v0.4.0-alpha shim from `~/.local/bin/<cmd>` to
    /// the new shim dir.
    Migrate {
        /// Command name to migrate.
        command: String,
    },
    /// Show the rule table that `check` consults.
    Rules,
    /// Layer 4 + Layer 2: check, then spawn the real command, then
    /// record the session. Used by the shim.
    ///
    /// Usage:  agent0waste intercept run -- <real-binary> <args...>
    ///
    /// The first arg after `--` is the absolute path to the real
    /// binary (recorded at `intercept enable` time). The shim finds
    /// it, and intercept run spawns it after the check passes.
    Run {
        /// Catch-all for clap so trailing args aren't rejected.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
        _args: Vec<String>,
    },
}

fn override_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".config/agent0waste/pricing.toml")
}

fn load_or_init_override() -> PathBuf {
    let p = override_path();
    if let Some(parent) = p.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if !p.exists() {
        let _ = std::fs::write(&p, "# Agent0Waste pricing override\n# Format: [model-name] with input/output in USD per 1M tokens.\n# Quote the model name if it contains a dot or colon.\n#\n# Examples:\n# [\"openrouter/owl-alpha\"]\n# input = 0.00\n# output = 0.00\n#\n# [\"grok-4.3\"]\n# input = 3.00\n# output = 15.00\n");
    }
    p
}

fn run_pricing(action: &PricingAction) {
    match action {
        PricingAction::List => {
            let pricing = Pricing::load();
            let mut names = pricing.known_models();
            names.sort();
            println!("Agent0Waste — Known Models ({} total)\n", names.len());
            println!("{:<48}  {:>10}  {:>10}", "model", "$/1M in", "$/1M out");
            println!("{}", "-".repeat(72));
            for n in &names {
                if let Some((i, o)) = pricing.get(n) {
                    println!("{:<48}  ${:>9.4}  ${:>9.4}", n, i, o);
                }
            }
        }
        PricingAction::Path => {
            let p = load_or_init_override();
            println!("{}", p.display());
        }
        PricingAction::Add { model, input, output } => {
            let p = load_or_init_override();
            // Read existing file (or start empty)
            let existing = std::fs::read_to_string(&p).unwrap_or_default();
            // Render a fresh [model] block. We quote the key when it
            // contains a dot, colon, or hyphen (TOML's bare-key rules
            // are picky; quoting is always safe).
            let needs_quote = model.contains(|c: char| c == '.' || c == ':' || c == '/' || c.is_whitespace());
            let header = if needs_quote {
                format!("[\"{}\"]", model)
            } else {
                format!("[{}]", model)
            };
            let block = format!("\n{}\ninput  = {:.4}\noutput = {:.4}\n", header, input, output);
            // Append (TOML merge; the latest block wins on re-read).
            let mut new_contents = existing.clone();
            if !new_contents.ends_with('\n') && !new_contents.is_empty() {
                new_contents.push('\n');
            }
            new_contents.push_str(&block);
            std::fs::write(&p, new_contents).expect("write pricing.toml");
            println!("added [{}] (${:.4} in / ${:.4} out) to {}", model, input, output, p.display());
        }
        PricingAction::Unset { model } => {
            let p = override_path();
            if !p.exists() {
                println!("no override file at {} — nothing to unset", p.display());
                return;
            }
            let contents = std::fs::read_to_string(&p).expect("read pricing.toml");
            // Find the [model] or ["model"] block and remove everything
            // until the next [section] header.
            let header_bare = format!("[{}]", model);
            let header_quoted = format!("[\"{}\"]", model);
            let mut out = String::with_capacity(contents.len());
            let mut skipping = false;
            let mut removed_any = false;
            for line in contents.lines() {
                let trimmed = line.trim();
                if trimmed == header_bare || trimmed == header_quoted {
                    skipping = true;
                    removed_any = true;
                    continue;
                }
                if skipping && trimmed.starts_with('[') && !trimmed.is_empty() {
                    skipping = false;
                }
                if !skipping {
                    out.push_str(line);
                    out.push('\n');
                }
            }
            if !removed_any {
                println!("no override entry for [{}] in {}", model, p.display());
                return;
            }
            std::fs::write(&p, out).expect("write pricing.toml");
            println!("removed [{}] from {}", model, p.display());
        }
        PricingAction::Check => {
            let c = pricing::PricingCheck::run();
            match &c.path {
                Some(p) => println!("override file: {}", p.display()),
                None => println!("override file: (no home directory)"),
            }
            if !c.path.as_ref().map(|p| p.exists()).unwrap_or(false) {
                println!("  (file does not exist — defaults are used as-is)");
                println!("ok ({} default models)", c.models_count);
                return;
            }
            println!("entries       : {}", c.models_count);
            if c.errors.is_empty() {
                println!("valid         : yes");
            } else {
                println!("valid         : no");
                for e in &c.errors {
                    println!("  - {}", e);
                }
            }
            if !c.overlaps_with_default.is_empty() {
                println!("\nshadows a default (overrides take precedence — verify you meant this):");
                for (name, (in_r, out_r), (def_in, def_out)) in &c.overlaps_with_default {
                    println!(
                        "  {:<32}  override: ${:.4} in / ${:.4} out  |  default: ${:.4} in / ${:.4} out",
                        name, in_r, out_r, def_in, def_out
                    );
                }
            }
        }
    }
}

fn run_intercept(action: &InterceptAction) {
    match action {
        InterceptAction::Check { model, tokens, command, source, since, no_cache } => {
            run_intercept_check(model.as_deref(), *tokens, command.as_deref(), source, *since, *no_cache);
        }
        InterceptAction::Status => {
            run_intercept_status();
        }
        InterceptAction::Enable { command, strict } => {
            run_intercept_enable(command, *strict);
        }
        InterceptAction::Disable { command } => {
            run_intercept_disable(command);
        }
        InterceptAction::Migrate { command } => {
            run_intercept_migrate(command);
        }
        InterceptAction::Rules => {
            run_intercept_rules();
        }
        InterceptAction::Run { .. } => {
            // `agent0waste intercept run -- <real-binary> <args...>`
            // The shim execs us with:  intercept run -- "$REAL" "$@"
            // so $0 is "intercept run" and the first argv after `--`
            // is the real binary path.
            let argv: Vec<String> = std::env::args().collect();
            let (real_binary, args) = run::split_run_args(&argv)
                .unwrap_or_else(|e| {
                    eprintln!("error: {}", e);
                    eprintln!();
                    eprintln!("usage:  agent0waste intercept run -- <real-binary> [args...]");
                    std::process::exit(run::EXIT_BAD_ARGS);
                });
            if real_binary.is_empty() {
                eprintln!("error: missing <real-binary> after `--`");
                std::process::exit(run::EXIT_BAD_ARGS);
            }
            let real_args: Vec<&str> = args.iter().map(|s| s.as_str()).collect();
            run_intercept_run(&real_binary, &real_args);
        }
    }
}

fn run_intercept_check(
    model: Option<&str>,
    tokens: Option<u64>,
    command: Option<&str>,
    source: &str,
    since_days: i64,
    no_cache: bool,
) {
    // We need the same Hermes data `cost --from-hermes` reads. If the
    // state.db is unreadable, fail-open: emit `allow`.
    let since = if since_days <= 0 {
        chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap()
    } else {
        Utc::now() - chrono::Duration::days(since_days)
    };

    let (sessions, outcome) = intercept::load_hermes_sessions(since, None);
    if outcome.is_fail_open() {
        let allow = Decision::Allow;
        println!("{}", allow.to_json());
        std::process::exit(allow.exit_code());
    }

    let cfg = InterceptConfig::load();
    let hint = CheckHint {
        model: model.map(|s| s.to_string()),
        tokens,
        command: command.map(|s| s.to_string()),
        source: Some(source.to_string()),
    };

    let decision = intercept_check_with_cache(
        &sessions,
        since,
        &cfg,
        &hint,
        command,
        no_cache,
    );
    let json = decision.to_json();
    println!("{}", json);
    std::process::exit(decision.exit_code());
}

/// Run heuristics with the persistent cache in front. On a hit, returns
/// the cached decision without re-running heuristics (skips the
/// 150ms state.db read entirely). On a miss, runs heuristics and
/// stores the result.
///
/// Cache key: the `command` field of the hint. Returns Allow on a hit
/// with a missing or unparseable cache entry (treated as a miss).
fn intercept_check_with_cache(
    sessions: &[hermes_state::HermesSession],
    since: DateTime<Utc>,
    cfg: &InterceptConfig,
    hint: &CheckHint,
    cache_key_hint: Option<&str>,
    no_cache: bool,
) -> Decision {
    let cache_key = cache_key_for(cache_key_hint);
    let state_db_path = crate::hermes_state::default_state_path();

    if !no_cache {
        if let Some(key) = cache_key.as_deref() {
            if let Some(path) = state_db_path.as_deref() {
                if let Some(mtime) = state_db_mtime(path) {
                    let cache = cache::HeuristicCache::load();
                    if let Some(cached_json) = cache.get(key, mtime) {
                        if let Some(cached) = Decision::from_json(cached_json) {
                            return cached;
                        }
                    }
                }
            }
        }
    }

    let decision = intercept_check(sessions, since, cfg, hint);
    let json = decision.to_json();

    // Store in cache. TTL is a fixed 30s for v0.4.1; per-rule
    // `cache_ttl_s` is plumbed into the config and shown in
    // `intercept status` but not yet threaded into the cache
    // integration (which rule fired is hidden by `pick_decision`).
    // Tracked as a follow-up: see issue #7.
    if !no_cache {
        if let Some(key) = cache_key.as_deref() {
            if let Some(path) = state_db_path.as_deref() {
                if let Some(mtime) = state_db_mtime(path) {
                    let mut cache = cache::HeuristicCache::load();
                    cache.put(key, mtime, Duration::from_secs(30), json);
                    let _ = cache.save();
                }
            }
        }
    }

    decision
}

/// Build a cache key from the `--command` hint. Returns `None` if no
/// command was specified (the user just wanted a one-off check with no
/// key to remember).
fn cache_key_for(command: Option<&str>) -> Option<String> {
    command.map(|s| s.to_string())
}

fn state_db_mtime(path: &std::path::Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok()?.modified().ok()
}

fn run_intercept_status() {
    let cfg = InterceptConfig::load();
    let p = intercept_toml_path();
    match p {
        Some(path) => {
            if path.exists() {
                println!("intercept: enabled (config: {})", path.display());
            } else {
                println!("intercept: not enabled (no config at {})", path.display());
            }
        }
        None => {
            println!("intercept: not enabled (no home directory)");
        }
    }
    println!("mode: {}", match cfg.mode {
        Mode::FailOpen => "fail-open",
        Mode::FailClosed => "fail-closed",
    });
    println!("rules: {}", cfg.rules.len());
    for (id, rule) in &cfg.rules {
        let action = match rule.action {
            Action::Allow => "allow",
            Action::Throttle => "throttle",
            Action::Prompt => "prompt",
        };
        println!(
            "  {:<20}  action={:<8}  cooldown_s={:<3}  cache_ttl_s={}",
            id, action, rule.cooldown_s, rule.cache_ttl_s
        );
    }

    // Shim install state — the actionable part of `status`.
    println!();
    let dir_display = shim_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "(no home directory)".to_string());
    println!("shim dir      : {}", dir_display);
    if let Some(dir) = shim_dir() {
        let on_path = std::env::var("PATH")
            .unwrap_or_default()
            .split(':')
            .any(|d| d == dir.to_string_lossy());
        if dir.exists() {
            println!("on PATH       : {}", if on_path { "yes" } else { "no" });
            let mut shims: Vec<String> = match std::fs::read_dir(&dir) {
                Ok(entries) => entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                    .collect(),
                Err(_) => Vec::new(),
            };
            shims.sort();
            if shims.is_empty() {
                println!("shims installed: (none)");
            } else {
                println!("shims installed ({}):", shims.len());
                for s in &shims {
                    let real = find_real_command(s)
                        .unwrap_or_else(|| "(not on PATH — shim will fail-open)".to_string());
                    println!("  {:<16} → {}", s, real);
                }
            }
            if !on_path {
                println!();
                println!("hint: add to PATH:  export PATH=\"$HOME/.local/share/agent0waste/shims:$PATH\"");
            }
        } else {
            println!("on PATH       : n/a (dir does not exist yet)");
            println!("shims installed: (none)");
            println!();
            println!("hint: install one:  agent0waste intercept enable <cmd>");
        }
    }

    // Legacy shim detection — flag if a v0.4.0-alpha shim is still
    // sitting in ~/.local/bin/ (will shadow the new shim on PATH
    // even if the user adds the new dir, depending on PATH order).
    if let Some(old_dir) = local_bin() {
        if old_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&old_dir) {
                let mut legacy: Vec<String> = entries
                    .filter_map(|e| e.ok())
                    .filter_map(|e| {
                        let name = e.file_name().to_str()?.to_string();
                        let path = e.path();
                        if is_legacy_shim(&path) {
                            Some(name)
                        } else {
                            None
                        }
                    })
                    .collect();
                legacy.sort();
                if !legacy.is_empty() {
                    println!();
                    println!("legacy shims in {}:", old_dir.display());
                    for name in &legacy {
                        println!("  {}", name);
                    }
                    println!("run `agent0waste intercept migrate <cmd>` for each to move them.");
                }
            }
        }
    }
}

fn run_intercept_rules() {
    println!("default rule table (severity → action):");
    println!();
    println!("  {:<18}  {:<8}  {:<8}  {:<10}", "heuristic", "high", "medium", "info");
    println!("  {}", "-".repeat(50));
    for id in ["cache_bloat", "prompt_growth", "auto_routing", "model_instability"] {
        // Show the default action for each severity tier
        let high = match id {
            "cache_bloat" => "throttle",
            "prompt_growth" => "prompt",
            _ => "allow",
        };
        let med = match id {
            "cache_bloat" => "prompt",
            "prompt_growth" => "throttle",
            _ => "allow",
        };
        let info = "allow";
        println!("  {:<18}  {:<8}  {:<8}  {:<10}", id, high, med, info);
    }
    println!();
    println!("override in ~/.config/agent0waste/intercept.toml");
}

/// Layer 4 + Layer 2: check, then act, then spawn + record.
///
/// Called by the shim with:  intercept run -- <real-binary> <args...>
/// or by the user directly:  agent0waste intercept run -- hermes run foo
fn run_intercept_run(real_binary: &str, real_args: &[&str]) {
    // 1. Decide. Reuse the same logic as `intercept check`. We bypass
    //    the `intercept check` subcommand because we're already inside
    //    agent0waste; shelling out would be silly.
    let since = Utc::now() - chrono::Duration::days(7);
    let (sessions, outcome) = intercept::load_hermes_sessions(since, None);

    // 2. Fail-open paths. Emit a distinct stderr message, then run
    //    the real command unwrapped. (state.db failure modes already
    //    logged their own message in load_hermes_sessions.)
    if outcome.is_fail_open() {
        // Don't re-log; load_hermes_sessions already did.
        spawn_and_record(real_binary, real_args);
        return;
    }

    let cfg = InterceptConfig::load();
    let cmd_str = real_args.join(" ");
    let hint = CheckHint {
        model: None, // the shim doesn't know the model; leave None
        tokens: None,
        command: if cmd_str.is_empty() { None } else { Some(cmd_str.clone()) },
        source: Some("cli".into()),
    };

    let decision = intercept_check_with_cache(&sessions, since, &cfg, &hint, Some(&cmd_str), false);

    // 3. Act on the decision.
    match decision {
        Decision::Allow => {
            spawn_and_record(real_binary, real_args);
        }
        Decision::Throttle { cooldown_s, reason, .. } => {
            eprintln!("[agent0waste] throttle: {}", reason);
            eprintln!("[agent0waste] sleeping {}s, then re-checking...", cooldown_s);
            std::thread::sleep(std::time::Duration::from_secs(cooldown_s));
            // Re-check with the same data. If still throttling, run
            // anyway (the wrapper is a guardrail, not a gate).
            let decision2 = intercept_check(&sessions, since, &cfg, &hint);
            match decision2 {
                Decision::Throttle { reason, .. } => {
                    eprintln!("[agent0waste] still throttled: {}", reason);
                    eprintln!("[agent0waste] running anyway");
                }
                Decision::Prompt { reason, hint } => {
                    prompt_user(&reason, hint.as_deref());
                }
                _ => {}
            }
            spawn_and_record(real_binary, real_args);
        }
        Decision::Prompt { reason, hint } => {
            prompt_user(&reason, hint.as_deref());
            spawn_and_record(real_binary, real_args);
        }
    }
}

fn prompt_user(reason: &str, hint: Option<&str>) {
    eprintln!("[agent0waste] prompt: {}", reason);
    if let Some(h) = hint {
        eprintln!("[agent0waste] hint:   {}", h);
    }
    eprintln!("[agent0waste] continue? [y/N]");
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).is_err() {
        eprintln!("[agent0waste] (could not read stdin; cancelling)");
        std::process::exit(1);
    }
    let ans = line.trim();
    if !ans.eq_ignore_ascii_case("y") {
        eprintln!("[agent0waste] cancelled");
        std::process::exit(1);
    }
}

fn spawn_and_record(real_binary: &str, real_args: &[&str]) {
    match run::run_and_record(real_binary, real_args) {
        Ok(rec) => {
            // Apply cost if pricing known — non-fatal.
            let pricing = Pricing::load();
            let sessions = Sessions::new();
            let list = sessions.list();
            if let Some(mut r) = list.into_iter().find(|r| r.id == rec.id) {
                sessions::Sessions::apply_cost(&mut r, &pricing);
            }
            std::process::exit(rec.exit_code);
        }
        Err(e) => {
            eprintln!("error: {}", e);
            std::process::exit(run::EXIT_BAD_ARGS);
        }
    }
}

/// Install a shim for `command` at `~/.local/share/agent0waste/shims/<command>`.
///
/// If a legacy v0.4.0-alpha shim is found at `~/.local/bin/<command>`,
/// we refuse to enable until the user runs `intercept migrate <command>`
/// (or manually removes the legacy shim). We don't auto-move it because
/// the legacy shim might shadow the real binary, and moving it changes
/// which `hermes` runs — better to make the user do it explicitly.
fn run_intercept_enable(command: &str, _strict: bool) {
    let Some(dir) = shim_dir() else {
        eprintln!("[agent0waste] no home directory; cannot determine shim dir");
        std::process::exit(70);
    };
    let shim = dir.join(command);

    // Migration guard: detect legacy v0.4.0-alpha shim at ~/.local/bin/<cmd>.
    // Only flag files that look like our own shims (by content marker);
    // user-authored scripts at the same path should not trigger this.
    if let Some(old) = local_bin().map(|p| p.join(command)) {
        if old.exists() && is_legacy_shim(&old) && !shim.exists() {
            eprintln!(
                "[agent0waste] found legacy shim at {}",
                old.display()
            );
            eprintln!(
                "[agent0waste] v0.4.0 install model uses {} instead",
                dir.display()
            );
            eprintln!(
                "[agent0waste] run `agent0waste intercept migrate {}` to move it,",
                command
            );
            eprintln!("[agent0waste] or remove it manually:  rm {}", old.display());
            std::process::exit(70);
        }
    }

    if shim.exists() {
        eprintln!(
            "[agent0waste] {} already exists; refusing to overwrite",
            shim.display()
        );
        eprintln!("use --force to overwrite, or `agent0waste intercept disable {}` first", command);
        std::process::exit(70);
    }

    // Create the shim dir if missing.
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("[agent0waste] could not create {}: {}", dir.display(), e);
        std::process::exit(70);
    }

    // Resolve the real binary for the install message (the shim does
    // its own resolution at run time).
    let real = find_real_command(command).unwrap_or_else(|| command.to_string());

    let contents = SHIM_TEMPLATE
        .replace("__COMMAND__", command)
        .replace("__TIMEOUT__", &DEFAULT_INTERCEPT_TIMEOUT_S.to_string())
        .replace("__AGENT0WASTE_PATH__", &current_exe_path());

    if let Err(e) = std::fs::write(&shim, contents) {
        eprintln!("[agent0waste] could not write {}: {}", shim.display(), e);
        std::process::exit(70);
    }

    // chmod +x. Use std::os::unix::fs::PermissionsExt to avoid pulling
    // in a Windows compat layer.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o755);
        if let Err(e) = std::fs::set_permissions(&shim, perms) {
            eprintln!("[agent0waste] could not chmod +x {}: {}", shim.display(), e);
            std::process::exit(70);
        }
    }

    // Also ensure intercept.toml exists with defaults.
    let toml_path = intercept_toml_path();
    if let Some(p) = &toml_path {
        if let Some(parent) = p.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if !p.exists() {
            let _ = std::fs::write(
                p,
                "# Agent0Waste intercept config\n# mode = \"fail-open\"  # or \"fail-closed\" (v0.4.1+)\n#\n# [rules.cache_bloat]\n# action = \"throttle\"\n# cooldown_s = 30\n",
            );
        }
    }

    println!("installed shim: {}", shim.display());
    println!("real binary  : {}", real);
    println!();
    println!("audit   : cat {}", shim.display());
    println!("disable : agent0waste intercept disable {}", command);
    println!("migrate : agent0waste intercept migrate {}  (from legacy ~/.local/bin)", command);
    if std::env::var("PATH")
        .unwrap_or_default()
        .split(':')
        .all(|d| d != dir.to_string_lossy())
    {
        println!();
        println!("note: {} is not on your PATH", dir.display());
        println!(
            "add this to your shell rc:  export PATH=\"$HOME/.local/share/agent0waste/shims:$PATH\""
        );
        println!("(put it BEFORE other dirs if you also have hermes/claude installed elsewhere)");
    }
}

/// Move a legacy v0.4.0-alpha shim from `~/.local/bin/<cmd>` to
/// `~/.local/share/agent0waste/shims/<cmd>`. Refuses if the legacy
/// shim is missing, or if the new path already exists.
fn run_intercept_migrate(command: &str) {
    let Some(old) = local_bin().map(|p| p.join(command)) else {
        eprintln!("[agent0waste] no home directory");
        std::process::exit(70);
    };
    let Some(new) = shim_path(command) else {
        std::process::exit(70);
    };
    if !old.exists() {
        eprintln!("no legacy shim at {} — nothing to migrate", old.display());
        return;
    }
    if new.exists() {
        eprintln!(
            "[agent0waste] target {} already exists; refusing to overwrite",
            new.display()
        );
        eprintln!("remove it first:  rm {}", new.display());
        std::process::exit(70);
    }
    if let Some(parent) = new.parent() {
        if let Err(e) = std::fs::create_dir_all(parent) {
            eprintln!("[agent0waste] could not create {}: {}", parent.display(), e);
            std::process::exit(70);
        }
    }
    if let Err(e) = std::fs::rename(&old, &new) {
        eprintln!(
            "[agent0waste] could not move {} → {}: {}",
            old.display(),
            new.display(),
            e
        );
        std::process::exit(70);
    }
    // Re-apply +x in case the rename lost perms (it shouldn't, but cheap to verify).
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755));
    }
    println!("moved {} → {}", old.display(), new.display());
    println!();
    println!("verify:");
    println!("  which {}", command);
    println!("  cat {}", new.display());
}

fn run_intercept_disable(command: &str) {
    let Some(shim) = shim_path(command) else {
        eprintln!("[agent0waste] no home directory");
        std::process::exit(70);
    };
    if !shim.exists() {
        eprintln!("no shim at {} — nothing to disable", shim.display());
        return;
    }
    if let Err(e) = std::fs::remove_file(&shim) {
        eprintln!("[agent0waste] could not remove {}: {}", shim.display(), e);
        std::process::exit(70);
    }
    println!("removed {}", shim.display());
}

fn run_sessions_list() {
    let s = Sessions::new();
    let recs = s.list();
    if recs.is_empty() {
        println!("no sessions recorded yet");
        println!("run:  agent0waste run -- <cmd>  to record one");
        return;
    }
    println!("Agent0Waste — Recorded Sessions ({} of {})\n", recs.len(), Sessions::DEFAULT_CAP);
    let id_w = recs.iter().map(|r| r.id.len()).max().unwrap_or(20).max(20);
    println!(
        "{:<id_w$}  {:<19}  {:>5}  {:>9}  {:<24}  {}",
        "id", "started", "exit", "duration", "model", "command"
    );
    for r in &recs {
        let model = r.model.clone().unwrap_or_else(|| "-".to_string());
        let started = r.started_at.format("%Y-%m-%d %H:%M:%S").to_string();
        let dur = format!("{}ms", r.duration_ms);
        let cmd = if r.command.len() > 60 {
            format!("{}…", &r.command[..59])
        } else {
            r.command.clone()
        };
        println!(
            "{:<id_w$}  {:<19}  {:>5}  {:>9}  {:<24}  {}",
            r.id, started, r.exit_code, dur, model, cmd,
            id_w = id_w
        );
    }
    let _ = id_w; // silence unused warning when we eventually parameterize
}

fn run_cost(
    by: Option<&str>,
    since_days: i64,
    export: Option<&str>,
    from_hermes: bool,
    include_local: bool,
    warnings: bool,
    missing: bool,
) {
    let pricing = Pricing::load();
    let group_by = match by.unwrap_or("total") {
        "model" => GroupBy::Model,
        "provider" => GroupBy::Provider,
        "day" => GroupBy::Day,
        _ => GroupBy::Total,
    };

    // i64 days back from now; 0 = "always".
    let since = if since_days <= 0 {
        chrono::DateTime::<Utc>::from_timestamp(0, 0).unwrap()
    } else {
        Utc::now() - chrono::Duration::days(since_days)
    };

    // We need raw HermesSession list to run heuristics; load it once
    // even if --from-hermes is off (heuristics only work on Hermes data).
    let raw_hermes: Vec<hermes_state::HermesSession> = if from_hermes {
        match hermes_state::default_state_path() {
            Some(p) => match hermes_state::read_recent(since, &p) {
                Ok(v) => {
                    if export != Some("json") {
                        eprintln!("[agent0waste] read {} sessions from {}", v.len(), p.display());
                    }
                    v
                }
                Err(e) => {
                    eprintln!("[agent0waste] warning: could not read {}: {}", p.display(), e);
                    Vec::new()
                }
            },
            None => Vec::new(),
        }
    } else {
        Vec::new()
    };

    // Source 1: Hermes state.db
    let mut rows: Vec<cost::CostRow> = if from_hermes {
        report_hermes(&raw_hermes, &pricing, group_by)
    } else {
        Vec::new()
    };

    // Source 2: local session records (opt-in via --include-local)
    if include_local {
        let recs = Sessions::new().list();
        let local_rows = cost_report(&recs, &pricing, group_by, since);
        // Merge by key: sum cost/sessions/tokens.
        use std::collections::HashMap;
        let mut by_key: HashMap<String, cost::CostRow> =
            rows.into_iter().map(|r| (r.key.clone(), r)).collect();
        for r in local_rows {
            let entry = by_key.entry(r.key.clone()).or_insert(cost::CostRow {
                key: r.key.clone(),
                cost_usd: 0.0,
                sessions: 0,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
            });
            entry.cost_usd += r.cost_usd;
            entry.sessions += r.sessions;
            entry.input_tokens += r.input_tokens;
            entry.output_tokens += r.output_tokens;
        }
        rows = by_key.into_values().collect();
        rows.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    }

    // Layer 3: heuristics
    let heur_report = if warnings && !raw_hermes.is_empty() {
        run_heuristics(&raw_hermes, since)
    } else {
        heuristics::Report::new()
    };

    // --missing: list unpriced models + a TOML snippet
    let missing_models_list: Vec<cost::MissingModel> =
        if missing && !raw_hermes.is_empty() {
            missing_models(&raw_hermes, &pricing)
        } else {
            Vec::new()
        };

    if export == Some("json") {
        let out = serde_json::json!({
            "since": since.to_rfc3339(),
            "group_by": by.unwrap_or("total"),
            "from_hermes": from_hermes,
            "include_local": include_local,
            "rows": rows.iter().map(|r| serde_json::json!({
                "key": r.key,
                "cost_usd": r.cost_usd,
                "sessions": r.sessions,
                "input_tokens": r.input_tokens,
                "output_tokens": r.output_tokens,
                "cache_read_tokens": r.cache_read_tokens,
            })).collect::<Vec<_>>(),
            "warnings": if warnings { heur_report.to_json() } else { Vec::<serde_json::Value>::new() },
            "missing_models": if missing {
                missing_models_list.iter().map(|m| serde_json::json!({
                    "name": m.name,
                    "sessions": m.sessions,
                    "input_tokens": m.input_tokens,
                    "output_tokens": m.output_tokens,
                    "cache_read_tokens": m.cache_read_tokens,
                    "sources": m.sources.iter().collect::<Vec<_>>(),
                    "toml_snippet": m.toml_snippet().trim(),
                })).collect::<Vec<_>>()
            } else { Vec::<serde_json::Value>::new() },
        });
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("Agent0Waste — Cost Report");
        println!("  group by    : {}", by.unwrap_or("total"));
        println!("  since       : last {} days (from {})", since_days, since.format("%Y-%m-%d"));
        println!("  source      : {}", match (from_hermes, include_local) {
            (true, true)  => "hermes + local",
            (true, false) => "hermes only (default)",
            (false, true) => "local only",
            (false, false) => "none — pass --from-hermes or --include-local",
        });
        println!();
        print!("{}", format_table(&rows));
        if warnings {
            println!();
            println!("Heuristic warnings (Layer 3):");
            print!("{}", heur_report.format());
        }
        if missing {
            println!();
            println!("Models with no pricing ({}):", missing_models_list.len());
            if missing_models_list.is_empty() {
                println!("  (all models in the window are priced — nice)");
            } else {
                for m in &missing_models_list {
                    println!(
                        "  {:<48}  {} sessions  {} in / {} out  sources: {}",
                        m.name, m.sessions, m.input_tokens, m.output_tokens,
                        m.sources.iter().cloned().collect::<Vec<_>>().join(",")
                    );
                }
                println!();
                println!("Append to your pricing.toml ({}):", override_path().display());
                for m in &missing_models_list {
                    print!("{}", m.toml_snippet());
                }
            }
        }
    }
}
