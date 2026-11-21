---
name: requesting-code-review
description: Phase 2 initial PR review and severity triage (CRITICAL / MINOR / ARCHITECTURAL) via Kimi K3.1 Pro on Cerebras.
---

# Requesting Code Review

## Protocol (static, byte-stable — never reorder)
1. Input is the compact single-line JSON emitted by `quickloop diff <pr>`.
2. Call the model with `reasoning_effort: low`, `max_tokens: 1500`, `stream: true`.
3. Classify every defect into exactly one bucket:
   - `CRITICAL`: logic defects, security vulnerabilities, data loss, broken contracts, failing build/tests. Drives the fix loop.
   - `MINOR`: naming, formatting, style, non-load-bearing duplication. Report only; never auto-patched.
   - `ARCHITECTURAL`: cross-cutting design decisions requiring human judgment. Halts with `NEEDS_HUMAN`.
4. Output STRICT JSON matching the contract below. Violating this output format is a protocol error.

## Output contract
`{"findings":[{"id":"F-01","severity":"CRITICAL","file":"<path>","line":<int>,"summary":"<one line>","evidence":"<short quote>"}]}`

## Hard rules
- Never modify files, run git commands, or decide merge readiness. All side effects belong to the `quickloop` worker.
- Never invent file paths or line numbers absent from the diff JSON.
- Never let `MINOR` findings drive patch generation; they are reported in the final summary only.
