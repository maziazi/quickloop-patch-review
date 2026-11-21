# QuickLoop — Execution Plan

**Status:** v0.2 · **Last updated:** 2026-11-22
**Deadline:** Sun Nov 22 2026, 10:00 PT (**Mon Nov 23 2026, 01:00 WIB**)
**Trial Quota Window:** Available through Sun 17:00 PT (**Mon 08:00 WIB**) for post-deadline testing.

---

## 1. Execution Priorities (Remaining Window)
1. **Verify Baseline First (`BL-01`–`BL-05`):** Confirm `moonshotai-kimi-k3.1-pro-trial` responds via OpenClaw CLI inside an isolated workspace; record `queue_time_s`, TTFT, `output_tps`, `e2e_tps`, and `cached_tokens > 0` on turn 2.
2. **Harden Core Loop on `demo-repo/`:** Confirm `READY_TO_MERGE`, untouched `MINOR` nit, and `BLOCKED` on test deletion.
3. **TPS Bounty Artifact (`bench/tps-bounty.md`):** Short prompt, long fluent output, reasoning OFF, streaming ON, warm vs cold explicitly labeled, API `usage` fields only.
4. **60-Second Demo Video:** Real-time `/looping-code-review 42`, side-by-side speed context, `READY_TO_MERGE` + untouched `MINOR` nit, anti-cheat `BLOCKED` moment, honest telemetry line, repo URL, `#KimiOnCerebras`.
5. **Open-Source Repo Polish & Submission:** Switch repo to public (after verifying zero secrets in git history), test clean clone, post to X (`@Cerebras`, `@Kimi_Moonshot`, `#KimiOnCerebras`) and `r/cerebras` ("Kimi K3.1 Pro Hackathon" flair).

## 2. Scope-Cut Order (Cut Bottom-Up If Short on Time)
| Priority | Item | Rule |
| :--- | :--- | :--- |
| **P0** | Working closed loop on `demo-repo/` + honest telemetry + ≤ 60 s video | Never cut |
| **P1** | Anti-cheat `BLOCKED` demo + TPS Bounty entry | Keep (highest impact) |
| **P2** | Side-by-side GPU video + Reddit post | Simplify GPU comparison to static numbers card if rushed |
| **P3** | `.quickloop.toml`, parallel fan-out, Predicted Outputs, non-Rust adapters | Cut first; hardcode Rust demo commands |