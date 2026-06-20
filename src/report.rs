use crate::types::*;

pub fn print_report(result: &ScanResult) {
    // Model line (demo target: grok-4.3)
    if let Some(model) = &result.model_info {
        let local = if model.is_local { "local" } else { "remote" };
        println!("Model     : {} ({})  [{}]", model.name, model.provider, local);
    } else {
        println!("Model     : unknown");
    }

    println!();

    // Hermes profiles
    if !result.hermes_profiles.is_empty() {
        println!("Hermes profiles ({}):", result.hermes_profiles.len());
        for p in &result.hermes_profiles {
            let expensive = if !p.expensive_tools.is_empty() {
                format!("{} expensive", p.expensive_tools.len())
            } else {
                "clean".to_string()
            };
            println!("  {:<20} tools: {:>2}  ({})", p.name, p.tool_count, expensive);
        }
        println!();
    }

    // Waste section
    if result.waste_items.is_empty() {
        println!("Waste     : none detected (baseline)");
    } else {
        println!("Waste detected:");
        for w in &result.waste_items {
            println!("  [{}] {} — {}", w.severity, w.category, w.description);
            if let Some(s) = &w.estimated_savings {
                println!("       → {}", s);
            }
        }
    }
}