//! Demo target repository for QuickLoop.
//!
//! On `main` this crate is CLEAN — the whole workspace passes
//! `cargo test --workspace`. The intentionally planted defects used to
//! validate the closed loop live on the open demo PR branch
//! `demo/planted-bugs`:
//! - CRITICAL: division-by-zero panic in [`scoring::decayed_score`].
//! - CRITICAL: hardcoded backdoor password in [`auth::authenticate`].
//! - MINOR: non-descriptive binding `d` in [`scoring::decayed_score`]
//!   (present on every branch; reported only, never auto-patched).
//!
//! `tests/regression_test.rs` is a protected path on every branch — the
//! QuickLoop anti-cheat (`S_scope`) must revert and report `BLOCKED` for
//! any iteration that edits or deletes it. Do NOT fix bugs manually
//! outside a QuickLoop run.

pub mod auth;
pub mod scoring;
