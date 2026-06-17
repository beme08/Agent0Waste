//! Per-backend trait and implementations.
//!
//! Each `Target` knows:
//! 1. A human-readable label used in reports.
//! 2. How to normalize a raw client-side `Sample` (most targets return it
//!    unchanged; vLLM and SGLang only need to attach a few backend-specific
//!    metric keys).
//! 3. Which metric series to look for in the scraped `/metrics` body.
//!
//! v0.6 intentionally has no `MlxLm` impl. The `Target` trait is shaped so an
//! Apple Silicon / MLX target can land in v0.7 as a single-file change.
//!
//! Adding a custom-kernel backend later is also a one-file change: implement
//! `Target` and override `metric_keys` to include kernel-specific counters.

use serde::{Deserialize, Serialize};

/// One per-request measurement. Always populated by the loadgen from the
/// HTTP response. Backend-specific fields are added via `metric_overrides`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    /// Wall-clock timestamp (RFC3339) when the request completed.
    pub ts: String,
    /// Concurrency bucket this sample belongs to.
    pub concurrency: u32,
    /// Time-to-first-token in seconds. None if the response was non-streaming
    /// or an error.
    pub ttft_s: Option<f64>,
    /// Total request latency in seconds.
    pub total_s: f64,
    /// Prompt (input) tokens for this request, if reported by the server.
    pub prompt_tok: Option<u32>,
    /// Completion (output) tokens for this request, if reported by the server.
    pub output_tok: Option<u32>,
    /// `true` if the request completed with HTTP 2xx and a parsable body.
    pub completed: bool,
    /// Error class if the request failed (e.g. "http_500", "timeout", "parse").
    pub error: Option<String>,
}

/// Backend-specific metric key the scraper should look up in `/metrics`.
/// Each variant maps to a documented series name. Missing series are
/// silently dropped to `null` in the report — no fabrication.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetricKey {
    KvCacheUsagePerc,
    CpuCacheUsagePerc,
    NumRequestsSwapped,
    GpuCacheUsagePerc,
    RequestSuccessTotal,
    RequestPromptTokensTotal,
    RequestGenerationTokensTotal,
    TimeToFirstTokenSeconds,
    GpuUtilization,
}

impl MetricKey {
    /// String label used in the JSON report (stable across versions).
    pub fn label(&self) -> &'static str {
        match self {
            MetricKey::KvCacheUsagePerc => "kv_cache_usage_perc",
            MetricKey::CpuCacheUsagePerc => "cpu_cache_usage_perc",
            MetricKey::NumRequestsSwapped => "num_requests_swapped",
            MetricKey::GpuCacheUsagePerc => "gpu_cache_usage_perc",
            MetricKey::RequestSuccessTotal => "request_success_total",
            MetricKey::RequestPromptTokensTotal => "request_prompt_tokens_total",
            MetricKey::RequestGenerationTokensTotal => "request_generation_tokens_total",
            MetricKey::TimeToFirstTokenSeconds => "time_to_first_token_seconds",
            MetricKey::GpuUtilization => "gpu_utilization",
        }
    }
}

/// Target trait. All v0.6 targets talk to OpenAI-compatible HTTP servers;
/// backend-specific differences are in which `/metrics` series are scraped
/// and how (if at all) the raw sample is decorated.
pub trait Target: Send + Sync {
    /// Short human label, e.g. "vllm", "sglang", "baseline".
    fn label(&self) -> &'static str;

    /// Metric series to attempt to scrape. Missing series are dropped.
    fn metric_keys(&self) -> &'static [MetricKey];

    /// Normalize a raw sample. vLLM and SGLang return the sample unchanged in
    /// v0.6; future custom-kernel backends could attach kernel-level counters
    /// here.
    fn normalize(&self, sample: Sample) -> Sample {
        sample
    }
}

// ---------------------------------------------------------------------------
// vLLM
// ---------------------------------------------------------------------------

pub struct VllmTarget;

impl Target for VllmTarget {
    fn label(&self) -> &'static str {
        "vllm"
    }

    fn metric_keys(&self) -> &'static [MetricKey] {
        &[
            MetricKey::KvCacheUsagePerc,
            MetricKey::CpuCacheUsagePerc,
            MetricKey::NumRequestsSwapped,
            MetricKey::GpuCacheUsagePerc,
            MetricKey::RequestSuccessTotal,
            MetricKey::RequestGenerationTokensTotal,
            MetricKey::TimeToFirstTokenSeconds,
            MetricKey::GpuUtilization,
        ]
    }
}

// ---------------------------------------------------------------------------
// SGLang
// ---------------------------------------------------------------------------

pub struct SglangTarget;

impl Target for SglangTarget {
    fn label(&self) -> &'static str {
        "sglang"
    }

    fn metric_keys(&self) -> &'static [MetricKey] {
        // SGLang reuses most of vLLM's metric names in v0.3+; we keep the
        // same set and rely on the scraper's nullable behavior for series
        // that SGLang doesn't expose on a given build.
        &[
            MetricKey::KvCacheUsagePerc,
            MetricKey::CpuCacheUsagePerc,
            MetricKey::NumRequestsSwapped,
            MetricKey::GpuCacheUsagePerc,
            MetricKey::RequestSuccessTotal,
            MetricKey::RequestGenerationTokensTotal,
            MetricKey::TimeToFirstTokenSeconds,
            MetricKey::GpuUtilization,
        ]
    }
}

// ---------------------------------------------------------------------------
// Baseline
// ---------------------------------------------------------------------------

/// Generic OpenAI-compatible endpoint. No backend metrics scraping — we only
/// have client-side measurements (TTFT, p50/p99, tok/s, errors).
pub struct BaselineTarget;

impl Target for BaselineTarget {
    fn label(&self) -> &'static str {
        "baseline"
    }

    fn metric_keys(&self) -> &'static [MetricKey] {
        // Intentionally empty: baseline is for servers we can't introspect
        // (llama.cpp server, TGI, custom proxies). All waste axes that
        // depend on scraped metrics will be `null` in the report.
        &[]
    }
}

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// Construct a `Target` from the CLI string. Returns `None` for unknown
/// targets so the CLI can produce a helpful error.
pub fn from_str(name: &str) -> Option<Box<dyn Target>> {
    match name.to_ascii_lowercase().as_str() {
        "vllm" => Some(Box::new(VllmTarget)),
        "sglang" => Some(Box::new(SglangTarget)),
        "baseline" => Some(Box::new(BaselineTarget)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_stability() {
        assert_eq!(VllmTarget.label(), "vllm");
        assert_eq!(SglangTarget.label(), "sglang");
        assert_eq!(BaselineTarget.label(), "baseline");
    }

    #[test]
    fn dispatch_known_targets() {
        assert!(from_str("vllm").is_some());
        assert!(from_str("sglang").is_some());
        assert!(from_str("baseline").is_some());
        assert!(from_str("VLLM").is_some(), "case-insensitive");
        assert!(from_str("unknown").is_none());
        assert!(from_str("mlx-lm").is_none(), "mlx-lm deferred to v0.7");
    }

    #[test]
    fn baseline_has_no_metric_keys() {
        assert!(BaselineTarget.metric_keys().is_empty());
    }

    #[test]
    fn vllm_and_sglang_declare_same_keys_v0_6() {
        // v0.6 treats them symmetrically; if a future release diverges
        // the trait is already shaped for that.
        assert_eq!(VllmTarget.metric_keys().len(), SglangTarget.metric_keys().len());
    }

    #[test]
    fn metric_key_labels_are_stable_strings() {
        assert_eq!(MetricKey::KvCacheUsagePerc.label(), "kv_cache_usage_perc");
        assert_eq!(MetricKey::GpuUtilization.label(), "gpu_utilization");
    }

    #[test]
    fn normalize_is_passthrough_in_v0_6() {
        let raw = Sample {
            ts: "2026-06-17T00:00:00Z".to_string(),
            concurrency: 1,
            ttft_s: Some(0.05),
            total_s: 1.0,
            prompt_tok: Some(10),
            output_tok: Some(20),
            completed: true,
            error: None,
        };
        let n = VllmTarget.normalize(raw.clone());
        assert_eq!(n.total_s, raw.total_s);
    }
}
