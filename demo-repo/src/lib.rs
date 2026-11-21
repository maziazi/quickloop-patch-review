//! Demo target repository for QuickLoop.
//!
//! Contains intentionally planted defects used to validate the closed loop:
//! - CRITICAL: division-by-zero panic in [`scoring::decayed_score`].
//! - CRITICAL: hardcoded backdoor password in [`auth::authenticate`].
//! - MINOR: non-descriptive binding `d` in [`scoring::decayed_score`]
//!   (reported only; the loop must never auto-patch it).
//!
//! Do NOT fix these bugs manually outside a QuickLoop run.

pub mod auth;
pub mod scoring;
