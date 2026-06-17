//! Concurrent HTTP load generator for the OpenAI-compatible
//! `/v1/chat/completions` endpoint.
//!
//! v0.6 talks to whatever server the user points it at (vLLM, SGLang, TGI,
//! llama.cpp server, anything OpenAI-compatible). The loadgen:
//!
//! 1. Builds N requests per concurrency level.
//! 2. Spawns `concurrency` worker tasks, each pulling work from a shared
//!    queue and POSTing to the server.
//! 3. Records TTFT (time to first SSE/data byte), total latency, prompt
//!    and output tokens (from `usage` in the response), and any error.
//! 4. Returns the full `Vec<Sample>` for the report.
//!
//! Streaming is supported — when `--stream` is on, we read the response as a
//! stream and treat the first chunk arrival as TTFT. When streaming is off,
//! TTFT is set to the total request time (best we can do without per-token
//! data).
//!
//! Concurrency sweep is sequential: we finish one level before starting the
//! next. This keeps the metrics scraper's per-level rollups clean and
//! matches how vLLM/SGLang's own bench scripts work.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::bench::dataset::Prompt;
use crate::bench::target::Sample;

/// A single chat-completion request body. Minimal — `stream` is the only
/// option v0.6 cares about; the rest are server defaults.
#[derive(Debug, Clone, Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Debug, Clone, Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Debug, Clone, Deserialize)]
struct ChatResponse {
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Debug, Clone, Deserialize)]
struct Usage {
    #[serde(default)]
    prompt_tokens: Option<u32>,
    #[serde(default)]
    completion_tokens: Option<u32>,
}

/// Configuration for a single bench run.
#[derive(Clone)]
pub struct LoadgenConfig {
    pub base_url: String,
    pub model: String,
    pub concurrency_sweep: Vec<u32>,
    pub num_requests: u32,
    pub max_output_tokens: u32,
    pub prompts: Vec<Prompt>,
    pub stream: bool,
    pub request_timeout: Duration,
    pub warmup: Duration,
    /// Optional callback the loadgen invokes after each request completes.
    /// Used by the bench runner to push the sample into a shared buffer for
    /// live progress reporting. None in tests.
    pub on_sample: Option<Arc<dyn Fn(Sample) + Send + Sync>>,
}

/// Run the full sweep. Returns all samples in completion order.
pub async fn run_sweep(
    client: reqwest::Client,
    cfg: LoadgenConfig,
) -> Result<Vec<Sample>, String> {
    let mut all = Vec::new();
    for &concurrency in &cfg.concurrency_sweep {
        // Warmup phase: a small number of requests at this concurrency so
        // the server's first allocations don't dominate the first sample.
        if !cfg.warmup.is_zero() {
            let warmup_n = concurrency.min(2);
            let warmup_prompts: Vec<Prompt> = cfg
                .prompts
                .iter()
                .cycle()
                .take(warmup_n as usize)
                .cloned()
                .collect();
            let _ = run_level(&client, &cfg, concurrency, &warmup_prompts, true).await;
            tokio::time::sleep(cfg.warmup).await;
        }

        let samples = run_level(&client, &cfg, concurrency, &cfg.prompts, false).await?;
        if let Some(cb) = &cfg.on_sample {
            for s in &samples {
                cb(s.clone());
            }
        }
        all.extend(samples);
    }
    Ok(all)
}

async fn run_level(
    client: &reqwest::Client,
    cfg: &LoadgenConfig,
    concurrency: u32,
    prompts: &[Prompt],
    _is_warmup: bool,
) -> Result<Vec<Sample>, String> {
    let n = cfg.num_requests as usize;
    let queue: Arc<parking_lot::Mutex<std::vec::IntoIter<Prompt>>> = Arc::new(
        parking_lot::Mutex::new(prompts.iter().cycle().take(n).cloned().collect::<Vec<_>>().into_iter()),
    );

    let mut handles = Vec::with_capacity(concurrency as usize);
    for _worker_id in 0..concurrency {
        let client = client.clone();
        let cfg = cfg.clone();
        let queue = Arc::clone(&queue);
        let handle = tokio::spawn(async move {
            let mut local = Vec::new();
            loop {
                let prompt = {
                    let mut q = queue.lock();
                    q.next()
                };
                let Some(prompt) = prompt else { break };
                let mut sample = send_one(&client, &cfg, &prompt).await;
                // Stamp the concurrency bucket on each sample.
                sample.concurrency = concurrency;
                local.push(sample);
            }
            local
        });
        handles.push(handle);
    }
    let mut all = Vec::new();
    for h in handles {
        match h.await {
            Ok(v) => all.extend(v),
            Err(e) => return Err(format!("worker join error: {e}")),
        }
    }
    Ok(all)
}

async fn send_one(client: &reqwest::Client, cfg: &LoadgenConfig, prompt: &Prompt) -> Sample {
    let url = format!("{}/v1/chat/completions", cfg.base_url.trim_end_matches('/'));
    let body = ChatRequest {
        model: &cfg.model,
        messages: vec![ChatMessage {
            role: "user",
            content: &prompt.text,
        }],
        max_tokens: cfg.max_output_tokens,
        stream: cfg.stream,
    };

    let started = Instant::now();
    let mut req = client
        .post(&url)
        .timeout(cfg.request_timeout)
        .json(&body);
    if cfg.stream {
        // `stream=true` requires the server to keep the connection open and
        // send SSE-style chunks. reqwest handles this with `bytes_stream`.
        req = req.header("accept", "text/event-stream");
    }

    let result = req.send().await;
    let mut sample = Sample {
        ts: chrono::Utc::now().to_rfc3339(),
        concurrency: 0, // filled in by caller (rollup); we don't know it here
        ttft_s: None,
        total_s: 0.0,
        prompt_tok: Some(prompt.approx_prompt_tokens),
        output_tok: None,
        completed: false,
        error: None,
    };

    match result {
        Err(e) => {
            sample.total_s = started.elapsed().as_secs_f64();
            sample.error = Some(classify_reqwest_error(&e));
            sample
        }
        Ok(resp) => {
            let status = resp.status();
            if !status.is_success() {
                sample.total_s = started.elapsed().as_secs_f64();
                sample.error = Some(format!("http_{}", status.as_u16()));
                return sample;
            }
            if cfg.stream {
                match stream_consume(resp, &mut sample, started).await {
                    Ok(()) => {
                        sample.completed = true;
                    }
                    Err(e) => {
                        sample.error = Some(format!("stream: {e}"));
                    }
                }
            } else {
                match resp.json::<ChatResponse>().await {
                    Ok(body) => {
                        sample.total_s = started.elapsed().as_secs_f64();
                        sample.ttft_s = Some(sample.total_s);
                        if let Some(u) = body.usage {
                            sample.prompt_tok = u.prompt_tokens.or(sample.prompt_tok);
                            sample.output_tok = u.completion_tokens;
                        }
                        sample.completed = true;
                    }
                    Err(e) => {
                        sample.total_s = started.elapsed().as_secs_f64();
                        sample.error = Some(format!("parse: {e}"));
                    }
                }
            }
            sample
        }
    }
}

async fn stream_consume(
    resp: reqwest::Response,
    sample: &mut Sample,
    started: Instant,
) -> Result<(), String> {
    use futures_util::StreamExt;
    let mut first = true;
    let mut bytes_seen = 0u64;
    let mut stream = resp.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        if first {
            sample.ttft_s = Some(started.elapsed().as_secs_f64());
            first = false;
        }
        bytes_seen += chunk.len() as u64;
    }
    sample.total_s = started.elapsed().as_secs_f64();
    // We can't recover exact token counts from a raw byte stream without
    // a server-specific parser. The server's `usage` block is the only
    // source of truth and is sent at the end of the stream as JSON-delta.
    // For v0.6 we leave output_tok None when streaming; the report's
    // throughput is approximated from latency instead. (See design doc.)
    let _ = bytes_seen;
    Ok(())
}

fn classify_reqwest_error(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".to_string()
    } else if e.is_connect() {
        "connect".to_string()
    } else if e.is_request() {
        format!("request: {e}")
    } else {
        format!("transport: {e}")
    }
}

/// Build a `reqwest::Client` with the v0.6 default settings.
pub fn build_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent(concat!("agent0waste/", env!("CARGO_PKG_VERSION"), " (Layer 6)"))
        .pool_max_idle_per_host(64)
        .build()
        .expect("reqwest client build")
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn cfg(base_url: String, prompts: Vec<Prompt>, n: u32) -> LoadgenConfig {
        LoadgenConfig {
            base_url,
            model: "test-model".into(),
            concurrency_sweep: vec![1],
            num_requests: n,
            max_output_tokens: 16,
            prompts,
            stream: false,
            request_timeout: Duration::from_secs(5),
            warmup: Duration::ZERO,
            on_sample: None,
        }
    }

    #[tokio::test]
    async fn sends_n_requests_and_records_samples() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": "x",
                "model": "test-model",
                "choices": [{"index": 0, "message": {"role": "assistant", "content": "hi"}}],
                "usage": {"prompt_tokens": 5, "completion_tokens": 7, "total_tokens": 12}
            })))
            .expect(4)
            .mount(&server)
            .await;

        let url = server.uri();
        let cfg = cfg(
            url,
            vec![Prompt {
                text: "hi".into(),
                approx_prompt_tokens: 1,
            }; 4],
            4,
        );
        let client = build_client();
        let samples = run_sweep(client, cfg).await.unwrap();
        assert_eq!(samples.len(), 4);
        for s in &samples {
            assert!(s.completed);
            assert!(s.error.is_none());
            assert!(s.output_tok == Some(7));
        }
    }

    #[tokio::test]
    async fn records_error_on_http_500() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let url = server.uri();
        let cfg = cfg(
            url,
            vec![Prompt {
                text: "x".into(),
                approx_prompt_tokens: 1,
            }; 2],
            2,
        );
        let client = build_client();
        let samples = run_sweep(client, cfg).await.unwrap();
        assert_eq!(samples.len(), 2);
        for s in &samples {
            assert!(!s.completed);
            assert_eq!(s.error.as_deref(), Some("http_500"));
        }
    }
}
