//! Honest telemetry types shared by the worker commands.
//!
//! All values are sourced directly from Cerebras API `usage`/`time_info`
//! fields and local monotonic clocks. Never estimate token counts with a
//! tokenizer, and always label warm-cache vs cold-cache runs.

use serde::Serialize;

/// Latency and throughput metrics for one model call or one full loop.
// Consumed by `feat/worker-publish` when telemetry recording lands.
#[allow(dead_code)]
#[derive(Debug, Default, Serialize)]
pub struct Metrics {
    /// `time_info.queue_time` from the API response (seconds), when present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queue_time_s: Option<f64>,
    /// Time from request sent to first content token (seconds).
    pub ttft_s: f64,
    /// Pure decode duration, last token minus first content token (seconds).
    pub gen_s: f64,
    /// `usage.completion_tokens / gen_s`.
    pub output_tps: f64,
    /// `usage.completion_tokens / (ttft_s + gen_s)`.
    pub e2e_tps: f64,
    /// `cached_tokens / prompt_tokens`; 0.0 on cold-cache runs.
    pub cache_hit_rate: f64,
    /// Full wall-clock duration from skill invocation to gate decision.
    pub wall_s: f64,
}

#[cfg(test)]
mod tests {
    use super::Metrics;

    #[test]
    fn metrics_serialize_compact_without_optional_fields() {
        let m = Metrics {
            ttft_s: 0.1,
            gen_s: 0.2,
            output_tps: 100.0,
            e2e_tps: 90.0,
            cache_hit_rate: 0.85,
            wall_s: 1.5,
            queue_time_s: None,
        };
        let s = serde_json::to_string(&m).expect("metrics should serialize");
        assert!(s.contains("\"ttft_s\":0.1"));
        assert!(s.contains("\"cache_hit_rate\":0.85"));
        assert!(
            !s.contains("queue_time_s"),
            "absent optional field must be skipped"
        );
        assert!(!s.contains('\n'), "telemetry JSON must stay compact");
    }
}
