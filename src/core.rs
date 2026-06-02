use crate::types::*;
use std::fs;
use std::path::PathBuf;
use serde_yaml::Value;

fn get_hermes_base_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("HERMES_ROOT") {
        PathBuf::from(dir)
    } else {
        dirs::home_dir().unwrap().join(".hermes")
    }
}

fn get_hermes_profiles_dir() -> PathBuf {
    get_hermes_base_dir().join("profiles")
}

/// Tool name patterns considered "expensive" (token-heavy or API-cost-heavy).
/// Single source of truth — used by both the per-profile detector and the
/// waste aggregator so the profile list and the waste line always agree.
pub const EXPENSIVE_PATTERNS: &[&str] = &[
    "web",
    "browser",
    "vision",
    "image_gen",
    "tts",
    "computer_use",
    "x_search",
];

/// Count how many of the configured tools in a toolset string match the
/// expensive-pattern list. Returns the matching tool names in encounter order.
pub fn expensive_tools_in(tools_str: &str) -> Vec<String> {
    let tools: Vec<String> = tools_str
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    tools
        .iter()
        .filter(|t| EXPENSIVE_PATTERNS.iter().any(|p| t.contains(p)))
        .cloned()
        .collect()
}


/// Scan all Hermes profiles for tool usage information.
pub fn scan_hermes() -> Vec<ProfileInfo> {
    let hermes_dir = get_hermes_profiles_dir();
    let mut profiles = Vec::new();
    if let Ok(entries) = fs::read_dir(&hermes_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                let config_path = path.join("config.yaml");
                let (tool_count, expensive_tools) = if config_path.exists() {
                    match fs::read_to_string(&config_path) {
                        Ok(contents) => {
                            let yaml: Value = serde_yaml::from_str(&contents).unwrap_or_default();
                            let tools_str = yaml
                                .get("toolsets")
                                .and_then(|ts| ts.get("default"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("");
                            let tools: Vec<String> = tools_str
                                .split(',')
                                .map(|s| s.trim().to_string())
                                .filter(|s| !s.is_empty())
                                .collect();
                            let count = tools.len();
                            let expensive = expensive_tools_in(tools_str);
                            (count, expensive)
                        }
                        Err(_) => (0, vec![]),
                    }
                } else {
                    (0, vec![])
                };
                profiles.push(ProfileInfo {
                    name,
                    tool_count,
                    expensive_tools,
                    last_used: None,
                });
            }
        }
    }
    profiles
}

/// Known model context windows (tokens). Extend as needed.
pub(crate) fn model_context_window(name: &str) -> u32 {
    let n = name.to_lowercase();
    if n.contains("qwen2.5") || n.contains("qwen-2.5") {
        128000
    } else if n.contains("claude-3-5") || n.contains("claude-3.5") {
        200000
    } else if n.contains("claude-3") {
        200000
    } else if n.contains("gpt-4o") || n.contains("gpt-4-turbo") {
        128000
    } else if n.contains("llama3.1") || n.contains("llama-3.1") {
        128000
    } else if n.contains("grok") {
        128000
    } else if n.contains("mistral") || n.contains("mixtral") {
        128000
    } else {
        8192 // safe default
    }
}

/// Detect the default model configuration.
/// Checks ~/.hermes/config.yaml (active config) first, then profiles/default/config.yaml.
pub fn detect_model() -> Option<ModelInfo> {
    let base = get_hermes_base_dir();
    let candidates = [
        base.join("config.yaml"),                          // active config (most common)
        base.join("profiles/default/config.yaml"),         // default profile
    ];

    for path in &candidates {
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(path) {
                if let Some(info) = parse_model_from_yaml(&contents) {
                    return Some(info);
                }
            }
        }
    }
    None
}

fn parse_model_from_yaml(contents: &str) -> Option<ModelInfo> {
    let yaml: Value = serde_yaml::from_str(contents).ok()?;
    let name = yaml
        .get("model")
        .and_then(|m| m.get("default"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let provider = yaml
        .get("model")
        .and_then(|m| m.get("provider"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let ctx = yaml
        .get("model")
        .and_then(|m| m.get("context_length"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let context_window = if ctx > 0 { ctx } else { model_context_window(&name) };
    let is_local = provider.contains("omlx")
        || provider.contains("local")
        || provider.contains("freellmapi")
        || name.contains("mlx")
        || name.contains("local");
    Some(ModelInfo {
        name,
        provider,
        is_local,
        context_window,
        cost_per_million: None,
    })
}
