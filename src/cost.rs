use crate::types::*;

pub fn estimate_waste(result: &ScanResult) -> Vec<WasteItem> {
    let mut waste = Vec::new();

    for profile in &result.hermes_profiles {
        // Flag any profile with expensive tools
        if !profile.expensive_tools.is_empty() {
            waste.push(WasteItem {
                category: "tool_bloat".to_string(),
                description: format!(
                    "Profile '{}' has {} expensive tools enabled (web/browser)",
                    profile.name,
                    profile.expensive_tools.len()
                ),
                severity: "medium".to_string(),
                estimated_savings: Some("~8-15k tokens/month".to_string()),
            });
        }

        // Flag profiles with very high tool counts
        if profile.tool_count > 10 {
            waste.push(WasteItem {
                category: "config_bloat".to_string(),
                description: format!(
                    "Profile '{}' has {} tools enabled (consider minimal set)",
                    profile.name,
                    profile.tool_count
                ),
                severity: "low".to_string(),
                estimated_savings: Some("reduced context + faster startup".to_string()),
            });
        }
    }

    waste
}