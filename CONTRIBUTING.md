# Contributing to `quickloop-patch-review`

Thank you for your interest in contributing to **`quickloop-patch-review`**! This project is a closed-loop PR review, patch, and verification agent built on **OpenClaw**, powered by **Kimi K3.1 Pro** on **Cerebras**, and enforced by a deterministic **Rust CLI worker (`quickloop`)**.

Because our core claim is **deterministic correctness paired with honest, reproducible speed**, every contribution—whether code, skill prompts, or documentation—follows the engineering standards below.

---

## 1. Before You Start

Please read these core documents to understand the invariants and design constraints:

> **Note:** The full agent-facing engineering contract (`AGENTS.md`) and its quick reference (`CLAUDE.md`) are maintained locally by the maintainer and intentionally excluded from the public repository. The public contract lives in the documents below.

1. [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md) — 4-layer AI architecture, pipeline data flow, and the `quickloop` CLI contract.
2. [`docs/SPEC.md`](./docs/SPEC.md) — Functional/non-functional requirements, quality gate, and acceptance criteria.
3. [`docs/DESIGN.md`](./docs/DESIGN.md) — Speed levers, Cerebras/Kimi hardware physics, token budget, and baseline benchmark suite (`BL-01`–`BL-05`).
4. [`docs/PLAN.md`](./docs/PLAN.md) — Hackathon deadline and scope-cut order.

---

## 2. Local Development Setup

### Prerequisites
- **Rust** 1.75+ (`cargo`, `clippy`, `rustfmt`)
- **Git** and **GitHub CLI** (`gh`, authenticated via `gh auth login`)
- **OpenClaw** CLI

> **Security & Isolation Recommendation:** When running autonomous review loops locally, we strongly recommend developing inside an isolated workspace, container, or lightweight VM (such as **LXD**) so filesystem and git operations stay cleanly sandboxed.

### Clone & Build
```bash
git clone https://github.com/<your-username>/quickloop-patch-review.git
cd quickloop-patch-review

# Build and install the `quickloop` Rust worker
cargo install --path worker
quickloop --version

# Install OpenClaw skills
cp -r skills/* ~/.openclaw/skills/
```

### Environment & Provider Configuration
1. Copy the template environment file and add your key locally (**never commit `.env`**):
   ```bash
   cp .env.example .env
   ```
2. Merge [`config/openclaw.example.json5`](./config/openclaw.example.json5) into `~/.openclaw/openclaw.json`.
3. **Hackathon API Rule:** If you are using a hackathon trial key for `moonshotai-kimi-k3.1-pro-trial`, all requests must run through the **OpenClaw** harness. For offline rehearsal or when testing worker logic without hitting the trial endpoint, you may switch the model ID in config to `gpt-oss-120b`.

---

## 3. Repository Structure & Where to Make Changes

```text
quickloop-patch-review/
├── worker/            # Rust CLI (`diff`, `patch`, `verify`, `publish`, telemetry)
├── skills/            # OpenClaw SKILL.md definitions (review, patch, loop)
├── config/            # Version-controlled OpenClaw JSON5 template
├── demo-repo/         # Target repo with intentional bugs & MINOR nit (do not fix manually!)
├── bench/             # Baseline suite (BL-01..BL-05) & TPS Bounty scripts
└── docs/              # ARCHITECTURE.md, SPEC.md, DESIGN.md, PLAN.md
```

**Important Coupling Rule:** `worker/` and `skills/*/SKILL.md` are coupled through the CLI JSON contract documented in [`docs/ARCHITECTURE.md`](./docs/ARCHITECTURE.md). If your PR changes CLI flags, JSON output fields, or exit codes, you **must** update both sides in the same PR.

---

## 4. Coding & Design Standards

### Rust Worker (`worker/`)
- Rust **Edition 2021**; format with `cargo fmt` and pass `cargo clippy -- -D warnings`.
- Keep dependencies minimal (`clap`, `serde`, `std::process::Command`) to preserve the **`< 15 ms` cold-start target**. Do not introduce heavy async runtimes (e.g., Tokio) just to spawn the four concurrent verification subprocesses.
- Return `Result` with clear context; avoid `.unwrap()` outside `#[cfg(test)]`.
- Emit **compact, single-line JSON** on `stdout` (whitespace wastes LLM context tokens).
- Keep `quickloop verify` returning **exit code `0` even when `score < 100`** (a failing score is normal loop data; reserve non-zero exit codes for verifier execution crashes).

### OpenClaw Skills (`skills/*/SKILL.md`)
- Keep static instructions at the very top in a byte-stable order.
- **Never** interpolate iteration counters, timestamps, run UUIDs, or elapsed timers above the dynamic failure tail—a single mutating prefix token breaks Cerebras prefix caching and burns the **100K uncached TPM** budget.
- Keep patch-phase outputs strictly to `SEARCH/REPLACE` fences (no prose, no whole-file rewrites).

### Security & Anti-Cheat (`S_scope`)
- Never add a flag or code path that bypasses `S_scope` protected-path verification, including in `--dry-run` or test modes.
- Never commit `.env`, API keys, tokens, or `.quickloop/` telemetry logs containing private paths.

### Documentation & Claims
- All repository documentation, code comments, commit messages, and PR descriptions must be in **English**.
- In `docs/DESIGN.md`, tag vendor-behavior claims as **[V]** (verified with citation), **[P]** (probable), or **[U]** (unverified judgment). Do not upgrade **[U]** to **[V]** without a cited source or measured run.

---

## 5. Required Verification Before Opening a PR

Run all local checks and paste the actual output in your PR description:

```bash
cargo fmt --manifest-path worker/Cargo.toml -- --check
cargo clippy --manifest-path worker/Cargo.toml -- -D warnings
cargo test --manifest-path worker/Cargo.toml
cargo build --manifest-path worker/Cargo.toml --release
```

### End-to-End Verification (When Touching Loop, Skills, or Verifier)
Run the loop against `demo-repo/` and verify all four conditions:
- [ ] `READY_TO_MERGE` is reached with $\text{score} = 100$.
- [ ] The planted `MINOR` naming nit remains untouched (proves severity filtering works).
- [ ] A planted test-deletion attempt reverts cleanly and returns `BLOCKED` (proves $S_{\text{scope}}$ works).
- [ ] `cached_tokens > 0` on iteration 2 (proves the prompt prefix is byte-stable).

### Performance & Latency PRs
If your PR modifies prompts, caching, context filtering, or request concurrency, include **before/after** numbers from the baseline suite (`BL-01`–`BL-05` in [`docs/DESIGN.md`](./docs/DESIGN.md)):
- Report **`queue_time_s`**, **`ttft_s`**, **`output_tps`**, **`e2e_tps`**, **`cache_hit_rate`**, and **`wall_s`**.
- Source all token counts strictly from API `usage` fields (never tokenizer estimates).
- Explicitly label **cold-cache** vs **warm-cache** runs.

---

## 6. Pull Request & Issue Process

1. **Keep PRs focused:** One logical change per PR (avoid bundling unrelated refactors with bug fixes).
2. **Branch naming:** Use descriptive branches such as `feat/worker-verify-sec`, `fix/patch-exact-match`, or `docs/architecture-update`.
3. **Commit messages:** Follow Conventional Commits (e.g., `feat(worker): enforce atomic rollback on patch mismatch`, `fix(skills): stabilize review prefix`).
4. **Bug reports:** Include OS, Rust version, OpenClaw version, exact command run, and sanitized telemetry JSON (with API keys stripped).

---

## 7. License

By contributing to `quickloop-patch-review`, you agree that your contributions will be licensed under the repository's open-source license (see [`LICENSE`](./LICENSE)).