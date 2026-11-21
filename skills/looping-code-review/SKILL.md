---
name: looping-code-review
description: Closed-loop orchestrator — diff, review, patch, verify until the deterministic gate passes (max 5 iterations).
---

# Looping Code Review

## Protocol (static, byte-stable — never reorder)
1. Run `quickloop diff <pr>` to obtain the filtered compact JSON diff.
2. Run Phase 2 review (`requesting-code-review`). Any `ARCHITECTURAL` finding → stop, `NEEDS_HUMAN`. No `CRITICAL` findings → stop, report `READY_TO_MERGE` evidence.
3. Bounded loop (max `--max-iterations`, default 5):
   a. Phase 3 patch synthesis (`receiving-code-review`) for the `CRITICAL` findings.
   b. `quickloop patch <file>` — atomic apply; any rejection aborts the whole set (exit 2).
   c. `quickloop verify` — it always exits 0; a score below 100 is loop data, not a crash.
   d. `score == 100` → `quickloop publish <pr> --iteration <n>` (pass `--dry-run` instead when the caller asked for zero remote writes). Publish re-runs the deterministic gate itself and refuses unverified state; otherwise REPLACE the failure tail with the latest verify output and retry — never append history.
4. Gate decisions in strict priority: `BLOCKED` (`S_scope == 0` or verifier crash) > `NEEDS_HUMAN` (iteration ceiling or `ARCHITECTURAL`) > `READY_TO_MERGE` (`score == 100`).

## Hard rules
- `READY_TO_MERGE` is decided ONLY by `quickloop verify` returning score 100. Model text never decides readiness; violating this is a protocol error.
- Zero dynamic tokens (iteration counters, timestamps, run IDs, timers) above the replaceable failure tail — the prompt prefix must stay byte-stable for Cerebras prefix caching.
- On HTTP 429 or `queue_time_s > 2`, disable fan-out and run serially.
- Publish exactly one upserted PR status comment per run via `quickloop publish`; never spam duplicates.
