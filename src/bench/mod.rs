//! Layer 6: Systems Profiler & Benchmark.
//!
//! v0.6 ships:
//! - Three first-class targets: `vllm`, `sglang`, `baseline`.
//! - Swept-concurrency chat-completion load.
//! - Optional Prometheus `/metrics` scraping.
//! - Explainable 0–100 `waste_score` (lower is better) with five axes
//!   and proration over unavailable axes.
//! - JSON + CSV output.
//!
//! v0.6 does **not** ship:
//! - Custom kernels (the `Target` trait is the seam for a future slot).
//! - MLX / oMLX Apple Silicon target (deferred to v0.7).
//! - Fabricated metrics. If a backend doesn't expose a series, the report
//!   shows `null` and the score is prorated.

pub mod dataset;
pub mod loadgen;
pub mod metrics;
pub mod report;
pub mod target;
pub mod waste;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::{Args, Subcommand};

use self::dataset::{synthetic_prompt, DatasetKind, Prompt};
use self::loadgen::{run_sweep, LoadgenConfig};
use self::metrics::MetricsSnapshot;
use self::report::{BenchReport, MetricsSummary};
use self::target::{from_str as target_from_str, Sample};
use self::waste::{compute, WasteInputs, WasteResult};

/// `agent0waste bench …` subcommand tree.
#[derive(Subcommand, Debug)]
pub enum BenchCmd {
    /// Run a benchmark against an inference server.
    Run(BenchRunArgs),
    /// Compare two stored reports.
    Compare {
        /// Path to the first report.
        a: PathBuf,
        /// Path to the second report.
        b: PathBuf,
    },
    /// Render a stored report as a markdown table.
    Report {
        /// Path to the report.
        path: PathBuf,
    },
}

#[derive(Args, Debug, Clone)]
pub struct BenchRunArgs {
    /// Target: vllm | sglang | baseline
    pub target: String,

    /// Server base URL (e.g. http://localhost:8000)
    #[arg(long)]
    pub base_url: String,

    /// Model name passed to the server.
    #[arg(long, default_value = "default")]
    pub model: String,

    /// Comma-separated concurrency levels to sweep.
    #[arg(long, default_value = "1,4,16,32")]
    pub concurrency: String,

    /// Number of requests per concurrency level.
    #[arg(long, default_value_t = 100)]
    pub num_requests: u32,

    /// Approximate input tokens for synthetic prompts.
    #[arg(long, default_value_t = 512)]
    pub input_tokens: u32,

    /// Max output tokens per request.
    #[arg(long, default_value_t = 256)]
    pub output_tokens: u32,

    /// Dataset: synthetic | sharegpt
    #[arg(long, default_value = "synthetic")]
    pub dataset: String,

    /// Optional override path to a ShareGPT JSON file.
    #[arg(long)]
    pub dataset_path: Option<PathBuf>,

    /// Stream responses (sets stream=true on the request).
    #[arg(long, default_value_t = false)]
    pub stream: bool,

    /// Scrape Prometheus /metrics each second.
    #[arg(long, default_value_t = true)]
    pub scrape_metrics: bool,

    /// Per-request timeout.
    #[arg(long, default_value = "30s")]
    pub request_timeout: String,

    /// Warmup duration before sampling each concurrency level.
    #[arg(long, default_value = "10s")]
    pub warmup: String,

    /// Write the JSON report here.
    #[arg(long)]
    pub output: Option<PathBuf>,

    /// Write a per-sample CSV here.
    #[arg(long)]
    pub csv: Option<PathBuf>,

    /// Optional model context window. Used by the context_oversize axis.
    /// If omitted, that axis is unavailable.
    #[arg(long)]
    pub context_window: Option<u32>,

    /// Run duration cap. If the sweep would exceed this, it stops cleanly.
    #[arg(long)]
    pub duration: Option<String>,
}

fn parse_csv_u32(s: &str) -> Result<Vec<u32>, String> {
    s.split(',')
        .map(|x| x.trim().parse::<u32>().map_err(|e| e.to_string()))
        .collect()
}

impl BenchRunArgs {
    /// Build the prompt list for this run.
    /// Parse the `--concurrency` string into a Vec<u32>.
    pub fn concurrency_vec(&self) -> Vec<u32> {
        parse_csv_u32(&self.concurrency).unwrap_or_else(|_| vec![1, 4, 16, 32])
    }

    pub fn build_prompts(&self) -> Vec<Prompt> {
        match DatasetKind::parse(&self.dataset) {
            Some(DatasetKind::ShareGpt) => {
                let path = self
                    .dataset_path
                    .clone()
                    .unwrap_or_else(|| PathBuf::from("data/sharegpt-tiny.json"));
                let entries = dataset::load_sharegpt(&path);
                if entries.is_empty() {
                    eprintln!(
                        "agent0waste: sharegpt dataset at {} is empty or missing; falling back to synthetic",
                        path.display()
                    );
                    vec![synthetic_prompt(self.input_tokens); self.num_requests as usize]
                } else {
                    entries.iter().map(dataset::sharegpt_to_prompt).collect()
                }
            }
            _ => vec![synthetic_prompt(self.input_tokens); self.num_requests as usize],
        }
    }

    /// Parse the duration strings with humantime.
    pub fn request_timeout_d(&self) -> Duration {
        parse_humantime(&self.request_timeout, Duration::from_secs(30))
    }
    pub fn warmup_d(&self) -> Duration {
        parse_humantime(&self.warmup, Duration::from_secs(10))
    }
}

#[cfg(feature = "bench")]
fn parse_humantime(s: &str, fallback: Duration) -> Duration {
    humantime::parse_duration(s).unwrap_or(fallback)
}

#[cfg(not(feature = "bench"))]
fn parse_humantime(_s: &str, fallback: Duration) -> Duration {
    fallback
}

/// Entry point invoked by `main` for the `bench` subcommand.
pub fn dispatch(cmd: &BenchCmd) -> Result<(), String> {
    match cmd {
        BenchCmd::Run(args) => run_bench(args.clone()),
        BenchCmd::Compare { a, b } => compare_reports(&a, &b),
        BenchCmd::Report { path } => render_report(&path),
    }
}

#[cfg(feature = "bench")]
fn run_bench(args: BenchRunArgs) -> Result<(), String> {
    use std::sync::Mutex;

    let target = target_from_str(&args.target)
        .ok_or_else(|| format!("unknown target '{}'; expected vllm, sglang, or baseline", args.target))?;

    let prompts = args.build_prompts();

    let live_buffer: Arc<Mutex<Vec<Sample>>> = Arc::new(Mutex::new(Vec::new()));
    let live_buffer_cb = Arc::clone(&live_buffer);
    let on_sample: Arc<dyn Fn(Sample) + Send + Sync> = Arc::new(move |s: Sample| {
        live_buffer_cb.lock().unwrap().push(s);
    });

    // Build a tokio runtime. The reqwest client, spawned tasks, and
    // any `.await` must all live inside it.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .map_err(|e| format!("tokio runtime: {e}"))?;

    let cfg = LoadgenConfig {
        base_url: args.base_url.clone(),
        model: args.model.clone(),
        concurrency_sweep: args.concurrency_vec(),
        num_requests: args.num_requests,
        max_output_tokens: args.output_tokens,
        prompts,
        stream: args.stream,
        request_timeout: args.request_timeout_d(),
        warmup: args.warmup_d(),
        on_sample: Some(on_sample),
    };

    // Drive both the loadgen and the metrics scraper concurrently inside
    // a single block_on. The scraper polls /metrics every second and stops
    // when the loadgen finishes.
    let (samples, metrics_snapshots): (Vec<Sample>, Vec<MetricsSnapshot>) = {
        let scrape_metrics = args.scrape_metrics;
        let base_url = args.base_url.clone();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_inner = Arc::clone(&stop);
        let join = async move {
            let client = loadgen::build_client();

            let loadgen_handle = {
                let client = client.clone();
                tokio::spawn(async move { run_sweep(client, cfg).await })
            };

            let scrape_handle = if scrape_metrics {
                let client = client.clone();
                let stop_inner = Arc::clone(&stop_inner);
                Some(tokio::spawn(async move { scrape_loop(&client, &base_url, stop_inner).await }))
            } else {
                None
            };

            let samples: Vec<Sample> = match loadgen_handle.await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return Err::<_, String>(format!("loadgen: {e}")),
                Err(e) => return Err(format!("loadgen join: {e}")),
            };

            // Signal the scraper to stop now that the loadgen is done.
            stop.store(true, std::sync::atomic::Ordering::Relaxed);

            let snaps: Vec<MetricsSnapshot> = if let Some(h) = scrape_handle {
                h.await.map_err(|e| format!("scraper join: {e}"))?
            } else {
                Vec::new()
            };

            Ok::<_, String>((samples, snaps))
        };
        rt.block_on(join)?
    };

    drop(rt);

    // Build the inputs for the waste score.
    let summary = summarize_metrics(&metrics_snapshots);
    let inputs = build_waste_inputs(&samples, &summary, args.context_window);
    let waste = compute(&inputs);

    let report = BenchReport::build(
        target.label(),
        &args.base_url,
        &args.model,
        &args.concurrency_vec(),
        &args.dataset,
        samples,
        summary,
        &waste,
    );

    print_summary(&report);

    if let Some(out) = &args.output {
        report
            .write_json(out)
            .map_err(|e| format!("write {}: {e}", out.display()))?;
        eprintln!("agent0waste: wrote JSON report to {}", out.display());
    }
    if let Some(out) = &args.csv {
        report
            .write_csv(out)
            .map_err(|e| format!("write {}: {e}", out.display()))?;
        eprintln!("agent0waste: wrote CSV report to {}", out.display());
    }

    // Drop the live buffer reference we held for the callback.
    let _ = live_buffer;

    Ok(())
}

#[cfg(not(feature = "bench"))]
fn run_bench(_args: BenchRunArgs) -> Result<(), String> {
    Err(
        "the `bench` subcommand requires the `bench` cargo feature; rebuild with \
         `cargo build --release --features bench`"
            .to_string(),
    )
}

#[cfg(feature = "bench")]
async fn scrape_loop(
    client: &reqwest::Client,
    base_url: &str,
    stop: Arc<std::sync::atomic::AtomicBool>,
) -> Vec<MetricsSnapshot> {
    let url = format!("{}/metrics", base_url.trim_end_matches('/'));
    let mut out = Vec::new();
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    // Poll /metrics every second until `stop` is set (loadgen finished) or
    // the hard cap of 10 minutes is reached as a safety net.
    for _ in 0..600usize {
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        tick.tick().await;
        match client.get(&url).timeout(Duration::from_secs(2)).send().await {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.text().await {
                    out.push(metrics::parse(&body));
                }
            }
            Ok(_) => {} // server returned non-2xx; skip silently
            Err(_) => {} // server down or scrape error; skip silently
        }
    }
    out
}

fn summarize_metrics(snaps: &[MetricsSnapshot]) -> MetricsSummary {
    if snaps.is_empty() {
        return MetricsSummary::default();
    }
    let mut kv = Vec::new();
    let mut gpu = Vec::new();
    let mut cpu = Vec::new();
    let mut swapped = Vec::new();
    for s in snaps {
        if let Some(v) = s.get(target::MetricKey::KvCacheUsagePerc) {
            kv.push(v);
        }
        if let Some(v) = s.get(target::MetricKey::GpuUtilization) {
            gpu.push(v);
        }
        if let Some(v) = s.get(target::MetricKey::CpuCacheUsagePerc) {
            cpu.push(v);
        }
        if let Some(v) = s.get(target::MetricKey::NumRequestsSwapped) {
            swapped.push(v);
        }
    }
    MetricsSummary {
        kv_cache_usage_perc_avg: avg_opt(&kv),
        gpu_utilization_avg: avg_opt(&gpu),
        cpu_cache_usage_perc_avg: avg_opt(&cpu),
        num_requests_swapped_avg: avg_opt(&swapped),
        request_success_total_delta: None, // delta not computed in v0.6
        request_generation_tokens_total_delta: None,
    }
}

fn avg_opt(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None
    }
    let s: f64 = xs.iter().sum();
    Some(s / xs.len() as f64)
}

fn build_waste_inputs(
    samples: &[Sample],
    summary: &MetricsSummary,
    context_window: Option<u32>,
) -> WasteInputs {
    // Roll up latencies and TTFTs across the whole run.
    let mut latencies: Vec<f64> = samples.iter().map(|s| s.total_s).collect();
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = report::percentile(&latencies, 0.50);
    let p99 = report::percentile(&latencies, 0.99);

    let ttft: Vec<f64> = samples.iter().filter_map(|s| s.ttft_s).collect();

    let prompts: Vec<u32> = samples
        .iter()
        .filter_map(|s| s.prompt_tok)
        .collect();

    WasteInputs {
        kv_cache_usage_perc_avg: summary.kv_cache_usage_perc_avg,
        gpu_utilization_avg: summary.gpu_utilization_avg,
        latency_p50_s: p50,
        latency_p99_s: p99,
        ttft_samples_s: ttft,
        prompt_tokens: prompts,
        context_window,
    }
}

fn print_summary(report: &BenchReport) {
    println!("\nAgent0Waste — Layer 6 Bench Summary");
    println!("  target       : {}", report.target);
    println!("  base_url     : {}", report.base_url);
    println!("  model        : {}", report.model);
    println!("  concurrency  : {:?}", report.concurrency_sweep);
    println!("  samples      : {}", report.samples.len());
    println!();
    println!("  Per-concurrency rollup:");
    for (k, v) in &report.rollup.by_concurrency {
        println!(
            "    c={:>3}  p50={:?}  p99={:?}  ttft_p50={:?}  tok/s={:.1}  ok={}  err={}",
            k, v.p50_s, v.p99_s, v.ttft_p50_s, v.tok_per_s, v.completed, v.errored
        );
    }
    println!();
    println!("  Scraped metrics (averages):");
    println!(
        "    kv_cache_usage_perc  = {:?}",
        report.metrics_summary.kv_cache_usage_perc_avg
    );
    println!(
        "    gpu_utilization      = {:?}",
        report.metrics_summary.gpu_utilization_avg
    );
    println!(
        "    cpu_cache_usage_perc = {:?}",
        report.metrics_summary.cpu_cache_usage_perc_avg
    );
    println!(
        "    num_requests_swapped = {:?}",
        report.metrics_summary.num_requests_swapped_avg
    );
    println!();
    match report.waste_score {
        Some(s) => {
            println!("  waste_score   : {:.1} / 100  (lower is better)", s);
            println!("  efficiency    : {:.1} / 100", 100.0 - s);
            println!("  waste_axes:");
            println!("    kv_cache_pressure   = {:?}", report.waste_axes.kv_cache_pressure);
            println!("    gpu_underutilization= {:?}", report.waste_axes.gpu_underutilization);
            println!("    tail_latency        = {:?}", report.waste_axes.tail_latency);
            println!("    ttft_jitter         = {:?}", report.waste_axes.ttft_jitter);
            println!("    context_oversize    = {:?}", report.waste_axes.context_oversize);
            if !report.waste_axes_unavailable.is_empty() {
                println!("  waste_axes_unavailable: {:?}", report.waste_axes_unavailable);
            }
        }
        None => {
            println!("  waste_score   : (no axes measurable)");
        }
    }
    println!();
}

fn compare_reports(a: &PathBuf, b: &PathBuf) -> Result<(), String> {
    let ra = BenchReport::read_json(a).map_err(|e| format!("read {}: {e}", a.display()))?;
    let rb = BenchReport::read_json(b).map_err(|e| format!("read {}: {e}", b.display()))?;
    println!("Agent0Waste — Bench Report Compare");
    println!("  A: {} (waste_score={:?})", a.display(), ra.waste_score);
    println!("  B: {} (waste_score={:?})", b.display(), rb.waste_score);
    if let (Some(sa), Some(sb)) = (ra.waste_score, rb.waste_score) {
        let delta = sb - sa;
        let direction = if delta < 0.0 { "better" } else if delta > 0.0 { "worse" } else { "same" };
        println!("  delta: {:.2} ({})", delta, direction);
    }
    Ok(())
}

fn render_report(path: &PathBuf) -> Result<(), String> {
    let r = BenchReport::read_json(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    println!("# Agent0Waste Bench Report — {}", r.target);
    println!();
    println!("- model: `{}`", r.model);
    println!("- base_url: `{}`", r.base_url);
    println!("- concurrency_sweep: `{:?}`", r.concurrency_sweep);
    println!("- generated_at: `{}`", r.generated_at);
    println!();
    println!("## Per-concurrency rollup");
    println!();
    println!("| c | p50 (s) | p99 (s) | ttft_p50 (s) | tok/s | ok | err |");
    println!("|---|---------|---------|--------------|-------|----|-----|");
    for (k, v) in &r.rollup.by_concurrency {
        println!(
            "| {} | {:?} | {:?} | {:?} | {:.1} | {} | {} |",
            k, v.p50_s, v.p99_s, v.ttft_p50_s, v.tok_per_s, v.completed, v.errored
        );
    }
    println!();
    println!("## Waste");
    println!();
    match r.waste_score {
        Some(s) => println!("- waste_score: **{:.1} / 100** (lower is better)", s),
        None => println!("- waste_score: (no axes measurable)"),
    }
    println!();
    println!("| axis | value |");
    println!("|------|-------|");
    println!("| kv_cache_pressure | {:?} |", r.waste_axes.kv_cache_pressure);
    println!("| gpu_underutilization | {:?} |", r.waste_axes.gpu_underutilization);
    println!("| tail_latency | {:?} |", r.waste_axes.tail_latency);
    println!("| ttft_jitter | {:?} |", r.waste_axes.ttft_jitter);
    println!("| context_oversize | {:?} |", r.waste_axes.context_oversize);
    if !r.waste_axes_unavailable.is_empty() {
        println!();
        println!("Unavailable axes: `{:?}`", r.waste_axes_unavailable);
    }
    Ok(())
}

// Suppress unused-import warnings for the no-bench-feature build.
#[cfg(not(feature = "bench"))]
#[allow(dead_code)]
fn _unused_imports_used() {
    let _ = (DatasetKind::Synthetic, compute as fn(&WasteInputs<'_>) -> WasteResult);
    let _ = target_from_str as fn(&str) -> Option<Box<dyn target::Target>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bench::target::Sample;

    #[test]
    fn parses_csv_concurrency() {
        assert_eq!(parse_csv_u32("1,4,16,32").unwrap(), vec![1, 4, 16, 32]);
        assert_eq!(parse_csv_u32(" 8 ,16 ").unwrap(), vec![8, 16]);
        assert!(parse_csv_u32("nope").is_err());
    }

    #[test]
    fn builds_synthetic_prompts_by_default() {
        let args = BenchRunArgs {
            target: "vllm".into(),
            base_url: "http://x".into(),
            model: "m".into(),
            concurrency: "1".into(),
            num_requests: 3,
            input_tokens: 32,
            output_tokens: 16,
            dataset: "synthetic".into(),
            dataset_path: None,
            stream: false,
            scrape_metrics: false,
            request_timeout: "5s".into(),
            warmup: "0s".into(),
            output: None,
            csv: None,
            context_window: None,
            duration: None,
        };
        let p = args.build_prompts();
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].approx_prompt_tokens, 32);
    }

    #[test]
    fn duration_parsing_has_fallback() {
        let args = BenchRunArgs {
            target: "vllm".into(),
            base_url: "http://x".into(),
            model: "m".into(),
            concurrency: "1".into(),
            num_requests: 1,
            input_tokens: 32,
            output_tokens: 16,
            dataset: "synthetic".into(),
            dataset_path: None,
            stream: false,
            scrape_metrics: false,
            request_timeout: "garbage".into(),
            warmup: "garbage".into(),
            output: None,
            csv: None,
            context_window: None,
            duration: None,
        };
        // Falls back to defaults when the input is unparseable.
        assert_eq!(args.request_timeout_d(), std::time::Duration::from_secs(30));
        assert_eq!(args.warmup_d(), std::time::Duration::from_secs(10));
    }

    #[test]
    fn summarize_metrics_empty_is_all_none() {
        let s = summarize_metrics(&[]);
        assert!(s.kv_cache_usage_perc_avg.is_none());
        assert!(s.gpu_utilization_avg.is_none());
    }

    #[test]
    fn summarize_metrics_averages_nonempty() {
        let mut s1 = MetricsSnapshot::default();
        s1.series.insert(
            "vllm:gpu_cache_usage_perc".into(),
            metrics::MetricValue::Untyped(0.5),
        );
        s1.series.insert(
            "DCGM_FI_DEV_GPU_UTIL".into(),
            metrics::MetricValue::Untyped(60.0),
        );
        let mut s2 = MetricsSnapshot::default();
        s2.series.insert(
            "vllm:gpu_cache_usage_perc".into(),
            metrics::MetricValue::Untyped(0.7),
        );
        s2.series.insert(
            "DCGM_FI_DEV_GPU_UTIL".into(),
            metrics::MetricValue::Untyped(80.0),
        );
        let s = summarize_metrics(&[s1, s2]);
        assert_eq!(s.kv_cache_usage_perc_avg, Some(0.6));
        assert_eq!(s.gpu_utilization_avg, Some(70.0));
    }

    #[test]
    fn waste_inputs_built_from_samples() {
        let samples = vec![
            Sample {
                ts: "t".into(),
                concurrency: 1,
                ttft_s: Some(0.1),
                total_s: 1.0,
                prompt_tok: Some(50),
                output_tok: Some(100),
                completed: true,
                error: None,
            },
            Sample {
                ts: "t".into(),
                concurrency: 1,
                ttft_s: Some(0.2),
                total_s: 2.0,
                prompt_tok: Some(50),
                output_tok: Some(100),
                completed: true,
                error: None,
            },
        ];
        let mut summary = MetricsSummary::default();
        summary.kv_cache_usage_perc_avg = Some(0.5);
        summary.gpu_utilization_avg = Some(80.0);
        let inputs = build_waste_inputs(&samples, &summary, Some(1024));
        assert_eq!(inputs.latency_p50_s, Some(1.5));
        assert_eq!(inputs.ttft_samples_s.len(), 2);
        assert_eq!(inputs.prompt_tokens, &[50, 50]);
        assert_eq!(inputs.context_window, Some(1024));
    }
}
