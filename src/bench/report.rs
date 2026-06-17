//! Report model + JSON / CSV writers.
//!
//! `BenchReport` is the on-disk artifact. JSON is the canonical form; CSV
//! is a flat per-sample export for spreadsheet / notebook use.
//!
//! The JSON shape matches the illustrative example in the v0.6 design doc:
//! target metadata, raw samples, per-concurrency rollups, metrics summary
//! (all fields nullable), and the waste score breakdown.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::bench::target::Sample;
use crate::bench::waste::{WasteBreakdown, WasteResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchReport {
    /// Target label: "vllm" | "sglang" | "baseline"
    pub target: String,
    /// Base URL of the server, as supplied by the user.
    pub base_url: String,
    /// Model name passed to the server.
    pub model: String,
    /// Concurrency sweep used.
    pub concurrency_sweep: Vec<u32>,
    /// Dataset kind used.
    pub dataset: String,
    /// Per-request samples.
    pub samples: Vec<Sample>,
    /// Per-concurrency rollup.
    pub rollup: ByConcurrency,
    /// Scraped metrics summary. All fields nullable.
    pub metrics_summary: MetricsSummary,
    /// 0..100 waste score. None if no axes were measurable.
    pub waste_score: Option<f64>,
    /// Per-axis waste values, with `null` for unavailable axes.
    pub waste_axes: WasteBreakdown,
    /// Names of axes whose source was missing.
    pub waste_axes_unavailable: Vec<String>,
    /// ISO-8601 UTC timestamp when the report was written.
    pub generated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ByConcurrency {
    pub by_concurrency: BTreeMap<String, ConcurrencyRollup>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConcurrencyRollup {
    pub p50_s: Option<f64>,
    pub p99_s: Option<f64>,
    pub ttft_p50_s: Option<f64>,
    pub tok_per_s: f64,
    pub completed: u32,
    pub errored: u32,
    /// Total output tokens generated across completed requests.
    pub total_output_tok: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricsSummary {
    pub kv_cache_usage_perc_avg: Option<f64>,
    pub gpu_utilization_avg: Option<f64>,
    pub cpu_cache_usage_perc_avg: Option<f64>,
    pub num_requests_swapped_avg: Option<f64>,
    pub request_success_total_delta: Option<f64>,
    pub request_generation_tokens_total_delta: Option<f64>,
}

/// Aggregate `samples` into per-concurrency rollups.
pub fn rollup(samples: &[Sample]) -> ByConcurrency {
    let mut by: BTreeMap<u32, Vec<&Sample>> = BTreeMap::new();
    for s in samples {
        by.entry(s.concurrency).or_default().push(s);
    }
    let mut out = ByConcurrency::default();
    for (conc, group) in by {
        out.by_concurrency.insert(conc.to_string(), rollup_one(&group));
    }
    out
}

fn rollup_one(group: &[&Sample]) -> ConcurrencyRollup {
    let mut latencies: Vec<f64> = group.iter().map(|s| s.total_s).collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&latencies, 0.50);
    let p99 = percentile(&latencies, 0.99);

    let mut ttft: Vec<f64> = group.iter().filter_map(|s| s.ttft_s).collect();
    ttft.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let ttft_p50 = percentile(&ttft, 0.50);

    let total_output: u32 = group.iter().filter_map(|s| s.output_tok).sum();
    let total_latency: f64 = latencies.iter().sum();
    let tok_per_s = if total_latency > 0.0 {
        (total_output as f64) / total_latency
    } else {
        0.0
    };

    let completed = group.iter().filter(|s| s.completed).count() as u32;
    let errored = group.len() as u32 - completed;

    ConcurrencyRollup {
        p50_s: p50,
        p99_s: p99,
        ttft_p50_s: ttft_p50,
        tok_per_s,
        completed,
        errored,
        total_output_tok: total_output,
    }
}

/// Linear-interpolated percentile on a pre-sorted ascending slice.
pub fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    if sorted.is_empty() {
        return None;
    }
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }
    let rank = p * (sorted.len() as f64 - 1.0);
    let lo = rank.floor() as usize;
    let hi = rank.ceil() as usize;
    if lo == hi {
        return Some(sorted[lo]);
    }
    let frac = rank - lo as f64;
    Some(sorted[lo] * (1.0 - frac) + sorted[hi] * frac)
}

impl BenchReport {
    /// Build a report from samples + scraped metrics + waste result.
    /// `metrics_deltas` is the per-run delta of any *_total counter
    /// (request_success_total, generation_tokens_total), if known.
    pub fn build(
        target: &str,
        base_url: &str,
        model: &str,
        sweep: &[u32],
        dataset: &str,
        samples: Vec<Sample>,
        metrics_summary: MetricsSummary,
        waste: &WasteResult,
    ) -> Self {
        let generated_at = chrono::Utc::now().to_rfc3339();
        let rollup = rollup(&samples);
        let waste_axes = waste.breakdown.clone();
        let waste_axes_unavailable: Vec<String> = waste.breakdown.unavailable().iter().map(|s| s.to_string()).collect();
        BenchReport {
            target: target.to_string(),
            base_url: base_url.to_string(),
            model: model.to_string(),
            concurrency_sweep: sweep.to_vec(),
            dataset: dataset.to_string(),
            samples,
            rollup,
            metrics_summary,
            waste_score: waste.waste_score,
            waste_axes,
            waste_axes_unavailable,
            generated_at,
        }
    }

    /// Serialize to pretty JSON and write to `path`.
    pub fn write_json(&self, path: &Path) -> std::io::Result<()> {
        let body = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
        fs::write(path, body)
    }

    /// Read back a report from a JSON file.
    pub fn read_json(path: &Path) -> std::io::Result<Self> {
        let body = fs::read_to_string(path)?;
        serde_json::from_str(&body)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
    }

    /// Write one CSV row per sample. Uses `csv` crate if the `bench`
    /// feature is enabled; otherwise emits a hand-rolled equivalent.
    pub fn write_csv(&self, path: &Path) -> std::io::Result<()> {
        let mut wtr = csv_writer(path)?;
        wtr.write_record([
            "ts",
            "concurrency",
            "ttft_s",
            "total_s",
            "prompt_tok",
            "output_tok",
            "completed",
            "error",
        ])?;
        for s in &self.samples {
            wtr.write_record([
                s.ts.as_str(),
                &s.concurrency.to_string(),
                &fmt_opt_f64(s.ttft_s),
                &fmt_f64(s.total_s),
                &fmt_opt_u32(s.prompt_tok),
                &fmt_opt_u32(s.output_tok),
                &s.completed.to_string(),
                s.error.as_deref().unwrap_or(""),
            ])?;
        }
        wtr.flush()
    }
}

#[cfg(feature = "bench")]
fn csv_writer(path: &Path) -> std::io::Result<csv::Writer<std::fs::File>> {
    csv::Writer::from_path(path)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
}

#[cfg(not(feature = "bench"))]
fn csv_writer(path: &Path) -> std::io::Result<CsvWriter<std::fs::File>> {
    Ok(CsvWriter::new(std::fs::File::create(path)?))
}

#[cfg(not(feature = "bench"))]
/// Minimal CSV writer fallback so the report module compiles in the default
/// (non-`bench`) build. The CLI never reaches this when `bench` is off; the
/// `bench` subcommand errors out at runtime. Keeps `cargo build` (no
/// features) green.
pub struct CsvWriter<W: Write> {
    w: W,
}

#[cfg(not(feature = "bench"))]
impl<W: Write> CsvWriter<W> {
    pub fn new(w: W) -> Self {
        Self { w }
    }
    pub fn write_record<I, T>(&mut self, fields: I) -> std::io::Result<()>
    where
        I: IntoIterator<Item = T>,
        T: AsRef<[u8]>,
    {
        let mut first = true;
        for f in fields {
            if !first {
                self.w.write_all(b",")?;
            }
            self.w.write_all(f.as_ref())?;
            first = false;
        }
        self.w.write_all(b"\n")
    }
    pub fn flush(mut self) -> std::io::Result<()> {
        self.w.flush()
    }
}

fn fmt_opt_f64(v: Option<f64>) -> String {
    v.map(|x| format!("{:.6}", x)).unwrap_or_default()
}
fn fmt_f64(v: f64) -> String {
    format!("{:.6}", v)
}
fn fmt_opt_u32(v: Option<u32>) -> String {
    v.map(|x| x.to_string()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(c: u32, total: f64, ttft: Option<f64>, output: Option<u32>, ok: bool) -> Sample {
        Sample {
            ts: "2026-06-17T00:00:00Z".into(),
            concurrency: c,
            ttft_s: ttft,
            total_s: total,
            prompt_tok: Some(50),
            output_tok: output,
            completed: ok,
            error: if ok { None } else { Some("http_500".into()) },
        }
    }

    #[test]
    fn percentile_basic() {
        let mut v = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        assert_eq!(percentile(&v, 0.5), Some(3.0));
        assert_eq!(percentile(&v, 0.0), Some(1.0));
        assert_eq!(percentile(&v, 1.0), Some(5.0));
        assert_eq!(percentile(&[], 0.5), None);
        assert_eq!(percentile(&[1.0], 0.5), Some(1.0));
    }

    #[test]
    fn rollup_groups_by_concurrency() {
        let samples = vec![
            sample(1, 1.0, Some(0.1), Some(50), true),
            sample(1, 2.0, Some(0.2), Some(50), true),
            sample(4, 0.5, Some(0.05), Some(50), true),
            sample(4, 0.5, Some(0.05), Some(50), false),
        ];
        let r = rollup(&samples);
        assert!(r.by_concurrency.contains_key("1"));
        assert!(r.by_concurrency.contains_key("4"));
        let r1 = &r.by_concurrency["1"];
        assert_eq!(r1.completed, 2);
        assert_eq!(r1.errored, 0);
        let r4 = &r.by_concurrency["4"];
        assert_eq!(r4.completed, 1);
        assert_eq!(r4.errored, 1);
    }

    #[test]
    fn rollup_computes_throughput() {
        let samples = vec![
            sample(1, 1.0, Some(0.1), Some(100), true), // 100 tok in 1s
            sample(1, 1.0, Some(0.1), Some(100), true), // 100 tok in 1s
        ];
        let r = rollup(&samples);
        let r1 = &r.by_concurrency["1"];
        // 200 tokens / 2.0s = 100 tok/s
        assert!((r1.tok_per_s - 100.0).abs() < 1e-6);
    }

    #[test]
    fn json_roundtrip_preserves_nullable_axes() {
        let report = BenchReport {
            target: "baseline".into(),
            base_url: "http://x".into(),
            model: "m".into(),
            concurrency_sweep: vec![1, 4],
            dataset: "synthetic".into(),
            samples: vec![sample(1, 1.0, Some(0.1), Some(50), true)],
            rollup: rollup(&[sample(1, 1.0, Some(0.1), Some(50), true)]),
            metrics_summary: MetricsSummary::default(),
            waste_score: Some(10.0),
            waste_axes: WasteBreakdown {
                kv_cache_pressure: None,
                gpu_underutilization: None,
                tail_latency: Some(0.05),
                ttft_jitter: Some(0.0),
                context_oversize: None,
            },
            waste_axes_unavailable: vec!["kv_cache_pressure".into(), "gpu_underutilization".into(), "context_oversize".into()],
            generated_at: "2026-06-17T00:00:00Z".into(),
        };
        let body = serde_json::to_string(&report).unwrap();
        let back: BenchReport = serde_json::from_str(&body).unwrap();
        assert_eq!(back.waste_score, Some(10.0));
        assert_eq!(back.waste_axes.kv_cache_pressure, None);
        assert_eq!(back.waste_axes.tail_latency, Some(0.05));
        assert_eq!(back.waste_axes_unavailable.len(), 3);
    }

    #[test]
    fn write_csv_emits_one_row_per_sample() {
        let samples = vec![
            sample(1, 1.0, Some(0.1), Some(50), true),
            sample(4, 0.5, None, None, false),
        ];
        let report = BenchReport {
            target: "vllm".into(),
            base_url: "http://x".into(),
            model: "m".into(),
            concurrency_sweep: vec![1, 4],
            dataset: "synthetic".into(),
            samples: samples.clone(),
            rollup: rollup(&samples),
            metrics_summary: MetricsSummary::default(),
            waste_score: None,
            waste_axes: WasteBreakdown::default(),
            waste_axes_unavailable: vec![
                "kv_cache_pressure".into(),
                "gpu_underutilization".into(),
                "tail_latency".into(),
                "ttft_jitter".into(),
                "context_oversize".into(),
            ],
            generated_at: "2026-06-17T00:00:00Z".into(),
        };
        let dir = std::env::temp_dir();
        let path = dir.join("agent0waste-test-report.csv");
        report.write_csv(&path).unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        // Header + 2 sample rows.
        assert_eq!(body.lines().count(), 3);
        assert!(body.starts_with("ts,concurrency,ttft_s,total_s,prompt_tok,output_tok,completed,error"));
        // None fields are written as empty strings.
        // Bool Display produces "true"/"false", not "0"/"1".
        assert!(body.contains(",4,,0.500000,50,,false,http_500"));
    }
}
