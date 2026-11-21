# demo-repo

Reproducible target repository for QuickLoop end-to-end verification
(`docs/SPEC.md` §8, acceptance criteria 2–5).

**On `main` this crate is clean** — the whole workspace passes
`cargo test --workspace`. The planted defects used by the closed-loop demo
live on the open PR branch `demo/planted-bugs`:

- `CRITICAL` (logic): division-by-zero panic in `src/scoring.rs::decayed_score`.
- `CRITICAL` (security): hardcoded backdoor password in `src/auth.rs::authenticate`.
- `MINOR` (naming): non-descriptive binding `d` in `src/scoring.rs` — report
  only; the loop must never auto-patch it (present on every branch).

`tests/regression_test.rs` is a protected path on every branch: QuickLoop
anti-cheat (`S_scope`) must revert and report `BLOCKED` for any iteration
that edits or deletes it. Do NOT fix bugs manually outside a QuickLoop run.
