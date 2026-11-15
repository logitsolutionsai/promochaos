//! Shopify Function resource limits (spec section 5a), checked against the
//! published documentation, with a warning for function-runner's lower
//! hard-coded output limit.

use super::{
    Check, LOGS_FIT, OutcomeKind, WITHIN_INSTRUCTION_LIMIT, WITHIN_MEMORY_LIMIT,
    WITHIN_OUTPUT_LIMIT,
};
use crate::cart::Cart;
use crate::eval::Evaluation;
use crate::money::group_int;

/// Up to 200 cart lines. Above that, limits scale with the line count.
pub const INSTRUCTIONS: u64 = 11_000_000;
/// Input size, bytes, before scaling.
pub const INPUT_BYTES: u64 = 128_000;
/// Output size in Shopify's documentation, bytes, before scaling.
pub const OUTPUT_BYTES_DOCS: u64 = 40_000;
/// Output size hard-coded in function-runner 9.2.3, bytes, before scaling.
pub const OUTPUT_BYTES_RUNNER: u64 = 20_000;
/// Linear memory, KiB (10 MiB).
pub const MEMORY_KIB: u64 = 10 * 1024;
/// Logs past this many bytes are truncated.
pub const LOG_BYTES: usize = 1_000;
/// `cart.lines @scaleLimits(rate: 0.005)`.
pub const LINES_RATE: f64 = 0.005;

/// The scale factor function-runner computes for a cart of `lines` lines when
/// the query selects `cart.lines`: `lines * 0.005`, clamped to 1 to 10.
pub fn scale_for_lines(lines: usize) -> f64 {
    (lines as f64 * LINES_RATE).clamp(1.0, 10.0)
}

/// A limit times the scale factor, rounded down as function-runner does
/// (`(scale * limit) as u64`).
pub fn scaled(limit: u64, scale: f64) -> u64 {
    let s = if scale.is_finite() {
        scale.clamp(1.0, 10.0)
    } else {
        1.0
    };
    (s * limit as f64) as u64
}

pub fn instruction_limit(scale: f64) -> u64 {
    scaled(INSTRUCTIONS, scale)
}

/// A run promochaos stopped at its wall-clock timeout. The interrupted run's
/// instruction count is not reported by function-runner, so no count is
/// claimed; Shopify would have stopped the run at the limit.
pub fn timed_out(cart: &Cart, eval: &Evaluation) -> Check {
    let lines = cart.lines.len();
    let noun = if lines == 1 { "line" } else { "lines" };
    let limit = instruction_limit(eval.metrics.map(|m| m.scale_factor).unwrap_or(1.0));
    let mut c = Check::fail(
        WITHIN_INSTRUCTION_LIMIT,
        OutcomeKind::OverLimit,
        "instructions|timeout",
        format!(
            "A cart of {lines} {noun} was still running when the timeout stopped it. Shopify stops a function at {} instructions and discards the discount.",
            group_int(limit)
        ),
    )
    .with_detail("Instructions", "not counted: the run was interrupted")
    .with_detail("Limit", group_int(limit));
    if let Some(msg) = &eval.crash {
        c = c.with_detail("Stopped", crate::util::first_line(msg));
    }
    c
}

pub fn check(cart: &Cart, eval: &Evaluation) -> Vec<Check> {
    let Some(m) = eval.metrics else {
        return Vec::new();
    };
    let mut checks = Vec::new();
    let lines = cart.lines.len();
    let noun = if lines == 1 { "line" } else { "lines" };

    let ilimit = instruction_limit(m.scale_factor);
    if m.instructions > ilimit {
        checks.push(
            Check::fail(
                WITHIN_INSTRUCTION_LIMIT,
                OutcomeKind::OverLimit,
                "instructions",
                format!(
                    "A cart of {lines} {noun} used {} instructions, over the limit of {}, so Shopify discards the discount.",
                    group_int(m.instructions),
                    group_int(ilimit)
                ),
            )
            .with_detail("Instructions", group_int(m.instructions))
            .with_detail("Limit", group_int(ilimit)),
        );
    } else {
        checks.push(Check::pass(WITHIN_INSTRUCTION_LIMIT));
    }

    let out = eval.output_bytes as u64;
    let docs_limit = scaled(OUTPUT_BYTES_DOCS, m.scale_factor);
    let runner_limit = scaled(OUTPUT_BYTES_RUNNER, m.scale_factor);
    if out > docs_limit {
        checks.push(Check::fail(
            WITHIN_OUTPUT_LIMIT,
            OutcomeKind::OverLimit,
            "output",
            format!(
                "The function's output was {} bytes, over the limit of {} bytes, so Shopify discards it.",
                group_int(out),
                group_int(docs_limit)
            ),
        ));
    } else if out > runner_limit {
        checks.push(Check::warn(
            WITHIN_OUTPUT_LIMIT,
            "output_runner",
            format!(
                "The output was {} bytes. Shopify's documentation allows {} bytes, but function-runner 9.2.3 stops at {} bytes, so local runs fail where checkout may not.",
                group_int(out),
                group_int(docs_limit),
                group_int(runner_limit)
            ),
        ));
    } else {
        checks.push(Check::pass(WITHIN_OUTPUT_LIMIT));
    }

    if m.memory_kib > MEMORY_KIB {
        checks.push(Check::fail(
            WITHIN_MEMORY_LIMIT,
            OutcomeKind::OverLimit,
            "memory",
            format!(
                "The function used {} KiB of memory, over the 10 MiB limit.",
                group_int(m.memory_kib)
            ),
        ));
    } else {
        checks.push(Check::pass(WITHIN_MEMORY_LIMIT));
    }

    if eval.logs.len() > LOG_BYTES {
        checks.push(Check::warn(
            LOGS_FIT,
            "logs",
            format!(
                "The function wrote {} bytes of logs. Shopify keeps the first {} bytes.",
                group_int(eval.logs.len() as u64),
                group_int(LOG_BYTES as u64)
            ),
        ));
    } else {
        checks.push(Check::pass(LOGS_FIT));
    }
    checks
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cart::test_support::*;

    /// Regression (QA 2026-11-15): an endless loop was reported as
    /// "no_trap: the function trapped" with 0 instructions. Shopify stops it
    /// at the instruction limit, so it is an over-limit failure.
    #[test]
    fn a_timed_out_run_is_over_the_instruction_limit() {
        let c = cart(vec![line(1, variant(1, "Tote", &[], false), 1, 4500)]);
        let mut e = Evaluation::crashed(
            serde_json::json!({}),
            "run interrupted after 2000 ms (timeout_ms)".into(),
            String::new(),
        );
        e.timed_out = true;
        e.metrics = Some(crate::eval::RunMetrics {
            instructions: 0,
            memory_kib: 0,
            module_kib: 0,
            scale_factor: 1.0,
        });
        let check = timed_out(&c, &e);
        assert_eq!(check.rule, WITHIN_INSTRUCTION_LIMIT);
        assert_eq!(check.kind, OutcomeKind::OverLimit);
        assert!(check.sentence.contains("11,000,000 instructions"));
        assert!(!check.sentence.contains(" 0 instructions"));
    }

    #[test]
    fn scaling_matches_function_runner() {
        assert_eq!(scale_for_lines(1), 1.0);
        assert_eq!(scale_for_lines(200), 1.0);
        assert_eq!(scale_for_lines(250), 1.25);
        assert_eq!(scale_for_lines(5000), 10.0);
        assert_eq!(instruction_limit(1.0), 11_000_000);
        assert_eq!(instruction_limit(1.25), 13_750_000);
        assert_eq!(scaled(OUTPUT_BYTES_DOCS, 1.0), 40_000);
    }
}
