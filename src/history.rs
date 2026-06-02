use chrono::{DateTime, Local};
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

    pub fn record(
        &mut self,
        efficiency: u32,
        profiles_count: usize,
        waste_count: usize,
        model: &str,
        waste_categories: Vec<String>,
    ) {
        let now: DateTime<Local> = Local::now();
        self.entries.push(HistoryEntry {
            timestamp: now.format("%Y-%m-%dT%H:%M:%S").to_string(),
            efficiency,
            profiles_count,
            waste_count,
            model: model.to_string(),
            waste_categories,
        });
        // Keep last 100 entries
        if self.entries.len() > 100 {
            self.entries.drain(0..self.entries.len() - 100);
        }
        self.save();
    }

    pub fn show_trend(&self) -> Option<String> {
        if self.entries.len() < 2 {
            return None;
        }
        let recent = &self.entries[self.entries.len().saturating_sub(5)..];
        let first = recent.first()?.efficiency;
        let last = recent.last()?.efficiency;
        let delta = last as i32 - first as i32;
        let arrow = if delta > 0 { "↑" } else if delta < 0 { "↓" } else { "→" };
        Some(format!("{}{} (last {} scans)", arrow, delta.abs(), recent.len()))
    }
}

fn history_path() -> PathBuf {
    let base = dirs::home_dir().unwrap_or_default();
    base.join(".local/share/agent0waste/history.json")
}
