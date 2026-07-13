//! Fail-closed safety invariants (INTERFACES.md Part 2 "Safety Invariants on Order"). The
//! pipeline checks these after every transform; a violation rolls that transform back
//! (pre-transform bytes restored) rather than shipping a corrupted or unsafe output.

use serde_json::Value;

/// Invariant 1: after any JSON transform, the output must still parse as JSON.
pub fn json_still_valid(bytes: &[u8]) -> bool {
    serde_json::from_slice::<Value>(bytes).is_ok()
}

/// Invariant 2: after any JSON transform, every object's key order must be unchanged.
/// Compares the depth-first sequence of object keys (not values, not array lengths — a
/// transform may legitimately shorten an array while leaving every object's key order alone).
pub fn json_key_order_preserved(before: &[u8], after: &[u8]) -> bool {
    let (Ok(before_value), Ok(after_value)) = (
        serde_json::from_slice::<Value>(before),
        serde_json::from_slice::<Value>(after),
    ) else {
        return false;
    };
    let mut before_keys = Vec::new();
    let mut after_keys = Vec::new();
    collect_key_order(&before_value, &mut before_keys);
    collect_key_order(&after_value, &mut after_keys);
    before_keys == after_keys
}

fn collect_key_order(value: &Value, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, nested) in map {
                out.push(key.clone());
                collect_key_order(nested, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_key_order(item, out);
            }
        }
        _ => {}
    }
}

/// Invariant 3: every protected-content segment (system turns, latest user message, diff
/// headers) must still be present, byte-for-byte, somewhere in the output. Segments are
/// checked individually (see `budget::protected_segments`) because concatenating them into
/// one blob would rarely be contiguous in the real document.
pub fn protected_segments_present(segments: &[Vec<u8>], output: &[u8]) -> bool {
    segments
        .iter()
        .all(|segment| contains_subslice(output, segment))
}

fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Invariant 4: `secret_redaction` output must not contain any pattern that matched in the
/// original. Re-running redaction over its own output and confirming no further match is
/// found is equivalent to a fresh scan, without needing a separate "scan only" API.
pub fn no_redaction_bypass(redacted_output: &[u8]) -> bool {
    crate::transforms::redaction::redact(redacted_output).redacted_count == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_still_valid_detects_broken_json() {
        assert!(json_still_valid(b"{\"a\":1}"));
        assert!(!json_still_valid(b"{not json"));
    }

    #[test]
    fn key_order_preserved_across_whitespace_only_change() {
        let before = br#"{"b": 1, "a": 2}"#;
        let after = br#"{"b":1,"a":2}"#;
        assert!(json_key_order_preserved(before, after));
    }

    #[test]
    fn key_order_violation_detected() {
        let before = br#"{"b": 1, "a": 2}"#;
        let after = br#"{"a":2,"b":1}"#;
        assert!(!json_key_order_preserved(before, after));
    }

    #[test]
    fn key_order_preserved_when_nested_array_is_shortened() {
        let before = br#"{"name":"x","examples":[1,2,3,4,5]}"#;
        let after = br#"{"name":"x","examples":[1]}"#;
        assert!(json_key_order_preserved(before, after));
    }

    #[test]
    fn protected_segments_present_checks_each_segment_independently() {
        let segments = vec![b"system prompt".to_vec(), b"latest question".to_vec()];
        let output = b"preamble system prompt middle latest question tail".to_vec();
        assert!(protected_segments_present(&segments, &output));
    }

    #[test]
    fn protected_segments_present_fails_when_one_segment_is_missing() {
        let segments = vec![b"system prompt".to_vec(), b"latest question".to_vec()];
        let output = b"preamble system prompt middle tail".to_vec();
        assert!(!protected_segments_present(&segments, &output));
    }

    #[test]
    fn empty_segment_list_is_trivially_satisfied() {
        assert!(protected_segments_present(&[], b"anything"));
    }

    #[test]
    fn no_redaction_bypass_true_after_clean_redaction() {
        let outcome = crate::transforms::redaction::redact(b"Bearer sk-abcdEFGH1234567890123456");
        assert!(no_redaction_bypass(&outcome.bytes));
    }
}
