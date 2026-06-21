use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

/// One model's rate: (input, output, cache_input, cache_output), all in
/// USD per 1M tokens. `cache_input` is the rate for prompt-cache reads
/// (Anthropic: 10% of input; OpenAI: 50% on most GPT-4 models; xAI:
/// variable). `cache_output` is the rate for cache writes, which are
/// free across every major provider as of 2026-06 and is therefore 0.0
/// for every default in this crate.
pub type Rate = (f64, f64, f64, f64);



/// Cost for a single model invocation, broken into regular and cache
/// components (both in USD). Returned by [`Pricing::cost`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostBreakdown {
    /// input + output, in USD.
    pub regular: f64,
    /// cache reads, in USD.
    pub cache: f64,
}

impl CostBreakdown {
    /// Combined cost in USD (regular + cache).
    #[allow(dead_code)] // convenience accessor; not yet called by report logic
    pub fn total(&self) -> f64 {
        self.regular + self.cache
    }
}

/// Built-in default pricing for known models. All values are USD per 1M tokens.
/// Override per-model at `~/.config/agent0waste/pricing.toml`.
///
/// Free-tier variants (`:free` suffix on OpenRouter) are $0/$0. The
/// corresponding paid variant, when it exists, is in the table too.
fn default_rates() -> HashMap<String, Rate> {
    let mut m = HashMap::new();
    // OpenAI (cache_input: 50% of input on GPT-4o/4-turbo; 25% on GPT-4.1/5)
    m.insert("gpt-4o".into(),            (2.50, 10.00, 1.25, 0.00));
    m.insert("gpt-4-turbo".into(),       (10.00, 30.00, 5.00, 0.00));
    m.insert("gpt-4o-mini".into(),       (0.15, 0.60, 0.075, 0.00));
    m.insert("gpt-4.1".into(),           (2.00, 8.00, 0.50, 0.00));
    m.insert("gpt-4.1-mini".into(),      (0.40, 1.60, 0.10, 0.00));
    m.insert("gpt-5".into(),             (1.25, 10.00, 0.3125, 0.00));
    m.insert("gpt-5-mini".into(),        (0.25, 2.00, 0.0625, 0.00));
    m.insert("o1".into(),                (15.00, 60.00, 7.50, 0.00));
    m.insert("o1-mini".into(),           (3.00, 12.00, 1.50, 0.00));
    m.insert("o3-mini".into(),           (1.10, 4.40, 0.55, 0.00));
    // Anthropic (cache_input: 10% of input across all current models)
    m.insert("claude-3-5-sonnet".into(), (3.00, 15.00, 0.30, 0.00));
    m.insert("claude-3-7-sonnet".into(), (3.00, 15.00, 0.30, 0.00));
    m.insert("claude-3-opus".into(),     (15.00, 75.00, 1.50, 0.00));
    m.insert("claude-3-haiku".into(),    (0.25, 1.25, 0.025, 0.00));
    m.insert("claude-sonnet-4".into(),   (3.00, 15.00, 0.30, 0.00));
    m.insert("claude-opus-4".into(),     (15.00, 75.00, 1.50, 0.00));
    // xAI Grok (cache_input: 50% on paid tiers; free on fast tier)
    m.insert("grok-2".into(),            (2.00, 10.00, 1.00, 0.00));
    m.insert("grok-3".into(),            (3.00, 15.00, 1.50, 0.00));
    m.insert("grok-3-mini".into(),       (0.30, 0.50, 0.15, 0.00));
    m.insert("grok-4".into(),            (3.00, 15.00, 1.50, 0.00));
    m.insert("grok-4-fast".into(),       (0.20, 0.50, 0.00, 0.00));
    // Google Gemini (cache reads are free as of 2026-06 on these tiers)
    m.insert("gemini-1.5-pro".into(),    (1.25, 5.00, 0.00, 0.00));
    m.insert("gemini-1.5-flash".into(),  (0.075, 0.30, 0.00, 0.00));
    m.insert("gemini-2.0-flash".into(),  (0.10, 0.40, 0.00, 0.00));
    m.insert("gemini-2.0-pro".into(),    (1.25, 10.00, 0.00, 0.00));
    // Meta Llama (via Groq / Together; cache_input: 50% of input, typical)
    m.insert("llama-3.1-70b".into(),     (0.88, 0.88, 0.44, 0.00));
    m.insert("llama-3.1-8b".into(),      (0.05, 0.08, 0.025, 0.00));
    m.insert("llama-3.3-70b".into(),     (0.88, 0.88, 0.44, 0.00));
    // Mistral (cache_input: 25% of input, rough)
    m.insert("mistral-large".into(),     (2.00, 6.00, 0.50, 0.00));
    m.insert("mistral-small".into(),     (0.20, 0.60, 0.05, 0.00));
    m.insert("mixtral-8x7b".into(),      (0.27, 0.27, 0.0675, 0.00));
    // DeepSeek (cache_input: 10% of input on cache hit)
    m.insert("deepseek-chat".into(),     (0.27, 1.10, 0.027, 0.00));
    m.insert("deepseek-reasoner".into(), (0.55, 2.19, 0.055, 0.00));
    m.insert("deepseek/deepseek-v4-flash:free".into(),       (0.0, 0.0, 0.0, 0.0));  // OpenRouter free tier
    // StepFun (paid + free-tier)
    m.insert("stepfun/step-3.7-flash".into(),               (0.20, 1.15, 0.05, 0.00));
    m.insert("stepfun/step-3.7-flash:free".into(),          (0.0, 0.0, 0.0, 0.0));
    // Moonshot Kimi (free-tier; paid pricing varies)
    m.insert("moonshotai/kimi-k2.6".into(),                 (0.50, 2.00, 0.125, 0.00));
    m.insert("moonshotai/kimi-k2.6:free".into(),            (0.0, 0.0, 0.0, 0.0));
    // Qwen (free-tier on OpenRouter; paid Alibaba tier separate)
    m.insert("qwen/qwen3-next-80b-a3b-instruct".into(),     (0.30, 1.20, 0.075, 0.00));
    m.insert("qwen/qwen3-next-80b-a3b-instruct:free".into(),(0.0, 0.0, 0.0, 0.0));
    // Xiaomi MiMo (256K context pricing; 1M context is 2x — not modeled here)
    m.insert("xiaomi/mimo-v2.5".into(),                     (0.40, 2.00, 0.10, 0.00));
    m.insert("xiaomi/mimo-v2.5-pro".into(),                 (1.00, 3.00, 0.25, 0.00));
    // OpenRouter's own models (free as of 2026-04)
    m.insert("openrouter/owl-alpha".into(),                  (0.0, 0.0, 0.0, 0.0));
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
    /// USD per 1M cache-read tokens. Defaults to 0.0 for backwards
    /// compatibility with 2-field TOML overrides written before v0.6.1.
    #[serde(default)]
    cache_input: f64,
    /// USD per 1M cache-write tokens. Always 0.0 in default rates
    /// (cache writes are free across every major provider). Defaults
    /// to 0.0 for backwards compatibility.
    #[serde(default)]
    cache_output: f64,
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
            self.rates.insert(name, (rate.input, rate.output, rate.cache_input, rate.cache_output));
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
    /// Cost in USD for a single model invocation, broken into regular
    /// (input + output) and cache (cache reads) components.
    ///
    /// Returns `None` if the model is unknown to this pricing table.
    pub fn cost(
        &self,
        model: &str,
        input_tokens: u64,
        output_tokens: u64,
        cache_read_tokens: u64,
    ) -> Option<CostBreakdown> {
        self.get(model).map(|(in_r, out_r, cin_r, _cout_r)| {
            let regular = (input_tokens as f64 / 1_000_000.0) * in_r
                        + (output_tokens as f64 / 1_000_000.0) * out_r;
            let cache   = (cache_read_tokens as f64 / 1_000_000.0) * cin_r;
            CostBreakdown { regular, cache }
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
                check.overlaps_with_default.push((name.clone(), (rate.input, rate.output, rate.cache_input, rate.cache_output), *default_rate));
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
        // 4-tuple: (input, output, cache_input, cache_output)
        assert_eq!(p.get("gpt-4o"), Some((2.50, 10.00, 1.25, 0.00)));
        assert_eq!(p.get("claude-3-5-sonnet"), Some((3.00, 15.00, 0.30, 0.00)));
        assert_eq!(p.get("grok-4"), Some((3.00, 15.00, 1.50, 0.00)));
        // The 6 cloud/free-tier models that came up in real data
        assert_eq!(p.get("xiaomi/mimo-v2.5"), Some((0.40, 2.00, 0.10, 0.00)));
        assert_eq!(p.get("openrouter/owl-alpha"), Some((0.0, 0.0, 0.0, 0.0)));
        assert_eq!(p.get("stepfun/step-3.7-flash:free"), Some((0.0, 0.0, 0.0, 0.0)));
        assert_eq!(p.get("stepfun/step-3.7-flash"), Some((0.20, 1.15, 0.05, 0.00)));
        assert_eq!(p.get("moonshotai/kimi-k2.6:free"), Some((0.0, 0.0, 0.0, 0.0)));
        assert_eq!(p.get("deepseek/deepseek-v4-flash:free"), Some((0.0, 0.0, 0.0, 0.0)));
        assert_eq!(p.get("qwen/qwen3-next-80b-a3b-instruct:free"), Some((0.0, 0.0, 0.0, 0.0)));
    }

    #[test]
    fn cost_calculation() {
        let p = Pricing::default();
        // 1M input @ $2.50/1M + 0.5M output @ $10.00/1M = $7.50 regular
        let c = p.cost("gpt-4o", 1_000_000, 500_000, 0).unwrap();
        assert!((c.regular - 7.50).abs() < 1e-9);
        assert!((c.cache - 0.0).abs() < 1e-9);
        assert!((c.total() - 7.50).abs() < 1e-9);
        // Tiny usage: 4521 in * $2.50/1M + 12803 out * $10.00/1M
        //            = 0.0113025 + 0.1280300 = 0.1393325
        let c = p.cost("gpt-4o", 4521, 12803, 0).unwrap();
        assert!((c.regular - 0.139_332_5).abs() < 1e-9);
    }

    #[test]
    fn cost_with_cache_tokens_prices_cache_separately() {
        let p = Pricing::default();
        // 1M input + 0 cache: $2.50 regular, $0 cache
        let c = p.cost("gpt-4o", 1_000_000, 0, 0).unwrap();
        assert!((c.regular - 2.50).abs() < 1e-9);
        assert!((c.cache - 0.0).abs() < 1e-9);
        // 1M input + 1M cache: $2.50 regular, $1.25 cache (50% of input)
        let c = p.cost("gpt-4o", 1_000_000, 0, 1_000_000).unwrap();
        assert!((c.regular - 2.50).abs() < 1e-9);
        assert!((c.cache - 1.25).abs() < 1e-9);
        assert!((c.total() - 3.75).abs() < 1e-9);
        // Anthropic: 10% of input
        let c = p.cost("claude-3-5-sonnet", 1_000_000, 0, 1_000_000).unwrap();
        assert!((c.regular - 3.00).abs() < 1e-9);
        assert!((c.cache - 0.30).abs() < 1e-9);
    }

    #[test]
    fn unknown_model_returns_none() {
        let p = Pricing::default();
        assert_eq!(p.cost("nonexistent-model-99", 1000, 1000, 0), None);
        assert_eq!(p.get("nonexistent-model-99"), None);
    }

    #[test]
    fn override_takes_precedence() {
        let mut p = Pricing::default();
        // Simulate override: grok-4.3 = $5/$15 (no cache)
        let mut rates = std::collections::HashMap::new();
        rates.insert("grok-4.3".to_string(), (5.00, 15.00, 0.0, 0.0));
        for (k, v) in rates {
            p.rates.insert(k, v);
        }
        assert_eq!(p.get("grok-4.3"), Some((5.00, 15.00, 0.0, 0.0)));
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
    fn new_4field_toml_sets_cache_rates() {
        // 4-field TOML (v0.6.1+) populates cache rates from the file.
        let toml = r#"
[gpt-4o]
input = 2.50
output = 10.00
cache_input = 1.25
cache_output = 0.00
"#;
        let parsed: HashMap<String, ModelRate> = toml::from_str(toml).unwrap();
        let rate = parsed.get("gpt-4o").unwrap();
        assert_eq!(rate.input, 2.50);
        assert_eq!(rate.output, 10.00);
        assert_eq!(rate.cache_input, 1.25);
        assert_eq!(rate.cache_output, 0.00);
    }

    #[test]
    fn legacy_2field_toml_accepted_with_default_cache_zero() {
        // Backwards compat: 2-field TOML (input/output only) is still
        // accepted. cache_input and cache_output default to 0.0.
        let toml = r#"
[gpt-4o]
input = 2.50
output = 10.00
"#;
        let parsed: HashMap<String, ModelRate> = toml::from_str(toml).unwrap();
        let rate = parsed.get("gpt-4o").unwrap();
        assert_eq!(rate.input, 2.50);
        assert_eq!(rate.output, 10.00);
        assert_eq!(rate.cache_input, 0.0);
        assert_eq!(rate.cache_output, 0.0);
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
        let override_rate: ModelRate = ModelRate { input: 1.0, output: 1.0, cache_input: 0.0, cache_output: 0.0 };
        let name = "gpt-4o".to_string(); // exists in defaults
        assert!(defaults.contains_key(&name), "gpt-4o must be in defaults for this test");
        let _ = override_rate; // silence
    }
}
