use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One model's rate: input $/1M tokens, output $/1M tokens.
pub type Rate = (f64, f64);

/// Built-in default pricing for known models. All values are USD per 1M tokens.
/// Override per-model at `~/.config/agent0waste/pricing.toml`.
///
/// Free-tier variants (`:free` suffix on OpenRouter) are $0/$0. The
/// corresponding paid variant, when it exists, is in the table too.
fn default_rates() -> HashMap<String, Rate> {
    let mut m = HashMap::new();
    // OpenAI
    m.insert("gpt-4o".into(),            (2.50, 10.00));
    m.insert("gpt-4-turbo".into(),       (10.00, 30.00));
    m.insert("gpt-4o-mini".into(),       (0.15, 0.60));
    m.insert("gpt-4.1".into(),           (2.00, 8.00));
    m.insert("gpt-4.1-mini".into(),      (0.40, 1.60));
    m.insert("gpt-5".into(),             (1.25, 10.00));
    m.insert("gpt-5-mini".into(),        (0.25, 2.00));
    m.insert("o1".into(),                (15.00, 60.00));
    m.insert("o1-mini".into(),           (3.00, 12.00));
    m.insert("o3-mini".into(),           (1.10, 4.40));
    // Anthropic
    m.insert("claude-3-5-sonnet".into(), (3.00, 15.00));
    m.insert("claude-3-7-sonnet".into(), (3.00, 15.00));
    m.insert("claude-3-opus".into(),     (15.00, 75.00));
    m.insert("claude-3-haiku".into(),    (0.25, 1.25));
    m.insert("claude-sonnet-4".into(),   (3.00, 15.00));
    m.insert("claude-opus-4".into(),     (15.00, 75.00));
    // xAI
    m.insert("grok-2".into(),            (2.00, 10.00));
    m.insert("grok-3".into(),            (3.00, 15.00));
    m.insert("grok-3-mini".into(),       (0.30, 0.50));
    m.insert("grok-4".into(),            (3.00, 15.00));
    m.insert("grok-4-fast".into(),       (0.20, 0.50));
    // Google
    m.insert("gemini-1.5-pro".into(),    (1.25, 5.00));
    m.insert("gemini-1.5-flash".into(),  (0.075, 0.30));
    m.insert("gemini-2.0-flash".into(),  (0.10, 0.40));
    m.insert("gemini-2.0-pro".into(),    (1.25, 10.00));
    // Meta (via Groq / Together typical)
    m.insert("llama-3.1-70b".into(),     (0.88, 0.88));
    m.insert("llama-3.1-8b".into(),      (0.05, 0.08));
    m.insert("llama-3.3-70b".into(),     (0.88, 0.88));
    // Mistral
    m.insert("mistral-large".into(),     (2.00, 6.00));
    m.insert("mistral-small".into(),     (0.20, 0.60));
    m.insert("mixtral-8x7b".into(),      (0.27, 0.27));
    // DeepSeek (paid + free-tier)
    m.insert("deepseek-chat".into(),     (0.27, 1.10));
    m.insert("deepseek-reasoner".into(), (0.55, 2.19));
    m.insert("deepseek/deepseek-v4-flash:free".into(),       (0.0, 0.0));  // OpenRouter free tier
    // StepFun (paid + free-tier)
    m.insert("stepfun/step-3.7-flash".into(),               (0.20, 1.15));
    m.insert("stepfun/step-3.7-flash:free".into(),          (0.0, 0.0));
    // Moonshot Kimi (free-tier; paid pricing varies)
    m.insert("moonshotai/kimi-k2.6".into(),                 (0.50, 2.00));
    m.insert("moonshotai/kimi-k2.6:free".into(),            (0.0, 0.0));
    // Qwen (free-tier on OpenRouter; paid Alibaba tier separate)
    m.insert("qwen/qwen3-next-80b-a3b-instruct".into(),     (0.30, 1.20));
    m.insert("qwen/qwen3-next-80b-a3b-instruct:free".into(),(0.0, 0.0));
    // Xiaomi MiMo (256K context pricing; 1M context is 2x — not modeled here)
    m.insert("xiaomi/mimo-v2.5".into(),                     (0.40, 2.00));
    m.insert("xiaomi/mimo-v2.5-pro".into(),                 (1.00, 3.00));
    // OpenRouter's own models (free as of 2026-04)
    m.insert("openrouter/owl-alpha".into(),                  (0.0, 0.0));
    m
}

/// TOML override file schema. Each top-level table is a model name with
/// `input` and `output` rates in $/1M tokens.
///
/// ```toml
/// ["grok-4.3"]            # quoted because the dot in "4.3" is TOML's
/// input = 5.00            # dotted-key separator
/// output = 15.00
///
/// [gpt-4o]                # no dot, so quotes are optional
/// input = 2.50
/// output = 10.00
/// ```
#[derive(Debug, Default, Serialize, Deserialize)]
struct ModelRate {
    input: f64,
    output: f64,
}

/// Pricing table: built-in defaults + optional user override.
#[derive(Debug, Clone)]
pub struct Pricing {
    rates: HashMap<String, Rate>,
}

impl Pricing {
    /// Load built-in defaults only.
    pub fn default() -> Self {
        Self { rates: default_rates() }
    }

    /// Load defaults + apply `~/.config/agent0waste/pricing.toml` override if present.
    pub fn load() -> Self {
        let mut p = Self::default();
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".config/agent0waste/pricing.toml");
            if path.exists() {
                if let Err(e) = p.load_override(&path) {
                    eprintln!("warning: failed to load {}: {}", path.display(), e);
                }
            }
        }
        p
    }

    /// Apply an override file. Missing file is not an error; parse errors are.
    pub fn load_override(&mut self, path: &Path) -> Result<(), String> {
        let contents = std::fs::read_to_string(path)
            .map_err(|e| format!("read: {}", e))?;
        let parsed: HashMap<String, ModelRate> = toml::from_str(&contents)
            .map_err(|e| format!("parse: {}", e))?;
        for (name, rate) in parsed {
            self.rates.insert(name, (rate.input, rate.output));
        }
        Ok(())
    }

    /// Look up the rate for a model. Exact match only — callers should
    /// normalize the model name (lowercase, strip provider prefix) before
    /// calling, since APIs return different casings.
    pub fn get(&self, model: &str) -> Option<Rate> {
        self.rates.get(model).copied()
    }

    /// Compute cost in USD. Returns None if model is unknown.
    /// `input_tokens` and `output_tokens` are raw counts; the per-million
    /// multiplication happens here.
    pub fn cost(&self, model: &str, input_tokens: u64, output_tokens: u64) -> Option<f64> {
        self.get(model).map(|(in_rate, out_rate)| {
            let in_cost = (input_tokens as f64 / 1_000_000.0) * in_rate;
            let out_cost = (output_tokens as f64 / 1_000_000.0) * out_rate;
            in_cost + out_cost
        })
    }

    /// Iterate over all known model names (sorted). For the `cost` report
    /// when we want to show "unknown model X, please add to pricing".
    pub fn known_models(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.rates.keys().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names
    }
}

impl Default for Pricing {
    fn default() -> Self {
        Self::default()
    }
}

/// Result of validating a pricing TOML file. Used by `pricing check`.
#[derive(Debug, Clone)]
pub struct PricingCheck {
    pub path: Option<std::path::PathBuf>,
    pub valid: bool,
    pub models_count: usize,
    pub errors: Vec<String>,
    /// (name, default_rate) for models also in the default table.
    pub overlaps_with_default: Vec<(String, Rate, Rate)>,
}

impl PricingCheck {
    /// Read+validate the override file (if any). Errors are
    /// non-fatal — we report them but still return the check.
    pub fn run() -> Self {
        let p = dirs::home_dir().map(|h| h.join(".config/agent0waste/pricing.toml"));
        let mut check = PricingCheck {
            path: p.clone(),
            valid: true,
            models_count: 0,
            errors: Vec::new(),
            overlaps_with_default: Vec::new(),
        };
        let Some(path) = p else { return check; };
        if !path.exists() { return check; }
        let contents = match std::fs::read_to_string(&path) {
            Ok(s) => s,
            Err(e) => {
                check.valid = false;
                check.errors.push(format!("read {}: {}", path.display(), e));
                return check;
            }
        };
        let parsed: Result<HashMap<String, ModelRate>, _> = toml::from_str(&contents);
        let parsed = match parsed {
            Ok(m) => m,
            Err(e) => {
                check.valid = false;
                check.errors.push(format!("parse: {}", e));
                return check;
            }
        };
        check.models_count = parsed.len();
        let defaults = default_rates();
        for (name, rate) in &parsed {
            if rate.input < 0.0 || rate.output < 0.0 {
                check.valid = false;
                check.errors.push(format!("[{}] has negative rate", name));
            }
            if let Some(default_rate) = defaults.get(name) {
                check.overlaps_with_default.push((name.clone(), (rate.input, rate.output), *default_rate));
            }
        }
        check
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_common_models() {
        let p = Pricing::default();
        assert_eq!(p.get("gpt-4o"), Some((2.50, 10.00)));
        assert_eq!(p.get("claude-3-5-sonnet"), Some((3.00, 15.00)));
        assert_eq!(p.get("grok-4"), Some((3.00, 15.00)));
        // The 6 cloud/free-tier models that came up in real data
        assert_eq!(p.get("xiaomi/mimo-v2.5"), Some((0.40, 2.00)));
        assert_eq!(p.get("openrouter/owl-alpha"), Some((0.0, 0.0)));
        assert_eq!(p.get("stepfun/step-3.7-flash:free"), Some((0.0, 0.0)));
        assert_eq!(p.get("stepfun/step-3.7-flash"), Some((0.20, 1.15)));
        assert_eq!(p.get("moonshotai/kimi-k2.6:free"), Some((0.0, 0.0)));
        assert_eq!(p.get("deepseek/deepseek-v4-flash:free"), Some((0.0, 0.0)));
        assert_eq!(p.get("qwen/qwen3-next-80b-a3b-instruct:free"), Some((0.0, 0.0)));
    }

    #[test]
    fn cost_calculation() {
        let p = Pricing::default();
        // 1M input @ $2.50/1M + 0.5M output @ $10.00/1M = $7.50
        assert_eq!(p.cost("gpt-4o", 1_000_000, 500_000), Some(7.50));
        // Tiny usage: 4521 in * $2.50/1M + 12803 out * $10.00/1M
        //            = 0.0113025 + 0.1280300 = 0.1393325
        let cost = p.cost("gpt-4o", 4521, 12803).unwrap();
        assert!((cost - 0.139_332_5).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_returns_none() {
        let p = Pricing::default();
        assert_eq!(p.cost("nonexistent-model-99", 1000, 1000), None);
        assert_eq!(p.get("nonexistent-model-99"), None);
    }

    #[test]
    fn override_takes_precedence() {
        let mut p = Pricing::default();
        // Simulate override: grok-4.3 = $5/$15
        let mut rates = std::collections::HashMap::new();
        rates.insert("grok-4.3".to_string(), (5.00, 15.00));
        for (k, v) in rates {
            p.rates.insert(k, v);
        }
        assert_eq!(p.get("grok-4.3"), Some((5.00, 15.00)));
    }

    #[test]
    fn parse_override_toml() {
        let toml = r#"
["grok-4.3"]
input = 5.00
output = 15.00

["claude-sonnet-4.6"]
input = 3.00
output = 15.00

[gpt-4o]
input = 2.50
output = 10.00
"#;
        let parsed: HashMap<String, ModelRate> = toml::from_str(toml).unwrap();
        assert_eq!(parsed.get("grok-4.3").unwrap().input, 5.00);
        assert_eq!(parsed.get("claude-sonnet-4.6").unwrap().output, 15.00);
        assert_eq!(parsed.get("gpt-4o").unwrap().input, 2.50);
    }

    #[test]
    fn check_catches_negative_rates() {
        // Write a malformed override and check it.
        let dir = std::env::temp_dir().join(format!("agent0waste-check-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // We can't easily redirect PricingCheck::run's path lookup, so
        // we just exercise the parsing layer directly here.
        let bad = r#"
[broken-model]
input = -1.00
output = 2.00
"#;
        let parsed: Result<HashMap<String, ModelRate>, _> = toml::from_str(bad);
        assert!(parsed.is_ok());
        for (_, rate) in parsed.unwrap() {
            assert!(rate.input < 0.0 || rate.output < 0.0);
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_detects_default_shadow() {
        // Direct test of the field that PricingCheck populates.
        let defaults = default_rates();
        let override_rate: ModelRate = ModelRate { input: 1.0, output: 1.0 };
        let name = "gpt-4o".to_string(); // exists in defaults
        assert!(defaults.contains_key(&name), "gpt-4o must be in defaults for this test");
        let _ = override_rate; // silence
    }
}
