# QuickLoop — System Design & Speed Engineering

**Status:** v0.2 · **Last updated:** 2026-11-22
**Evidence Tags:** **[V]** Verified against vendor docs/console · **[P]** Probable · **[U]** Unverified judgment (measure before trusting).

---

## 1. Hardware & Model Physics (Cerebras WSE + Kimi K3.1 Pro)
Designing for speed requires aligning the harness with the underlying hardware and model architecture:
- **Decode is memory-bandwidth bound:** Kimi K3.1 Pro has **2.8T total / ~104B active parameters** per generated token **[V]**. Each decode step reads active weights; therefore, **output token volume directly determines decode duration**.
- **Cerebras WSE SRAM & pipeline parallelism:** Each wafer holds **44 GB on-wafer SRAM** near compute cores with 4-bit weights chained across wafers via pipeline parallelism. Larger microbatches raise aggregate throughput at the expense of per-user latency—and under hackathon load, shared queueing (`time_info.queue_time`) can dominate wall clock even when decode remains fast.
- **Hybrid Attention (93 layers = 69 KDA + 24 DSA):**
  - **69 KDA (Kimi Delta Attention) layers:** Fixed-size hidden state with constant memory/compute.
  - **24 DSA (DeepSeek Sparse Attention) layers:** Lightweight indexer selects top-2,048 tokens, **but full KV cache still grows with context length**, reducing concurrent request capacity as context expands.

**Architectural implication:** Keep context aggressively filtered, replace (never append) failure logs, keep prefixes byte-stable, and minimize output tokens.

---

## 2. Speed Levers (Ordered by Impact)
1. **Byte-stable prompt prefix `[V]`:** Keep static `SKILL.md` rules, pinned tool schemas (`exec`, `read`), and filtered diff immutable at the top. Cached tokens cut TTFT and do not consume the **100K uncached TPM** ceiling. Assert `cached_tokens > 0` on iteration 2; treat `< 80%` hit rate as a defect.
2. **Minimal `SEARCH/REPLACE` patches `[U]`:** ~400 output tokens vs ~5,000 for whole-file rewrites. Cap `max_tokens` (review `1,500`, patch `800`). Forbid prose outside patch fences.
3. **Asymmetric reasoning dial `[V]`:** `reasoning_effort: low` once in Phase 2 review; **OFF** (unset) during patch synthesis.
4. **Replaceable dynamic tail `[U]`:** `failing_evidence` overwrites the previous iteration's failure output instead of appending, preventing linear KV-cache growth in DSA layers.
5. **Tool policy pinning & session pruning `[V]`:** Restrict OpenClaw tools to `exec` + `read`; prune superseded tool logs between turns.
6. **Streaming & payload compression `[V]`:** Enable `stream: true` and `msgpack + gzip` when supported by client.
7. **Queue-aware disjoint fan-out `[U]`:** For independent `CRITICAL` files, fan out up to 4 concurrent patch requests sharing the cached prefix, applying patches sequentially. Automatically degrade to serial mode on `HTTP 429` or `queue_time_s > 2 s`.
8. **Predicted Outputs `[P]`:** Feature-flagged probe at startup; silently disable on error.

---

## 3. Token Budget Under Verified Trial Limits `[V]`
Verified console limits for `moonshotai-kimi-k3.1-pro-trial`:
- **Context / Max Output:** `262,144` / `40,960`
- **Requests:** `1,000 / min` · `60,000 / hr` · `1,440,000 / day`
- **Total Tokens:** `1,000,000 / min`
- **Uncached Tokens:** `100,000 / min` (the binding constraint for rapid loops)

| Component | Est. Tokens | Consumes 100K Uncached TPM? |
| :--- | :--- | :--- |
| Static prefix (skills + pinned tools) | ~2,500 | Iteration 0 only (cached thereafter) |
| Filtered diff | ~2,500 | Iteration 0 only (if prefix stays stable) |
| Replaced failure evidence | ~600 | Yes, per iteration |
| Output (`SEARCH/REPLACE` patch) | ~400 | Yes, per iteration |
| **Marginal cost after iteration 0** | **~1,000 / iter** | Fits 5 iterations easily (~8.5K uncached total) |

---

## 4. Measurement Contract & Baseline Suite (`bench/`)

### Required Metrics (Never Estimate from Tokenizer)
- `queue_time_s`: `time_info.queue_time` from response (isolates server congestion from model speed).
- `ttft_s`: `t_first_content_token - t_request_sent`.
- `gen_s`: `t_last_token - t_first_content_token`.
- `output_tps`: `usage.completion_tokens / gen_s`.
- `e2e_tps`: `usage.completion_tokens / (ttft_s + gen_s)`.
- `cache_hit_rate`: `usage.prompt_tokens_details.cached_tokens / usage.prompt_tokens`.
- `wall_s`: full skill invocation to gate decision.

### Standard Baseline Suite (Run 3–5×, Report Median)
| Code | Scenario | Input Context | Target Output | Concurrency | Purpose |
| :--- | :--- | :--- | :--- | :--- | :--- |
| **BL-01** | Sanity short reply | `< 500 tok` | `100–150 tok` | 1 | Measure baseline `queue_time`, TTFT, peak burst TPS |
| **BL-02** | Medium code generation | `1K–4K tok` | `800–1,200 tok` | 1 | Measure sustained decode TPS |
| **BL-03** | 1-step repo diff + patch | `4K–16K tok` | `500–1,000 tok` | 1 | Measure context + worker overhead |
| **BL-04** | Multi-step loop (≥ 3 turns) | Bounded tail | `~400 tok/iter` | 1 | Verify `cached_tokens > 0` & E2E wall clock |
| **BL-05** | Disjoint parallel fan-out | Medium | Medium | 2–4 | Measure throughput vs per-user speed & `429` safety |