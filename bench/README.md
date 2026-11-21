# Baseline Suite (BL-01 – BL-05) & TPS Bounty

Run each scenario 3–5× and report the median. All numbers must come from
Cerebras API `usage` fields and local monotonic clocks — never tokenizer
estimates. Label every run cold-cache or warm-cache.

| Code | Scenario | Input Context | Target Output | Concurrency |
| :--- | :--- | :--- | :--- | :--- |
| BL-01 | Sanity short reply | < 500 tok | 100–150 tok | 1 |
| BL-02 | Medium code generation | 1K–4K tok | 800–1,200 tok | 1 |
| BL-03 | 1-step repo diff + patch | 4K–16K tok | 500–1,000 tok | 1 |
| BL-04 | Multi-step loop (≥ 3 turns) | bounded tail | ~400 tok/iter | 1 |
| BL-05 | Disjoint parallel fan-out | medium | medium | 2–4 |

Required metrics per run: `queue_time_s`, `ttft_s`, `gen_s`, `output_tps`,
`e2e_tps`, `cache_hit_rate`, `wall_s` (formulas in `docs/DESIGN.md` §4).

Raw run logs belong in `bench/raw-runs/` (gitignored).
