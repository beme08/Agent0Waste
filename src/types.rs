use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanResult {
    pub hermes_profiles: Vec<ProfileInfo>,
    pub crons: Vec<CronInfo>,
    pub tool_bloat: Vec<ToolBloat>,
    pub model_info: Option<ModelInfo>,
    pub ram_usage_mb: u64,
    pub waste_items: Vec<WasteItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileInfo {
    pub name: String,
    pub tool_count: usize,
    pub expensive_tools: Vec<String>,
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CronInfo {
    pub job_id: String,
    pub schedule: String,
    pub uses_expensive_tools: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolBloat {
    pub tool: String,
    pub reason: String,
    pub estimated_monthly_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub provider: String,
    pub is_local: bool,
    pub context_window: u32,
    pub cost_per_million: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasteItem {
    pub category: String,
    pub description: String,
    pub severity: String, // low, medium, high
    pub estimated_savings: Option<String>,
}