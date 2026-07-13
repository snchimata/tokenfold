//! Property-based invariants (ENGINEERING.md "Property-Based Tests"). These generate random
//! inputs and assert the transform/pipeline contracts hold for all of them, not just the
//! hand-picked fixtures in `golden.rs` / inline unit tests.

use proptest::prelude::*;
use tokenfold_core::budget::protected_floor;
use tokenfold_core::token_estimator::{ByteHeuristicEstimator, TokenEstimator};
use tokenfold_core::transforms::{json, logs, redaction};
use tokenfold_core::{CompressionInput, CompressionPolicy};

fn printable_ascii() -> impl Strategy<Value = String> {
    "[ -~]{0,40}"
}

proptest! {
    #[test]
    fn json_minify_round_trip_is_semantically_equal(a in printable_ascii(), b in 0i64..1_000_000) {
        let value = serde_json::json!({"a": a, "b": b, "nested": {"x": [1, 2, 3]}});
        let input = serde_json::to_vec(&value).unwrap();
        let minified = json::minify_json(&input).unwrap();
        let reparsed: serde_json::Value = serde_json::from_slice(&minified).unwrap();
        prop_assert_eq!(reparsed, value);
    }

    #[test]
    fn json_minify_preserves_key_order(a in printable_ascii(), b in 0i64..1_000_000) {
        // Search for the literal key names (not the arbitrary values) so the check is
        // robust regardless of what characters `a` happens to contain.
        let value = serde_json::json!({"zetaKey": a, "alphaKey": b});
        let input = serde_json::to_vec(&value).unwrap();
        let minified = json::minify_json(&input).unwrap();
        let text = String::from_utf8_lossy(&minified);
        let zeta_pos = text.find("\"zetaKey\"").expect("zetaKey present");
        let alpha_pos = text.find("\"alphaKey\"").expect("alphaKey present");
        prop_assert!(zeta_pos < alpha_pos);
    }

    #[test]
    fn json_minify_is_idempotent(a in printable_ascii(), b in 0i64..1_000_000) {
        let value = serde_json::json!({"a": a, "b": b});
        let input = serde_json::to_vec(&value).unwrap();
        let once = json::minify_json(&input).unwrap();
        let twice = json::minify_json(&once).unwrap();
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn log_compaction_is_idempotent(lines in proptest::collection::vec("[a-c]", 0..12)) {
        let text = lines.join("\n");
        let once = logs::compact(&text, false);
        let twice = logs::compact(&once, false);
        prop_assert_eq!(once, twice);
    }

    #[test]
    fn redaction_is_idempotent(text in "[ -~]{0,80}") {
        let once = redaction::redact(text.as_bytes());
        let twice = redaction::redact(&once.bytes);
        prop_assert_eq!(once.bytes, twice.bytes);
    }

    #[test]
    fn protected_floor_never_exceeds_original_tokens(system in "[ -~]{0,60}", user in "[ -~]{0,60}") {
        let payload = serde_json::json!({
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": user},
            ]
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::openai_json(bytes.clone());
        let policy = CompressionPolicy::builder().build().unwrap();
        let estimator = ByteHeuristicEstimator;
        let floor = protected_floor(&input, &policy, &estimator);
        let original = estimator.count_bytes(&bytes);
        prop_assert!(floor <= original);
    }

    #[test]
    fn compress_never_reports_savings_larger_than_original(text in "[ -~]{0,120}") {
        // Uses the heuristic estimator directly rather than the public `compress()` facade:
        // `compress()` re-selects (and re-initializes) the tiktoken backend on every call,
        // which is fine for a single real invocation but turns a 256-case proptest into a
        // multi-minute run for no benefit — this property doesn't depend on which estimator
        // backend is in play.
        let input = CompressionInput::plain_text(text.into_bytes());
        let policy = CompressionPolicy::builder().build().unwrap();
        let output =
            tokenfold_core::compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        prop_assert!(output.report.compressed_tokens <= output.report.original_tokens);
        prop_assert_eq!(
            output.report.saved_tokens,
            output.report.original_tokens - output.report.compressed_tokens
        );
    }
}
