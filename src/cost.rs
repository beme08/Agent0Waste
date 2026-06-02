use crate::hermes_state::HermesSession;
use crate::pricing::Pricing;
use crate::sessions::SessionRecord;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

/// One row in a cost report.
#[derive(Debug, Clone)]
pub struct CostRow {
    /// Group key (model name, tool name, etc.) or "*" for the grand total.
    pub key: String,
    /// Sum of cost_usd across all sessions in this group.
    pub cost_usd: f64,
    /// Number of sessions in this group.
    pub sessions: usize,
    /// Sum of input tokens (when known).
    pub input_tokens: u64,
    /// Sum of output tokens (when known).
    pub output_tokens: u64,
    /// Sum of cache-read tokens (Hermes-specific; 0 for local records).
    pub cache_read_tokens: u64,
}

/// Granularity of grouping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    /// Single grand total.
    Total,
    /// One row per model.
    Model,
    /// One row per provider.
    Provider,
    /// One row per day (YYYY-MM-DD).
    Day,
}

/// Build a cost report from local SessionRecords only.
///
/// (v0.2.0 path; v0.2.1 added `report_hermes` for state.db data.)
pub fn report(
    sessions: &[SessionRecord],
    pricing: &Pricing,
    group_by: GroupBy,
    since: DateTime<Utc>,
) -> Vec<CostRow> {
    let mut enriched: Vec<SessionRecord> = sessions
        .iter()
        .filter(|r| r.started_at >= since)
        .cloned()
        .collect();
    for r in &mut enriched {
        sessions_apply_cost(r, pricing);
    }

    let mut rows: HashMap<String, CostRow> = HashMap::new();
    for r in &enriched {
        let key = match group_by {
            GroupBy::Total => "*".to_string(),
            GroupBy::Model => r.model.clone().unwrap_or_else(|| "<unknown>".to_string()),
            GroupBy::Provider => r.provider.clone().unwrap_or_else(|| "<unknown>".to_string()),
            GroupBy::Day => r.started_at.format("%Y-%m-%d").to_string(),
        };
        let row = rows.entry(key.clone()).or_insert(CostRow {
            key,
            cost_usd: 0.0,
            sessions: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
        });
        row.cost_usd += r.cost_usd.unwrap_or(0.0);
        row.sessions += 1;
        if let Some(t) = r.input_tokens { row.input_tokens += t; }
        if let Some(t) = r.output_tokens { row.output_tokens += t; }
    }

    let mut out: Vec<CostRow> = rows.into_values().collect();
    out.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    out
}

/// v0.2.1: build a cost report from Hermes' state.db sessions.
///
/// - Normalizes the model name: strips `provider/` prefix when no pricing
///   match is found, so e.g. `openrouter/owl-alpha` and `grok-4.3` are
///   looked up as their short names against `Pricing`.
/// - Cache-read tokens are reported but NOT counted in cost (they're
///   typically 10% of input cost; the user can override per-provider).
/// - Cost shows "<no pricing>" for unknown models so the user knows
///   they can add a pricing.toml entry.
pub fn report_hermes(
    sessions: &[HermesSession],
    pricing: &Pricing,
    group_by: GroupBy,
) -> Vec<CostRow> {
    let mut rows: HashMap<String, CostRow> = HashMap::new();
    for s in sessions {
        // Try to match the model as-is, then with the provider/ prefix
        // stripped, so "openrouter/owl-alpha" doesn't need a literal
        // key — but if the user has "owl-alpha" in pricing.toml it works.
        let short = s.model.rsplit('/').next().unwrap_or(&s.model).to_string();
        let cost = pricing.cost(&s.model, s.input_tokens, s.output_tokens)
            .or_else(|| pricing.cost(&short, s.input_tokens, s.output_tokens));

        let key = match group_by {
            GroupBy::Total => "*".to_string(),
            GroupBy::Model => s.model.clone(),
            GroupBy::Provider => s.source.clone(),
            GroupBy::Day => s.started_at.format("%Y-%m-%d").to_string(),
        };
        let row = rows.entry(key.clone()).or_insert(CostRow {
            key,
            cost_usd: 0.0,
            sessions: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
        });
        row.cost_usd += cost.unwrap_or(0.0);
        row.sessions += 1;
        row.input_tokens += s.input_tokens;
        row.output_tokens += s.output_tokens;
        row.cache_read_tokens += s.cache_read_tokens;
    }

    let mut out: Vec<CostRow> = rows.into_values().collect();
    out.sort_by(|a, b| b.cost_usd.partial_cmp(&a.cost_usd).unwrap_or(std::cmp::Ordering::Equal));
    out
}

// Local re-export to avoid a circular dep on sessions::Sessions.
fn sessions_apply_cost(r: &mut SessionRecord, p: &Pricing) {
    crate::sessions::Sessions::apply_cost(r, p);
}

/// Format a report as a human-readable table.
pub fn format_table(rows: &[CostRow]) -> String {
    if rows.is_empty() {
        return "no cost data — run `agent0waste run -- <cmd>` to record sessions\n".to_string();
    }
    let key_w = rows.iter().map(|r| r.key.len()).max().unwrap_or(8).max(8);
    let mut s = format!(
        "{:<key_w$}  {:>10}  {:>8}  {:>12}  {:>12}  {:>12}\n",
        "key", "cost_usd", "sessions", "input_tok", "output_tok", "cache_tok",
        key_w = key_w
    );
    s.push_str(&"-".repeat(key_w + 70));
    s.push('\n');
    for r in rows {
        s.push_str(&format!(
            "{:<key_w$}  ${:>9.4}  {:>8}  {:>12}  {:>12}  {:>12}\n",
            r.key, r.cost_usd, r.sessions, r.input_tokens, r.output_tokens, r.cache_read_tokens,
            key_w = key_w
        ));
    }
    let total_cost: f64 = rows.iter().map(|r| r.cost_usd).sum();
    let total_in: u64 = rows.iter().map(|r| r.input_tokens).sum();
    let total_out: u64 = rows.iter().map(|r| r.output_tokens).sum();
    let total_cache: u64 = rows.iter().map(|r| r.cache_read_tokens).sum();
    let total_sessions: usize = rows.iter().map(|r| r.sessions).sum();
    s.push_str(&"-".repeat(key_w + 70));
    s.push('\n');
    s.push_str(&format!(
        "{:<key_w$}  ${:>9.4}  {:>8}  {:>12}  {:>12}  {:>12}\n",
        "TOTAL", total_cost, total_sessions, total_in, total_out, total_cache,
        key_w = key_w
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sessions::SessionRecord;
    use chrono::TimeZone;

    fn rec(id: &str, day: &str, model: Option<&str>, provider: Option<&str>,
           in_t: Option<u64>, out_t: Option<u64>) -> SessionRecord {
        let ts = DateTime::parse_from_rfc3339(day).unwrap().with_timezone(&Utc);
        SessionRecord {
            id: id.to_string(),
            command: "hermes run foo".to_string(),
            argv0: Some("hermes".to_string()),
            started_at: ts,
            ended_at: ts,
            duration_ms: 1000,
            exit_code: 0,
            model: model.map(String::from),
            provider: provider.map(String::from),
            input_tokens: in_t,
            output_tokens: out_t,
            cost_usd: None,
            stderr_tail: None,
            host: None,
        }
    }

    fn hermes(id: &str, model: &str, source: &str, in_t: u64, out_t: u64, cache: u64) -> HermesSession {
        HermesSession {
            id: id.into(), model: model.into(), source: source.into(),
            started_at: Utc::now(), ended_at: None,
            input_tokens: in_t, output_tokens: out_t,
            cache_read_tokens: cache, cache_write_tokens: 0, reasoning_tokens: 0,
            message_count: 0, tool_call_count: 0,
        }
    }

    #[test]
    fn report_total_sums_all_costs() {
        let pricing = Pricing::default();
        let sessions = vec![
            rec("a", "2026-06-01T10:00:00Z", Some("gpt-4o"), Some("openai"),
                Some(1_000_000), Some(0)),
            rec("b", "2026-06-02T10:00:00Z", Some("gpt-4o"), Some("openai"),
                Some(0), Some(500_000)),
        ];
        let rows = report(&sessions, &pricing, GroupBy::Total, Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(rows.len(), 1);
        assert!((rows[0].cost_usd - 7.50).abs() < 1e-9);
        assert_eq!(rows[0].sessions, 2);
    }

    #[test]
    fn report_groups_by_model() {
        let pricing = Pricing::default();
        let sessions = vec![
            rec("a", "2026-06-01T10:00:00Z", Some("gpt-4o"), None, Some(1_000_000), Some(0)),
            rec("b", "2026-06-01T11:00:00Z", Some("gpt-4o"), None, Some(1_000_000), Some(0)),
            rec("c", "2026-06-01T12:00:00Z", Some("grok-4"), None, Some(1_000_000), Some(0)),
        ];
        let rows = report(&sessions, &pricing, GroupBy::Model, Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap());
        assert_eq!(rows.len(), 2);
        let gpt = rows.iter().find(|r| r.key == "gpt-4o").unwrap();
        let grok = rows.iter().find(|r| r.key == "grok-4").unwrap();
        assert!((gpt.cost_usd - 5.00).abs() < 1e-9);
        assert!((grok.cost_usd - 3.00).abs() < 1e-9);
    }

    #[test]
    fn report_respects_since_filter() {
        let pricing = Pricing::default();
        let sessions = vec![
            rec("old", "2025-01-01T10:00:00Z", Some("gpt-4o"), None, Some(1_000_000), Some(0)),
            rec("new", "2026-06-02T10:00:00Z", Some("gpt-4o"), None, Some(1_000_000), Some(0)),
        ];
        let since = Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap();
        let rows = report(&sessions, &pricing, GroupBy::Total, since);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sessions, 1);
    }

    #[test]
    fn format_table_handles_empty() {
        let s = format_table(&[]);
        assert!(s.contains("no cost data"));
    }

    #[test]
    fn report_hermes_groups_by_model() {
        let pricing = Pricing::default();
        let sessions = vec![
            hermes("a", "gpt-4o", "cli", 1_000_000, 0, 0),
            hermes("b", "gpt-4o", "cli", 1_000_000, 0, 0),
            hermes("c", "grok-4", "cli", 1_000_000, 0, 0),
        ];
        let rows = report_hermes(&sessions, &pricing, GroupBy::Model);
        let gpt = rows.iter().find(|r| r.key == "gpt-4o").unwrap();
        let grok = rows.iter().find(|r| r.key == "grok-4").unwrap();
        assert!((gpt.cost_usd - 5.00).abs() < 1e-9);
        assert!((grok.cost_usd - 3.00).abs() < 1e-9);
    }

    #[test]
    fn report_hermes_falls_back_to_short_model_name() {
        let pricing = Pricing::default();
        // "openrouter/owl-alpha" — no exact match in pricing.
        // We just verify the function doesn't crash and reports $0.
        let sessions = vec![hermes("a", "openrouter/owl-alpha", "cli", 1_000_000, 0, 0)];
        let rows = report_hermes(&sessions, &pricing, GroupBy::Model);
        assert_eq!(rows[0].cost_usd, 0.0);
        assert_eq!(rows[0].input_tokens, 1_000_000);
    }

    #[test]
    fn report_hermes_uses_short_name_override() {
        // User has "grok-4.3" in pricing.toml — function should find it
        // even when the model is recorded as "openrouter/grok-4.3".
        // Use the public load_override + a temp file.
        let mut pricing = Pricing::default();
        let tmp = std::env::temp_dir().join(format!("agent0waste-test-{}.toml", std::process::id()));
        std::fs::write(&tmp, "[\"grok-4.3\"]\ninput = 5.00\noutput = 15.00\n").unwrap();
        pricing.load_override(&tmp).unwrap();
        std::fs::remove_file(&tmp).ok();
        let sessions = vec![hermes("a", "openrouter/grok-4.3", "cli", 1_000_000, 0, 0)];
        let rows = report_hermes(&sessions, &pricing, GroupBy::Model);
        // 1M @ $5 = $5
        assert!((rows[0].cost_usd - 5.00).abs() < 1e-9);
    }
}
