<div align="center">

# quickloop-patch-review

**Deterministic closed-loop PR review, minimal patching, and verification agent for OpenClaw — powered by Kimi K3.1 Pro on Cerebras.**

<p>
  <a href="./LICENSE">
    <img src="https://img.shields.io/badge/License-Apache_2.0-blue.svg?style=flat-square" alt="License: Apache-2.0" />
  </a>
  <a href="https://github.com/openclaw/openclaw">
    <img src="https://img.shields.io/badge/Harness-OpenClaw-6366F1?style=flat-square" alt="Harness: OpenClaw" />
  </a>
  <a href="https://www.cerebras.ai/">
    <img src="https://img.shields.io/badge/Inference-Cerebras_WSE-F59E0B?style=flat-square" alt="Inference: Cerebras" />
  </a>
  <a href="https://www.moonshot.ai/">
    <img src="https://img.shields.io/badge/Model-Kimi_K3.1_Pro-10B981?style=flat-square" alt="Model: Kimi K3.1 Pro" />
  </a>
  <a href="./worker">
    <img src="https://img.shields.io/badge/Worker-Rust_CLI_(%3C15ms)-DEA584?style=flat-square&logo=rust&logoColor=black" alt="Worker: Rust CLI" />
  </a>
  <a href="./bench">
    <img src="https://img.shields.io/badge/Telemetry-API_Sourced_TPS-0EA5E9?style=flat-square" alt="Honest Telemetry" />
  </a>
</p>

<p>
  <a href="#demo">Demo</a> •
  <a href="#benchmarks-and-honest-tps-telemetry">Benchmarks</a> •
  <a href="#workflow">Workflow</a> •
  <a href="#layered-ai-architecture">Architecture</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#documentation">Documentation</a>
</p>

</div>

---

> **Summary:** Comment-only AI code reviewers leave the slowest part of a Pull Request—editing files, re-running tests, and verifying scope—entirely manual. **QuickLoop** closes the loop autonomously inside OpenClaw, emitting minimal `SEARCH/REPLACE` patches at wafer-scale decode speed and enforcing a deterministic Rust merge gate (`score == 100`) with structural anti-cheat protection (`S_scope`).

> ⚠️ **Pre-release.** Measured figures below (`TBD`) will be filled from real Cerebras API runs once the acceptance criteria in [`docs/SPEC.md`](./docs/SPEC.md) pass.

---

## Demo

<!--
OPTION A (Recommended for GitHub): Drag and drop your .mp4 recording into any GitHub Issue or PR comment box,
copy the generated `https://github.com/user-attachments/assets/<uuid>` URL, and paste it on its own line below.
GitHub automatically renders an inline native video player.
-->

<div align="center">

https://github.com/user-attachments/assets/REPLACE_WITH_YOUR_DEMO_VIDEO_ID

<!--
OPTION B (If hosted externally): Replace Option A with a linked preview image:
<a href="https://youtu.be/YOUR_VIDEO_ID">
  <img src="./docs/assets/demo-thumbnail.png" alt="60-Second Real-Time Demo" width="85%" />
</a>
-->

<sub><b>60-second real-time run</b> on <code>demo-repo/</code> (PR #5): triaging <code>CRITICAL</code> defects, preserving the <code>MINOR</code> naming finding, verifying <code>score == 100</code>, demonstrating anti-cheat <code>BLOCKED</code> enforcement on test deletion, and publishing API-sourced telemetry.</sub>

</div>

---

## Why QuickLoop?

| Capability | Comment-Only AI Reviewers | Naive Autonomous Loops | QuickLoop |
| :--- | :---: | :---: | :---: |
| Automatically fixes and verifies `CRITICAL` defects | No | Unbounded retries | Bounded loop (default max 5) |
| Merge readiness decided by deterministic exit codes | No | LLM self-reports | Rust gate (`score == 100`) |
| Prevents deleting or weakening failing tests (`S_scope`) | N/A | Vulnerable | Immediate revert and `BLOCKED` |
| Avoids bikeshedding `MINOR` style and naming findings | Noisy comments | Infinite churn | Reported only, never patched |
| Byte-stable prefix caching (`> 80%` cache hit rate) | No | History appended | Static prefix with replaced tail |
| Honest API telemetry (`queue`, `output_tps`, `e2e_tps`) | No | Often omitted | Logged per run from API fields |

---

## Benchmarks and Honest TPS Telemetry

Every performance figure in this repository is recorded directly from Cerebras API `usage` / `time_info` fields and local monotonic clocks into `.quickloop/run-<id>.json`—never estimated with a local tokenizer. Server queueing (`queue_time_s`), pure decode rate (`output_tps`), and caller-experienced throughput (`e2e_tps`) are reported separately across **3–5 repeated runs** to demonstrate both speed and run-to-run consistency.

### Engineering Targets vs. Measured Hardware and Worker Overhead

| Component | Target | Measured (Median) | Notes |
| :--- | :--- | :--- | :--- |
| Rust worker cold start (`quickloop --version`) | `< 15 ms` | `TBD ms` | Minimal dependencies (`clap`, `serde`, `std::process::Command`) |
| Diff extraction and noise filtering (`quickloop diff`) | `< 400 ms` | `TBD ms` | Strips lockfiles, `dist/`, `vendor/`, and binary diffs |
| Concurrent 4-check gate (`quickloop verify`) | `< 300 ms` | `TBD ms` | Runs `test`, `lint`, `sec`, and `scope` checks concurrently |
| Prefix cache hit rate (Iteration >= 2) | `> 80%` | `TBD %` | Keeps usage well inside the `100K` uncached TPM ceiling |
| Full 3-iteration closed loop (`wall_s`) | `< 10 s` (`< 15 s` gate) | `TBD s` | Excludes external queue spikes (logged via `queue_time_s`) |

### Visual Proof: Throughput and Run-to-Run Consistency

<div align="center">
  <img src="./docs/assets/benchmark-tps.png" alt="QuickLoop TPS and Latency Consistency Benchmark" width="90%" />
  <br />
  <sub><i>Measured across repeated runs on <code>moonshotai-kimi-k3.1-pro-trial</code> via OpenClaw and Cerebras WSE. See <a href="./bench/tps-bounty.md">bench/tps-bounty.md</a> for raw JSON logs and methodology.</i></sub>
</div>

### Baseline Suite (`BL-01`–`BL-05`) — Median and Consistency Range (N = 5 runs)

| Suite | Scenario | Cache State | `queue_time_s` (Med) | `ttft_s` (Med) | `output_tps` (Med / Min–Max) | `e2e_tps` (Med) | `cache_hit_rate` |
| :--- | :--- | :---: | :---: | :---: | :---: | :---: | :---: |
| **BL-01** | Sanity short reply (`< 500` in -> `100–150` out) | Cold / Warm | `TBD` | `TBD` | `TBD` (`TBD–TBD`) | `TBD` | `TBD` |
| **BL-02** | Medium code generation (`1K–4K` in -> `800–1.2K` out) | Warm | `TBD` | `TBD` | `TBD` (`TBD–TBD`) | `TBD` | `TBD` |
| **BL-03** | Single-step repo diff and `SEARCH/REPLACE` patch | Warm | `TBD` | `TBD` | `TBD` (`TBD–TBD`) | `TBD` | `TBD` |
| **BL-04** | Multi-step loop (>= 3 turns, replaced failure tail) | Cold -> Warm | `TBD` | `TBD` | `TBD` (`TBD–TBD`) | `TBD` | `> 80% (TBD)` |
| **BL-05** | Disjoint parallel fan-out (`concurrency = 2–4`) | Warm | `TBD` | `TBD` | `TBD` (`TBD–TBD`) | `TBD` | `TBD` |

### Sample Run Output (`demo-repo/` PR #5)

```text
QuickLoop · PR #5 · READY_TO_MERGE
  iterations        3  (iter 0 review: cold · iter 1–2 patch: warm cache)
  wall clock        TBD s
  inference         TBD s   (output TBD tok/s · e2e TBD tok/s · TTFT TBD s · queue TBD s)
  tokens            in TBD (cached TBD · TBD%) · out TBD
  checks            tests PASS · lint clean · security clean · scope ok (score 100/100)
  preserved nits    1 MINOR finding reported in PR comment (untouched)
```

<details>
<summary><b>Reproducing the Benchmarks</b></summary>

```bash
# Inspect benchmark methodology and TPS Bounty logs
cat bench/tps-bounty.md

# Run local worker latency checks
quickloop diff 5
quickloop verify
```

Full measurement definitions and formulas are documented in [`docs/DESIGN.md`](./docs/DESIGN.md) and [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md).

</details>

---

## Workflow

QuickLoop executes a **5-phase bounded closed loop**: **Diff Intake -> Severity Triage -> Minimal Patch Synthesis -> Concurrent Verification and Anti-Cheat -> Idempotent Publish**.

<div align="center">
  <img src="./docs/assets/workflow.svg" alt="QuickLoop Closed-Loop Workflow Diagram" width="95%" />
</div>

### Deterministic Merge Gate and Anti-Cheat (`S_scope`)

The language model **never** decides whether a PR is ready to merge. Readiness is computed strictly by `quickloop verify` from local subprocess exit codes:

$$\text{score} = 40 \cdot S_{\text{test}} + 30 \cdot S_{\text{lint}} + 20 \cdot S_{\text{sec}} + 10 \cdot S_{\text{scope}}$$

- **`READY_TO_MERGE`** — `score == 100` (all four binary checks pass). Commits, pushes, and upserts **one** structured PR status comment.
- **`NEEDS_HUMAN`** — Any `ARCHITECTURAL` finding on Phase 0 review, or `max_iterations` (default `5`) reached.
- **`BLOCKED`** — Protected test files modified or deleted (`S_scope == 0`). `quickloop` immediately reverts the iteration's patch and halts execution.

---

## Layered AI Architecture

QuickLoop cleanly separates **LLM reasoning and patch synthesis** from **deterministic local execution and guardrails** across four architectural layers:

<div align="center">
  <img src="./docs/assets/architecture.svg" alt="QuickLoop 4-Layer AI Architecture Diagram" width="95%" />
</div>

### Speed and Hardware Alignment (Cerebras WSE + Kimi K3.1 Pro)

Latency optimization is aligned with the physical characteristics of `moonshotai-kimi-k3.1-pro-trial` (2.8T total / ~104B active parameters; 69 KDA fixed-state layers and 24 DSA growing-KV layers) on Cerebras:

| Speed Lever | Architectural Mechanism | Impact |
| :--- | :--- | :--- |
| **Byte-Stable Prompt Prefix** | Zero dynamic variables (counters, timestamps, UUIDs) above the tail | Sustains `> 80%` prefix caching; fits inside the `100K` uncached TPM ceiling |
| **Minimal `SEARCH/REPLACE` Hunks** | Emits ~400 output tokens instead of ~5,000-token full-file rewrites | Cuts memory-bandwidth-bound decode duration by ~5–10x |
| **Asymmetric Reasoning Dial** | `reasoning_effort: low` in Phase 0 review; **OFF** in patch phase | Eliminates redundant reasoning tokens on the hot patch path |
| **Replaceable Failure Tail** | Overwrites prior `verify` failure output rather than appending | Prevents linear KV-cache bloat across Kimi's 24 DSA layers |
| **Pinned Tool Policy + Rust CLI** | OpenClaw restricted to `exec` and `read`; `< 15 ms` native Rust binary | Zero unused tool schema overhead and sub-300 ms local verification |

See [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) for the complete system specification, schemas, nested control loops, and failure recovery matrix.

---

## Quick Start

### Prerequisites
- **Rust 1.75+** (`cargo`)
- **`git`** and **GitHub CLI (`gh`)** authenticated (`gh auth login`)
- **OpenClaw CLI** (`npm i -g openclaw`), gateway running (`openclaw gateway start`)
- **Cerebras API key** with access to `moonshotai-kimi-k3.1-pro-trial`

### 1. Clone and Install the Rust Worker

```bash
git clone https://github.com/<your-username>/quickloop-patch-review.git
cd quickloop-patch-review

# Build and install the `quickloop` binary onto PATH
cargo install --path worker
quickloop --version
```

### 2. Configure Environment and OpenClaw Skills

```bash
# Copy environment template and set CEREBRAS_API_KEY (never commit .env)
cp .env.example .env

# Install the three QuickLoop skills into OpenClaw
cp -r skills/* ~/.openclaw/skills/
```

Register the Cerebras provider with `openclaw config patch` — a validated single write; the key is referenced by env-var name, never stored in the config:

```bash
set -a && source .env && set +a   # make CEREBRAS_API_KEY resolvable for the validated write
openclaw config patch --file config/openclaw.example.json5
```

The template (`config/openclaw.example.json5`) applies:

```json5
{
  models: {
    mode: "merge",
    providers: {
      cerebras: {
        baseUrl: "https://api.cerebras.ai/v1",
        api: "openai-completions",
        apiKey: { source: "env", provider: "default", id: "CEREBRAS_API_KEY" },
        models: [
          {
            id: "moonshotai-kimi-k3.1-pro-trial",
            name: "Kimi K3.1 Pro (Trial)",
            contextWindow: 262144,
            maxTokens: 40960,
          },
        ],
      },
    },
  },
  agents: { defaults: { model: "cerebras/moonshotai-kimi-k3.1-pro-trial" } },
}
```

Setup notes that bite in practice:

- **The gateway process must see `CEREBRAS_API_KEY`.** Export it in your shell profile, or append `export CEREBRAS_API_KEY=...` to `~/.openclaw/service-env/ai.openclaw.gateway.env`, then `openclaw gateway stop --force && openclaw gateway start`.
- **Per-agent model overrides win over defaults.** If your agent pins an explicit `model` (e.g. `agents.entries.main.model`), pin the model per QuickLoop run: `openclaw agent --model cerebras/moonshotai-kimi-k3.1-pro-trial ...`.
- **Thinking levels on the trial model are `off` and `ultra` only.** QuickLoop runs use `--thinking off` (reasoning OFF on the patch path — minimal output tokens).
- **The trial endpoint is shared:** HTTP 429 (`queue_exceeded`) is normal under load — wait and retry.
- For offline rehearsal without calling the trial endpoint, switch the model id to `gpt-oss-120b` in configuration only.

### 3. Connect a Chat Channel (Optional)

QuickLoop works from any OpenClaw session; point the session at a repo checkout and invoke the skills. To drive it from chat, connect a channel (`openclaw channels list --all` shows the catalog). Example — **iMessage** on a Mac signed into Messages:

```bash
openclaw plugins install @openclaw/imessage   # gateway-side plugin
brew install steipete/tap/imsg                # imsg bridge on the Messages Mac
openclaw channels status --probe
```

Grant macOS **Full Disk Access** (chat database) and **Automation** permission (sending via Messages.app) to the process context; approve the first DM via `openclaw pairing approve imessage <CODE>`. Advanced actions (tapbacks, effects, polls) additionally require SIP relaxed — basic send/receive works without it. Full guide: [docs.openclaw.ai/channels/imessage/setup](https://docs.openclaw.ai/channels/imessage/setup).

### 4. Run on a Pull Request

```bash
/requesting-code-review 5            # Review only — structured CRITICAL/MINOR/ARCHITECTURAL findings
/receiving-code-review 5             # Synthesize and apply atomic SEARCH/REPLACE patches
/looping-code-review 5               # Full closed loop until READY_TO_MERGE / NEEDS_HUMAN / BLOCKED
```

From a terminal (no chat channel needed), drive the same loop directly:

```bash
openclaw agent --model cerebras/moonshotai-kimi-k3.1-pro-trial --thinking off \
  -m "/looping-code-review 5"        # add --session-id <id> for a fresh session
```

The publish step supports `quickloop publish <pr> --dry-run` (gate re-check only — zero remote push or comment) and is idempotent: re-running it upserts the single PR status comment instead of spamming.

---

## Repository Structure

```text
quickloop-patch-review/
├── worker/                        # Native Rust CLI (`quickloop`: diff, patch, verify, publish)
├── skills/                        # OpenClaw SKILL.md definitions (review, patch, loop)
├── config/                        # Version-controlled OpenClaw JSON5 provider template
├── demo-repo/                     # Reproducible target repo with planted bugs and MINOR finding
├── bench/                         # Baseline suite (BL-01..BL-05) and TPS Bounty artifacts
├── docs/
│   ├── assets/                    # Exported architecture/workflow diagrams and benchmark charts
│   ├── ARCHITECTURE.md            # 4-layer AI architecture, pipeline deep-dive, and data contracts
│   ├── SPEC.md                    # Functional/non-functional specs, gate, and acceptance criteria
│   ├── DESIGN.md                  # Speed levers, Cerebras/Kimi physics, and token budget
│   └── PLAN.md                    # Hackathon timeline and demo checklist
├── CONTRIBUTING.md                # Contributor setup, checks, and benchmark PR rules
└── LICENSE                        # Apache License 2.0
```

---

## Documentation

| Document | Description |
| :--- | :--- |
| [**`docs/ARCHITECTURE.md`**](./docs/ARCHITECTURE.md) | 4-layer AI system architecture, 5-phase pipeline, control loops, and data flow |
| [**`docs/SPEC.md`**](./docs/SPEC.md) | Functional/non-functional requirements, deterministic quality gate, and acceptance criteria |
| [**`docs/DESIGN.md`**](./docs/DESIGN.md) | Speed engineering, Cerebras WSE + Kimi K3.1 Pro physics, token budgets, and `BL-01`–`BL-05` |
| [**`docs/PLAN.md`**](./docs/PLAN.md) | Hackathon execution plan, scope priorities, and demo structure |
| [**`bench/tps-bounty.md`**](./bench/tps-bounty.md) | Reproducible TPS measurements, warm vs. cold runs, and raw telemetry proof |
| [**`CONTRIBUTING.md`**](./CONTRIBUTING.md) | Isolated dev setup, Rust/skill conventions, and required PR verification checks |

---

## Contributing

Contributions are welcome. Please read [`CONTRIBUTING.md`](./CONTRIBUTING.md) before opening a Pull Request. All PRs must pass:

```bash
cargo fmt --manifest-path worker/Cargo.toml -- --check
cargo clippy --manifest-path worker/Cargo.toml -- -D warnings
cargo test --manifest-path worker/Cargo.toml
cargo build --manifest-path worker/Cargo.toml --release
```

---

## License

Licensed under the **Apache License 2.0**. See [`LICENSE`](./LICENSE) for details.
