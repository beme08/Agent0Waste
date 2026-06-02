use chrono::Utc;
use clap::{Parser, Subcommand};
use std::io::Write;
use std::path::PathBuf;

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
    /// Show the rule table that `check` consults.
    Rules,
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
        InterceptAction::Check { model, tokens, command, source, since } => {
            run_intercept_check(model.as_deref(), *tokens, command.as_deref(), source, *since);
        }
        InterceptAction::Status => {
            run_intercept_status();
        }
        InterceptAction::Enable { command, strict: _ } => {
            eprintln!("[agent0waste] intercept enable {} is not implemented in this build", command);
            eprintln!("planned: installs a shim in ~/.local/bin/{} (no RC file edits)", command);
            eprintln!("see docs/v0.4-design.md for the shim-binary approach");
            std::process::exit(70);
        }
        InterceptAction::Disable { command } => {
            eprintln!("[agent0waste] intercept disable {} is not implemented in this build", command);
            eprintln!("planned: removes ~/.local/bin/{} shim", command);
            std::process::exit(70);
        }
        InterceptAction::Rules => {
            run_intercept_rules();
        }
    }
}

fn run_intercept_check(
    model: Option<&str>,
    tokens: Option<u64>,
    command: Option<&str>,
    source: &str,
    since_days: i64,
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

    let decision = intercept_check(&sessions, since, &cfg, &hint);
    println!("{}", decision.to_json());
    std::process::exit(decision.exit_code());
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
        println!("  {:<20}  action={:<8}  cooldown_s={}", id, action, rule.cooldown_s);
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
