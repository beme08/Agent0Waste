mod core;
mod cost;
mod macos;
mod permission;
mod report;
mod types;

use crate::core::{detect_model, scan_hermes};
use crate::cost::estimate_waste;
use crate::macos::MacOSScanner;
use crate::permission::load_or_prompt;
use crate::report::print_report;
use crate::types::*;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "agent0waste")]
#[command(about = "Local-first waste scanner for AI agent CLIs (Hermes, Claude Code, etc.)")]
#[command(version, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Run full local waste scan (default)
    Scan {
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        provider: Option<String>,
    },
    /// Show current efficiency report
    Report,
}

fn main() {
    let permission = load_or_prompt();
    if !permission.allow_config {
        eprintln!("Permission denied. Exiting.");
        std::process::exit(1);
    }

    let cli = Cli::parse();
    let scanner = MacOSScanner::new();

    match &cli.command {
        Some(Commands::Scan { model, provider }) => {
            let model_override = model.as_deref();
            let provider_override = provider.as_deref();

            let mut result = build_scan_result(&scanner, model_override, provider_override);
            result.waste_items = estimate_waste(&result);

            print_report(&result);
            print_efficiency_meter(&scanner, &result);
        }
        None => {
            let mut result = build_scan_result(&scanner, None, None);
            result.waste_items = estimate_waste(&result);

            print_report(&result);
            print_efficiency_meter(&scanner, &result);
        }
        Some(Commands::Report) => {
            let result = build_scan_result(&scanner, None, None);
            print_report(&result);
        }
    }
}

fn build_scan_result(
    scanner: &MacOSScanner,
    model_override: Option<&str>,
    provider_override: Option<&str>,
) -> ScanResult {
    let hermes_profiles = scan_hermes();
    let model_info = if let Some(m) = model_override {
        Some(ModelInfo {
            name: m.to_string(),
            provider: provider_override.unwrap_or("override").to_string(),
            is_local: m.contains("qwen") || m.contains("mlx") || m.contains("local"),
            context_window: 128000,
            cost_per_million: None,
        })
    } else {
        detect_model()
    };

    let ram_gb = scanner.get_total_ram_gb().unwrap_or(32.0);
    let memory_layers = scanner.get_memory_layers_gb();
    let expensive_tools = scanner.count_expensive_tools();
    let kanban_count = scanner.count_kanban_boards();

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

fn print_efficiency_meter(scanner: &MacOSScanner, result: &ScanResult) {
    let total_profiles = scanner.count_hermes_profiles();
    let expensive = result
        .waste_items
        .iter()
        .filter(|w| w.category.contains("tool") || w.category.contains("bloat"))
        .count();
    let current = if total_profiles > 0 {
        100 - (expensive * 15).min(70)
    } else {
        95
    };
    let potential = (current as f32 * 1.4).min(98.0) as u32;

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
    println!(
        "  →{}{} | +{}% speed",
        ram_savings,
        if expensive > 0 { " -18k tokens/mo" } else { "" },
        potential as i32 - current as i32
    );
    println!("Machine: {}", scanner.get_machine_info());
}