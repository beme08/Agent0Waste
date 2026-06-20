use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: String,
    pub efficiency: u32,
    pub profiles_count: usize,
    pub waste_count: usize,
    pub model: String,
    pub waste_categories: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct History {
    pub entries: Vec<HistoryEntry>,
}

impl History {
    pub fn load() -> Self {
        let path = history_path();
        if path.exists() {
            if let Ok(contents) = fs::read_to_string(&path) {
                if let Ok(h) = serde_json::from_str(&contents) {
                    return h;
                }
            }
        }
        History { entries: Vec::new() }
    }

    pub fn save(&self) {
        let path = history_path();
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, json);
        }
    }
}

fn history_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_default();
    base.join(".local/share/agent0waste/history.json")
}
