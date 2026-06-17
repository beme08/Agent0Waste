//! Datasets for the loadgen.
//!
//! Two modes:
//! - `Synthetic` — fixed-token prompts. Token count is approximated by a
//!   simple word->token heuristic (1 token ≈ 0.75 words, rounded up).
//!   This is **not** a real tokenizer; it's good enough to produce prompts
//!   of the requested approximate length without pulling a model-specific
//!   tokenizer into the CLI.
//! - `ShareGpt` — loads a small bundled JSON fixture at
//!   `data/sharegpt-tiny.json` (≤200 prompts). The user can override the
//!   path with `--dataset-path`.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DatasetKind {
    Synthetic,
    ShareGpt,
}

impl DatasetKind {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "synthetic" => Some(DatasetKind::Synthetic),
            "sharegpt" => Some(DatasetKind::ShareGpt),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Prompt {
    pub text: String,
    /// Approximate prompt-token count, as reported back to the loadgen so
    /// the sample carries `prompt_tok`.
    pub approx_prompt_tokens: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareGptEntry {
    pub conversations: Vec<ShareGptTurn>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareGptTurn {
    pub from: String,
    pub value: String,
}

/// Generate a synthetic prompt of approximately `n_tokens` tokens using a
/// fixed word-list. The prompt starts with a marker so the synthetic shape
/// is recognizable in server logs.
pub fn synthetic_prompt(n_tokens: u32) -> Prompt {
    // Average English word ~ 0.75 tokens, so we need ~ n_tokens/0.75 words.
    // Use a small repeating block for reproducibility.
    const BLOCK: &str = "the quick brown fox jumps over the lazy dog while reading agent0waste telemetry from inference servers and counting tokens per request across all concurrency buckets";
    let n_words = ((n_tokens as f64) / 0.75).ceil() as usize;
    let mut words: Vec<&str> = Vec::with_capacity(n_words);
    while words.len() < n_words {
        words.extend(BLOCK.split_whitespace());
    }
    words.truncate(n_words);
    let body = words.join(" ");
    let text = format!("[synthetic approx_tokens={}] {}", n_tokens, body);
    Prompt {
        text,
        approx_prompt_tokens: n_tokens,
    }
}

/// Load ShareGPT entries from a JSON file. Returns an empty Vec if the file
/// is missing or malformed (the caller can fall back to synthetic).
pub fn load_sharegpt(path: &Path) -> Vec<ShareGptEntry> {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return Vec::new(),
    };
    serde_json::from_slice::<Vec<ShareGptEntry>>(&bytes).unwrap_or_default()
}

/// Extract a single user-turn prompt from a ShareGPT conversation.
/// Falls back to concatenating all turns if no `human` turn is found.
pub fn sharegpt_to_prompt(entry: &ShareGptEntry) -> Prompt {
    let user_turn = entry
        .conversations
        .iter()
        .find(|t| t.from == "human" || t.from == "user")
        .map(|t| t.value.clone());
    let text = user_turn.unwrap_or_else(|| {
        entry
            .conversations
            .iter()
            .map(|t| format!("{}: {}", t.from, t.value))
            .collect::<Vec<_>>()
            .join("\n")
    });
    let approx_prompt_tokens = approx_tokens(&text);
    Prompt {
        text,
        approx_prompt_tokens,
    }
}

/// Approximate a string's token count as ceil(word_count / 0.75). The same
/// heuristic is used everywhere in v0.6 so numbers are comparable.
pub fn approx_tokens(s: &str) -> u32 {
    let words = s.split_whitespace().count() as f64;
    ((words / 0.75).ceil() as u32).max(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_prompt_length_matches_request() {
        let p = synthetic_prompt(512);
        // The actual text is longer than 512 words because words/tokens ~ 0.75,
        // but `approx_prompt_tokens` is what the loadgen reports.
        assert_eq!(p.approx_prompt_tokens, 512);
        assert!(p.text.contains("[synthetic approx_tokens=512]"));
    }

    #[test]
    fn synthetic_prompt_starts_with_marker() {
        let p = synthetic_prompt(10);
        assert!(p.text.starts_with("[synthetic approx_tokens=10]"));
    }

    #[test]
    fn approx_tokens_is_monotonic() {
        let small = approx_tokens("hello world");
        let big = approx_tokens("hello world ".repeat(50).as_str());
        assert!(big > small);
    }

    #[test]
    fn approx_tokens_handles_empty() {
        assert_eq!(approx_tokens(""), 1);
        assert_eq!(approx_tokens("   "), 1);
    }

    #[test]
    fn sharegpt_picks_user_turn() {
        let entry = ShareGptEntry {
            conversations: vec![
                ShareGptTurn {
                    from: "system".into(),
                    value: "You are a helpful assistant.".into(),
                },
                ShareGptTurn {
                    from: "human".into(),
                    value: "What is 2+2?".into(),
                },
                ShareGptTurn {
                    from: "gpt".into(),
                    value: "4".into(),
                },
            ],
        };
        let p = sharegpt_to_prompt(&entry);
        assert_eq!(p.text, "What is 2+2?");
    }

    #[test]
    fn sharegpt_falls_back_to_all_turns() {
        let entry = ShareGptEntry {
            conversations: vec![
                ShareGptTurn {
                    from: "gpt".into(),
                    value: "hello".into(),
                },
            ],
        };
        let p = sharegpt_to_prompt(&entry);
        assert!(p.text.contains("gpt: hello"));
    }

    #[test]
    fn load_sharegpt_missing_file_returns_empty() {
        let p = std::path::Path::new("/tmp/agent0waste-nonexistent-sharegpt.json");
        let v = load_sharegpt(p);
        assert!(v.is_empty());
    }

    #[test]
    fn dataset_kind_parse() {
        assert_eq!(DatasetKind::parse("synthetic"), Some(DatasetKind::Synthetic));
        assert_eq!(DatasetKind::parse("sharegpt"), Some(DatasetKind::ShareGpt));
        assert_eq!(DatasetKind::parse("SHAREGPT"), Some(DatasetKind::ShareGpt));
        assert_eq!(DatasetKind::parse("nope"), None);
    }
}
