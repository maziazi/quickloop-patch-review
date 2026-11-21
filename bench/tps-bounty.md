# TPS Bounty Entry — Kimi K3.1 Pro on Cerebras

Status: PENDING — produced after `BL-01`/`BL-02` baselines per `docs/PLAN.md` §1.3.

Method:

- Short prompt, long fluent output; reasoning OFF; `stream: true`.
- Report `queue_time_s`, `ttft_s`, `gen_s`, `output_tps`, `e2e_tps` from API
  `usage` fields only.
- Warm-cache vs cold-cache runs labeled explicitly; medians of 3–5 runs.
- Model: `moonshotai-kimi-k3.1-pro-trial`; execution inside OpenClaw per
  hackathon rules.
