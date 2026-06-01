use std::process::Command;
use std::fs;

/// macOS-specific data collector for Agent0Waste
pub struct MacOSScanner;

impl MacOSScanner {
    pub fn new() -> Self {
        Self
    }

    pub fn get_total_ram_gb(&self) -> Option<f64> {
        let output = Command::new("sysctl")
            .args(["-n", "hw.memsize"])
            .output()
            .ok()?;

        let bytes: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .ok()?;

        Some(bytes as f64 / 1024.0 / 1024.0 / 1024.0)
    }

    pub fn count_hermes_profiles(&self) -> usize {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = format!("{}/.hermes/profiles", home);
        fs::read_dir(path).map(|d| d.count()).unwrap_or(0)
    }

    pub fn count_hermes_crons(&self) -> usize {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = format!("{}/.hermes/cron", home);
        fs::read_dir(path)
            .map(|d| d.filter_map(|e| e.ok()).count())
            .unwrap_or(0)
    }

    pub fn count_expensive_tools(&self) -> u32 {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = format!("{}/.hermes/config.yaml", home);

        if let Ok(content) = fs::read_to_string(path) {
            let expensive = [
                "web",
                "browser",
                "vision",
                "image_gen",
                "tts",
                "computer_use",
                "x_search",
            ];
            return expensive.iter().filter(|t| content.contains(*t)).count() as u32;
        }
        0
    }

    pub fn count_kanban_boards(&self) -> u32 {
        let home = std::env::var("HOME").unwrap_or_default();
        let path = format!("{}/.hermes/kanban/boards", home);
        fs::read_dir(path)
            .map(|d| d.filter_map(|e| e.ok()).count() as u32)
            .unwrap_or(0)
    }

    pub fn get_memory_layers_gb(&self) -> (f64, f64) {
        let home = std::env::var("HOME").unwrap_or_default();
        let mnemosyne_path = format!("{}/.hermes/memory", home);
        let sessions_path = format!("{}/.hermes/sessions", home);

        let mnemosyne = self.dir_size_gb(&mnemosyne_path);
        let sessions = self.dir_size_gb(&sessions_path);

        (mnemosyne, sessions)
    }

    fn dir_size_gb(&self, path: &str) -> f64 {
        let mut total: u64 = 0;

        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if meta.is_file() {
                        total += meta.len();
                    }
                }
            }
        }

        total as f64 / 1024.0 / 1024.0 / 1024.0
    }

    pub fn get_machine_info(&self) -> String {
        let model = Command::new("sysctl")
            .args(["-n", "hw.model"])
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|| "Unknown".to_string());

        format!("macOS • {}", model)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_machine_info_not_empty() {
        let scanner = MacOSScanner::new();
        let info = scanner.get_machine_info();
        assert!(!info.is_empty());
    }

    #[test]
    fn test_ram_detection() {
        let scanner = MacOSScanner::new();
        let ram = scanner.get_total_ram_gb();
        assert!(ram.is_some());
        assert!(ram.unwrap() > 0.0);
    }
}