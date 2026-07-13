//! Output-shaping savings reports.
//!
//! Builds [`tokenfold_core::report::OutputSavingsReport`] values that clearly separate two
//! kinds of output-token savings claims:
//!
//! - **measured**: both the baseline and shaped output text are available, so token counts
//!   are computed exactly via a [`TokenEstimator`](tokenfold_core::token_estimator::TokenEstimator).
//! - **estimated**: no real shaped-output text exists yet, so savings are projected from the
//!   input compression ratio. This is a rough heuristic proxy, not a measurement, and the
//!   report stays clearly labeled as such via the `profile`/`provenance` fields.

use tokenfold_core::report::OutputSavingsReport;
use tokenfold_core::token_estimator::TokenEstimator;

/// Build a "measured" output savings report by exactly counting tokens in both the baseline
/// and shaped output byte slices via `estimator`.
///
/// Use this when real shaped-output text is available (e.g. after an output-shaping transform
/// has actually run), so the savings figure is an exact count rather than a projection.
pub fn measure_output_savings(
    baseline_output: &[u8],
    shaped_output: &[u8],
    estimator: &dyn TokenEstimator,
) -> OutputSavingsReport {
    let baseline_tokens = estimator.count_bytes(baseline_output);
    let shaped_tokens = estimator.count_bytes(shaped_output);
    let backend = estimator.info().backend;

    OutputSavingsReport {
        profile: "measured".to_string(),
        estimated_output_tokens_saved: None,
        measured_output_tokens_saved: Some(baseline_tokens.saturating_sub(shaped_tokens)),
        provenance: format!(
            "measured: exact token counts for baseline and shaped output via {backend}"
        ),
    }
}

/// Build an "estimated" output savings report by projecting `input_savings_ratio` onto a
/// hinted baseline output token count.
///
/// This is explicitly a rough proxy, not a real measurement: it exists for callers that have
/// no real shaped-output text to count exactly yet (e.g. before an output-shaping transform has
/// run). Downstream consumers can always tell this apart from a real measurement via the
/// `profile` field (`"estimated"` vs `"measured"`) and the `provenance` string, which spells
/// out that this is a heuristic projection rather than a measurement.
pub fn estimate_output_savings(
    input_savings_ratio: f64,
    baseline_output_tokens_hint: usize,
) -> OutputSavingsReport {
    // Round-half-away-from-zero (f64::round), which is the natural "nearest whole token" choice
    // for a rough projection; input ratios and hints are both non-negative in practice.
    let estimated = (baseline_output_tokens_hint as f64 * input_savings_ratio).round() as usize;

    OutputSavingsReport {
        profile: "estimated".to_string(),
        estimated_output_tokens_saved: Some(estimated),
        measured_output_tokens_saved: None,
        provenance: "estimated: heuristic projection from input compression ratio; no real \
            shaped output text was measured"
            .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenfold_core::token_estimator::ByteHeuristicEstimator;

    #[test]
    fn measured_and_estimated_are_mutually_exclusive() {
        let estimator = ByteHeuristicEstimator;
        let measured = measure_output_savings(b"hello world, this is baseline", b"hi", &estimator);
        assert!(measured.measured_output_tokens_saved.is_some());
        assert!(measured.estimated_output_tokens_saved.is_none());

        let estimated = estimate_output_savings(0.3, 100);
        assert!(estimated.estimated_output_tokens_saved.is_some());
        assert!(estimated.measured_output_tokens_saved.is_none());
    }

    #[test]
    fn measured_counts_are_exact() {
        let estimator = ByteHeuristicEstimator;
        let baseline = [b'a'; 40]; // ByteHeuristicEstimator: bytes.len().div_ceil(4) => 10
        let shaped = [b'b'; 20]; // => 5
        let report = measure_output_savings(&baseline, &shaped, &estimator);
        assert_eq!(report.measured_output_tokens_saved, Some(10 - 5));
    }

    #[test]
    fn estimate_scales_with_ratio() {
        assert_eq!(
            estimate_output_savings(0.5, 1000).estimated_output_tokens_saved,
            Some(500)
        );
        // Non-round ratio: 0.333 * 1000 == 333.0 (modulo float noise), and f64::round takes it
        // to the nearest whole token, which is the documented rounding behavior.
        assert_eq!(
            estimate_output_savings(0.333, 1000).estimated_output_tokens_saved,
            Some(333)
        );
    }

    #[test]
    fn profile_field_is_labeled_correctly() {
        let estimator = ByteHeuristicEstimator;
        assert_eq!(
            measure_output_savings(b"aaaa", b"bb", &estimator).profile,
            "measured"
        );
        assert_eq!(estimate_output_savings(0.3, 100).profile, "estimated");
    }
}
