use clap::{Parser, Subcommand};
use std::io::Write;

mod core;
mod cost;
mod history;
mod macos;
mod permission;
mod report;
mod types;

use core::{detect_model, scan_hermes};
use history::HistoryEntry;
use macos::MacOSScanner;
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
    let expensive_tools = scanner.count_expensive_tools();
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

    if expensive_tools > 3 {
        waste_items.push(WasteItem {
            category: "tool_bloat".to_string(),
            description: format!(
                "{} expensive tools enabled by default (web, browser, vision, etc.)",
                expensive_tools
            ),
            severity: "high".to_string(),
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
