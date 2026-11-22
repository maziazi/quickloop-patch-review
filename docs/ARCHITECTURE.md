# QuickLoop Architecture

**`quickloop-patch-review`** (CLI binary: `quickloop`) is a closed-loop pull request review, patch, and verification agent built on the **OpenClaw** harness, powered by **Kimi K3.1 Pro** (`moonshotai-kimi-k3.1-pro-trial`) on **Cerebras Wafer-Scale Inference**, and enforced by a deterministic **Rust CLI worker**.

Unlike comment-only AI reviewers that shift the edit–test–re-review bottleneck back onto developers, QuickLoop executes an autonomous, bounded fix-and-verify loop until a deterministic local merge gate passes.

This document explains the system architecture for developers who want to understand the design, reproduce its speed and caching behavior, contribute, or adapt these patterns.

---

## 1. The Core Insight

Most AI code reviewers run a single LLM pass over a raw PR diff and post advisory comments. That leaves the slowest part of code review—editing files, re-running tests, fixing regressions, and verifying scope—entirely manual. Conversely, naive autonomous coding agents fail in practice for four predictable reasons:

1. **Unbounded token and wall-clock latency:** Rewriting full files and appending raw test logs across iterations causes context windows to explode and generation times to balloon.
2. **Prefix cache invalidation:** Interpolating dynamic counters, timestamps, or run IDs above the prompt tail destroys prefix caching, multiplying TTFT and burning uncached rate-limit budgets.
3. **Infinite bikeshedding loops:** Without strict severity filtering, autonomous reviewers endlessly chase naming or formatting nits and never converge.
4. **Reward hacking ("cheating"):** Any loop rewarded solely for making tests pass has a structural incentive to delete or weaken failing tests unless prevented deterministically.

QuickLoop resolves all four by treating **speed and correctness as coupled architectural invariants**:

* **Authority separation (Model proposes, Worker decides):** LLMs never compose mutating shell commands and never declare a PR ready. `READY_TO_MERGE` is computed strictly from local process exit codes in a compiled Rust binary.
* **Computation in Rust, reasoning in Kimi K3.1 Pro:** Diff stripping, hunk matching, test/lint execution, scope checking, and telemetry recording run in native Rust (`< 15 ms` cold start). The LLM is invoked only for defect triage and minimal patch synthesis.
* **Output volume as the primary latency budget:** On high-throughput decode hardware, wall-clock time is governed by generated tokens, TTFT, and queue time. QuickLoop emits `SEARCH/REPLACE` hunks (~400 tokens) rather than whole-file rewrites (~5,000 tokens) and disables reasoning tokens during patch transcription.
* **Byte-stable prefix + replaceable failure tail:** Static skill instructions remain immutable at the top of the message array, while failing verification evidence **replaces** (never appends to) the previous iteration's tail—sustaining `> 80%` prefix cache hits and protecting the **100K uncached TPM** trial ceiling.
* **Structural anti-cheat enforcement ($S_{\text{scope}}$):** Protected test globs are checked on every verification pass; touching or deleting a test immediately zeroes $S_{\text{scope}}$, reverts the iteration's patch, and halts with `BLOCKED`.

---

## 2. Layered AI System Architecture

QuickLoop is structured into four cleanly separated layers so inference speed, deterministic safety, and observability can be reasoned about independently:

```text
┌─────────────────────────────────────────────────────────────────────────┐
│ LAYER 1 — INTERACTION & HARNESS LAYER (OpenClaw)                        │
│  · Trigger: /looping-code-review <pr> (CLI recommended, isolated VM/dir)│
│  · Control Plane: ~/.openclaw/openclaw.json (JSON5)                     │
│  · Skills: requesting-code-review · receiving-code-review · looping-... │
│  · Guards: Tool policy pinned (exec + read) · Session pruning · CacheKey│
└──────────────────────────────────┬──────────────────────────────────────┘
                                   │ HTTPS Stream (SSE / JSON)
                                   ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ LAYER 2 — INFERENCE & MODEL COMPUTE LAYER (Cerebras WSE + Kimi K3.1 Pro)│
│  · Endpoint: https://api.cerebras.ai/v1 (OpenClaw-only hackathon key)   │
│  · Model: moonshotai-kimi-k3.1-pro-trial (2.8T total / ~104B active)    │
│  · Hardware: 44 GB on-wafer SRAM · pipeline parallelism · microbatching │
│  · Attention: 93 layers = 69 KDA (fixed state) + 24 DSA (growing KV)    │
│  · Asymmetric Reasoning: Phase 2 = low · Phase 3 (patch) = OFF          │
└──────────────────────────────────┬──────────────────────────────────────┘
                                   │ Compact JSON / SEARCH-REPLACE
                                   ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ LAYER 3 — DETERMINISTIC EXECUTION & GUARDRAIL LAYER (Rust CLI Worker)   │
│  · Binary: `quickloop` (< 15 ms cold start, zero async runtime tax)     │
│  · `diff`    → strips lockfiles, dist, vendor, binaries (< 400 ms)      │
│  · `patch`   → exact single-match SEARCH/REPLACE atomic apply (~20 ms)  │
│  · `verify`  → 4 concurrent checks (< 300 ms) · exits 0 on score < 100  │
│  · `publish` → git commit/push + single upserted PR comment             │
└──────────────────────────────────┬──────────────────────────────────────┘
                                   │ Local I/O, Subprocess, Git & GH API
                                   ▼
┌─────────────────────────────────────────────────────────────────────────┐
│ LAYER 4 — WORKSPACE, EXTERNAL SYSTEMS & HONEST TELEMETRY LAYER          │
│  · Isolated Target Repo + Toolchain (cargo/pytest/vitest, .quickloop.toml)│
│  · GitHub PR State (diff source, branch commits, single status comment) │
│  · Telemetry Store (.quickloop/run-<id>.json): API usage + local clock  │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 3. End-to-End Pipeline Overview

QuickLoop executes a **5-phase closed-loop pipeline**. Phases 1–2 run once per PR invocation to establish a clean baseline. Phases 3–4 form the bounded remediation loop (up to `max_iterations`, default `5`), with optional disjoint-file fan-out. Phase 5 runs once upon reaching $\text{score} = 100$.

```text
┌─────────────────────────────────────────────────────────────────────────┐
│                          QuickLoop Pipeline                             │
│                                                                         │
│  ┌──────────────┐     ┌──────────────────────────────────────────────┐  │
│  │   Phase 1    │     │                   Phase 2                    │  │
│  │  DIFF INTAKE │────▶│          INITIAL REVIEW & TRIAGE             │  │
│  │  Rust Worker │     │     Kimi K3.1 Pro (reasoning_effort: low)    │  │
│  └──────────────┘     └──────────────────────┬───────────────────────┘  │
│                                              │                          │
│                        ┌─────────────────────┼────────────────────┐     │
│                        ▼                     ▼                    ▼     │
│                 [ARCHITECTURAL]      [No CRITICAL]          [CRITICAL]  │
│                        │                     │                    │     │
│                        ▼                     ▼                    │     │
│                 ┌─────────────┐       ┌─────────────┐             │     │
│                 │ NEEDS_HUMAN │       │READY_TO_MRG │             │     │
│                 └─────────────┘       └─────────────┘             │     │
│                                                                   │     │
│        ┌──────────────────────────────────────────────────────────┘     │
│        ▼                                                                │
│  ╔═══════════════════════════════════════════════════════════════════╗  │
│  ║  Bounded Remediation Loop (iteration 1 .. max_iterations)         ║  │
│  ║                                                                   ║  │
│  ║  ┌─────────────────────────┐      ┌────────────────────────────┐  ║  │
│  ║  │        Phase 3          │      │          Phase 4           │  ║  │
│  ║  │    PATCH SYNTHESIS      │─────▶│   ATOMIC APPLY & VERIFY    │  ║  │
│  ║  │ Kimi (reasoning: OFF)   │      │  Rust Worker (< 320 ms)    │  ║  │
│  ║  │ SEARCH/REPLACE only     │      │  patch + 4 concurrent chk  │  ║  │
│  ║  └────────────▲────────────┘      └─────────────┬──────────────┘  ║  │
│  ║               │                                 │                 ║  │
│  ║               │   score < 100                   ├── S_scope == 0 ─┼─▶ BLOCKED
│  ║               └── (replace failure tail) ───────┘                 ║  │
│  ║                                                 │ score == 100    ║  │
│  ╚═════════════════════════════════════════════════╪═════════════════╝  │
│                                                    ▼                    │
│                                            ┌──────────────┐             │
│                                            │   Phase 5    │             │
│                                            │   PUBLISH    │             │
│                                            │  Rust Worker │             │
│                                            └──────────────┘             │
└─────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Phase-by-Phase Deep Dive

### Phase 1: Diff Intake & Context Hygiene
* **Primitive:** Deterministic Rust CLI (`quickloop diff <pr>`)
* **Target Latency:** `< 400 ms` (network-bound on `gh`; `< 15 ms` worker cold start)
* **Purpose:** Extract, sanitize, and bound the PR diff before any token enters the LLM context.

Piping raw `gh pr diff` output into an LLM wastes thousands of input tokens on lockfiles, compiled bundles, vendor directories, and binary markers. Because Kimi K3.1 Pro's 24 DeepSeek Sparse Attention (DSA) layers maintain a KV cache that grows with context length, bloated diffs directly reduce concurrent request capacity and burn uncached TPM allowance.

`quickloop diff` executes deterministically in Rust:
1. **Path exclusion:** Strips `**/*.lock`, `dist/**`, `vendor/**`, `**/generated/**`, and binary diffs.
2. **Bounded hunk context:** Retains 3 lines of surrounding context per hunk (full file slices are fetched only if two patch attempts fail on the same hunk).
3. **Compact single-line JSON:** Emits whitespace-free JSON so formatting characters never waste prefix tokens.

```json
{"pr":42,"base_sha":"a1b2c3d","head_sha":"e4f5g6h","test_file_count":6,"files":[{"path":"src/score.rs","hunks":[{"old_start":18,"new_start":18,"patch":"@@ -18,4 +18,4 @@\n- let decay = age_days / 30;\n+ let decay = age_days / 0;"}]}]}
```

---

### Phase 2: Initial Review & Severity Triage
* **Primitive:** OpenClaw Skill (`requesting-code-review`) + Kimi K3.1 Pro (`reasoning_effort: low`)
* **Token Cap:** `max_tokens: 1,500`
* **Purpose:** Classify defects into actionable buckets and decide whether autonomous remediation should run.

On Cerebras, Kimi K3.1 Pro runs with reasoning **off by default** and enables deliberation via `reasoning_effort`. QuickLoop pays for `reasoning_effort: low` **once** during Phase 2—where multi-file logic and security triage benefit from deliberation—so every subsequent patch iteration can run with reasoning completely off.

**Severity Buckets:**
* **`CRITICAL`:** Logic defects, security vulnerabilities (injections, auth bypasses), data loss risks, broken contracts, or build/test failures. **Drives the autonomous fix loop.**
* **`MINOR`:** Variable naming nits, formatting preferences, non-load-bearing duplication, or comment style. **Reported in the final PR summary, never auto-patched.**
* **`ARCHITECTURAL`:** Cross-cutting design decisions requiring human judgment. **Immediately halts execution with `NEEDS_HUMAN`.**

```json
{
  "findings": [
    {
      "id": "F-01",
      "severity": "CRITICAL",
      "file": "src/score.rs",
      "line": 19,
      "summary": "Division by zero when computing decay factor",
      "evidence": "age_days / 0 panics at runtime"
    },
    {
      "id": "F-02",
      "severity": "MINOR",
      "file": "src/score.rs",
      "line": 14,
      "summary": "Variable name `d` could be more descriptive",
      "evidence": "Single-letter binding in public helper"
    }
  ]
}
```

---

### Phase 3: Minimal Patch Synthesis (Hot Path)
* **Primitive:** OpenClaw Skill (`receiving-code-review`) + Kimi K3.1 Pro (`reasoning: OFF`)
* **Token Cap:** `max_tokens: 800`
* **Purpose:** Generate minimal, byte-exact `SEARCH/REPLACE` blocks with zero prose overhead.

This phase is QuickLoop's primary latency lever. Decode on Kimi K3.1 Pro is memory-bandwidth bound (~104B active parameters per generated token). Emitting a ~400-token `SEARCH/REPLACE` block instead of a ~5,000-token full-file rewrite cuts decode time by ~5–10×.

**Strict Output Contract:** Any text outside a fenced `patch` block is treated as a protocol violation:

````markdown
```patch path=src/score.rs
<<<<<<< SEARCH
    let decay = age_days / 0;
=======
    let decay = (age_days as f64 / 30.0).floor() as u32;
>>>>>>> REPLACE
```
````

**Queue-Aware Disjoint-File Fan-Out (Optional):**
When Phase 2 identifies independent `CRITICAL` findings across disjoint files (`file_A != file_B`), OpenClaw can issue concurrent patch calls per file (capped at `4` concurrent calls):
* All parallel calls share the exact same cached prefix, keeping marginal uncached input cost near zero.
* Wall-clock latency becomes $\max(\text{call}_i)$ rather than $\sum \text{call}_i$.
* Patches are still applied sequentially and atomically by the worker once responses land.
* **Automatic Backpressure:** If Cerebras returns `HTTP 429` or `time_info.queue_time > 2 s` during peak hackathon load, fan-out degrades automatically to serial mode.

---

### Phase 4: Atomic Patch Application & Deterministic Verification
* **Primitive:** Deterministic Rust CLI (`quickloop patch` + `quickloop verify`)
* **Target Latency:** `~20 ms` (`patch`) + `< 300 ms` (`verify`)
* **Purpose:** Apply edits safely, enforce anti-cheat rules, and compute an objective quality score.

#### 4.1 Atomic Patch Application (`quickloop patch <file>`)
* **Byte-exact single-match check:** Every `SEARCH` block must match the target file byte-for-byte **exactly once**. Zero matches or $> 1$ ambiguous matches abort the entire patch set atomically (exit code `2`).
* **Bounded retry:** A rejected patch gets at most **1** retry with the exact rejection reason. A second rejection triggers a targeted file-slice re-read rather than blind guessing.
* **No file creation/deletion:** `quickloop patch` cannot create or delete files.

#### 4.2 Concurrent Verification & Quality Score (`quickloop verify`)
`quickloop verify` spawns four independent checks concurrently using `std::process::Command` (avoiding heavy async runtimes to preserve `< 15 ms` startup) and **always exits `0`** when the verifier itself runs—treating a failing score as structured loop data rather than a process crash:

$$\text{score} = 40 \cdot S_{\text{test}} + 30 \cdot S_{\text{lint}} + 20 \cdot S_{\text{sec}} + 10 \cdot S_{\text{scope}}$$

| Term | Weight | Evaluation Rule | Failure Behavior |
| :--- | :--- | :--- | :--- |
| **$S_{\text{test}}$** | `40` | Binary (`1.0` iff 100% of configured tests pass) | `0.0`; captures concise failing test output |
| **$S_{\text{lint}}$** | `30` | Binary (`1.0` iff linter exits clean at configured severity) | `0.0`; captures linter diagnostics |
| **$S_{\text{sec}}$** | `20` | Binary (`1.0` iff static security scan finds zero new issues) | `0.0`; captures matched security pattern |
| **$S_{\text{scope}}$** | `10` | Binary (`1.0` iff zero protected paths/tests were edited or deleted) | `0.0`; **reverts patch immediately & exits `BLOCKED`** |

#### 4.3 Structural Anti-Cheat Enforcement ($S_{\text{scope}}$)
`quickloop verify` cross-checks `git diff --stat` and the current test-file count against Phase 1's baseline:
* Any modification or deletion matching `scope.protected` (`tests/**`, `spec/**`, `**/*_test.*`) sets $S_{\text{scope}} = 0$.
* The worker immediately reverts all working-tree changes from that iteration and terminates with **`BLOCKED`** (enforced in every mode, including `--dry-run`).

#### 4.4 Replaceable Failure Tail
If $S_{\text{scope}} = 1$ and $\text{score} < 100$, the concise failure output **replaces** the previous iteration's failure block rather than appending to history, keeping DSA KV-cache usage flat across iterations 1–5.

---

### Phase 5: Idempotent Publish & Honest Telemetry
* **Primitive:** Deterministic Rust CLI (`quickloop publish <pr>`)
* **Purpose:** Commit verified changes, upsert a single PR status comment, and write API-sourced telemetry.

When $\text{score} = 100$:
1. **Commit & Push:** Commits patched files with an iteration trailer (`QuickLoop-Iteration: <n>`) and pushes to the PR branch (skipped under `--dry-run`).
2. **Single Upserted PR Comment:** Creates or updates one structured status comment (never spamming duplicates), listing gate state, fixed `CRITICAL` issues, and preserved `MINOR` suggestions.
3. **Telemetry Record (`.quickloop/run-<id>.json`):** Persists verified latency, queue time, cache hit rate, and token throughput.

---

## 5. Three Nested Control Loops

QuickLoop bounds autonomy at three levels so runaway retries never flood shared hackathon queues or exhaust rate limits:

| Loop Level | Scope | Trigger | Hard Cap & Recovery |
| :--- | :--- | :--- | :--- |
| **Inner Loop** | Single hunk application | `SEARCH` block fails byte-exact match | Max **1** retry with mismatch reason; then re-read file slice |
| **Middle Loop** | Disjoint-file patch fan-out | Independent `CRITICAL` files in Phase 2 | Max **4** concurrent requests; degrades to serial on `429` or `queue_time_s > 2` |
| **Outer Loop** | Full fix → verify cycle | $\text{score} < 100$ with $S_{\text{scope}} = 1$ | Max **5** iterations (`max_iterations`); exits `NEEDS_HUMAN` on ceiling |

---

## 6. Hardware Alignment, Prompt Cache & Token Budget

### Verified Trial Quota (`moonshotai-kimi-k3.1-pro-trial`)
* **Context / Max Output:** `262,144` / `40,960` tokens
* **Requests:** `1,000 / min` · `60,000 / hr` · `1,440,000 / day`
* **Total Tokens:** `1,000,000 / min`
* **Uncached Tokens:** **`100,000 / min`** *(the binding constraint for rapid loops)*
* **Images:** up to `10` per request

### Cerebras WSE + Kimi K3.1 Pro Physics
* **44 GB on-wafer SRAM & pipeline parallelism:** Layers are chained across wafers so activations move with minimal inter-stage latency. Larger microbatches raise aggregate throughput at the expense of per-user latency, and shared queueing (`time_info.queue_time`) can spike under heavy concurrent agent loops.
* **Hybrid Attention (69 KDA + 24 DSA):** KDA layers keep constant hidden-state memory, while DSA layers select top-2,048 tokens with an indexer but still grow their KV cache linearly with context length. Keeping context small and replacing failure logs directly preserves capacity.

### Message Array Memory Layout

```text
┌──────────────────────────────────────────────────────────────────────┐
│  IMMUTABLE BYTE-STABLE PREFIX (Cached on Cerebras · 0 uncached TPM)  │
│  ──────────────────────────────────────────────────────────────────  │
│  [1] System Contract & SKILL.md Rules                    (~1,800 tok)│
│  [2] Pinned Tool Schemas: `exec` + `read` ONLY             (~700 tok)│
│  [3] Filtered PR Diff + Phase 2 CRITICAL Findings        (~2,500 tok)│
├──────────────────────────────────────────────────────────────────────┤
│  REPLACEABLE DYNAMIC TAIL (Overwritten every iteration · ~600 tok)   │
│  ──────────────────────────────────────────────────────────────────  │
│  [4] Latest `quickloop verify` failure output (or patch reject note) │
└──────────────────────────────────────────────────────────────────────┘
```

**Four Cache-Preservation Rules:**
1. Zero dynamic variables (timestamps, iteration counters, UUIDs, timers) above section `[4]`.
2. OpenClaw tool policy restricted strictly to `exec` and `read`.
3. OpenClaw session pruning drops superseded tool logs between turns.
4. Every run asserts `cached_tokens > 0` on iteration $\ge 2$; a `cache_hit_rate < 80%` is flagged as a prefix-mutation bug.

---

## 7. LLM vs. Deterministic Code Assignment

| Step | Execution Primitive | Rationale |
| :--- | :--- | :--- |
| PR diff fetch & noise stripping | **Rust Code** (`quickloop diff`) | Eliminates lockfiles/binaries before inference |
| Severity triage (`CRITICAL` / `MINOR` / `ARCHITECTURAL`) | **LLM** (`reasoning_effort: low`) | Semantic bug and security reasoning |
| Minimal patch synthesis (`SEARCH/REPLACE`) | **LLM** (`reasoning: OFF`) | Fast, low-token structured transcription |
| Hunk matching & atomic file write | **Rust Code** (`quickloop patch`) | Exact single-match safety; zero shell-injection risk |
| Test, lint, security, and scope checks | **Rust Code** (`quickloop verify`) | Concurrent subprocesses (`< 300 ms`); objective exit codes |
| Quality score & merge gate decision | **Rust Code** (`quickloop verify`) | Pure mathematical function; model cannot talk past gate |
| Anti-cheat revert on test modification | **Rust Code** (`quickloop verify`) | Structural protection against test deletion |
| Git commit, push, PR comment, telemetry | **Rust Code** (`quickloop publish`) | Idempotent publishing and honest API-sourced telemetry |

---

## 8. Inter-Component Data Flow

| Edge | Format | Why |
| :--- | :--- | :--- |
| `quickloop diff` → OpenClaw | **Compact single-line JSON** | Minimal prefix tokens; zero formatting waste |
| OpenClaw → Cerebras (Phase 2) | **Prompt + filtered diff** | Semantic classification with `reasoning_effort: low` |
| Cerebras → OpenClaw (Phase 2) | **Structured JSON findings** | Deterministic routing (`ARCHITECTURAL` / `CRITICAL` / `MINOR`) |
| Cerebras → `quickloop patch` | **Fenced `SEARCH/REPLACE` blocks** | ~400 output tokens; exact single-match verification |
| `quickloop verify` → OpenClaw | **Compact JSON score + failures** | Replaces dynamic tail; exit code `0` even on score `< 100` |
| `quickloop publish` → GitHub | **Git push + upserted comment** | Single clean PR status thread with telemetry summary |

---

## 9. Honest Telemetry & Measurement Contract

All performance claims are sourced strictly from Cerebras API `usage` fields and monotonic local clocks—never tokenizer estimates. Separating `queue_time_s` from `gen_s` prevents misattributing shared hackathon congestion to model decode speed.

| Metric | Formula / Source | Purpose |
| :--- | :--- | :--- |
| `queue_time_s` | `time_info.queue_time` (when returned by API) | Isolates server-side queue wait from model execution |
| `ttft_s` | $t_{\text{first\_content\_token}} - t_{\text{request\_sent}}$ | Measures prefill + queue + network latency |
| `gen_s` | $t_{\text{last\_token}} - t_{\text{first\_content\_token}}$ | Pure decode duration |
| `output_tps` | $\text{usage.completion\_tokens} / \text{gen\_s}$ | Generation-phase throughput |
| `e2e_tps` | $\text{usage.completion\_tokens} / (\text{ttft\_s} + \text{gen\_s})$ | Caller-experienced end-to-end throughput |
| `cache_hit_rate` | $\text{cached\_tokens} / \text{prompt\_tokens}$ | Verifies byte-stable prefix effectiveness (`> 80%` target) |
| `wall_s` | $t_{\text{gate\_decision}} - t_{\text{skill\_invoked}}$ | Full wall-clock time including `git`, `gh`, patch, verify |

---

## 10. Failure Modes & Recovery Matrix

| Failure Condition | Detection Mechanism | Automated Recovery |
| :--- | :--- | :--- |
| `SEARCH` block not found | Byte-level match in `quickloop patch` | Atomic abort (exit `2`); retry once with mismatch reason, then re-read slice |
| Ambiguous `SEARCH` block (`> 1` match) | Match counter in `quickloop patch` | Atomic abort; request wider surrounding context lines |
| Protected test/file edited | Glob & test-count diff in `quickloop verify` | Revert iteration patch immediately; exit `BLOCKED` |
| Prefix cache collapse (`< 80%` on iter $\ge 2$) | `usage.prompt_tokens_details.cached_tokens` | Emit telemetry warning to pinpoint accidental prefix mutation |
| Rate limit (`429`) or queue spike | HTTP status & `time_info.queue_time` | Exponential backoff; disable parallel fan-out, fall back to serial |
| Model emits prose instead of patch fence | Parser finds zero valid `patch` blocks | Single corrective retry enforcing protocol; `NEEDS_HUMAN` on second failure |
| Iteration ceiling reached (`> max_iterations`) | Loop counter | Exit `NEEDS_HUMAN` and post structured findings + diagnostics |

---

## 11. Source Code Layout

```text
quickloop-patch-review/
├── README.md                      # Project overview, badges, quick start, honest telemetry
├── CONTRIBUTING.md                # Contributor setup, checks, and benchmark PR rules
├── LICENSE                        # Open-source license (MIT or MIT OR Apache-2.0)
├── AGENTS.md                      # Engineering contract for AI coding agents (GLM 5.3 / ZCode)
├── CLAUDE.md                      # Claude Code quick-reference pointing to AGENTS.md
├── .env.example                   # Template for CEREBRAS_API_KEY (never commit .env)
├── docs/
│   ├── ARCHITECTURE.md            # This document
│   ├── SPEC.md                    # Requirements, quality gate, acceptance criteria
│   ├── DESIGN.md                  # Speed levers, hardware physics, token budget, BL-01..BL-05
│   └── PLAN.md                    # Hackathon schedule, scope-cut order, video plan
├── config/
│   └── openclaw.example.json5     # Cerebras provider template (SecretRef key ref + agent model default)
├── skills/
│   ├── requesting-code-review/
│   │   └── SKILL.md               # Phase 2 initial review & severity triage skill
│   ├── receiving-code-review/
│   │   └── SKILL.md               # Phase 3 minimal SEARCH/REPLACE patch skill
│   └── looping-code-review/
│       └── SKILL.md               # Closed-loop orchestrator skill
├── worker/                        # Native Rust CLI (`quickloop`, < 15 ms cold start)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs                # CLI entrypoint (`diff`, `patch`, `verify`, `publish`)
│       ├── diff.rs                # PR diff extraction & noise filtering + loop baseline
│       ├── patch.rs               # Atomic SEARCH/REPLACE parser & applier
│       ├── verify.rs              # Concurrent test/lint/sec/scope runner & scorer
│       ├── publish.rs             # Gate re-check, commit/push & idempotent PR comment upsert
│       ├── telemetry.rs           # Worker-measured run records (.quickloop/run-<pr>.json)
│       ├── gh.rs                  # Fixed-args `gh` CLI wrapper (no shell composition)
│       └── git.rs                 # Fixed-args `git` CLI wrapper (no shell composition)
├── demo-repo/                     # Reproducible target repo with planted bugs & MINOR nit
└── bench/                         # Baseline suite (BL-01..BL-05) & TPS Bounty artifacts
```