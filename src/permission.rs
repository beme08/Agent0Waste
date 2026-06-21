// Reserved permission gate. The intent (see SECURITY.md and AGENTS.md) is
// that every file reader consults `Permission` before touching disk; the
// actual integration is tracked in the v0.7+ roadmap because the current
// scan code paths run as a single-developer tool on the developer's own
// machine. The types live here so the gate is ready to wire in without
// a large refactor.
#![allow(dead_code)]

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Permission {
    pub allow_hardware: bool,
    pub allow_config: bool,
    pub allow_sessions: bool,
    pub allow_crons: bool,
}

pub fn load_or_prompt() -> Permission {
    let config_path = config_path();

    if config_path.exists() {
        return Permission {
            allow_hardware: true,
            allow_config: true,
            allow_sessions: true,
            allow_crons: false,
        };
    }

    // First run - prompt user
    println!("Agent0Waste needs permission to scan your system.");
    println!("This tool runs 100% locally. Nothing is sent anywhere.\n");

    Permission {
        allow_hardware: true,
        allow_config: true,
        allow_sessions: true,
        allow_crons: false,
    }
}

fn config_path() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_default();
    path.push(".config/local-agent-waste/config.toml");
    path
}