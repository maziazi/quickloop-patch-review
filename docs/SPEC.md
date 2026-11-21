# QuickLoop — Specification

**Status:** v0.2 · **Last updated:** 2026-11-22
**Event:** Cerebras × Moonshot AI (Kimi K3.1 Pro) × OpenClaw Weekend Hackathon

---

## 1. Problem Statement
Comment-only AI reviewers relocate the merge bottleneck rather than removing it. Conversely, naive autonomous loops burn minutes on slow inference, bloat context windows across iterations, or delete failing tests to pass checks.

**`quickloop-patch-review` closes the loop deterministically and keeps every fix-verify iteration fast enough to run by default.**

## 2. Scope
- **In scope:** Local Rust CLI worker (`quickloop`), three OpenClaw skills (`requesting-code-review`, `receiving-code-review`, `looping-code-review`), deterministic merge gate (`score = 100`), anti-cheat `S_scope`, and API-sourced telemetry (`queue_time_s`, `ttft_s`, `output_tps`, `e2e_tps`, `cache_hit_rate`).
- **Out of scope (v0.1):** Multi-repo/monorepo review, hosted web UI/GitHub App, architectural redesigns.

## 3. Actors & Division of Authority
- **Developer:** Triggers `/looping-code-review <pr>` in OpenClaw.
- **OpenClaw:** Harness loading skills, managing Cerebras API connection, pinning tool policy (`exec`, `read`), and pruning superseded logs.
- **Kimi K3.1 Pro on Cerebras (`moonshotai-kimi-k3.1-pro-trial`):** Defect triage and minimal `SEARCH/REPLACE` patch synthesis.
- **`quickloop` worker (Rust):** Deterministic executor owning all filesystem, `git`, `gh`, verification, and scope-guard side effects.

## 4. Runtime & Config Inputs
- **Runtime:** PR reference (`42`, `#42`, or URL), `--max-iterations` (default `5`), `--severity` (`critical` default | `all`), `--dry-run`.
- **Optional `.quickloop.toml`** in target repo: `[verify]` (`test`, `lint`, `timeout_secs`), `[scope]` (`protected`, `ignore`), `[loop]` (`max_iterations`, `severity_floor`).

## 5. Outputs & Terminal States
1. **Patched & verified working tree** committed with iteration trailer (skipped on `--dry-run`).
2. **Single upserted PR status comment** (never spammed across re-runs).
3. **Exactly one gate decision:**
   - `READY_TO_MERGE`: `score == 100` and zero failing checks.
   - `NEEDS_HUMAN`: iteration ceiling hit, or any `ARCHITECTURAL` finding.
   - `BLOCKED`: `S_scope == 0` (protected path/test touched) or verifier crash.
4. **Telemetry record** in `.quickloop/run-<id>.json` and stdout summary.

## 6. Deterministic Quality Gate & Anti-Cheat
`score = 40·S_test + 30·S_lint + 20·S_sec + 10·S_scope`
- `S_test`, `S_lint`, `S_sec`, `S_scope` are strictly binary (`1.0` or `0.0`).
- **Severity filter:** Only `CRITICAL` drives the loop; `MINOR` is reported only; `ARCHITECTURAL` exits `NEEDS_HUMAN`.
- **Anti-cheat (`S_scope`):** Comparing `git diff --stat` and test-file count against baseline; modifying or deleting any `scope.protected` path immediately zeroes `S_scope`, reverts the patch, and exits `BLOCKED`.

## 7. Functional & Non-Functional Requirements
- **FR-1..FR-8:** `diff` compact JSON; `patch` atomic single-match `SEARCH/REPLACE`; `verify` concurrent 4-check JSON exiting `0` even on failing score; `publish` idempotent upsert; guaranteed loop termination; telemetry emitted even on failure; `--dry-run` guarantees zero remote writes.
- **NFR-1:** Worker cold start `< 15 ms`.
- **NFR-2:** `quickloop diff` `< 400 ms`.
- **NFR-3:** `quickloop verify` `< 300 ms` on `demo-repo/`.
- **NFR-4:** Prefix cache hit rate `> 80%` on iteration ≥ 2.
- **NFR-5:** Output tokens `< 800` per patch iteration.
- **NFR-6:** Full 3-iteration loop `< 10 s` target (`< 15 s` acceptance) under normal queue conditions (`queue_time_s < 1 s`).
- **NFR-7:** Stays within verified trial limits: **262,144** context / **40,960** max output / **1,000 RPM** / **1,000,000 total TPM** / **100,000 uncached TPM**.

## 8. Acceptance Criteria (Clean Clone)
1. `quickloop --version` runs from release build.
2. `demo-repo/` PR contains 1 logic bug (`CRITICAL`), 1 security bug (`CRITICAL`), 1 naming nit (`MINOR`), and protected tests.
3. `/looping-code-review 42` reaches `READY_TO_MERGE` under 15 s (excluding external server queue spikes, which are logged separately).
4. The `MINOR` naming nit remains untouched.
5. A planted test-deletion attempt triggers `BLOCKED` and reverts cleanly.
6. Telemetry reports API-sourced `output_tps`, `e2e_tps`, `queue_time_s`, and `cached_tokens > 0` on iteration 2.
7. PR contains exactly one upserted QuickLoop comment.
