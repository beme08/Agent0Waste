//! Layer 4: Interception
//!
//! Maps heuristic findings to a decision (`allow` / `throttle` / `prompt`)
//! based on `~/.config/agent0waste/intercept.toml` (or defaults).
//!
//! This module is the unit-testable surface. The CLI wrapper (`wrap.sh`)
//! and the `intercept enable` installer are separate concerns; see
//! `docs/v0.4-design.md` for the full design.

use std::collections::HashMap;
use std::path::PathBuf;

use chrono::{DateTime, Utc};

use crate::heuristics::{self, Report, Severity};
use crate::hermes_state::HermesSession;

/// What the wrapper does after `check` returns.
#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    /// Run the command. Exit 0.
    Allow,
    /// Sleep N seconds, re-check once, then run. Exit 64.
    Throttle {
        cooldown_s: u64,
        reason: String,
        hint: Option<String>,
    },
    /// Print a message to stderr, ask the user `[y/N]`. Exit 65.
    Prompt {
        reason: String,
        hint: Option<String>,
    },
}

impl Decision {
    /// Exit code for the wrapper. 0=allow, 64=throttle, 65=prompt.
    pub fn exit_code(&self) -> i32 {
        match self {
            Decision::Allow => 0,
            Decision::Throttle { .. } => 64,
            Decision::Prompt { .. } => 65,
        }
    }

    /// JSON document the wrapper parses (stdout). Human message goes to
    /// stderr; this is the machine-readable form.
    pub fn to_json(&self) -> String {
        match self {
            Decision::Allow => r#"{"decision":"allow"}"#.to_string(),
            Decision::Throttle { cooldown_s, reason, hint } => {
                let hint = hint
                    .as_ref()
                    .map(|h| format!(r#","hint":{}"#, json_str(h)))
                    .unwrap_or_default();
                format!(
                    r#"{{"decision":"throttle","cooldown_s":{},"reason":{}{}}}"#,
                    cooldown_s,
                    json_str(reason),
                    hint
                )
            }
            Decision::Prompt { reason, hint } => {
                let hint = hint
                    .as_ref()
                    .map(|h| format!(r#","hint":{}"#, json_str(h)))
                    .unwrap_or_default();
                format!(
                    r#"{{"decision":"prompt","reason":{}{}}}"#,
                    json_str(reason),
                    hint
                )
            }
        }
    }
}

fn json_str(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n");
    format!("\"{}\"", escaped)
}

/// What the user did (or is about to do). `check` uses this to *label*
/// the decision; it does not change the action. Future versions may
/// weight the decision by source (cron = always throttle, cli = ask).
#[derive(Debug, Clone, Default)]
pub struct CheckHint {
    pub model: Option<String>,
    pub tokens: Option<u64>,
    pub command: Option<String>,
    pub source: Option<String>,
}

/// Action the wrapper should take when a heuristic fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Throttle,
    Prompt,
}

impl Action {
    fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "allow" => Some(Action::Allow),
            "throttle" => Some(Action::Throttle),
            "prompt" => Some(Action::Prompt),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// LLM call proceeds if Agent0Waste is unreachable.
    FailOpen,
    /// LLM call is blocked if Agent0Waste is unreachable (v0.4.1+).
    FailClosed,
}

impl Mode {
    fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "fail-open" => Some(Mode::FailOpen),
            "fail-closed" => Some(Mode::FailClosed),
            _ => None,
        }
    }
}

/// Per-heuristic rule. Overrides the default action for that heuristic.
#[derive(Debug, Clone)]
pub struct RuleConfig {
    pub action: Action,
    pub cooldown_s: u64,
}

impl RuleConfig {
    fn from_toml(table: &toml::Table) -> Option<Self> {
        let action = table.get("action").and_then(|v| v.as_str()).and_then(Action::from_str)?;
        let cooldown_s = table.get("cooldown_s").and_then(|v| v.as_integer()).map(|i| i.max(0) as u64).unwrap_or(30);
        Some(Self { action, cooldown_s })
    }
}

/// Top-level config from `intercept.toml`. Missing file = defaults.
#[derive(Debug, Clone)]
pub struct InterceptConfig {
    pub mode: Mode,
    pub rules: HashMap<String, RuleConfig>,
}

impl InterceptConfig {
    /// Hardcoded defaults. Used when no override file exists or when
    /// a rule in the file is malformed.
    pub fn defaults() -> Self {
        let mut rules = HashMap::new();
        rules.insert(
            "cache_bloat".into(),
            RuleConfig { action: Action::Throttle, cooldown_s: 30 },
        );
        rules.insert(
            "prompt_growth".into(),
            RuleConfig { action: Action::Throttle, cooldown_s: 60 },
        );
        rules.insert(
            "auto_routing".into(),
            RuleConfig { action: Action::Allow, cooldown_s: 0 },
        );
        rules.insert(
            "model_instability".into(),
            RuleConfig { action: Action::Allow, cooldown_s: 0 },
        );
        Self {
            mode: Mode::FailOpen,
            rules,
        }
    }

    /// Read `~/.config/agent0waste/intercept.toml` and merge with defaults.
    /// Missing file = all defaults. Malformed table = that rule's default.
    pub fn load() -> Self {
        let Some(path) = intercept_toml_path() else {
            return Self::defaults();
        };
        if !path.exists() {
            return Self::defaults();
        }
        let Ok(contents) = std::fs::read_to_string(&path) else {
            return Self::defaults();
        };
        let Ok(parsed) = contents.parse::<toml::Table>() else {
            eprintln!("[agent0waste] warning: could not parse {}; using defaults", path.display());
            return Self::defaults();
        };

        let mut out = Self::defaults();

        // Top-level mode
        if let Some(m) = parsed.get("mode").and_then(|v| v.as_str()).and_then(Mode::from_str) {
            out.mode = m;
        }

        // Per-heuristic rules
        if let Some(rules_table) = parsed.get("rules").and_then(|v| v.as_table()) {
            for (heuristic_id, value) in rules_table {
                if let Some(rule_table) = value.as_table() {
                    if let Some(rule) = RuleConfig::from_toml(rule_table) {
                        out.rules.insert(heuristic_id.clone(), rule);
                    }
                }
            }
        }

        out
    }
}

pub fn intercept_toml_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".config/agent0waste/intercept.toml"))
}

/// Default action for a (heuristic, severity) pair, used when the user
/// has not overridden the rule.
fn default_action(heuristic_id: &str, severity: Severity) -> Action {
    match (heuristic_id, severity) {
        ("cache_bloat", Severity::High) => Action::Throttle,
        ("cache_bloat", _) => Action::Prompt,
        ("prompt_growth", Severity::High) => Action::Prompt,
        ("prompt_growth", _) => Action::Throttle,
        ("auto_routing", _) => Action::Allow,
        ("model_instability", _) => Action::Allow,
        _ => Action::Allow,
    }
}

/// Run heuristics over the given sessions, apply the rule table, and
/// return the highest-priority decision.
///
/// The decision ordering is:
/// 1. Sort findings by severity (High > Medium > Info)
/// 2. For each finding, look up the rule (or default)
/// 3. If the rule's action is Allow, skip
/// 4. Otherwise return that action (the highest-severity match wins)
///
/// If no findings fire, returns `Allow`.
pub fn check(
    sessions: &[HermesSession],
    since: DateTime<Utc>,
    config: &InterceptConfig,
    hint: &CheckHint,
) -> Decision {
    let report = heuristics::run_all(sessions, since);
    pick_decision(&report, config, hint)
}

fn pick_decision(report: &Report, config: &InterceptConfig, hint: &CheckHint) -> Decision {
    // Sort findings by severity desc. Stable sort preserves specificity
    // (cache_bloat before prompt_growth when both are high).
    let mut sorted: Vec<_> = report.findings.iter().collect();
    sorted.sort_by(|a, b| severity_rank(b.severity).cmp(&severity_rank(a.severity)));

    for finding in sorted {
        let rule = config.rules.get(finding.id);
        let action = rule.map(|r| r.action).unwrap_or_else(|| default_action(finding.id, finding.severity));
        let cooldown = rule.map(|r| r.cooldown_s).unwrap_or(30);

        if action == Action::Allow {
            continue;
        }

        let reason = format_decision_reason(finding, hint);
        let hint_text = finding.hint.clone();

        return match action {
            Action::Throttle => Decision::Throttle {
                cooldown_s: cooldown,
                reason,
                hint: hint_text,
            },
            Action::Prompt => Decision::Prompt { reason, hint: hint_text },
            Action::Allow => unreachable!(),
        };
    }

    Decision::Allow
}

fn severity_rank(s: Severity) -> u8 {
    match s {
        Severity::High => 3,
        Severity::Warn => 2,
        Severity::Info => 1,
    }
}

fn format_decision_reason(f: &heuristics::Finding, hint: &CheckHint) -> String {
    let mut s = f.message.clone();
    if let Some(m) = &hint.model {
        s.push_str(&format!(" (model: {})", m));
    }
    if let Some(t) = hint.tokens {
        s.push_str(&format!(" (est ~{} in)", t));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(model: &str, source: &str, day: &str, in_t: u64, out_t: u64, cache: u64) -> HermesSession {
        HermesSession {
            id: format!("{}-{}-{}", model, source, day),
            model: model.into(),
            source: source.into(),
            started_at: DateTime::parse_from_rfc3339(day).unwrap().with_timezone(&Utc),
            ended_at: None,
            input_tokens: in_t,
            output_tokens: out_t,
            cache_read_tokens: cache,
            cache_write_tokens: 0,
            reasoning_tokens: 0,
            message_count: 0,
            tool_call_count: 0,
        }
    }

    // --- Decision tests ---

    #[test]
    fn decision_exit_codes() {
        assert_eq!(Decision::Allow.exit_code(), 0);
        let t = Decision::Throttle { cooldown_s: 30, reason: "x".into(), hint: None };
        assert_eq!(t.exit_code(), 64);
        let p = Decision::Prompt { reason: "x".into(), hint: None };
        assert_eq!(p.exit_code(), 65);
    }

    #[test]
    fn decision_json_allow_is_minimal() {
        let d = Decision::Allow;
        assert_eq!(d.to_json(), r#"{"decision":"allow"}"#);
    }

    #[test]
    fn decision_json_throttle_includes_cooldown_and_reason() {
        let d = Decision::Throttle {
            cooldown_s: 30,
            reason: "cache_bloat fired".into(),
            hint: Some("trim context".into()),
        };
        let j = d.to_json();
        assert!(j.contains(r#""decision":"throttle""#));
        assert!(j.contains(r#""cooldown_s":30"#));
        assert!(j.contains(r#""reason":"cache_bloat fired""#));
        assert!(j.contains(r#""hint":"trim context""#));
    }

    #[test]
    fn decision_json_prompt_omits_cooldown() {
        let d = Decision::Prompt {
            reason: "r".into(),
            hint: None,
        };
        let j = d.to_json();
        assert!(j.contains(r#""decision":"prompt""#));
        assert!(!j.contains("cooldown"));
    }

    #[test]
    fn decision_json_escapes_quotes() {
        let d = Decision::Prompt {
            reason: r#"He said "hi""#.into(),
            hint: None,
        };
        let j = d.to_json();
        assert!(j.contains(r#""He said \"hi\"""#), "got: {}", j);
    }

    // --- check() tests ---

    #[test]
    fn check_allows_on_clean_data() {
        // 1 session, no findings
        let s = vec![hs("gpt-4o", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 0)];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let cfg = InterceptConfig::defaults();
        let d = check(&s, since, &cfg, &CheckHint::default());
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn check_throttles_on_high_cache_bloat() {
        // 3 sessions, 5x cache ratio each → cache_bloat (high)
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 5_000),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let cfg = InterceptConfig::defaults();
        let d = check(&s, since, &cfg, &CheckHint::default());
        match d {
            Decision::Throttle { cooldown_s, reason, .. } => {
                assert_eq!(cooldown_s, 30); // default for cache_bloat
                assert!(reason.contains("cache"));
            }
            other => panic!("expected Throttle, got {:?}", other),
        }
    }

    #[test]
    fn check_user_can_override_cache_bloat_to_prompt() {
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 5_000),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let mut cfg = InterceptConfig::defaults();
        cfg.rules.insert(
            "cache_bloat".into(),
            RuleConfig { action: Action::Prompt, cooldown_s: 0 },
        );
        let d = check(&s, since, &cfg, &CheckHint::default());
        assert!(matches!(d, Decision::Prompt { .. }), "got: {:?}", d);
    }

    #[test]
    fn check_user_can_override_cache_bloat_to_allow() {
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 5_000),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let mut cfg = InterceptConfig::defaults();
        cfg.rules.insert(
            "cache_bloat".into(),
            RuleConfig { action: Action::Allow, cooldown_s: 0 },
        );
        let d = check(&s, since, &cfg, &CheckHint::default());
        assert_eq!(d, Decision::Allow);
    }

    #[test]
    fn check_high_severity_wins_over_low() {
        // Two findings fire: cache_bloat (high) and auto_routing (info).
        // High should win → Throttle, not Allow.
        let s = vec![
            // cache_bloat high
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 5_000),
            // auto_routing
            hs("auto", "cli", "2026-06-02T13:00:00Z", 100, 0, 0),
            hs("auto", "cli", "2026-06-02T14:00:00Z", 100, 0, 0),
            hs("auto", "cli", "2026-06-02T15:00:00Z", 100, 0, 0),
            hs("auto", "cli", "2026-06-02T16:00:00Z", 100, 0, 0),
            hs("auto", "cli", "2026-06-02T17:00:00Z", 100, 0, 0),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let cfg = InterceptConfig::defaults();
        let d = check(&s, since, &cfg, &CheckHint::default());
        // cache_bloat (high) → throttle; auto_routing (info) → allow.
        // The high severity wins.
        assert!(matches!(d, Decision::Throttle { .. }), "got: {:?}", d);
    }

    #[test]
    fn check_includes_hint_in_decision() {
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 5_000),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let cfg = InterceptConfig::defaults();
        let hint = CheckHint {
            model: Some("grok-4.3".into()),
            tokens: Some(2_000),
            command: Some("hermes run foo".into()),
            source: Some("cli".into()),
        };
        let d = check(&s, since, &cfg, &hint);
        match d {
            Decision::Throttle { reason, .. } => {
                assert!(reason.contains("grok-4.3"), "reason: {}", reason);
                assert!(reason.contains("2000"), "reason: {}", reason);
            }
            other => panic!("expected Throttle, got {:?}", other),
        }
    }

    #[test]
    fn default_action_matrix() {
        // High-severity cache_bloat → Throttle
        assert_eq!(default_action("cache_bloat", Severity::High), Action::Throttle);
        // Warn-severity cache_bloat → Prompt
        assert_eq!(default_action("cache_bloat", Severity::Warn), Action::Prompt);
        // High-severity prompt_growth → Prompt
        assert_eq!(default_action("prompt_growth", Severity::High), Action::Prompt);
        // Warn-severity prompt_growth → Throttle
        assert_eq!(default_action("prompt_growth", Severity::Warn), Action::Throttle);
        // info heuristics → Allow regardless
        assert_eq!(default_action("auto_routing", Severity::Info), Action::Allow);
        assert_eq!(default_action("model_instability", Severity::Info), Action::Allow);
    }

    #[test]
    fn config_load_falls_back_to_defaults_when_no_file() {
        // We can't easily test the "no file" case without manipulating
        // the home dir, but we can test that `defaults()` is well-formed.
        let cfg = InterceptConfig::defaults();
        assert_eq!(cfg.mode, Mode::FailOpen);
        assert_eq!(cfg.rules.len(), 4);
        assert!(cfg.rules.contains_key("cache_bloat"));
        assert!(cfg.rules.contains_key("prompt_growth"));
        assert!(cfg.rules.contains_key("auto_routing"));
        assert!(cfg.rules.contains_key("model_instability"));
    }

    #[test]
    fn config_load_parses_overrides() {
        // Write a temp intercept.toml and verify the loader picks up
        // user overrides.
        let tmp = std::env::temp_dir().join(format!(
            "agent0waste-test-{}.toml",
            std::process::id()
        ));
        std::fs::write(
            &tmp,
            r#"
mode = "fail-closed"

[rules.cache_bloat]
action = "prompt"
cooldown_s = 5

[rules.auto_routing]
action = "throttle"
cooldown_s = 10
"#,
        )
        .unwrap();

        // Parse the file directly using the same logic as load() but
        // pointed at our temp path. We do this by calling the same
        // parsing code that `load` uses internally — but `load` reads
        // the home dir. So we replicate the parse path here for the test.
        let contents = std::fs::read_to_string(&tmp).unwrap();
        let parsed = contents.parse::<toml::Table>().unwrap();
        let mut cfg = InterceptConfig::defaults();
        if let Some(m) = parsed.get("mode").and_then(|v| v.as_str()).and_then(Mode::from_str) {
            cfg.mode = m;
        }
        if let Some(rules) = parsed.get("rules").and_then(|v| v.as_table()) {
            for (id, value) in rules {
                if let Some(t) = value.as_table() {
                    if let Some(rule) = RuleConfig::from_toml(t) {
                        cfg.rules.insert(id.clone(), rule);
                    }
                }
            }
        }
        std::fs::remove_file(&tmp).ok();

        assert_eq!(cfg.mode, Mode::FailClosed);
        assert_eq!(cfg.rules.get("cache_bloat").unwrap().action, Action::Prompt);
        assert_eq!(cfg.rules.get("cache_bloat").unwrap().cooldown_s, 5);
        assert_eq!(cfg.rules.get("auto_routing").unwrap().action, Action::Throttle);
        assert_eq!(cfg.rules.get("auto_routing").unwrap().cooldown_s, 10);
        // Untouched rules keep defaults
        assert_eq!(cfg.rules.get("prompt_growth").unwrap().action, Action::Throttle);
    }

    #[test]
    fn action_from_str_rejects_unknown() {
        assert_eq!(Action::from_str("allow"), Some(Action::Allow));
        assert_eq!(Action::from_str("throttle"), Some(Action::Throttle));
        assert_eq!(Action::from_str("prompt"), Some(Action::Prompt));
        assert_eq!(Action::from_str("block"), None);
        assert_eq!(Action::from_str(""), None);
    }

    #[test]
    fn mode_from_str_rejects_unknown() {
        assert_eq!(Mode::from_str("fail-open"), Some(Mode::FailOpen));
        assert_eq!(Mode::from_str("fail-closed"), Some(Mode::FailClosed));
        assert_eq!(Mode::from_str("strict"), None);
    }
}
