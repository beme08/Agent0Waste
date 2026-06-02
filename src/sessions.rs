use crate::pricing::Pricing;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One recorded session. Persisted to one file per session under
/// `~/.local/share/agent0waste/sessions/<id>.json`.
///
/// Schema is intentionally flat — every field is also a CLI flag or
/// a future report column. Adding a field is backwards-compatible
/// because `unknown` deserialization is a soft error we can ignore.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionRecord {
    /// Unique id: `sess-<unix_ms>-<rand>` (e.g. `sess-1717350000123-7f3a`).
    pub id: String,
    /// Command line as it was invoked (with argv). For display + grouping.
    pub command: String,
    /// Argv[0] for sanity; can be re-extracted from `command` if needed.
    pub argv0: Option<String>,
    /// Wall-clock start time, UTC.
    pub started_at: DateTime<Utc>,
    /// Wall-clock end time, UTC.
    pub ended_at: DateTime<Utc>,
    /// Duration in milliseconds. `ended_at - started_at`.
    pub duration_ms: u64,
    /// Process exit status: 0 = success, anything else = non-zero exit.
    pub exit_code: i32,
    /// LLM model used, if detected (e.g. "grok-4.3 (xai-oauth)"). None
    /// when the run was a non-LLM command.
    pub model: Option<String>,
    /// Provider prefix before the model name, e.g. "xai-oauth". None
    /// when the run was a non-LLM command.
    pub provider: Option<String>,
    /// Tokens read from the tool log (best-effort, see v0.2 design).
    /// None when no log was parseable.
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    /// Computed cost in USD. None when pricing is unknown for the model
    /// OR when token counts are missing.
    pub cost_usd: Option<f64>,
    /// Stderr last line, truncated to 500 chars. Useful when a session
    /// fails — the user can grep for it.
    pub stderr_tail: Option<String>,
    /// Hostname of the machine that recorded the session. Helps when
    /// syncing logs across devices later.
    pub host: Option<String>,
}

impl SessionRecord {
    /// Generate a session id. Uses ms-precision timestamp + 4 hex chars
    /// of entropy from /dev/urandom when available, else zeros.
    pub fn new_id() -> String {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0);
        let entropy: u16 = {
            // Read exactly 2 bytes — never read_to_end on a stream device.
            let mut f = std::fs::File::open("/dev/urandom").ok();
            let mut buf = [0u8; 2];
            if let Some(ref mut file) = f {
                let _ = std::io::Read::read_exact(file, &mut buf);
            }
            u16::from_le_bytes(buf)
        };
        format!("sess-{}-{:04x}", now, entropy)
    }
}

/// Storage backend: directory of `<id>.json` files.
#[derive(Debug, Clone)]
pub struct Sessions {
    base: PathBuf,
    cap: usize,
}

impl Sessions {
    pub const DEFAULT_CAP: usize = 2000;

    /// New storage at `~/.local/share/agent0waste/sessions/`. Creates
    /// the directory if missing. Cap defaults to 2000 (was 500 in
    /// v0.2.0-beta; bumped because real users hit ~334 sessions/day).
    /// Honors `AGENT0WASTE_SESSIONS_CAP` env var if set (0 = no cap).
    pub fn new() -> Self {
        let base = dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("agent0waste")
            .join("sessions");
        let _ = std::fs::create_dir_all(&base);
        let cap = std::env::var("AGENT0WASTE_SESSIONS_CAP")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(Self::DEFAULT_CAP);
        Self { base, cap }
    }

    /// New storage at a custom path. Used by tests.
    pub fn at(base: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&base);
        Self { base, cap: Self::DEFAULT_CAP }
    }

    /// New storage at a custom path with a custom cap. Use this when
    /// the user passes --cap N or --no-cap.
    pub fn at_with_cap(base: PathBuf, cap: usize) -> Self {
        let _ = std::fs::create_dir_all(&base);
        Self { base, cap }
    }

    pub fn base(&self) -> &Path { &self.base }
    pub fn cap(&self) -> usize { self.cap }

    /// Persist a record. Atomic write via temp file + rename. After
    /// writing, enforces the FIFO cap by deleting the oldest files.
    /// Returns `true` if any files were dropped to stay under the cap.
    pub fn record(&self, rec: &SessionRecord) -> Result<bool, String> {
        let path = self.base.join(format!("{}.json", rec.id));
        let tmp = self.base.join(format!(".{}.tmp", rec.id));
        let json = serde_json::to_string_pretty(rec)
            .map_err(|e| format!("serialize: {}", e))?;
        std::fs::write(&tmp, &json).map_err(|e| format!("write: {}", e))?;
        std::fs::rename(&tmp, &path).map_err(|e| format!("rename: {}", e))?;
        Ok(self.enforce_cap())
    }

    /// Load all records, sorted by `started_at` descending. Missing
    /// or malformed files are skipped silently (logged to stderr).
    pub fn list(&self) -> Vec<SessionRecord> {
        let mut recs: Vec<SessionRecord> = match std::fs::read_dir(&self.base) {
            Ok(rd) => rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
                .filter_map(|e| {
                    let bytes = std::fs::read(e.path()).ok()?;
                    serde_json::from_slice::<SessionRecord>(&bytes).ok()
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        recs.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        recs
    }

    /// Apply cost to a record using a Pricing table. Mutates and returns
    /// the same record. No-op if cost is already set or if model is unknown.
    pub fn apply_cost(rec: &mut SessionRecord, pricing: &Pricing) {
        if rec.cost_usd.is_some() { return; }
        if let (Some(model), Some(in_t), Some(out_t)) = (&rec.model, rec.input_tokens, rec.output_tokens) {
            rec.cost_usd = pricing.cost(model, in_t, out_t);
        }
    }

    /// Returns the number of files deleted to stay under the cap.
    /// Public for the `sessions` subcommand to report drops.
    pub fn enforce_cap(&self) -> bool {
        if self.cap == 0 { return false; } // 0 = "no cap"; do not drop
        let mut paths: Vec<_> = match std::fs::read_dir(&self.base) {
            Ok(rd) => rd.filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("json"))
                .filter_map(|e| {
                    let m = e.metadata().ok()?;
                    let modified = m.modified().ok()?;
                    Some((e.path(), modified))
                })
                .collect(),
            Err(_) => return false,
        };
        if paths.len() <= self.cap { return false; }
        paths.sort_by_key(|(_, t)| *t);
        let to_delete = paths.len() - self.cap;
        for (path, _) in paths.iter().take(to_delete) {
            let _ = std::fs::remove_file(path);
        }
        true
    }
}

impl Default for Sessions {
    fn default() -> Self { Self::new() }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(id: &str, started: &str, model: Option<&str>, cost: Option<f64>) -> SessionRecord {
        SessionRecord {
            id: id.to_string(),
            command: "hermes run foo".to_string(),
            argv0: Some("hermes".to_string()),
            started_at: DateTime::parse_from_rfc3339(started).unwrap().with_timezone(&Utc),
            ended_at: DateTime::parse_from_rfc3339(started).unwrap().with_timezone(&Utc),
            duration_ms: 1234,
            exit_code: 0,
            model: model.map(String::from),
            provider: None,
            input_tokens: Some(1000),
            output_tokens: Some(500),
            cost_usd: cost,
            stderr_tail: None,
            host: None,
        }
    }

    #[test]
    fn round_trip_persists_record() {
        let dir = tempdir();
        let s = Sessions::at(dir.path().to_path_buf());
        let r = rec("sess-test-1", "2026-06-02T10:00:00Z", Some("gpt-4o"), Some(0.01));
        s.record(&r).unwrap();
        let loaded = s.list();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].id, "sess-test-1");
        assert_eq!(loaded[0].cost_usd, Some(0.01));
    }

    #[test]
    fn list_sorts_newest_first() {
        let dir = tempdir();
        let s = Sessions::at(dir.path().to_path_buf());
        s.record(&rec("sess-a", "2026-06-02T10:00:00Z", None, None)).unwrap();
        s.record(&rec("sess-b", "2026-06-02T12:00:00Z", None, None)).unwrap();
        s.record(&rec("sess-c", "2026-06-02T11:00:00Z", None, None)).unwrap();
        let loaded = s.list();
        assert_eq!(loaded.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
                   vec!["sess-b", "sess-c", "sess-a"]);
    }

    #[test]
    fn fifo_cap_deletes_oldest() {
        let dir = tempdir();
        let mut s = Sessions::at(dir.path().to_path_buf());
        s.cap = 3;
        s.record(&rec("sess-1", "2026-06-02T10:00:00Z", None, None)).unwrap();
        s.record(&rec("sess-2", "2026-06-02T11:00:00Z", None, None)).unwrap();
        s.record(&rec("sess-3", "2026-06-02T12:00:00Z", None, None)).unwrap();
        s.record(&rec("sess-4", "2026-06-02T13:00:00Z", None, None)).unwrap();
        let loaded = s.list();
        assert_eq!(loaded.len(), 3);
        let ids: Vec<&str> = loaded.iter().map(|r| r.id.as_str()).collect();
        assert!(!ids.contains(&"sess-1"), "oldest should be evicted");
        assert!(ids.contains(&"sess-4"), "newest should be kept");
    }

    #[test]
    fn apply_cost_fills_in_when_missing() {
        let _dir = tempdir();
        let mut r = rec("sess-5", "2026-06-02T10:00:00Z", Some("gpt-4o"), None);
        r.input_tokens = Some(1_000_000);
        r.output_tokens = Some(500_000);
        let pricing = Pricing::default();
        Sessions::apply_cost(&mut r, &pricing);
        assert_eq!(r.cost_usd, Some(7.50));
    }

    #[test]
    fn apply_cost_no_op_when_already_set() {
        let _dir = tempdir();
        let mut r = rec("sess-6", "2026-06-02T10:00:00Z", Some("gpt-4o"), Some(0.99));
        let pricing = Pricing::default();
        Sessions::apply_cost(&mut r, &pricing);
        assert_eq!(r.cost_usd, Some(0.99));
    }

    #[test]
    fn session_id_is_unique_enough() {
        let a = SessionRecord::new_id();
        let b = SessionRecord::new_id();
        assert_ne!(a, b);
        assert!(a.starts_with("sess-"));
        // The /dev/urandom read should be non-zero on real systems;
        // we don't assert that (CI without /dev/urandom should still pass).
        let _ = a;
        let _ = b;
    }

    // -- helpers --

    fn tempdir() -> tempfile_like::Tmp {
        tempfile_like::Tmp::new()
    }

    /// Tiny inline TempDir wrapper. We avoid pulling in `tempfile` to
    /// keep deps minimal — this uses std::env::temp_dir() + a unique
    /// suffix. The directory is removed on Drop.
    mod tempfile_like {
        use std::path::PathBuf;
        pub struct Tmp(pub PathBuf);
        impl Tmp {
            pub fn new() -> Self {
                let mut p = std::env::temp_dir();
                let n: u64 = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64;
                p.push(format!("agent0waste-sessions-test-{}-{}", n, std::process::id()));
                std::fs::create_dir_all(&p).unwrap();
                Tmp(p)
            }
            pub fn path(&self) -> &std::path::Path { &self.0 }
        }
        impl Drop for Tmp {
            fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
        }
    }
}
