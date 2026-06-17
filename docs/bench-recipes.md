# Bench Recipes

Copy-pasteable commands for the most common `agent0waste bench` workflows.
All examples assume Layer 6 is enabled:

```bash
cargo build --release --features bench
# or:
cargo install --git https://github.com/beme08/Agent0Waste --features bench
```

> Apple Silicon / MLX recipe planned for **v0.7**. v0.6 ships
> `vllm` / `sglang` / `baseline` targets only.

## 1. Benchmark a remote vLLM server

```bash
# Start vLLM (on the GPU box) — out of band:
#   python -m vllm.entrypoints.openai.api_server \
#     --model meta-llama/Meta-Llama-3-8B-Instruct --port 8000

# Run the bench (on any host that can reach the GPU box):
agent0waste bench run vllm \
  --base-url http://gpu-box:8000 \
  --model meta-llama/Meta-Llama-3-8B-Instruct \
  --concurrency 1,4,16,32 \
  --num-requests 100 \
  --input-tokens 512 \
  --output-tokens 256 \
  --scrape-metrics \
  --output bench-vllm.json \
  --csv bench-vllm.csv
```

## 2. Benchmark a remote SGLang server

```bash
# Start SGLang (on the GPU box) — out of band:
#   python -m sglang.launch_server \
#     --model-path meta-llama/Meta-Llama-3-8B-Instruct --port 30000

agent0waste bench run sglang \
  --base-url http://gpu-box:30000 \
  --model meta-llama/Meta-Llama-3-8B-Instruct \
  --concurrency 1,4,16,32 \
  --num-requests 100 \
  --scrape-metrics \
  --output bench-sglang.json \
  --csv bench-sglang.csv
```

## 3. Benchmark a generic OpenAI-compatible endpoint (`baseline`)

Use this for TGI, llama.cpp server, or any compatible proxy. The `baseline`
target does **client-side measurements only**; no backend metrics are
scraped. The `gpu_underutilization` and `kv_cache_pressure` axes will be
`null` in the report.

```bash
agent0waste bench run baseline \
  --base-url http://localhost:8080 \
  --model my-model \
  --concurrency 1,4,16 \
  --num-requests 50 \
  --scrape-metrics=false \
  --output bench-baseline.json
```

> Even though `--scrape-metrics=true` is the default, the `baseline` target
> declares zero metric keys, so the scraper is a no-op. Passing
> `--scrape-metrics=false` makes the intent explicit.

## 4. Compare two reports

```bash
agent0waste bench compare bench-vllm.json bench-sglang.json
# Output:
#   A: bench-vllm.json  (waste_score=Some(28.4))
#   B: bench-sglang.json (waste_score=Some(31.7))
#   delta: 3.30 (worse)
```

## 5. Render a report as Markdown

```bash
agent0waste bench report bench-vllm.json > bench-vllm.md
# Embed in a blog post or PR description.
```

## 6. Use the bundled ShareGPT dataset

```bash
agent0waste bench run vllm \
  --base-url http://gpu-box:8000 \
  --model meta-llama/Meta-Llama-3-8B-Instruct \
  --dataset sharegpt \
  --concurrency 1,4,16 \
  --num-requests 25 \
  --output bench-sharegpt.json
```

The bundled fixture is at `data/sharegpt-tiny.json` (25 prompts). For a
larger workload, point `--dataset-path` at your own ShareGPT export.

## 7. CI smoke test (no GPU required)

Spin up a `wiremock` (or any HTTP stub) that returns canned
`/v1/chat/completions` responses, then:

```bash
agent0waste bench run vllm \
  --base-url http://localhost:18080 \
  --model test-model \
  --concurrency 1,2 \
  --num-requests 4 \
  --warmup 0s \
  --scrape-metrics=false \
  --output /tmp/smoke.json

# Assert the report is well-formed and in range:
python3 -c '
import json
r = json.load(open("/tmp/smoke.json"))
assert 0 <= r["waste_score"] <= 100, r["waste_score"]
for axis in ["kv_cache_pressure","gpu_underutilization",
             "tail_latency","ttft_jitter","context_oversize"]:
    assert axis in r["waste_axes"], axis
print("smoke ok, waste_score =", r["waste_score"])
'
```

## Reading the report

```json
{
  "target": "vllm",
  "waste_score": 28.4,            // lower is better
  "waste_axes": {
    "kv_cache_pressure": 12.4,
    "gpu_underutilization": 4.4,
    "tail_latency": 7.1,
    "ttft_jitter": 3.9,
    "context_oversize": 0.6
  },
  "waste_axes_unavailable": [],
  "rollup": {
    "by_concurrency": {
      "16": {"p50_s": 1.42, "p99_s": 3.10, "tok_per_s": 1842.3, "ttft_p50_s": 0.07, "completed": 100, "errored": 0}
    }
  }
}
```

`efficiency = 100 - waste_score` — matches the existing "efficiency /
if-cleaned delta" framing in the README.

## What `null` means in a report

If `kv_cache_pressure` is `null` and `waste_axes_unavailable` lists
`kv_cache_pressure`, the backend didn't expose a KV-cache gauge. The score
is prorated over the remaining axes — we never invent a "hit rate" we can't
derive. Add a vLLM/SGLang option that exposes the series, or pass
`--scrape-metrics=false` to skip the axis entirely.
