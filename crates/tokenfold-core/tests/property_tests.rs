//! Property-based invariants. These generate random
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

// --- EP-06 structured allocation ---------------------------------------------
//
// These assert the properties that must hold for *any* generated group set, not just the
// hand-picked cases in `allocation.rs`'s own tests.

use tokenfold_core::allocation::{
    Candidate, Group, SelectRejection, allocate, allocate_with_scorer,
};

/// A candidate id unique within its group, so a group's members are never conflated.
fn groups(counts: impl Iterator<Item = (usize, bool, f64)>) -> Vec<Group> {
    counts
        .enumerate()
        .map(|(gi, (cost, required, score))| Group {
            id: format!("g{gi}"),
            members: (0..2)
                .map(|mi| Candidate {
                    id: format!("g{gi}-m{mi}"),
                    cost: cost / 2,
                    score,
                })
                .collect(),
            required,
        })
        .collect()
}

proptest! {
    /// Whole groups only: a group is either fully present in the result or fully absent. A
    /// partially-retained group would emit a record whose partner is gone.
    #[test]
    fn allocation_never_returns_a_partially_retained_group(
        spec in prop::collection::vec((0usize..200, any::<bool>(), 0.0f64..10.0), 0..8),
        budget in 0usize..600,
    ) {
        let groups = groups(spec.into_iter());
        let (kept, report) = allocate(&groups, budget);
        prop_assert_eq!(kept.len() + report.dropped_groups, groups.len());
        for id in &kept {
            prop_assert!(groups.iter().any(|g| g.id == *id), "kept an unknown id {id}");
        }
        // Determinism: the same input twice gives the same answer.
        prop_assert_eq!(kept, allocate(&groups, budget).0);
    }

    /// Declared requirements always survive, whatever the budget or the scores.
    #[test]
    fn a_required_group_is_always_retained(
        spec in prop::collection::vec((0usize..200, any::<bool>(), 0.0f64..10.0), 0..8),
        budget in 0usize..600,
    ) {
        let groups = groups(spec.into_iter());
        let (kept, report) = allocate(&groups, budget);
        for group in groups.iter().filter(|g| g.required) {
            prop_assert!(
                kept.contains(&group.id),
                "required group {} was dropped: {kept:?}",
                group.id
            );
        }
        prop_assert_eq!(report.kept_required, groups.iter().filter(|g| g.required).count());
    }

    /// The result is in source order: applying it can never reorder the source document.
    #[test]
    fn the_result_is_always_in_source_order(
        spec in prop::collection::vec((0usize..200, any::<bool>(), 0.0f64..10.0), 0..8),
        budget in 0usize..600,
    ) {
        let groups = groups(spec.into_iter());
        let (kept, _) = allocate(&groups, budget);
        let positions: Vec<usize> = kept
            .iter()
            .map(|id| groups.iter().position(|g| g.id == *id).unwrap())
            .collect();
        prop_assert!(positions.windows(2).all(|w| w[0] < w[1]), "out of order: {positions:?}");
    }

    /// A misbehaving scorer degrades to the fallback ranking and never empties the context
    /// while there was something to keep.
    #[test]
    fn a_failing_scorer_never_empties_a_non_empty_selection(
        spec in prop::collection::vec((0usize..200, any::<bool>(), 0.0f64..10.0), 1..8),
        budget in 0usize..600,
    ) {
        let groups = groups(spec.into_iter());
        let baseline = allocate(&groups, budget).0;
        for scored in [
            None,
            Some(std::collections::BTreeMap::from([("ghost".to_string(), 1.0f64)])),
        ] {
            let out = allocate_with_scorer(&groups, budget, scored);
            prop_assert!(out.rejection.is_some());
            prop_assert!(out.report.used_fallback_ranking);
            prop_assert_eq!(
                out.kept_group_ids, baseline.clone(),
                "a failing scorer changed the selection"
            );
            prop_assert!(out.report.kept_groups + out.report.dropped_groups >= 1);
        }
    }

    /// Requirements survive a failing scorer too.
    #[test]
    fn a_failing_scorer_never_drops_a_requirement(
        spec in prop::collection::vec((0usize..200, any::<bool>(), 0.0f64..10.0), 1..8),
        budget in 0usize..600,
    ) {
        let groups = groups(spec.into_iter());
        let out = allocate_with_scorer(&groups, budget, None);
        for group in groups.iter().filter(|g| g.required) {
            prop_assert!(
                out.kept_group_ids.contains(&group.id),
                "a missing scorer dropped required group {}",
                group.id
            );
        }
        prop_assert_eq!(
            out.rejection,
            Some(SelectRejection::Unavailable)
        );
    }
}
