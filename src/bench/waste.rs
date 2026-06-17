//! Waste score computation.
//!
//! Five axes, each with weight 20. Each axis produces a normalized
//! **waste value** in [0, 1] (0 = no waste, 1 = fully saturated waste).
//! The final `waste_score` is the weighted sum expressed on a 0–100 scale.
//!
//! Lower is better. 0 = no detectable waste on the measured axes; 100 =
//! every available axis is fully saturated.
//!
//! If a required input is missing for an axis, that axis is `None` in the
//! report and the score is prorated over the remaining axes (their weights
//! are rescaled to sum to 100, not silently treated as 0). The list of
//! unavailable axes is exposed in `WasteBreakdown::unavailable` so the
//! reader can see exactly what was and wasn't measured.
//!
//! The existing "efficiency / if-cleaned delta" framing in the README
//! becomes: `efficiency = 100 - waste_score`.

use serde::{Deserialize, Serialize};

/// Per-axis waste contribution. `None` means the source was missing and
/// the axis was excluded from the prorated score.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WasteBreakdown {
    pub kv_cache_pressure: Option<f64>,
    pub gpu_underutilization: Option<f64>,
    pub tail_latency: Option<f64>,
    pub ttft_jitter: Option<f64>,
    pub context_oversize: Option<f64>,
}

impl WasteBreakdown {
    /// Names of axes whose source was missing, in a stable order.
    pub fn unavailable(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.kv_cache_pressure.is_none() {
            out.push("kv_cache_pressure");
        }
        if self.gpu_underutilization.is_none() {
            out.push("gpu_underutilization");
        }
        if self.tail_latency.is_none() {
            out.push("tail_latency");
        }
        if self.ttft_jitter.is_none() {
            out.push("ttft_jitter");
        }
        if self.context_oversize.is_none() {
            out.push("context_oversize");
        }
        out
    }
}

/// Inputs to `compute`. All `None` fields produce `None` axes in the
/// breakdown. Owns its sample/prompt collections so the caller doesn't have
/// to manage lifetimes.
#[derive(Debug, Clone, Default)]
pub struct WasteInputs {
    /// Mean of scraped KV-cache usage gauge over the run. Expected in [0, 1].
    pub kv_cache_usage_perc_avg: Option<f64>,
    /// Mean GPU utilization over the run, percent in [0, 100].
    pub gpu_utilization_avg: Option<f64>,
    /// Client-side p50 total latency in seconds. Required for tail_latency.
    pub latency_p50_s: Option<f64>,
    /// Client-side p99 total latency in seconds. Required for tail_latency.
    pub latency_p99_s: Option<f64>,
    /// Client-side TTFT samples. Required for ttft_jitter.
    pub ttft_samples_s: Vec<f64>,
    /// Per-request prompt token counts. Required for context_oversize.
    pub prompt_tokens: Vec<u32>,
    /// Model context window. Required for context_oversize.
    pub context_window: Option<u32>,
}

/// Output of `compute`. `waste_score` is `None` only if no axes were
/// measurable (e.g. an empty run with no scraped metrics and no samples).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasteResult {
    pub waste_score: Option<f64>,
    pub breakdown: WasteBreakdown,
}

const WEIGHT: f64 = 20.0;

/// Map a raw waste value to a 0..1 normalized range, clamped.
fn clamp01(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else if x < 0.0 {
        0.0
    } else if x > 1.0 {
        1.0
    } else {
        x
    }
}

/// `mean(x)` for a slice. Returns `None` if the slice is empty.
fn mean(xs: &[f64]) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    let s: f64 = xs.iter().copied().sum();
    Some(s / xs.len() as f64)
}

/// Coefficient of variation: stddev / mean. Returns `None` if mean is
/// zero or the slice has fewer than 2 elements.
fn cv(xs: &[f64]) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let m = mean(xs)?;
    if m == 0.0 {
        return None;
    }
    let var: f64 = xs.iter().map(|x| (x - m).powi(2)).sum::<f64>() / xs.len() as f64;
    Some(var.sqrt() / m)
}

/// Axis 1: KV cache pressure. Higher usage = more waste. Capped at 1.0.
pub fn axis_kv_cache_pressure(kv_cache_usage_perc_avg: Option<f64>) -> Option<f64> {
    Some(clamp01(kv_cache_usage_perc_avg?))
}

/// Axis 2: GPU underutilization. 1 - utilization (in [0, 1]).
pub fn axis_gpu_underutilization(gpu_utilization_avg: Option<f64>) -> Option<f64> {
    let u = gpu_utilization_avg?;
    let frac = u / 100.0;
    Some(clamp01(1.0 - frac))
}

/// Axis 3: Tail latency. Maps p99/p50 ratio from 1.0 (no tail) to 5.0+ (saturated).
/// Always available when both p50 and p99 are present.
pub fn axis_tail_latency(p50: Option<f64>, p99: Option<f64>) -> Option<f64> {
    let p50 = p50?;
    let p99 = p99?;
    if p50 <= 0.0 {
        return None;
    }
    let ratio = p99 / p50;
    Some(clamp01((ratio - 1.0) / 4.0))
}

/// Axis 4: TTFT jitter. Maps coefficient of variation from 0 to 0.5+.
/// Always available when ≥ 2 samples are present.
pub fn axis_ttft_jitter(samples: &[f64]) -> Option<f64> {
    let c = cv(samples)?;
    Some(clamp01(c / 0.5))
}

/// Axis 5: Context oversize. Fraction of requests with prompt tokens
/// > 0.9 × context window. None if ctx window unknown or no requests.
pub fn axis_context_oversize(prompt_tokens: &[u32], context_window: Option<u32>) -> Option<f64> {
    let ctx = context_window? as f64;
    if prompt_tokens.is_empty() || ctx <= 0.0 {
        return None;
    }
    let oversize_threshold = 0.9 * ctx;
    let count = prompt_tokens
        .iter()
        .filter(|&&t| (t as f64) > oversize_threshold)
        .count();
    Some(clamp01(count as f64 / prompt_tokens.len() as f64))
}

/// Compute the waste score and breakdown from raw inputs. The result is
/// the weighted average (rescaled to 100) of the available axes.
pub fn compute(inputs: &WasteInputs) -> WasteResult {
    let breakdown = WasteBreakdown {
        kv_cache_pressure: axis_kv_cache_pressure(inputs.kv_cache_usage_perc_avg),
        gpu_underutilization: axis_gpu_underutilization(inputs.gpu_utilization_avg),
        tail_latency: axis_tail_latency(inputs.latency_p50_s, inputs.latency_p99_s),
        ttft_jitter: axis_ttft_jitter(&inputs.ttft_samples_s),
        context_oversize: axis_context_oversize(&inputs.prompt_tokens, inputs.context_window),
    };

    let mut weighted_sum = 0.0;
    let mut weight_total = 0.0;
    for axis in [
        breakdown.kv_cache_pressure,
        breakdown.gpu_underutilization,
        breakdown.tail_latency,
        breakdown.ttft_jitter,
        breakdown.context_oversize,
    ] {
        if let Some(v) = axis {
            weighted_sum += WEIGHT * v;
            weight_total += WEIGHT;
        }
    }

    let waste_score = if weight_total > 0.0 {
        Some(round1(100.0 * weighted_sum / weight_total))
    } else {
        None
    };

    WasteResult {
        waste_score,
        breakdown,
    }
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(
        kv: Option<f64>,
        gpu: Option<f64>,
        p50: Option<f64>,
        p99: Option<f64>,
        ttft: &[f64],
        prompts: &[u32],
        ctx: Option<u32>,
    ) -> WasteInputs {
        WasteInputs {
            kv_cache_usage_perc_avg: kv,
            gpu_utilization_avg: gpu,
            latency_p50_s: p50,
            latency_p99_s: p99,
            ttft_samples_s: ttft.to_vec(),
            prompt_tokens: prompts.to_vec(),
            context_window: ctx,
        }
    }

    #[test]
    fn all_axes_present_full_waste_equals_100() {
        let ttft: Vec<f64> = (0..50).map(|i| 0.1 + (i as f64) * 0.05).collect();
        let prompts = vec![10_000u32; 100]; // all over 0.9 * 1024
        let r = compute(&inputs(
            Some(1.0),
            Some(0.0),
            Some(1.0),
            Some(10.0), // ratio 10 → clamped to 1.0
            &ttft,
            &prompts,
            Some(1024),
        ));
        assert_eq!(r.waste_score, Some(100.0));
    }

    #[test]
    fn no_waste_equals_zero() {
        let ttft: Vec<f64> = (0..50).map(|_| 0.05).collect(); // cv = 0
        let prompts = vec![10u32; 100];
        let r = compute(&inputs(
            Some(0.0),
            Some(100.0),
            Some(1.0),
            Some(1.0), // ratio 1 → 0
            &ttft,
            &prompts,
            Some(1024),
        ));
        assert_eq!(r.waste_score, Some(0.0));
    }

    #[test]
    fn missing_axes_prorate_over_remaining() {
        // No kv, no gpu, no ctx — only tail_latency and ttft_jitter are
        // computable. Tail ratio 3 → (3-1)/4 = 0.5. TTFT constant → 0.
        // Weighted sum = 20*0.5 + 20*0 = 10. Total weight = 40.
        // waste_score = 100 * 10 / 40 = 25.0
        let ttft = vec![0.1; 50];
        let r = compute(&inputs(None, None, Some(1.0), Some(3.0), &ttft, &[], None));
        assert_eq!(r.waste_score, Some(25.0));
        assert!(r.breakdown.kv_cache_pressure.is_none());
        assert!(r.breakdown.gpu_underutilization.is_none());
        assert!(r.breakdown.context_oversize.is_none());
        assert_eq!(
            r.breakdown.unavailable(),
            vec!["kv_cache_pressure", "gpu_underutilization", "context_oversize"],
        );
    }

    #[test]
    fn all_axes_missing_means_null_score() {
        let r = compute(&inputs(None, None, None, None, &[], &[], None));
        assert_eq!(r.waste_score, None);
        assert_eq!(r.breakdown.unavailable().len(), 5);
    }

    #[test]
    fn waste_score_is_lower_is_better() {
        // Increase any axis → waste_score must monotonically increase.
        let ttft_low: Vec<f64> = vec![0.05; 50];
        let ttft_high: Vec<f64> = (0..50).map(|i| 0.1 + (i as f64) * 0.05).collect();

        let r_low = compute(&inputs(
            Some(0.1),
            Some(90.0),
            Some(1.0),
            Some(1.5),
            &ttft_low,
            &[10; 50],
            Some(1024),
        ));
        let r_high = compute(&inputs(
            Some(0.1),
            Some(90.0),
            Some(1.0),
            Some(1.5),
            &ttft_high,
            &[10; 50],
            Some(1024),
        ));
        assert!(r_high.waste_score.unwrap() > r_low.waste_score.unwrap());

        // Increase kv pressure, keep everything else the same.
        let r_kv_low = compute(&inputs(
            Some(0.1),
            Some(90.0),
            Some(1.0),
            Some(1.5),
            &ttft_low,
            &[10; 50],
            Some(1024),
        ));
        let r_kv_high = compute(&inputs(
            Some(0.9),
            Some(90.0),
            Some(1.0),
            Some(1.5),
            &ttft_low,
            &[10; 50],
            Some(1024),
        ));
        assert!(r_kv_high.waste_score.unwrap() > r_kv_low.waste_score.unwrap());
    }

    #[test]
    fn waste_score_is_in_zero_to_hundred() {
        let ttft: Vec<f64> = (0..100).map(|i| 0.01 * (i as f64)).collect();
        let prompts: Vec<u32> = (0..100).map(|i| i as u32).collect();
        let r = compute(&inputs(
            Some(0.5),
            Some(50.0),
            Some(0.5),
            Some(2.0),
            &ttft,
            &prompts,
            Some(1024),
        ));
        let s = r.waste_score.unwrap();
        assert!((0.0..=100.0).contains(&s), "score {} out of range", s);
    }

    #[test]
    fn waste_score_is_rounded_to_one_decimal() {
        // Construct a case where the unrounded value would have > 1 decimal
        // of precision. 1/3 axis weight ratios with float math can produce
        // these naturally.
        let r = compute(&inputs(
            Some(0.5),
            Some(50.0),
            Some(1.0),
            Some(1.0),
            &[0.1, 0.2, 0.3, 0.4],
            &[10; 4],
            Some(1024),
        ));
        let s = r.waste_score.unwrap();
        // Check that the string representation has at most 1 decimal place.
        let formatted = format!("{:.1}", s);
        let parsed: f64 = formatted.parse().unwrap();
        assert!((s - parsed).abs() < 1e-9);
    }

    #[test]
    fn tail_latency_axis_curves_correctly() {
        // ratio 1 → 0
        assert_eq!(axis_tail_latency(Some(1.0), Some(1.0)), Some(0.0));
        // ratio 5 → 1
        assert_eq!(axis_tail_latency(Some(1.0), Some(5.0)), Some(1.0));
        // ratio 10 → clamped to 1
        assert_eq!(axis_tail_latency(Some(1.0), Some(10.0)), Some(1.0));
        // ratio 3 → 0.5
        assert_eq!(axis_tail_latency(Some(1.0), Some(3.0)), Some(0.5));
    }

    #[test]
    fn tail_latency_rejects_zero_p50() {
        assert_eq!(axis_tail_latency(Some(0.0), Some(1.0)), None);
        assert_eq!(axis_tail_latency(None, Some(1.0)), None);
    }

    #[test]
    fn ttft_jitter_axis_curves_correctly() {
        // constant → cv ≈ 0 → 0
        let flat = vec![0.1; 20];
        let v = axis_ttft_jitter(&flat).unwrap();
        assert!(v.abs() < 1e-9, "expected ~0, got {}", v);
        // single sample → None
        assert_eq!(axis_ttft_jitter(&[0.1]), None);
        // large spread → cv > 0.5 → clamped to 1
        let wild: Vec<f64> = (0..20).map(|i| 0.001 * (i as f64) * (i as f64)).collect();
        let v = axis_ttft_jitter(&wild).unwrap();
        assert!((0.0..=1.0).contains(&v));
    }

    #[test]
    fn context_oversize_counts_above_90_percent() {
        let prompts = vec![100u32, 200, 300, 950, 1000]; // ctx=1024
        // 950, 1000 are > 0.9*1024=921.6 → 2/5 = 0.4
        let v = axis_context_oversize(&prompts, Some(1024)).unwrap();
        assert!((v - 0.4).abs() < 1e-9);
    }

    #[test]
    fn context_oversize_none_without_window() {
        let prompts = vec![10, 20, 30];
        assert_eq!(axis_context_oversize(&prompts, None), None);
    }

    #[test]
    fn context_oversize_none_with_empty_prompts() {
        assert_eq!(axis_context_oversize(&[], Some(1024)), None);
    }

    #[test]
    fn gpu_underutilization_curves_correctly() {
        assert_eq!(axis_gpu_underutilization(Some(0.0)), Some(1.0));
        assert_eq!(axis_gpu_underutilization(Some(50.0)), Some(0.5));
        assert_eq!(axis_gpu_underutilization(Some(100.0)), Some(0.0));
        assert_eq!(axis_gpu_underutilization(Some(150.0)), Some(0.0));
        assert_eq!(axis_gpu_underutilization(None), None);
    }

    #[test]
    fn kv_cache_pressure_curves_correctly() {
        assert_eq!(axis_kv_cache_pressure(Some(0.0)), Some(0.0));
        assert_eq!(axis_kv_cache_pressure(Some(0.5)), Some(0.5));
        assert_eq!(axis_kv_cache_pressure(Some(1.0)), Some(1.0));
        assert_eq!(axis_kv_cache_pressure(Some(2.0)), Some(1.0));
        assert_eq!(axis_kv_cache_pressure(None), None);
    }
}
