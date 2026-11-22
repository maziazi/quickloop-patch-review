# TPS Bounty Entry — Kimi K3.1 Pro on Cerebras

Status: **MEASURED (2026-11-22)** — via OpenClaw 2026.10.9 (`openclaw infer model run`, embedded runner), model `moonshotai-kimi-k3.1-pro-trial`, `--thinking off`, shared trial endpoint.

## What was measured (and how)

- **End-to-end wall clock** per call, from local monotonic clocks around the full OpenClaw CLI invocation (includes process spawn + queue + TTFT + decode + teardown — these components are **not separable** because the `infer` CLI does not surface Cerebras `queue_time_s` / `ttft_s`; per §3.8 they are reported as absent, never estimated).
- **Token + cache figures** come from the official closed-loop run's harness usage report (`result.meta.agentMeta.usage`): `input` / `output` / `cacheRead` / `reasoningTokens`.
- Raw evidence: `bench/raw-runs/official-kimi-pr5-{run,openclaw}.json` (official loop, gitignored locally), collection scripts in session logs; medians of N=3–4 unless noted.

## Official closed-loop run (PR #5, 2026-11-22 13:49 WIB)

| Metric | Value |
| :--- | :--- |
| Verdict | `READY_TO_MERGE`, `score: 100/100`, iteration 1 |
| Prompt tokens | 16,995 input |
| Output tokens | 3,901 |
| **Cache read** | **189,440 tokens (≈ 91.8% of prompt traffic served from prefix cache)** |
| Reasoning tokens | 1,629 |
| Worker `verify` wall | 2.60 s |
| Worker `publish` wall | 9.53 s |

The 91.8% cache hit rate is the byte-stable-prompt-prefix lever working as designed: 189,440 tokens were served from cache instead of counting against the 100K uncached TPM ceiling.

## Baseline suite (BL-01–BL-05, wall-clock medians, N as run)

| Suite | Scenario | N (ok/attempts) | Median wall | Min–Max |
| :--- | :--- | :---: | :---: | :---: |
| BL-01 | Sanity short reply (16 chars out) | 3/3 | 15.05 s | 12.86–15.36 |
| BL-02 | Medium code generation (~2.1 KB Rust out) | 3/3 | 20.95 s | 20.67–26.46 |
| BL-03 | Single-step `SEARCH/REPLACE` patch block (105 chars out) | 1/1 | 7.58 s | — |
| BL-04 | Multi-step closed loop (PR #5 official run) | 1 | turn usage above; worker gates 2.60 s verify / 9.53 s publish | — |
| BL-05 | 4 disjoint parallel calls (fan-out) | 4/4 | 9.61 s | 9.57–9.73 |

Notes:

- BL-05's four concurrent calls landed within a 160 ms spread (9.57–9.73 s) — the shared endpoint dispatched them without serializing.
- The absolute wall figures include shared-endpoint queue time (the trial endpoint returned HTTP 429 `queue_exceeded` on 4 of 17 collection attempts during peak congestion; those attempts are excluded, and retry behavior is bounded).
- `output_tps` (decode-only) and `ttft_s` are **absent** — the harness surface does not expose them yet; wiring API `time_info` into the run record is Roadmap item #2.

## Reproducing

```bash
# environment: OpenClaw CLI configured per README Quick Start, CEREBRAS_API_KEY in gateway env
openclaw infer model run --model cerebras/moonshotai-kimi-k3.1-pro-trial \
  --thinking off --prompt "Reply with exactly three words: Quick Loop Works" --json --local
# time the call with a monotonic clock; medians over 3 runs
```
