use chrono::{DateTime, TimeZone, Utc};
use rusqlite::Connection;
use std::path::{Path, PathBuf};

/// One row read from Hermes' `state.db` `sessions` table.
///
/// Hermes' schema (verified 2026-06-02):
///   id, source, user_id, model, model_config, system_prompt,
///   parent_session_id, started_at, ended_at, end_reason,
///   message_count, tool_call_count,
///   input_tokens, output_tokens, cache_read_tokens,
///   cache_write_tokens, reasoning_tokens,
///   billing_provider, billing_base_url, billing_mode,
///   estimated_cost_usd, actual_cost_usd, cost_status, cost_source,
///   pricing_version, title, api_call_count, handoff_*,
///   cwd, rewind_count, archived
///
/// We expose only the fields relevant to waste accounting; the rest
/// stays in Hermes.
#[derive(Debug, Clone)]
pub struct HermesSession {
    pub id: String,
    pub model: String,
    pub source: String,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub reasoning_tokens: u64,
    pub message_count: u32,
    pub tool_call_count: u32,
}

impl HermesSession {
    /// Total tokens billed (input + output, ignoring cache). Matches
    /// the way most providers bill: cache reads are cheap (often 10%
    /// of input) but the *full* input gets reported. We expose raw
    /// numbers and let cost.rs decide what to do.
    pub fn total_billed(&self) -> u64 {
        self.input_tokens + self.output_tokens
    }
}

/// Default path to Hermes' state database.
pub fn default_state_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".hermes").join("state.db"))
}

/// Read all sessions started at-or-after `since` (UTC). Returns an
/// empty Vec if the file doesn't exist (Hermes not installed) — we
/// don't error in that case so a fresh machine can still run `cost`.
pub fn read_recent(since: DateTime<Utc>, path: &Path) -> Result<Vec<HermesSession>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let conn = Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    ).map_err(|e| format!("open {}: {}", path.display(), e))?;

    // `started_at` is stored as a unix epoch float. We compare as f64
    // to leverage the index (Hermes uses one on `started_at`).
    let since_ts = since.timestamp() as f64;

    let mut stmt = conn.prepare(
        "SELECT id, model, source, started_at, ended_at,
                COALESCE(input_tokens, 0), COALESCE(output_tokens, 0),
                COALESCE(cache_read_tokens, 0), COALESCE(cache_write_tokens, 0),
                COALESCE(reasoning_tokens, 0),
                COALESCE(message_count, 0), COALESCE(tool_call_count, 0)
         FROM sessions
         WHERE started_at >= ?1 AND archived = 0
         ORDER BY started_at DESC"
    ).map_err(|e| format!("prepare: {}", e))?;

    let rows = stmt.query_map([since_ts], |row| {
        let started_f: f64 = row.get(3)?;
        let ended_f: Option<f64> = row.get(4)?;
        let started_at = Utc.timestamp_opt(started_f as i64, 0).single().unwrap_or_else(Utc::now);
        let ended_at = ended_f.and_then(|f| Utc.timestamp_opt(f as i64, 0).single());
        Ok(HermesSession {
            id: row.get(0)?,
            model: row.get::<_, Option<String>>(1)?.unwrap_or_else(|| "unknown".to_string()),
            source: row.get::<_, Option<String>>(2)?.unwrap_or_else(|| "unknown".to_string()),
            started_at,
            ended_at,
            input_tokens: row.get(5)?,
            output_tokens: row.get(6)?,
            cache_read_tokens: row.get(7)?,
            cache_write_tokens: row.get(8)?,
            reasoning_tokens: row.get(9)?,
            message_count: row.get(10)?,
            tool_call_count: row.get(11)?,
        })
    }).map_err(|e| format!("query: {}", e))?;

    let mut out = Vec::new();
    for r in rows {
        out.push(r.map_err(|e| format!("row: {}", e))?);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};

    fn make_test_db(path: &Path) {
        let conn = Connection::open(path).unwrap();
        // Mirror the real Hermes schema (just the columns we read).
        conn.execute_batch(r#"
            CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                model TEXT,
                source TEXT,
                started_at REAL NOT NULL,
                ended_at REAL,
                input_tokens INTEGER DEFAULT 0,
                output_tokens INTEGER DEFAULT 0,
                cache_read_tokens INTEGER DEFAULT 0,
                cache_write_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                message_count INTEGER DEFAULT 0,
                tool_call_count INTEGER DEFAULT 0,
                archived INTEGER DEFAULT 0
            );
        "#).unwrap();
        let now = Utc::now().timestamp() as f64;
        conn.execute(
            "INSERT INTO sessions (id, model, source, started_at, ended_at, input_tokens, output_tokens, cache_read_tokens) VALUES (?,?,?,?,?,?,?,?)",
            params!["s1", "grok-4.3", "cli", now - 60.0, Some(now - 30.0), 1000u64, 200u64, 5000u64],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, model, source, started_at, ended_at, input_tokens, output_tokens) VALUES (?,?,?,?,?,?,?)",
            params!["s2", "gpt-4o", "telegram", now - 3600.0, Some(now - 3500.0), 500u64, 50u64],
        ).unwrap();
        conn.execute(
            "INSERT INTO sessions (id, model, source, started_at, ended_at) VALUES (?,?,?,?,?)",
            params!["s3", "stepfun/step-3.7-flash:free", "cron", now - 86400.0, Some(now - 86300.0)],
        ).unwrap();
    }

    #[test]
    fn read_recent_returns_only_within_window() {
        let dir = std::env::temp_dir().join(format!("agent0waste-hermes-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("state.db");
        make_test_db(&db);
        let one_hour_ago = Utc::now() - chrono::Duration::hours(1);
        let sessions = read_recent(one_hour_ago, &db).unwrap();
        assert_eq!(sessions.len(), 2); // s1 (1 min ago) and s2 (1 hr ago); s3 (1 day ago) excluded
        let ids: Vec<&str> = sessions.iter().map(|s| s.id.as_str()).collect();
        assert!(ids.contains(&"s1"));
        assert!(ids.contains(&"s2"));
        assert!(!ids.contains(&"s3"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn read_recent_handles_missing_file() {
        let p = std::env::temp_dir().join("definitely-missing-state.db");
        let recs = read_recent(Utc::now(), &p).unwrap();
        assert!(recs.is_empty());
    }

    #[test]
    fn total_billed_sums_input_and_output() {
        let s = HermesSession {
            id: "x".into(), model: "gpt-4o".into(), source: "cli".into(),
            started_at: Utc::now(), ended_at: None,
            input_tokens: 100, output_tokens: 50,
            cache_read_tokens: 0, cache_write_tokens: 0, reasoning_tokens: 0,
            message_count: 0, tool_call_count: 0,
        };
        assert_eq!(s.total_billed(), 150);
    }
}
