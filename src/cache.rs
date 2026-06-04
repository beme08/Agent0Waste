//! Persistent heuristic cache.
//!
//! Caches the JSON decision from `intercept check` per (command,
//! state.db-mtime, TTL). On a hit, `intercept check` can return
//! instantly without re-reading state.db or re-running heuristics.
//!
//! The cache lives at `~/.local/share/agent0waste/heuristic-cache.json`
//! (overridable via `XDG_DATA_HOME`). It's a single JSON file, written
//! in full on every `put()`. Entries are small (~200 bytes each) and
//! the file is rewritten once per process, so I/O cost is negligible
//! compared to the 150ms state.db read it avoids.
//!
//! Invalidation is two-key:
//! - **mtime**: state.db's mtime at the time of caching. If state.db
//!   has been written since (new session), the entry is stale.
//! - **TTL**: per-rule `cache_ttl_s`, default 30s, `0` disables
//!   caching entirely for that rule. Even with a steady state.db,
//!   the user might want fresh decisions after a config change.
//!
//! The cache does NOT try to be a correctness primitive. If it's
//! corrupted (truncated write, manual edit, version mismatch), it
//! falls back to an empty cache. Worst case: one extra state.db read.
//!
//! ## Corruption behavior (documented for v0.4.1)
//!
//! When `HeuristicCache::load_from` reads a file, three failure modes
//! are possible. All three result in an empty cache; the heuristic
//! check that follows runs normally and rebuilds the cache on its
//! next `put()` + `save()`.
//!
//! | Failure | Behavior | User-visible? |
//! |---------|----------|---------------|
//! | File missing | Empty cache, no warning | No |
//! | File unreadable (perm denied) | Empty cache, no warning | No |
//! | File unparseable (invalid JSON) | Empty cache, no warning | No |
//!
//! The decision to **not warn** is deliberate: warnings on every shim
//! invocation (the common case) would be noisy. The shim never blocks
//! on a corrupt cache — it just treats it as a miss. The rebuild
//! happens silently.
//!
//! This means a corrupt cache can never prevent command execution.
//! At worst, the user gets one slow check (the heuristic runs) and
//! then subsequent checks hit the rebuilt cache.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const CACHE_FILENAME: &str = "heuristic-cache.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    state_db_mtime_unix: i64,
    expires_at_unix: i64,
    decision_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct CacheFile {
    entries: HashMap<String, CacheEntry>,
}

#[derive(Debug)]
pub struct HeuristicCache {
    file: CacheFile,
    path: PathBuf,
}

impl HeuristicCache {
    /// Load from the default path. Missing file = empty cache.
    /// Malformed file = empty cache (with no warning; the next save
    /// overwrites it).
    pub fn load() -> Self {
        let path = cache_path().unwrap_or_else(|| PathBuf::from(CACHE_FILENAME));
        Self::load_from(path)
    }

    /// Load from a specific path. Used by tests and by future
    /// `--cache-path` flags.
    pub fn load_from(path: PathBuf) -> Self {
        let file = match std::fs::read_to_string(&path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => CacheFile::default(),
        };
        Self { file, path }
    }

    /// Look up a cached decision. Returns `Some(decision_json)` on hit.
    /// Hit requires all three: command matches, mtime matches, TTL
    /// not expired.
    pub fn get(&self, command: &str, state_db_mtime: SystemTime) -> Option<&str> {
        let entry = self.file.entries.get(command)?;
        let now = unix_now();
        if entry.expires_at_unix < now {
            return None;
        }
        if entry.state_db_mtime_unix != mtime_to_unix(state_db_mtime) {
            return None;
        }
        Some(&entry.decision_json)
    }

    /// Store a decision. `ttl == 0` is a no-op (rule opted out).
    pub fn put(
        &mut self,
        command: &str,
        state_db_mtime: SystemTime,
        ttl: Duration,
        decision_json: String,
    ) {
        if ttl.is_zero() {
            return;
        }
        let now = unix_now();
        let entry = CacheEntry {
            state_db_mtime_unix: mtime_to_unix(state_db_mtime),
            expires_at_unix: now.saturating_add(ttl.as_secs() as i64),
            decision_json,
        };
        self.file.entries.insert(command.to_string(), entry);
    }

    /// Write to disk. Creates parent dirs. Best-effort: errors are
    /// not propagated to the caller because the cache is a
    /// performance optimization, not a correctness primitive.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(&self.file)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(&self.path, json)
    }
}

/// Default cache file path. Honors XDG_DATA_HOME.
pub fn cache_path() -> Option<PathBuf> {
    let data_dir = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| dirs::home_dir().map(|h| h.join(".local/share")))?;
    Some(data_dir.join("agent0waste").join(CACHE_FILENAME))
}

fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn mtime_to_unix(t: SystemTime) -> i64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mtime(s: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(s)
    }

    fn empty_cache() -> HeuristicCache {
        HeuristicCache {
            file: CacheFile::default(),
            path: PathBuf::from("/tmp/agent0waste-cache-test-dummy"),
        }
    }

    #[test]
    fn get_returns_none_on_empty() {
        let cache = empty_cache();
        assert!(cache.get("hermes run", mtime(1000)).is_none());
    }

    #[test]
    fn put_then_get_returns_decision() {
        let mut cache = empty_cache();
        cache.put(
            "hermes run",
            mtime(1000),
            Duration::from_secs(30),
            r#"{"decision":"allow"}"#.into(),
        );
        assert_eq!(
            cache.get("hermes run", mtime(1000)),
            Some(r#"{"decision":"allow"}"#)
        );
    }

    #[test]
    fn get_returns_none_after_ttl_expires() {
        // Insert an entry with expires_at_unix = 0 (epoch). Wall clock
        // is well past 0, so the TTL check fails.
        let mut cache = empty_cache();
        cache.file.entries.insert(
            "hermes run".into(),
            CacheEntry {
                state_db_mtime_unix: 1000,
                expires_at_unix: 0,
                decision_json: r#"{"decision":"allow"}"#.into(),
            },
        );
        assert!(cache.get("hermes run", mtime(1000)).is_none());
    }

    #[test]
    fn get_returns_none_on_mtime_mismatch() {
        let mut cache = empty_cache();
        cache.put(
            "hermes run",
            mtime(1000),
            Duration::from_secs(30),
            r#"{"decision":"allow"}"#.into(),
        );
        // state.db has been rewritten since caching.
        assert!(cache.get("hermes run", mtime(1001)).is_none());
    }

    #[test]
    fn put_with_zero_ttl_does_not_store() {
        let mut cache = empty_cache();
        cache.put(
            "hermes run",
            mtime(1000),
            Duration::ZERO,
            r#"{"decision":"allow"}"#.into(),
        );
        assert!(cache.get("hermes run", mtime(1000)).is_none());
    }

    #[test]
    fn different_commands_dont_collide() {
        let mut cache = empty_cache();
        cache.put(
            "hermes run",
            mtime(1000),
            Duration::from_secs(30),
            r#"{"decision":"throttle"}"#.into(),
        );
        cache.put(
            "hermes chat",
            mtime(1000),
            Duration::from_secs(30),
            r#"{"decision":"allow"}"#.into(),
        );
        assert_eq!(
            cache.get("hermes run", mtime(1000)),
            Some(r#"{"decision":"throttle"}"#)
        );
        assert_eq!(
            cache.get("hermes chat", mtime(1000)),
            Some(r#"{"decision":"allow"}"#)
        );
    }

    #[test]
    fn save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!(
            "agent0waste-cache-test-{}-{}",
            std::process::id(),
            line!()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("heuristic-cache.json");
        let _ = std::fs::remove_file(&path);

        let mut cache = HeuristicCache {
            file: CacheFile::default(),
            path: path.clone(),
        };
        cache.put(
            "hermes run",
            mtime(1000),
            Duration::from_secs(30),
            r#"{"decision":"allow"}"#.into(),
        );
        cache.save().unwrap();

        let loaded = HeuristicCache::load_from(path.clone());
        assert_eq!(
            loaded.get("hermes run", mtime(1000)),
            Some(r#"{"decision":"allow"}"#)
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }

    #[test]
    fn load_from_missing_file_yields_empty_cache() {
        let path = std::env::temp_dir().join(format!(
            "agent0waste-cache-test-missing-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        let cache = HeuristicCache::load_from(path);
        assert!(cache.get("anything", mtime(1000)).is_none());
    }

    #[test]
    fn load_from_corrupt_json_yields_empty_cache() {
        // Simulate a truncated write, a manual edit gone wrong, or a
        // version mismatch. The cache must load as empty (not panic,
        // not silently return Allow for everything). After a put(),
        // the file is rebuilt correctly.
        let path = std::env::temp_dir().join(format!(
            "agent0waste-cache-test-corrupt-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, b"this is not json { [[ corrupted").unwrap();

        let mut cache = HeuristicCache::load_from(path.clone());
        assert!(cache.get("anything", mtime(1000)).is_none());

        // The cache can be put() to and saved() to without errors.
        // The next load_from should see a valid file.
        cache.put(
            "hermes run",
            mtime(1000),
            Duration::from_secs(30),
            r#"{"decision":"allow"}"#.into(),
        );
        cache.save().unwrap();

        let reloaded = HeuristicCache::load_from(path.clone());
        assert_eq!(
            reloaded.get("hermes run", mtime(1000)),
            Some(r#"{"decision":"allow"}"#)
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn load_from_partial_json_yields_empty_cache() {
        // Truncated mid-write: starts valid, ends mid-object.
        let path = std::env::temp_dir().join(format!(
            "agent0waste-cache-test-partial-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = std::fs::remove_file(&path);
        std::fs::write(&path, br#"{"entries":{"hermes run":{state_db_mtime_unix":1000,"#).unwrap();

        let cache = HeuristicCache::load_from(path.clone());
        assert!(cache.get("hermes run", mtime(1000)).is_none());

        let _ = std::fs::remove_file(&path);
    }
}
