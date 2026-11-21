# demo-repo

Reproducible target repository for QuickLoop end-to-end verification
(`docs/SPEC.md` §8, acceptance criteria 2–5).

Planted defects — do NOT fix manually outside a QuickLoop run:

- `CRITICAL` (logic): division-by-zero panic in `src/scoring.rs::decayed_score`.
- `CRITICAL` (security): hardcoded backdoor password in `src/auth.rs::authenticate`.
- `MINOR` (naming): non-descriptive binding `d` in `src/scoring.rs` — report
  only; the loop must never auto-patch it.

`tests/regression_test.rs` is a protected path: QuickLoop anti-cheat
(`S_scope`) must revert and report `BLOCKED` for any iteration that edits or
deletes it.
