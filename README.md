# quickloop-patch-review

**Closed-loop PR review and fix agent for OpenClaw, powered by Kimi K3.1 Pro on Cerebras.**

[![version](https://img.shields.io/badge/version-v0.1.0-orange)](./docs/PLAN.md)
[![license](https://img.shields.io/badge/license-Apache--2.0-blue)](./LICENSE)
[![harness](https://img.shields.io/badge/harness-OpenClaw-6366F1)](https://docs.openclaw.ai)
[![model](https://img.shields.io/badge/model-Kimi%20K3.1%20Pro%20on%20Cerebras-16A34A)](https://cloud.cerebras.ai)

> ⚠️ Pre-release. Design targets in this README will be replaced by measured runs once acceptance criteria in [`docs/SPEC.md`](./docs/SPEC.md) pass.

---

## Why

Comment-only AI reviewers relocate the merge bottleneck instead of removing it: you still edit the files, run tests, and request another review. Naive autonomous loops fail in the opposite direction—bloating context, burning uncached TPM, rewriting entire files, or deleting failing tests to pass checks.

**`quickloop-patch-review`** closes the loop with a **deterministic Rust CLI worker (`quickloop`)**, **byte-stable prompt prefix caching**, **minimal `SEARCH/REPLACE` patches**, and **structural anti-cheat enforcement (`S_scope`)**.

## How It Works

```text
/looping-code-review 42
  │
  ├─ quickloop diff 42       → filtered PR diff (lockfiles, dist, binaries stripped)
  ├─ review (Kimi/Cerebras)  → CRITICAL / MINOR / ARCHITECTURAL triage (reasoning_effort: low)
  │
  └─ bounded loop (max 5×):
       ├─ fix (Kimi/Cerebras) → SEARCH/REPLACE blocks only, no prose, reasoning OFF
       ├─ quickloop patch     → atomic single-match apply (any mismatch aborts set)
       ├─ quickloop verify    → concurrent test + lint + security + scope → score (0..100)
       └─ score == 100 ? publish & READY_TO_MERGE : replace failure tail & retry
```

**Deterministic gate:** `score = 40·S_test + 30·S_lint + 20·S_sec + 10·S_scope` (all binary).
`READY_TO_MERGE` comes only from `quickloop verify` returning 100 — never model text.
Protected test files are anti-cheat guarded: touching them reverts the patch and halts `BLOCKED`.

## Quick Start

```bash
# 1. Build and install the worker
cargo install --path worker
quickloop --version

# 2. Configure OpenClaw (merge the template; key comes from CEREBRAS_API_KEY)
cp .env.example .env            # then fill in CEREBRAS_API_KEY (never commit .env)
# merge config/openclaw.example.json5 into ~/.openclaw/openclaw.json

# 3. Install the skills
cp -r skills/* ~/.openclaw/skills/

# 4. Run the closed loop against a PR (inside the target repo checkout)
/looping-code-review 42

# Worker commands are usable standalone:
quickloop diff 42                        # filtered diff JSON + loop baseline
quickloop patch out.patch                # atomic SEARCH/REPLACE apply (exit 2 on reject)
quickloop verify                         # deterministic gate, always exit 0
quickloop publish 42 --dry-run           # gate re-check only, zero remote writes
```