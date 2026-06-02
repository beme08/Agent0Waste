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

/// Build a cost report from a list of session records.
///
/// - Filters to records with `started_at >= since` (inclusive)
/// - Fills in `cost_usd` for records missing it, using `pricing`
/// - Groups by the chosen granularity
pub fn report(
    sessions: &[SessionRecord],
    pricing: &Pricing,
    group_by: GroupBy,
    since: DateTime<Utc>,
) -> Vec<CostRow> {
    // Apply missing costs first, then filter, then group.
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
        "{:<key_w$}  {:>10}  {:>8}  {:>12}  {:>12}\n",
        "key", "cost_usd", "sessions", "input_tok", "output_tok",
        key_w = key_w
    );
    s.push_str(&"-".repeat(key_w + 50));
    s.push('\n');
    for r in rows {
        s.push_str(&format!(
            "{:<key_w$}  ${:>9.4}  {:>8}  {:>12}  {:>12}\n",
            r.key, r.cost_usd, r.sessions, r.input_tokens, r.output_tokens,
            key_w = key_w
        ));
    }
    let total: f64 = rows.iter().map(|r| r.cost_usd).sum();
    s.push_str(&"-".repeat(key_w + 50));
    s.push('\n');
    s.push_str(&format!(
        "{:<key_w$}  ${:>9.4}  {:>8}\n",
        "TOTAL", total, rows.iter().map(|r| r.sessions).sum::<usize>(),
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
        // 1M @ $2.50 + 0.5M @ $10 = 2.50 + 5.00 = 7.50
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
        // gpt-4o: 2M @ $2.50 = $5.00
        // grok-4: 1M @ $3.00 = $3.00
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
}
