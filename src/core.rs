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
                            let expensive: Vec<String> = tools
                                .iter()
                                .filter(|t| t.contains("web") || t.contains("browser"))
                                .cloned()
                                .collect();
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

/// Detect the default model configuration from the default profile.
pub fn detect_model() -> Option<ModelInfo> {
    let default_path = get_hermes_base_dir().join("profiles/default/config.yaml");
    if default_path.exists() {
        if let Ok(contents) = fs::read_to_string(&default_path) {
            let yaml: Value = serde_yaml::from_str(&contents).ok()?;
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
            Some(ModelInfo {
                name,
                provider,
                is_local: false,
                context_window: yaml
                    .get("model")
                    .and_then(|m| m.get("context_length"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as u32,
                cost_per_million: None,
            })
        } else {
            None
        }
    } else {
        None
    }
}
