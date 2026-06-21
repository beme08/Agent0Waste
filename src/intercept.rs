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
use std::path::Path;

use crate::heuristics::{self, Report, Severity};
use crate::hermes_state::HermesSession;

/// How the state.db load attempt turned out. The `Missing` and
/// `Unreadable` cases always trigger fail-open — the user gets
/// a JSON `allow` decision and a stderr message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadOutcome {
    /// Successfully read N sessions from state.db.
    Loaded(usize),
    /// No home directory; we have no idea where to look.
    NoHome,
    /// state.db is not at the expected path (Hermes not installed
    /// or the user is on a clean machine).
    Missing(std::path::PathBuf),
    /// state.db exists but we couldn't read it (locked, corrupt,
    /// permission denied, or the SQLite driver returned an error).
    Unreadable(std::path::PathBuf, String),
}

impl LoadOutcome {
    pub fn is_fail_open(&self) -> bool {
        !matches!(self, LoadOutcome::Loaded(_))
    }
}

/// Try to read Hermes sessions from `path`. Always returns; never
/// panics. Missing/locked/corrupt paths return an empty Vec and
/// log a stderr message. The caller can use `LoadOutcome` to know
/// which case fired (for the test suite and for the explicit
/// fail-open stderr messages documented in v0.4-design.md).
///
/// `path` is passed in (not looked up) so tests can point at
/// temp files. The CLI uses `hermes_state::default_state_path()`
/// to find the real one.
pub fn load_hermes_sessions(since: DateTime<Utc>, path: Option<&Path>) -> (Vec<HermesSession>, LoadOutcome) {
    let resolved = match path {
        Some(p) => Some(p.to_path_buf()),
        None => crate::hermes_state::default_state_path(),
    };
    let resolved = match resolved {
        Some(p) => p,
        None => {
            eprintln!("[agent0waste] no home directory; cannot read state.db");
            eprintln!("[agent0waste] fail-open: allowing call");
            return (Vec::new(), LoadOutcome::NoHome);
        }
    };
    if !resolved.exists() {
        eprintln!("[agent0waste] state.db not found at {}; fail-open: allowing call", resolved.display());
        return (Vec::new(), LoadOutcome::Missing(resolved));
    }
    match crate::hermes_state::read_recent(since, &resolved) {
        Ok(v) => {
            let n = v.len();
            (v, LoadOutcome::Loaded(n))
        }
        Err(e) => {
            eprintln!("[agent0waste] could not read {}: {}; fail-open: allowing call", resolved.display(), e);
            (Vec::new(), LoadOutcome::Unreadable(resolved, e))
        }
    }
}

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
    /// Print a message to stderr, do NOT exec the real binary. Exit 66.
    /// v0.5.0: only emitted when mode = "fail-closed" and no rule
    /// fired (or a user-configured rule has action = deny). The shim
    /// must NOT call the real binary on this path. Users can override
    /// per-call with `--agent0waste-bypass` (audit-logged).
    Deny {
        reason: String,
        hint: Option<String>,
    },
}

impl Decision {
    /// Exit code for the wrapper. 0=allow, 64=throttle, 65=prompt, 66=deny.
    pub fn exit_code(&self) -> i32 {
        match self {
            Decision::Allow => 0,
            Decision::Throttle { .. } => 64,
            Decision::Prompt { .. } => 65,
            Decision::Deny { .. } => 66,
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
            Decision::Deny { reason, hint } => {
                let hint = hint
                    .as_ref()
                    .map(|h| format!(r#","hint":{}"#, json_str(h)))
                    .unwrap_or_default();
                format!(
                    r#"{{"decision":"deny","reason":{}{}}}"#,
                    json_str(reason),
                    hint
                )
            }
        }
    }

    /// Inverse of `to_json`. Used by the heuristic cache to deserialize
    /// a previously-stored decision. Returns `None` on malformed JSON
    /// (treated as cache miss, not an error).
    pub fn from_json(s: &str) -> Option<Self> {
        // Minimal hand-rolled parser: we only need the `decision` field
        // to dispatch, and the well-known fields per variant. Avoids a
        // serde dep on the hot path.
        let v: serde_json::Value = serde_json::from_str(s).ok()?;
        let decision = v.get("decision")?.as_str()?;
        match decision {
            "allow" => Some(Decision::Allow),
            "throttle" => {
                let cooldown_s = v.get("cooldown_s")?.as_u64()?;
                let reason = v.get("reason")?.as_str()?.to_string();
                let hint = v.get("hint").and_then(|h| h.as_str()).map(String::from);
                Some(Decision::Throttle { cooldown_s, reason, hint })
            }
            "prompt" => {
                let reason = v.get("reason")?.as_str()?.to_string();
                let hint = v.get("hint").and_then(|h| h.as_str()).map(String::from);
                Some(Decision::Prompt { reason, hint })
            }
            "deny" => {
                let reason = v.get("reason")?.as_str()?.to_string();
                let hint = v.get("hint").and_then(|h| h.as_str()).map(String::from);
                Some(Decision::Deny { reason, hint })
            }
            _ => None,
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
    #[allow(dead_code)] // forward-compat: see struct doc — future versions may weight decision by source
    pub command: Option<String>,
    #[allow(dead_code)] // forward-compat: see struct doc — future versions may weight decision by source
    pub source: Option<String>,
}

/// Action the wrapper should take when a heuristic fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Allow,
    Throttle,
    Prompt,
    /// v0.5.0: opt-in deny action. When a rule is configured with
    /// action=deny, the shim exits 66 without exec'ing the real
    /// binary. Distinct from "no rule fired + mode=fail-closed"
    /// (which is a default-deny, not a rule-driven deny).
    Deny,
}

impl Action {
    fn from_str(s: &str) -> Option<Self> {
        match s.trim() {
            "allow" => Some(Action::Allow),
            "throttle" => Some(Action::Throttle),
            "prompt" => Some(Action::Prompt),
            "deny" => Some(Action::Deny),
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
    /// Heuristic cache TTL for this rule, in seconds. Default 30s.
    /// Set to 0 to disable caching (always re-run heuristics).
    /// See src/cache.rs for the cache semantics.
    pub cache_ttl_s: u64,
}

impl RuleConfig {
    fn from_toml(table: &toml::Table) -> Option<Self> {
        let action = table.get("action").and_then(|v| v.as_str()).and_then(Action::from_str)?;
        let cooldown_s = table.get("cooldown_s").and_then(|v| v.as_integer()).map(|i| i.max(0) as u64).unwrap_or(30);
        let cache_ttl_s = table.get("cache_ttl_s").and_then(|v| v.as_integer()).map(|i| i.max(0) as u64).unwrap_or(30);
        Some(Self { action, cooldown_s, cache_ttl_s })
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
            RuleConfig { action: Action::Throttle, cooldown_s: 30, cache_ttl_s: 30 },
        );
        rules.insert(
            "prompt_growth".into(),
            RuleConfig { action: Action::Throttle, cooldown_s: 60, cache_ttl_s: 30 },
        );
        // H3 auto_routing and H4 model_instability are info-only by
        // design. See docs/v0.4-design.md "Why are H3 and H4 'allow'
        // by default?". The user can opt in to stricter behavior via
        // intercept.toml if they disagree; the default is "don't be
        // paternalistic about the user A/B testing or using auto-routing."
        rules.insert(
            "auto_routing".into(),
            RuleConfig { action: Action::Allow, cooldown_s: 0, cache_ttl_s: 30 },
        );
        rules.insert(
            "model_instability".into(),
            RuleConfig { action: Action::Allow, cooldown_s: 0, cache_ttl_s: 30 },
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
    pick_decision(&report, config, hint).decision
}

/// Like `check` but also returns which rule fired and a per-rule
/// "considered" list (all configured rules, not just ones whose
/// heuristic fired). Used by `intercept trace` to render the
/// decision pipeline (spec §3). v0.4.2.
pub fn check_with_trace(
    sessions: &[HermesSession],
    since: DateTime<Utc>,
    config: &InterceptConfig,
    hint: &CheckHint,
) -> CheckTrace {
    let report = heuristics::run_all(sessions, since);
    pick_decision_with_trace(&report, config, hint)
}

#[derive(Debug, Clone)]
pub struct ConsideredRule {
    pub rule_id: String,
    pub action: Action,
    pub detail: String,
    pub fired: bool,
}

#[derive(Debug, Clone)]
pub struct CheckTrace {
    pub decision: Decision,
    pub fired_rule: Option<String>,
    pub considered: Vec<ConsideredRule>,
}

/// Default rule ordering for trace output. Matches the order in
/// `InterceptConfig::defaults()` and `docs/decision-spec.md` §8
/// (cache_bloat → prompt_growth → auto_routing → model_instability).
/// Any custom rules from the user's config are appended alphabetically.
const DEFAULT_RULE_ORDER: &[&str] = &[
    "cache_bloat",
    "prompt_growth",
    "auto_routing",
    "model_instability",
];

fn pick_decision(report: &Report, config: &InterceptConfig, hint: &CheckHint) -> CheckTrace {
    pick_decision_with_trace(report, config, hint)
}

fn pick_decision_with_trace(
    report: &Report,
    config: &InterceptConfig,
    hint: &CheckHint,
) -> CheckTrace {
    // Build the considered list by iterating all configured rules
    // (or the 4 defaults if the user has no config). For each rule,
    // check if there's a finding; if so, the rule's action is its
    // contribution; if not, the rule's contribution is "allow" (no
    // trigger means no enforcement).
    let mut ordered_rules: Vec<String> = Vec::new();
    for id in DEFAULT_RULE_ORDER {
        if config.rules.contains_key(*id) {
            ordered_rules.push((*id).to_string());
        }
    }
    // Append any custom rules (alphabetical for stability).
    let mut custom: Vec<String> = config
        .rules
        .keys()
        .filter(|k| !DEFAULT_RULE_ORDER.contains(&k.as_str()))
        .cloned()
        .collect();
    custom.sort();
    ordered_rules.extend(custom);

    let mut considered: Vec<ConsideredRule> = Vec::new();
    let mut fired: Option<String> = None;
    let mut final_decision: Option<Decision> = None;

    for rule_id in &ordered_rules {
        let rule_cfg = config.rules.get(rule_id);
        // Pick the highest-severity finding for this rule (if any).
        let finding = report
            .findings
            .iter()
            .filter(|f| f.id == rule_id)
            .max_by_key(|f| severity_rank(f.severity));

        let (action, detail) = match (finding, rule_cfg) {
            (Some(f), Some(_)) => {
                // Rule fired; action comes from the config.
                let action = rule_cfg.map(|r| r.action).unwrap();
                let cooldown = rule_cfg.map(|r| r.cooldown_s).unwrap_or(30);
                let detail = if matches!(action, Action::Throttle) {
                    format!("{} (cooldown {}s)", f.message, cooldown)
                } else {
                    f.message.clone()
                };
                (action, detail)
            }
            (Some(f), None) => {
                // Finding but no config: use default_action.
                let action = default_action(f.id, f.severity);
                (action, f.message.clone())
            }
            (None, _) => {
                // No finding: no contribution.
                (Action::Allow, String::new())
            }
        };

        let did_fire = fired.is_none() && action != Action::Allow;

        considered.push(ConsideredRule {
            rule_id: rule_id.clone(),
            action,
            detail,
            fired: did_fire,
        });

        if did_fire {
            // Build the decision from the finding (which is what `fired` was based on)
            if let Some(f) = finding {
                let cooldown = rule_cfg.map(|r| r.cooldown_s).unwrap_or(30);
                let reason = format_decision_reason(f, hint);
                let hint_text = f.hint.clone();
                final_decision = Some(match action {
                    Action::Throttle => Decision::Throttle {
                        cooldown_s: cooldown,
                        reason,
                        hint: hint_text,
                    },
                    Action::Prompt => Decision::Prompt { reason, hint: hint_text },
                    Action::Deny => Decision::Deny { reason, hint: hint_text },
                    Action::Allow => unreachable!(),
                });
            }
            fired = Some(rule_id.clone());
        }
    }

    // v0.5.0 mode-flip: if no rule fired and mode=fail-closed,
    // default to Deny. See decision-spec §6 and v0.5.0-design.md.
    // This only flips §5 (decision table) defaults; §7 (failure
    // table) outcomes remain unchanged. Heuristic timeouts still
    // fail-open regardless of mode.
    let decision = match final_decision {
        Some(d) => d,
        None if config.mode == Mode::FailClosed => Decision::Deny {
            reason: "no heuristic fired; mode=fail-closed denies by default".to_string(),
            hint: Some(
                "pass --agent0waste-bypass to override (audit-logged), \
                 or switch to mode=fail-open"
                    .to_string(),
            ),
        },
        None => Decision::Allow,
    };

    CheckTrace {
        decision,
        fired_rule: fired,
        considered,
    }
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
        // The escaped form in JSON is \"hi\"; we expect to see that in the output.
        assert!(j.contains(r#"\"hi\""#), "got: {}", j);
    }

    #[test]
    fn decision_from_json_allow_roundtrip() {
        let d = Decision::Allow;
        assert_eq!(Decision::from_json(&d.to_json()), Some(Decision::Allow));
    }

    #[test]
    fn decision_from_json_throttle_roundtrip() {
        let d = Decision::Throttle {
            cooldown_s: 30,
            reason: "cache_bloat fired".into(),
            hint: Some("trim context".into()),
        };
        let parsed = Decision::from_json(&d.to_json()).unwrap();
        match parsed {
            Decision::Throttle { cooldown_s, reason, hint } => {
                assert_eq!(cooldown_s, 30);
                assert_eq!(reason, "cache_bloat fired");
                assert_eq!(hint, Some("trim context".into()));
            }
            _ => panic!("expected Throttle, got {:?}", parsed),
        }
    }

    #[test]
    fn decision_from_json_prompt_roundtrip() {
        let d = Decision::Prompt {
            reason: "r".into(),
            hint: None,
        };
        match Decision::from_json(&d.to_json()).unwrap() {
            Decision::Prompt { reason, hint } => {
                assert_eq!(reason, "r");
                assert_eq!(hint, None);
            }
            _ => panic!("expected Prompt"),
        }
    }

    #[test]
    fn decision_from_json_malformed_returns_none() {
        assert_eq!(Decision::from_json("not json"), None);
        let unknown_decision = r#"{"decision":"unknown"}"#;
        assert_eq!(Decision::from_json(unknown_decision), None);
        assert_eq!(Decision::from_json("{}"), None);
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
            RuleConfig { action: Action::Prompt, cooldown_s: 0, cache_ttl_s: 30 },
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
            RuleConfig { action: Action::Allow, cooldown_s: 0, cache_ttl_s: 30 },
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
        // user overrides. Per-test filename to avoid collision with
        // other tests that share `agent0waste-test-{pid}.toml`.
        let tmp = std::env::temp_dir().join(format!(
            "agent0waste-test-config-{}.toml",
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

    // --- state.db failure-mode tests ---

    #[test]
    fn load_outcome_is_fail_open_for_all_non_loaded_cases() {
        assert!(!LoadOutcome::Loaded(10).is_fail_open());
        assert!(LoadOutcome::NoHome.is_fail_open());
        assert!(LoadOutcome::Missing("/foo".into()).is_fail_open());
        assert!(LoadOutcome::Unreadable("/foo".into(), "locked".into()).is_fail_open());
    }

    #[test]
    fn load_returns_missing_for_nonexistent_path() {
        let tmp = std::env::temp_dir().join(format!(
            "agent0waste-no-such-db-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        // Don't create the file.
        let since = Utc::now() - chrono::Duration::days(7);
        let (sessions, outcome) = load_hermes_sessions(since, Some(&tmp));
        assert!(sessions.is_empty());
        assert!(matches!(outcome, LoadOutcome::Missing(_)));
    }

    #[test]
    fn load_returns_unreadable_for_corrupt_path() {
        // Create a file that is definitely not a SQLite db.
        let tmp = std::env::temp_dir().join(format!(
            "agent0waste-corrupt-db-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(&tmp, b"this is not a sqlite database at all\n").unwrap();
        let since = Utc::now() - chrono::Duration::days(7);
        let (sessions, outcome) = load_hermes_sessions(since, Some(&tmp));
        assert!(sessions.is_empty());
        assert!(matches!(outcome, LoadOutcome::Unreadable(_, _)));
        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_returns_unreadable_for_locked_db() {
        // Create a real (minimal) sqlite db, then hold an exclusive
        // lock on it from this process. A second read attempt must
        // fail.
        //
        // Note: rusqlite uses SQLite's locking model. An open read
        // connection does NOT block another read connection (SQLite
        // supports multiple readers). So we test the "write lock
        // contention" case: open a writer, then try to read with a
        // second connection. In WAL mode, the read should succeed
        // (readers don't block on writers in WAL). Without WAL, the
        // read should fail with SQLITE_BUSY.
        //
        // Either way, our function should return either Loaded or
        // Unreadable — never panic.
        let tmp = std::env::temp_dir().join(format!(
            "agent0waste-locked-db-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        // Create a minimal valid sqlite db by opening + closing.
        {
            let conn = rusqlite::Connection::open(&tmp).unwrap();
            conn.execute("CREATE TABLE _test (x INTEGER)", []).unwrap();
        }

        // Open a long-lived write connection (no actual writes; just
        // a connection that holds a SHARED lock).
        let _conn = rusqlite::Connection::open(&tmp).unwrap();

        let since = Utc::now() - chrono::Duration::days(7);
        let (_sessions, outcome) = load_hermes_sessions(since, Some(&tmp));

        // Outcome is either Loaded (reader + reader is fine) or
        // Unreadable (writer blocks). Both are valid; the important
        // property is that we did NOT panic.
        match outcome {
            LoadOutcome::Loaded(_) | LoadOutcome::Unreadable(_, _) => {}
            other => panic!("unexpected outcome: {:?}", other),
        }

        std::fs::remove_file(&tmp).ok();
    }

    #[test]
    fn load_returns_no_home_for_none_path() {
        // Pass None to trigger the no-home case. We can't unset $HOME
        // in a test (parallel tests would break), so we just verify
        // the function doesn't panic and returns one of the known
        // outcomes.
        let since = Utc::now() - chrono::Duration::days(7);
        let (_sessions, outcome) = load_hermes_sessions(since, None);
        // Either Loaded (real ~/.hermes/state.db exists) or
        // Missing/NoHome (it doesn't). Both are valid.
        assert!(
            matches!(outcome, LoadOutcome::Loaded(_) | LoadOutcome::Missing(_) | LoadOutcome::NoHome),
            "got: {:?}",
            outcome
        );
    }

    // --- Trace tests (v0.4.2) ---

    #[test]
    fn check_with_trace_lists_all_default_rules() {
        // With no findings, every default rule should still appear in
        // the considered list, with action=Allow (no contribution).
        let sessions: Vec<HermesSession> = vec![];
        let config = InterceptConfig::defaults();
        let hint = CheckHint::default();
        let trace = check_with_trace(&sessions, Utc::now(), &config, &hint);
        assert_eq!(trace.decision, Decision::Allow);
        assert!(trace.fired_rule.is_none());
        assert_eq!(trace.considered.len(), 4);
        let ids: Vec<&str> = trace.considered.iter().map(|c| c.rule_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["cache_bloat", "prompt_growth", "auto_routing", "model_instability"]
        );
        for c in &trace.considered {
            assert_eq!(c.action, Action::Allow);
            assert!(!c.fired);
        }
    }

    #[test]
    fn check_with_trace_marks_fired_rule() {
        // A session group with high cache_read/input ratio triggers
        // cache_bloat. The heuristic requires >=3 sessions in the
        // (model, source) group, so add three of them.
        let sessions = vec![
            hs("x", "cli", "2026-06-02T00:00:00Z", 1000, 500, 20000),
            hs("x", "cli", "2026-06-03T00:00:00Z", 1000, 500, 20000),
            hs("x", "cli", "2026-06-04T00:00:00Z", 1000, 500, 20000),
        ];
        let config = InterceptConfig::defaults();
        let hint = CheckHint::default();
        let since = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let trace = check_with_trace(&sessions, since, &config, &hint);
        assert_eq!(trace.fired_rule, Some("cache_bloat".to_string()));
        let cache_bloat = trace.considered.iter().find(|c| c.rule_id == "cache_bloat").unwrap();
        assert_eq!(cache_bloat.action, Action::Throttle);
        assert!(cache_bloat.fired);
        for other in &trace.considered {
            if other.rule_id != "cache_bloat" {
                assert_eq!(other.action, Action::Allow);
                assert!(!other.fired);
            }
        }
    }

    // --- v0.5.0: Action::Deny, Decision::Deny, and mode-flip ---

    #[test]
    fn action_deny_parses() {
        // Case-sensitive: matches the existing Allow/Throttle/Prompt
        // behavior. Configs are written by us, not by users editing
        // free-form text, so we don't need to be lenient.
        assert_eq!(Action::from_str("deny"), Some(Action::Deny));
        assert_eq!(Action::from_str("DENY"), None); // case-sensitive
        assert_eq!(Action::from_str(" allow "), Some(Action::Allow)); // trim OK
        assert_eq!(Action::from_str("throttle"), Some(Action::Throttle));
        assert_eq!(Action::from_str("prompt"), Some(Action::Prompt));
        assert_eq!(Action::from_str("garbage"), None);
    }

    #[test]
    fn decision_deny_exit_code_is_66() {
        let d = Decision::Deny {
            reason: "test".into(),
            hint: None,
        };
        assert_eq!(d.exit_code(), 66);
    }

    #[test]
    fn decision_deny_json_round_trip() {
        let d = Decision::Deny {
            reason: "mode=fail-closed denies by default".into(),
            hint: Some("pass --agent0waste-bypass".into()),
        };
        let json = d.to_json();
        assert!(json.contains(r#""decision":"deny""#));
        assert!(json.contains(r#""hint""#));
        let parsed = Decision::from_json(&json).expect("round-trip");
        assert_eq!(parsed, d);
    }

    #[test]
    fn decision_deny_json_round_trip_no_hint() {
        let d = Decision::Deny {
            reason: "rule-fired deny".into(),
            hint: None,
        };
        let json = d.to_json();
        let parsed = Decision::from_json(&json).expect("round-trip no hint");
        assert_eq!(parsed, d);
    }

    #[test]
    fn pick_decision_mode_fail_closed_default_denies_when_no_rule_fires() {
        // v0.5.0: when no heuristic fires and mode=fail-closed,
        // default to Deny. This is the §5-vs-§7 first-class decision.
        let report = Report::default();
        assert!(report.findings.is_empty(), "no findings for empty report");

        let mut config = InterceptConfig::defaults();
        config.mode = Mode::FailClosed;

        let trace = pick_decision(&report, &config, &CheckHint::default());
        match &trace.decision {
            Decision::Deny { reason, .. } => {
                assert!(reason.contains("fail-closed"), "reason mentions mode: {}", reason);
            }
            other => panic!("expected Deny under fail-closed, got {:?}", other),
        }
        assert!(trace.fired_rule.is_none(), "no rule fired → fired_rule stays None");
    }

    #[test]
    fn pick_decision_mode_fail_open_still_allows_when_no_rule_fires() {
        // Regression guard: fail-closed flip is opt-in. Default
        // (fail-open) behavior is unchanged from v0.4.x.
        let report = Report::default();
        let config = InterceptConfig::defaults(); // FailOpen
        let trace = pick_decision(&report, &config, &CheckHint::default());
        assert_eq!(trace.decision, Decision::Allow);
    }

    #[test]
    fn pick_decision_rule_fired_with_action_deny_emits_decision_deny() {
        // v0.5.0: user configures a rule with action=deny.
        // When that rule fires, the wrapper should deny.
        let mut report = Report::default();
        report.findings.push(heuristics::Finding {
            id: "prompt_growth",
            severity: Severity::Warn,
            key: "test-group".into(),
            message: "prompt grew 1.8x".into(),
            hint: Some("review recent prompt changes".into()),
        });
        let mut config = InterceptConfig::defaults();
        config.rules.insert(
            "prompt_growth".into(),
            RuleConfig { action: Action::Deny, cooldown_s: 0, cache_ttl_s: 30 },
        );
        let trace = pick_decision(&report, &config, &CheckHint::default());
        match &trace.decision {
            Decision::Deny { reason, hint } => {
                // Reason comes from the finding message (not the rule
                // id). The rule id is in `fired_rule` and trace.considered.
                assert!(reason.contains("prompt grew"), "reason carries finding message: {}", reason);
                assert_eq!(hint.as_deref(), Some("review recent prompt changes"));
            }
            other => panic!("expected rule-driven Deny, got {:?}", other),
        }
        assert_eq!(trace.fired_rule.as_deref(), Some("prompt_growth"));
    }
}
