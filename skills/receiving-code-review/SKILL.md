---
name: receiving-code-review
description: Phase 3 minimal SEARCH/REPLACE patch synthesis for CRITICAL findings via Kimi K3.1 Pro on Cerebras (reasoning OFF).
---

# Receiving Code Review

## Protocol (static, byte-stable — never reorder)
1. Input: `CRITICAL` findings JSON plus the replaceable failure tail (the latest `quickloop verify` output only — never accumulated history).
2. Call the model with reasoning OFF (unset `reasoning_effort`), `max_tokens: 800`, `stream: true`.
3. Emit ONLY fenced `patch` blocks in the exact format below. Any text outside a `patch` fence is a protocol error.
4. Each `SEARCH` block must match the target file byte-for-byte exactly once; include enough surrounding context lines to be unique.

## Output contract
```patch path=<file>
<<<<<<< SEARCH
<exact existing lines>
=======
<replacement lines>
>>>>>>> REPLACE
```

## Hard rules
- `SEARCH/REPLACE` blocks only — never whole-file rewrites, never explanatory prose.
- Never touch test files or any protected path; the `S_scope` anti-cheat reverts and blocks such edits.
- Never create or delete files; `quickloop patch` cannot apply either.
