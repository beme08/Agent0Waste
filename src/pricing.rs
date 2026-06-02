use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One model's rate: input $/1M tokens, output $/1M tokens.
pub type Rate = (f64, f64);

/// Built-in default pricing for known models. All values are USD per 1M tokens.
/// Override per-model at `~/.config/agent0waste/pricing.toml`.
fn default_rates() -> HashMap<String, Rate> {
    let mut m = HashMap::new();
    // OpenAI
    m.insert("gpt-4o".into(),            (2.50, 10.00));
    m.insert("gpt-4-turbo".into(),       (10.00, 30.00));
    m.insert("gpt-4o-mini".into(),       (0.15, 0.60));
    m.insert("gpt-5-mini".into(),        (0.25, 2.00));
    m.insert("gpt-5".into(),             (1.25, 10.00));
    // Anthropic
    m.insert("claude-3-5-sonnet".into(), (3.00, 15.00));
    m.insert("claude-3-opus".into(),     (15.00, 75.00));
    m.insert("claude-3-haiku".into(),    (0.25, 1.25));
    m.insert("claude-sonnet-4".into(),   (3.00, 15.00));
    m.insert("claude-opus-4".into(),     (15.00, 75.00));
    // xAI
    m.insert("grok-2".into(),            (2.00, 10.00));
    m.insert("grok-3".into(),            (3.00, 15.00));
    m.insert("grok-4".into(),            (3.00, 15.00));
    m.insert("grok-4-fast".into(),       (0.20, 0.50));
    // Google
    m.insert("gemini-1.5-pro".into(),    (1.25, 5.00));
    m.insert("gemini-1.5-flash".into(),  (0.075, 0.30));
    m.insert("gemini-2.0-flash".into(),  (0.10, 0.40));
    // Meta (via Groq / Together typical)
    m.insert("llama-3.1-70b".into(),     (0.88, 0.88));
    m.insert("llama-3.1-8b".into(),      (0.05, 0.08));
    // Mistral
    m.insert("mistral-large".into(),     (2.00, 6.00));
    m.insert("mixtral-8x7b".into(),      (0.27, 0.27));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_has_common_models() {
        let p = Pricing::default();
        assert_eq!(p.get("gpt-4o"), Some((2.50, 10.00)));
        assert_eq!(p.get("claude-3-5-sonnet"), Some((3.00, 15.00)));
        assert_eq!(p.get("grok-4"), Some((3.00, 15.00)));
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
}
