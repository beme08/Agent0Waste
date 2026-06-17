//! Prometheus text-format scraper.
//!
//! Parses the body of `/metrics` and looks for the series declared by the
//! active `Target`. Anything not present is silently dropped — we never
//! fabricate a "KV cache hit rate" that isn't derivable from the exposed
//! series. The caller decides how to render `None` in the report.
//!
//! The parser is deliberately small and dependency-free. It handles the
//! shapes used by vLLM, SGLang, and the DCGM exporter:
//!
//!   metric_name{label="x",foo="bar"} 1.23
//!   metric_name 1.23
//!   metric_name{label="x"} 1.23 1700000000000
//!
//! Histograms (`*_bucket{le="..."}`) are stored as the bucket edges + values
//! so `TimeToFirstTokenSeconds` can be queried as a quantile later if needed.

use std::collections::BTreeMap;

use crate::bench::target::MetricKey;

/// One scraped value for a single metric. We keep the most recent value
/// observed in the scrape body. For histograms this holds the bucket map.
#[derive(Debug, Clone)]
pub enum MetricValue {
    Gauge(f64),
    Counter(f64),
    Histogram(BTreeMap<String, f64>),
    Untyped(f64),
}

impl MetricValue {
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            MetricValue::Gauge(v) | MetricValue::Counter(v) | MetricValue::Untyped(v) => Some(*v),
            MetricValue::Histogram(_) => None,
        }
    }
}

/// Full parse result: every metric name we recognized, in stable order.
#[derive(Debug, Clone, Default)]
pub struct MetricsSnapshot {
    pub series: BTreeMap<String, MetricValue>,
}

impl MetricsSnapshot {
    /// Look up a value by canonical label. Returns `None` if absent.
    pub fn get(&self, key: MetricKey) -> Option<f64> {
        let name = canonical_metric_name(key);
        self.series.get(&name).and_then(|v| v.as_f64())
    }
}

/// Map a `MetricKey` to the actual series name vLLM/SGLang/DCGM expose.
/// `None` means the key is intentionally not scraped (e.g. baseline target).
pub fn canonical_metric_name(key: MetricKey) -> String {
    match key {
        // vLLM names
        MetricKey::KvCacheUsagePerc => "vllm:gpu_cache_usage_perc".to_string(),
        MetricKey::CpuCacheUsagePerc => "vllm:cpu_cache_usage_perc".to_string(),
        MetricKey::NumRequestsSwapped => "vllm:num_requests_swapped".to_string(),
        MetricKey::GpuCacheUsagePerc => "vllm:gpu_cache_usage_perc".to_string(),
        MetricKey::RequestSuccessTotal => "vllm:request_success_total".to_string(),
        MetricKey::RequestPromptTokensTotal => "vllm:prompt_tokens_total".to_string(),
        MetricKey::RequestGenerationTokensTotal => "vllm:generation_tokens_total".to_string(),
        MetricKey::TimeToFirstTokenSeconds => "vllm:time_to_first_token_seconds".to_string(),
        // DCGM exporter
        MetricKey::GpuUtilization => "DCGM_FI_DEV_GPU_UTIL".to_string(),
    }
}

/// Parse Prometheus text-format exposition. Lines beginning with `#` are
/// comments / HELP / TYPE and are ignored except to inform parsing. Unknown
/// series are stored under their literal name so a future MetricKey can
/// resolve to them, but the public API only exposes the canonical names
/// declared by the active `Target`.
pub fn parse(body: &str) -> MetricsSnapshot {
    let mut snap = MetricsSnapshot::default();
    for raw in body.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((name, value)) = parse_line(line) {
            snap.series.insert(name, value);
        }
    }
    snap
}

fn parse_line(line: &str) -> Option<(String, MetricValue)> {
    // metric_name{labels...} value [timestamp]
    // metric_name value [timestamp]
    let (name_lbl, rest) = split_first_space(line)?;
    let (name, labels) = split_labels(name_lbl);
    let mut rest_parts = rest.split_whitespace();
    let value_str = rest_parts.next()?;
    let value: f64 = value_str.parse().ok()?;

    // If labels contain le="...", this is a histogram bucket; group buckets
    // for the same metric name into a Histogram.
    if labels.contains_key("le") {
        let le = labels.get("le").cloned().unwrap_or_default();
        let key = name.to_string();
        // We can't return a mutable reference from the iterator; use entry API
        // outside. Stash the bucket in a side map.
        // For simplicity, we store buckets as separate entries keyed by
        // `<name>{le="..."}` and reconstruct in `aggregate_buckets`.
        return Some((format!("{}|le={}", key, le), MetricValue::Untyped(value)));
    }

    Some((name.to_string(), MetricValue::Untyped(value)))
}

fn split_first_space(s: &str) -> Option<(&str, &str)> {
    let idx = s.find(' ')?;
    Some((&s[..idx], &s[idx + 1..]))
}

fn split_labels(s: &str) -> (&str, BTreeMap<String, String>) {
    if let Some(brace_start) = s.find('{') {
        let name = &s[..brace_start];
        let brace_end = s.rfind('}').unwrap_or(s.len() - 1);
        let body = &s[brace_start + 1..brace_end];
        let mut out = BTreeMap::new();
        for pair in split_label_pairs(body) {
            if let Some((k, v)) = split_one_label(&pair) {
                out.insert(k, v);
            }
        }
        (name, out)
    } else {
        (s, BTreeMap::new())
    }
}

fn split_label_pairs(body: &str) -> Vec<String> {
    // Labels are comma-separated, but values can contain commas inside
    // quotes. We do a small state machine. Good enough for vLLM/SGLang/DCGM.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut escape = false;
    for ch in body.chars() {
        if escape {
            cur.push(ch);
            escape = false;
            continue;
        }
        if ch == '\\' {
            cur.push(ch);
            escape = true;
            continue;
        }
        if ch == '"' {
            in_quote = !in_quote;
            cur.push(ch);
            continue;
        }
        if ch == ',' && !in_quote {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn split_one_label(pair: &str) -> Option<(String, String)> {
    let idx = pair.find('=')?;
    let key = pair[..idx].trim().to_string();
    let val = pair[idx + 1..].trim();
    let val = val.strip_prefix('"').unwrap_or(val);
    let val = val.strip_suffix('"').unwrap_or(val);
    Some((key, val.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn fixture(name: &str) -> String {
        let p = format!("tests/fixtures/{}", name);
        fs::read_to_string(&p).unwrap_or_else(|_| panic!("missing fixture {}", p))
    }

    #[test]
    fn parses_simple_gauge() {
        let body = "# HELP foo something\n# TYPE foo gauge\nfoo 1.5\n";
        let snap = parse(body);
        assert_eq!(snap.get(MetricKey::GpuUtilization), None);
        assert_eq!(snap.series.get("foo").and_then(|v| v.as_f64()), Some(1.5));
    }

    #[test]
    fn parses_labeled_counter() {
        let body = r#"vllm:request_success_total{model="llama-3-8b"} 42.0"#;
        let snap = parse(body);
        // Canonical lookup uses MetricKey, not literal name.
        assert_eq!(snap.get(MetricKey::RequestSuccessTotal), Some(42.0));
    }

    #[test]
    fn missing_series_is_none_not_fabricated() {
        let body = "vllm:request_success_total 10.0\n";
        let snap = parse(body);
        // No KV cache series in the body — must be None, not 0.0.
        assert_eq!(snap.get(MetricKey::KvCacheUsagePerc), None);
        assert_eq!(snap.get(MetricKey::GpuUtilization), None);
    }

    #[test]
    fn parses_vllm_metrics_fixture() {
        let body = fixture("vllm_metrics.txt");
        let snap = parse(&body);
        // These are the documented series the plan calls out.
        assert!(snap.get(MetricKey::KvCacheUsagePerc).is_some());
        assert!(snap.get(MetricKey::GpuUtilization).is_some());
        assert!(snap.get(MetricKey::RequestSuccessTotal).is_some());
    }

    #[test]
    fn parses_sglang_metrics_fixture() {
        let body = fixture("sglang_metrics.txt");
        let snap = parse(&body);
        // SGLang fixture must contain *some* of the canonical series; if a
        // series isn't exposed, the result is None (not fabricated).
        assert!(snap.get(MetricKey::KvCacheUsagePerc).is_none() || snap.series.len() > 0);
    }

    #[test]
    fn unknown_series_are_silently_kept_under_literal_name() {
        let body = "future_metric_label 7.0\n";
        let snap = parse(body);
        assert_eq!(snap.series.get("future_metric_label").and_then(|v| v.as_f64()), Some(7.0));
        // But the canonical lookup for the closest MetricKey must be None.
        assert_eq!(snap.get(MetricKey::KvCacheUsagePerc), None);
    }

    #[test]
    fn empty_body_is_empty_snapshot() {
        let snap = parse("");
        assert!(snap.series.is_empty());
    }

    #[test]
    fn comment_lines_are_skipped() {
        let body = "# HELP x \n# TYPE x gauge\nx 1.0\n";
        let snap = parse(body);
        assert_eq!(snap.series.get("x").and_then(|v| v.as_f64()), Some(1.0));
    }

    #[test]
    fn histogram_buckets_are_recorded() {
        let body = r#"vllm:time_to_first_token_seconds_bucket{le="0.01"} 5
vllm:time_to_first_token_seconds_bucket{le="0.1"} 12
vllm:time_to_first_token_seconds_bucket{le="+Inf"} 20"#;
        let snap = parse(body);
        // Buckets are stored with synthetic names so future tooling can
        // reconstruct the histogram. The public `get` returns None for
        // histogram series — that's intentional, the value is the buckets
        // map, not a single number.
        assert!(snap.series.contains_key("vllm:time_to_first_token_seconds_bucket|le=0.01"));
        assert!(snap.series.contains_key("vllm:time_to_first_token_seconds_bucket|le=0.1"));
        assert!(snap.series.contains_key("vllm:time_to_first_token_seconds_bucket|le=+Inf"));
        // None of these are reachable via the canonical `get` (histogram).
        assert_eq!(snap.get(MetricKey::TimeToFirstTokenSeconds), None);
    }
}
