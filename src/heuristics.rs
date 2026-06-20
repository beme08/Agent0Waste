use crate::hermes_state::HermesSession;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// Severity of a heuristic finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Worth knowing, not urgent.
    Info,
    /// Real money or time being left on the table.
    Warn,
    /// Should fix soon — clear waste pattern.
    High,
}

impl Severity {
    pub fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warn => "warn",
            Severity::High => "high",
        }
    }
}

/// One finding from a heuristic check.
#[derive(Debug, Clone)]
pub struct Finding {
    pub id: &'static str,
    pub severity: Severity,
    /// Group key (e.g. a model name, a date, "global"). Empty for global.
    pub key: String,
    /// Human-readable one-liner.
    pub message: String,
    /// Optional follow-up suggestion.
    pub hint: Option<String>,
}

/// All findings over a list of Hermes sessions.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn new() -> Self { Self::default() }
    pub fn push(&mut self, f: Finding) { self.findings.push(f); }
    #[allow(dead_code)] // consumed by #[cfg(test)] callers only
    pub fn is_empty(&self) -> bool { self.findings.is_empty() }

    /// Render as a human-readable block.
    pub fn format(&self) -> String {
        if self.findings.is_empty() {
            return "  (no warnings — your usage looks clean)\n".to_string();
        }
        let mut s = String::new();
        // Sort: High first, then Warn, then Info; stable within group.
        let order = |sev: Severity| match sev {
            Severity::High => 0,
            Severity::Warn => 1,
            Severity::Info => 2,
        };
        let mut sorted = self.findings.clone();
        sorted.sort_by_key(|f| order(f.severity));
        for f in &sorted {
            s.push_str(&format!("  [{}] {} — {}\n", f.severity.label(), f.id, f.message));
            if let Some(h) = &f.hint {
                s.push_str(&format!("         → {}\n", h));
            }
        }
        s
    }

    /// Render as JSON array (for --export json).
    pub fn to_json(&self) -> Vec<serde_json::Value> {
        self.findings.iter().map(|f| serde_json::json!({
            "id": f.id,
            "severity": f.severity.label(),
            "key": f.key,
            "message": f.message,
            "hint": f.hint,
        })).collect()
    }
}

// --------------------------------------------------------------------------
// H1: Cache bloat
// --------------------------------------------------------------------------
//
// A session is "cache bloat" when cache_read_tokens dwarfs input_tokens.
// Cache reads are typically 10% the cost of input, so a 10x ratio means
// we're paying 100% of input cost for what was already in the cache — i.e.
// the user's context is full of repeated content that should be trimmed.
//
// Thresholds:
//   - per-session: cache_read / input >= 3.0 AND cache_read >= 1000
//   - per-group:   at least 3 sessions before we surface a finding
//     (a 1-session finding is almost always noise — a single test).
pub fn h1_cache_bloat(sessions: &[HermesSession]) -> Vec<Finding> {
    let mut out = Vec::new();
    // Aggregate per (model, source) so we don't 1000-line the report.
    let mut by_group: HashMap<(String, String), (u64, u64, usize)> = HashMap::new();
    for s in sessions {
        if s.input_tokens == 0 { continue; }
        let ratio = s.cache_read_tokens as f64 / s.input_tokens as f64;
        if ratio < 3.0 { continue; }
        if s.cache_read_tokens < 1000 { continue; }
        let k = (s.model.clone(), s.source.clone());
        let e = by_group.entry(k).or_insert((0, 0, 0));
        e.0 += s.input_tokens;
        e.1 += s.cache_read_tokens;
        e.2 += 1;
    }
    for ((model, source), (in_t, cache_t, n)) in by_group {
        if n < 3 { continue; } // skip noise: <3 sessions
        let ratio = cache_t as f64 / in_t as f64;
        out.push(Finding {
            id: "cache_bloat",
            severity: if ratio >= 8.0 { Severity::High } else { Severity::Warn },
            key: format!("{}:{}", model, source),
            message: format!(
                "{} sessions on {} ({}) had cache_read/input = {:.1}x ({} in / {} cache)",
                n, model, source, ratio, in_t, cache_t
            ),
            hint: Some("context is full of repeated content — trim skills, history, or system prompt".to_string()),
        });
    }
    out
}

// --------------------------------------------------------------------------
// H2: Prompt growth
// --------------------------------------------------------------------------
//
// For each (model, source) pair, group input_tokens by day. If the most
// recent 3 days are > 1.5x the prior 3 days, that's growing. Catches the
// "I haven't added a tool, why are prompts 2x larger?" case.
//
// Threshold: at least 3 days in each window (so we have signal).
pub fn h2_prompt_growth(sessions: &[HermesSession], since: DateTime<Utc>) -> Vec<Finding> {
    let mut out = Vec::new();
    // Per (model, source, day) sum of input_tokens.
    let mut by_day: HashMap<(String, String, String), u64> = HashMap::new();
    for s in sessions {
        if s.started_at < since { continue; }
        let day = s.started_at.format("%Y-%m-%d").to_string();
        *by_day.entry((s.model.clone(), s.source.clone(), day)).or_insert(0) += s.input_tokens;
    }
    // For each (model, source), compare recent-3 vs prior-3.
    let mut by_group: HashMap<(String, String), Vec<(String, u64)>> = HashMap::new();
    for ((m, s, d), t) in by_day {
        by_group.entry((m, s)).or_default().push((d, t));
    }
    for ((model, source), mut days) in by_group {
        days.sort_by(|a, b| b.0.cmp(&a.0)); // newest first
        if days.len() < 6 { continue; } // need 3 + 3 distinct days
        let recent: u64 = days.iter().take(3).map(|(_, t)| *t).sum::<u64>().max(1);
        let prior: u64 = days.iter().skip(3).take(3).map(|(_, t)| *t).sum::<u64>().max(1);
        let growth = recent as f64 / prior as f64;
        if growth >= 1.5 {
            out.push(Finding {
                id: "prompt_growth",
                severity: if growth >= 2.5 { Severity::High } else { Severity::Warn },
                key: format!("{}:{}", model, source),
                message: format!(
                    "{} ({}) prompts grew {:.1}x week-over-week ({} → {} input tok/day)",
                    model, source, growth, prior / 3, recent / 3
                ),
                hint: Some("check for new skills, expanded system prompt, or accumulating tool history".to_string()),
            });
        }
    }
    out
}

// --------------------------------------------------------------------------
// H3: Auto-routing
// --------------------------------------------------------------------------
//
// Sessions where the model is literally "auto" — Hermes is choosing
// for you. That's fine for some, but it's the #1 reason cost varies
// unpredictably between days. Flag it (info-level).
//
// Threshold: at least 5 sessions. Below that, 'auto' use is intentional
// exploration, not a pattern.
pub fn h3_auto_routing(sessions: &[HermesSession]) -> Vec<Finding> {
    let mut n = 0usize;
    let mut in_t = 0u64;
    for s in sessions {
        if s.model == "auto" {
            n += 1;
            in_t += s.input_tokens;
        }
    }
    if n < 5 { return Vec::new(); }
    vec![Finding {
        id: "auto_routing",
        severity: Severity::Info,
        key: "auto".to_string(),
        message: format!(
            "{} sessions used model='auto' ({} input tok total) — Hermes picks the model",
            n, in_t
        ),
        hint: Some("pin a model in your config for predictable cost; 'auto' is for exploration".to_string()),
    }]
}

// --------------------------------------------------------------------------
// H4: Model instability
// --------------------------------------------------------------------------
//
// Same source, but the model changed across the window. Suggests the user
// is experimenting, or the system is auto-rotating, or something else.
// Flag if any source uses 3+ different models (2 is a normal A/B).
pub fn h4_model_instability(sessions: &[HermesSession], since: DateTime<Utc>) -> Vec<Finding> {
    let mut by_source: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
    for s in sessions {
        if s.started_at < since { continue; }
        by_source.entry(s.source.clone()).or_default().insert(s.model.clone());
    }
    let mut out = Vec::new();
    for (source, models) in by_source {
        if models.len() < 3 { continue; }
        let mut sorted: Vec<String> = models.into_iter().collect();
        sorted.sort();
        out.push(Finding {
            id: "model_instability",
            severity: Severity::Info,
            key: source.clone(),
            message: format!(
                "{} used {} different models in the window: {}",
                source,
                sorted.len(),
                sorted.join(", ")
            ),
            hint: Some("if you meant to pin a model, set it; if you're A/B testing, this is fine".to_string()),
        });
    }
    out
}

/// Run all heuristics against a set of sessions.
pub fn run_all(sessions: &[HermesSession], since: DateTime<Utc>) -> Report {
    let mut r = Report::new();
    for f in h1_cache_bloat(sessions) { r.push(f); }
    for f in h2_prompt_growth(sessions, since) { r.push(f); }
    for f in h3_auto_routing(sessions) { r.push(f); }
    for f in h4_model_instability(sessions, since) { r.push(f); }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hs(model: &str, source: &str, day: &str, in_t: u64, out_t: u64, cache: u64) -> HermesSession {
        HermesSession {
            id: format!("{}-{}-{}", model, source, day),
            model: model.into(), source: source.into(),
            started_at: DateTime::parse_from_rfc3339(day).unwrap().with_timezone(&Utc),
            ended_at: None,
            input_tokens: in_t, output_tokens: out_t,
            cache_read_tokens: cache, cache_write_tokens: 0, reasoning_tokens: 0,
            message_count: 0, tool_call_count: 0,
        }
    }

    #[test]
    fn h1_flags_cache_bloat() {
        // Need 3+ sessions in the same (model, source) group.
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 5_000),
        ];
        let f = h1_cache_bloat(&s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "cache_bloat");
    }

    #[test]
    fn h1_skips_small_groups() {
        // 2 sessions — below the 3-session threshold, should not flag.
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 5_000),
            hs("grok-4.3", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 5_000),
        ];
        assert!(h1_cache_bloat(&s).is_empty());
    }

    #[test]
    fn h1_ignores_low_cache_ratios() {
        let s = vec![
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 500), // 0.5x
        ];
        assert!(h1_cache_bloat(&s).is_empty());
    }

    #[test]
    fn h2_flags_growth() {
        // Need 6+ distinct days (3 + 3) to fire.
        let s = vec![
            hs("gpt-4o", "cli", "2026-05-20T10:00:00Z", 1_000, 0, 0),
            hs("gpt-4o", "cli", "2026-05-21T10:00:00Z", 1_000, 0, 0),
            hs("gpt-4o", "cli", "2026-05-22T10:00:00Z", 1_000, 0, 0),
            hs("gpt-4o", "cli", "2026-05-25T10:00:00Z", 2_500, 0, 0),
            hs("gpt-4o", "cli", "2026-05-26T10:00:00Z", 2_500, 0, 0),
            hs("gpt-4o", "cli", "2026-05-27T10:00:00Z", 2_500, 0, 0),
        ];
        let since = DateTime::parse_from_rfc3339("2026-05-20T00:00:00Z").unwrap().with_timezone(&Utc);
        let f = h2_prompt_growth(&s, since);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "prompt_growth");
    }

    #[test]
    fn h3_flags_auto_routing() {
        // Need 5+ sessions to fire.
        let s = vec![
            hs("auto", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 0),
            hs("auto", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 0),
            hs("auto", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 0),
            hs("auto", "cli", "2026-06-02T13:00:00Z", 1_000, 100, 0),
            hs("auto", "cli", "2026-06-02T14:00:00Z", 1_000, 100, 0),
        ];
        let f = h3_auto_routing(&s);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "auto_routing");
    }

    #[test]
    fn h3_ignores_few_auto_sessions() {
        // Only 3 auto sessions — under threshold, not flagged.
        let s = vec![
            hs("auto", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 0),
            hs("auto", "cli", "2026-06-02T11:00:00Z", 1_000, 100, 0),
            hs("auto", "cli", "2026-06-02T12:00:00Z", 1_000, 100, 0),
        ];
        assert!(h3_auto_routing(&s).is_empty());
    }

    #[test]
    fn h3_ignores_when_no_auto() {
        let s = vec![hs("gpt-4o", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 0)];
        assert!(h3_auto_routing(&s).is_empty());
    }

    #[test]
    fn h4_flags_model_instability() {
        // Same source "cli", but 3+ different models in the window.
        let s = vec![
            hs("gpt-4o", "cli", "2026-06-01T10:00:00Z", 100, 0, 0),
            hs("grok-4.3", "cli", "2026-06-02T10:00:00Z", 100, 0, 0),
            hs("claude-3.7-sonnet", "cli", "2026-06-03T10:00:00Z", 100, 0, 0),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-01T00:00:00Z").unwrap().with_timezone(&Utc);
        let f = h4_model_instability(&s, since);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].id, "model_instability");
    }

    #[test]
    fn run_all_produces_no_findings_on_clean_data() {
        let s = vec![
            hs("gpt-4o", "cli", "2026-06-02T10:00:00Z", 1_000, 100, 0),
        ];
        let since = DateTime::parse_from_rfc3339("2026-06-02T00:00:00Z").unwrap().with_timezone(&Utc);
        let r = run_all(&s, since);
        assert!(r.is_empty());
    }
}
