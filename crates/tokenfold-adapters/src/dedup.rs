//! EP-07 / NF-03: an exact duplicate-observation codec.
//!
//! # What this does
//!
//! A host transcript frequently contains the *same* tool result many times -- the same file read,
//! the same query, replayed across turns, or duplicated across parallel calls. Every occurrence
//! pays full price. This codec replaces all but the first occurrence of an exactly-repeated
//! observation with a short reference, and can expand them all back.
//!
//! # What this deliberately does not do
//!
//! It is an **exact** codec. Near-repeats (one byte different) are left alone: a "similar enough"
//! matcher would need a similarity definition, and guessing at one would silently rewrite content
//! that the host meant literally. It also never suppresses tool execution, never reorders, and
//! never touches anything but `role: "tool"` message content.
//!
//! # Round-trip is the contract
//!
//! `expand(compact(x)) == x` byte-for-byte, or `compact` returns the untouched baseline. This
//! mirrors the observation adapter's rule: a candidate that cannot be proven safe is never emitted.
//! The baseline is re-derived by expanding the candidate and compared to the original bytes.
//!
//! # Marker collisions
//!
//! The marker is plain text, so user content could in principle contain something that looks like
//! one. Two rules keep that safe:
//!
//! `compact` checks the candidate with the public decoder and keeps the baseline on any
//! collision or byte mismatch (including non-canonical JSON). No private position list is
//! required to decode emitted candidates. `expand` rejects dangling and forward references.
//! A future escaping frame can extend eligibility without weakening this gate.

use serde_json::Value;

/// The codec's own frame version, carried in every marker.
pub const DEDUP_SCHEMA_VERSION: u32 = 1;

/// Marker grammar: `[tf-dedup:v=<version>:ref=<index>]`. Kept explicit and versioned so a future
/// frame change is distinguishable from a host string that merely looks similar.
const MARKER_PREFIX: &str = "[tf-dedup:v=";
const MARKER_MIDDLE: &str = ":ref=";
const MARKER_SUFFIX: &str = "]";

/// Why a body was or was not compacted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DedupDisposition {
    /// The feature is off. The default.
    Disabled,
    /// Not a JSON object carrying a `messages` array.
    NotAChatRequest,
    /// No tool result occurred exactly twice or more above the size threshold.
    NoExactRepeat,
    /// The candidate was verified and emitted.
    Compacted,
}

impl DedupDisposition {
    pub fn is_compacted(&self) -> bool {
        matches!(self, DedupDisposition::Compacted)
    }
}

/// Explicitly opt-in; off by default like every other adapter increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DedupPolicy {
    pub enabled: bool,
    /// Occurrences smaller than this are left alone even when repeated. A marker costs more than a
    /// few bytes, so deduplicating a tiny string is a loss; the threshold keeps it a win or a
    /// no-op.
    pub min_repeat_bytes: usize,
}

impl Default for DedupPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            min_repeat_bytes: 64,
        }
    }
}

/// What `compact` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DedupOutcome {
    /// The compacted body, or the untouched baseline when nothing was proven safe.
    pub bytes: Vec<u8>,
    pub disposition: DedupDisposition,
    /// How many occurrences were replaced by a reference.
    pub deduped_results: usize,
    /// Bytes saved, measured on the serialized body.
    pub saved_bytes: usize,
}

/// Why an expansion could not be completed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DedupError {
    /// A marker referred to a message index that does not exist, or that does not hold content.
    DanglingReference { ref_index: usize },
    /// The body was not a JSON object with a `messages` array.
    NotAChatRequest,
}

/// Parses `content` as one of our markers, returning the referenced message index.
///
/// Strict on purpose: the version must match exactly and the reference must be a plain decimal
/// index. Anything else is not a marker, and is therefore host data to be left untouched.
pub fn parse_marker(content: &str) -> Option<usize> {
    let rest = content.strip_prefix(MARKER_PREFIX)?;
    let (version, rest) = rest.split_once(MARKER_MIDDLE)?;
    if version.parse::<u32>().ok()? != DEDUP_SCHEMA_VERSION {
        return None;
    }
    let index = rest.strip_suffix(MARKER_SUFFIX)?;
    // `parse` rejects `+1`, whitespace and leading zeros is accepted but harmless and unambiguous.
    if index.is_empty() || !index.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    index.parse::<usize>().ok()
}

fn marker_for(ref_index: usize) -> String {
    format!("{MARKER_PREFIX}{DEDUP_SCHEMA_VERSION}{MARKER_MIDDLE}{ref_index}{MARKER_SUFFIX}")
}

/// Every `role: "tool"` message's string `content`, paired with its message index.
///
/// Non-tool messages and tool messages with non-string content are simply not eligible; they are
/// never rewritten and never used as a dedup target.
fn tool_contents(body: &[u8]) -> Result<Vec<(usize, String)>, DedupError> {
    let value: Value = serde_json::from_slice(body).map_err(|_| DedupError::NotAChatRequest)?;
    let messages = value
        .get("messages")
        .and_then(Value::as_array)
        .ok_or(DedupError::NotAChatRequest)?;
    Ok(messages
        .iter()
        .enumerate()
        .filter_map(|(index, message)| {
            (message.get("role").and_then(Value::as_str) == Some("tool"))
                .then(|| {
                    message
                        .get("content")
                        .and_then(Value::as_str)
                        .map(|content| (index, content.to_string()))
                })
                .flatten()
        })
        .collect())
}

/// Expands every marker in `body`, restoring each referenced observation.
///
/// Errors on a dangling reference rather than guessing. That is the difference between a codec and
/// a data-loss bug: a reference that cannot be resolved means the body is not one this codec
/// produced, and silently emitting it would hand the host a transcript with a hole in it.
pub fn expand(body: &[u8]) -> Result<Vec<u8>, DedupError> {
    let mut value: Value = serde_json::from_slice(body).map_err(|_| DedupError::NotAChatRequest)?;
    let messages = value
        .get_mut("messages")
        .and_then(Value::as_array_mut)
        .ok_or(DedupError::NotAChatRequest)?;

    // Snapshot the resolvable contents first: expansion must read from the ORIGINAL occurrences,
    // never from a position this same pass has already rewritten (which would let one reference
    // resolve to another reference and cascade).
    let contents: Vec<Option<String>> = messages
        .iter()
        .map(|message| {
            (message.get("role").and_then(Value::as_str) == Some("tool"))
                .then(|| {
                    message
                        .get("content")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .flatten()
        })
        .collect();

    for (message_index, message) in messages.iter_mut().enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let Some(content) = message.get("content").and_then(Value::as_str) else {
            continue;
        };
        let Some(ref_index) = parse_marker(content) else {
            continue;
        };
        // A marker may only point at real content that is itself not a marker.
        let target = contents
            .get(ref_index)
            .filter(|_| ref_index < message_index)
            .and_then(|slot| slot.as_deref())
            .filter(|restored| parse_marker(restored).is_none())
            .ok_or(DedupError::DanglingReference { ref_index })?;
        if let Some(slot) = message.get_mut("content") {
            *slot = Value::String(target.to_string());
        }
    }

    serde_json::to_vec(&value).map_err(|_| DedupError::NotAChatRequest)
}

/// Replaces every occurrence but the first of an exactly-repeated tool result with a reference.
///
/// Returns the untouched baseline unless a compacted candidate is both (a) provably reversible and
/// (b) actually smaller. Order is preserved: the surviving first occurrence stays where it was, so
/// the transcript reads in its original sequence.
pub fn compact(body: &[u8], policy: &DedupPolicy) -> DedupOutcome {
    let keep = |disposition: DedupOutcome| disposition;
    if !policy.enabled {
        return keep(DedupOutcome {
            bytes: body.to_vec(),
            disposition: DedupDisposition::Disabled,
            deduped_results: 0,
            saved_bytes: 0,
        });
    }

    let contents = match tool_contents(body) {
        Ok(contents) => contents,
        Err(_) => {
            return keep(DedupOutcome {
                bytes: body.to_vec(),
                disposition: DedupDisposition::NotAChatRequest,
                deduped_results: 0,
                saved_bytes: 0,
            });
        }
    };

    // Exact, byte-for-byte grouping. Near-repeats never land in the same bucket, which is the
    // point: only genuine duplicates are worth collapsing.
    let mut seen: Vec<(String, usize)> = Vec::new();
    let mut replacements: Vec<(usize, usize)> = Vec::new();
    for (index, content) in &contents {
        match seen.iter().find(|(existing, _)| existing == content) {
            // Never rewrite content that is itself marker-shaped: doing so could create a marker
            // that resolves to another marker, and it is not a duplicate we introduced.
            Some((_, first_index)) if parse_marker(content).is_none() => {
                if content.len() >= policy.min_repeat_bytes {
                    // Reference the FIRST occurrence, which stays inline; this occurrence is the
                    // one being replaced.
                    replacements.push((*index, *first_index));
                }
            }
            Some(_) => {}
            None => seen.push((content.clone(), *index)),
        }
    }

    if replacements.is_empty() {
        return keep(DedupOutcome {
            bytes: body.to_vec(),
            disposition: DedupDisposition::NoExactRepeat,
            deduped_results: 0,
            saved_bytes: 0,
        });
    }

    // Build the candidate.
    let mut value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return keep(DedupOutcome {
                bytes: body.to_vec(),
                disposition: DedupDisposition::NotAChatRequest,
                deduped_results: 0,
                saved_bytes: 0,
            });
        }
    };
    let first_for_content = |content: &str| -> Option<usize> {
        contents
            .iter()
            .find(|(_, existing)| existing == content)
            .map(|(index, _)| *index)
    };
    let Some(messages) = value.get_mut("messages").and_then(Value::as_array_mut) else {
        return keep(DedupOutcome {
            bytes: body.to_vec(),
            disposition: DedupDisposition::NotAChatRequest,
            deduped_results: 0,
            saved_bytes: 0,
        });
    };
    for (message_index, first_index) in &replacements {
        let Some(message) = messages.get_mut(*message_index) else {
            continue;
        };
        let Some(content) = message.get("content").and_then(Value::as_str) else {
            continue;
        };
        if first_for_content(content) != Some(*first_index) {
            continue;
        }
        if let Some(slot) = message.get_mut("content") {
            *slot = Value::String(marker_for(*first_index));
        }
    }

    let candidate = match serde_json::to_vec(&value) {
        Ok(candidate) => candidate,
        Err(_) => {
            return keep(DedupOutcome {
                bytes: body.to_vec(),
                disposition: DedupDisposition::NotAChatRequest,
                deduped_results: 0,
                saved_bytes: 0,
            });
        }
    };

    // Use the public decoder, not a privileged decoder with out-of-band knowledge.
    // Marker collisions and non-canonical JSON keep the baseline until an escaping/byte codec
    // exists: a value-equality check cannot certify a byte-exact public round trip.
    if expand(&candidate).as_deref() != Ok(body) {
        return keep(DedupOutcome {
            bytes: body.to_vec(),
            disposition: DedupDisposition::NoExactRepeat,
            deduped_results: 0,
            saved_bytes: 0,
        });
    }

    // Gate 2: it must actually be a saving. Counted on the serialized envelope, including the
    // marker overhead, so a small repeat cannot claim a win it did not deliver.
    if candidate.len() >= body.len() {
        return keep(DedupOutcome {
            bytes: body.to_vec(),
            disposition: DedupDisposition::NoExactRepeat,
            deduped_results: 0,
            saved_bytes: 0,
        });
    }

    DedupOutcome {
        saved_bytes: body.len() - candidate.len(),
        bytes: candidate,
        disposition: DedupDisposition::Compacted,
        deduped_results: replacements.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_emitted_candidate_must_round_trip_through_the_public_decoder() {
        let repeated = "long repeated observation with real bytes and provenance";
        let original = tool_body(&[
            ("tool", repeated),
            ("tool", "[tf-dedup:v=1:ref=0]"),
            ("tool", repeated),
        ]);
        let out = compact(&original, &policy());
        if out.disposition.is_compacted() {
            assert_eq!(expand(&out.bytes).unwrap(), original);
        } else {
            assert_eq!(out.bytes, original);
        }
    }

    #[test]
    fn pretty_printed_input_is_not_claimed_as_a_byte_exact_round_trip() {
        let repeated = "long repeated observation with enough bytes to warrant deduplication";
        let value: Value =
            serde_json::from_slice(&tool_body(&[("tool", repeated), ("tool", repeated)])).unwrap();
        let original = serde_json::to_vec_pretty(&value).unwrap();
        let out = compact(&original, &policy());
        if out.disposition.is_compacted() {
            assert_eq!(expand(&out.bytes).unwrap(), original);
        } else {
            assert_eq!(out.bytes, original);
        }
    }

    fn policy() -> DedupPolicy {
        DedupPolicy {
            enabled: true,
            min_repeat_bytes: 8,
        }
    }

    fn tool_body(pairs: &[(&str, &str)]) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "messages": pairs
                .iter()
                .map(|(role, content)| json!({"role": role, "content": content}))
                .collect::<Vec<Value>>(),
        }))
        .unwrap()
    }

    // --- the fixtures EP-07 names ---------------------------------------------

    #[test]
    fn an_exact_repeat_is_compacted_and_every_occurrence_round_trips() {
        // The exact-repeat fixture: the same observation twice.
        let repeated = "the same tool result repeated verbatim across turns, byte for byte";
        let original = tool_body(&[
            ("user", "read the file twice please"),
            ("tool", repeated),
            ("assistant", "here it is"),
            ("tool", repeated),
        ]);

        let out = compact(&original, &policy());
        assert!(out.disposition.is_compacted(), "{:?}", out.disposition);
        assert_eq!(out.deduped_results, 1);
        assert!(out.saved_bytes > 0);

        // Every occurrence comes back, byte for byte.
        assert_eq!(expand(&out.bytes).unwrap(), original);
    }

    #[test]
    fn a_near_repeat_is_left_completely_alone() {
        // One byte different is NOT a duplicate. A similarity matcher would have to guess, and a
        // wrong guess silently rewrites content the host meant literally.
        let base = "the same tool result repeated verbatim across turns, byte for byte";
        let near = "the same tool result repeated verbatim across turns, byte for byteX";
        let original = tool_body(&[("tool", base), ("tool", near)]);

        let out = compact(&original, &policy());
        assert_eq!(out.disposition, DedupDisposition::NoExactRepeat);
        assert_eq!(out.bytes, original, "a near-repeat was rewritten");
        assert_eq!(out.saved_bytes, 0);
    }

    #[test]
    fn user_supplied_marker_shaped_text_is_never_reinterpreted() {
        // A marker-collision fixture: the host's own content looks exactly like our frame.
        let decoy = "[tf-dedup:v=1:ref=0]";
        let repeated = "a genuinely repeated observation body that is comfortably long";
        let original = tool_body(&[("tool", decoy), ("tool", repeated), ("tool", repeated)]);

        let out = compact(&original, &policy());
        assert_eq!(out.disposition, DedupDisposition::NoExactRepeat);
        assert_eq!(out.bytes, original);
        assert_eq!(out.deduped_results, 0);
    }

    #[test]
    fn a_body_without_marker_shaped_host_text_still_round_trips_exactly() {
        // The control for the test above: with no decoy present, the same shape round-trips
        // perfectly. So the refusal above is caused by the collision, not by the codec.
        let repeated = "a genuinely repeated observation body that is comfortably long";
        let original = tool_body(&[("tool", repeated), ("tool", repeated)]);
        let out = compact(&original, &policy());
        assert!(out.disposition.is_compacted());
        assert_eq!(expand(&out.bytes).unwrap(), original);
    }

    #[test]
    fn a_dangling_reference_is_rejected_rather_than_guessed() {
        // A marker pointing at a message that does not exist means this is not a body this codec
        // produced. Emitting it anyway would hand the host a transcript with a hole in it.
        let body = serde_json::to_vec(&json!({
            "messages": [{"role": "tool", "content": "[tf-dedup:v=1:ref=7]"}],
        }))
        .unwrap();

        assert_eq!(
            expand(&body),
            Err(DedupError::DanglingReference { ref_index: 7 })
        );
    }

    #[test]
    fn a_marker_pointing_at_another_marker_is_rejected() {
        // Resolving through a marker would cascade; a reference must land on real content.
        let body = serde_json::to_vec(&json!({
            "messages": [
                {"role": "tool", "content": "[tf-dedup:v=1:ref=1]"},
                {"role": "tool", "content": "[tf-dedup:v=1:ref=1]"},
            ],
        }))
        .unwrap();
        assert_eq!(
            expand(&body),
            Err(DedupError::DanglingReference { ref_index: 1 })
        );
    }

    // --- framing and version ---------------------------------------------------

    #[test]
    fn a_marker_from_a_different_version_is_not_ours() {
        // Version discrimination: a future frame change must be distinguishable from today\'s.
        assert_eq!(parse_marker("[tf-dedup:v=1:ref=0]"), Some(0));
        assert_eq!(parse_marker("[tf-dedup:v=2:ref=0]"), None);
        assert_eq!(parse_marker("[tf-dedup:v=1:ref=]"), None);
        assert_eq!(parse_marker("[tf-dedup:v=1:ref=-1]"), None);
        assert_eq!(parse_marker("ordinary content"), None);
    }

    #[test]
    fn a_three_way_duplicate_keeps_one_and_references_two() {
        let repeated = "an observation seen three separate times in one transcript";
        let original = tool_body(&[
            ("tool", repeated),
            ("assistant", "ack"),
            ("tool", repeated),
            ("tool", repeated),
        ]);
        let out = compact(&original, &policy());
        assert_eq!(out.deduped_results, 2);
        assert_eq!(expand(&out.bytes).unwrap(), original);
    }

    // --- safety ----------------------------------------------------------------

    #[test]
    fn the_feature_is_off_by_default() {
        let repeated = "a repeated observation that must not be touched while the feature is off";
        let original = tool_body(&[("tool", repeated), ("tool", repeated)]);
        let out = compact(&original, &DedupPolicy::default());
        assert_eq!(out.disposition, DedupDisposition::Disabled);
        assert_eq!(
            out.bytes, original,
            "the default must be byte-for-byte unchanged"
        );
    }

    #[test]
    fn a_repeat_below_the_threshold_is_not_worth_a_marker() {
        // A marker costs more than a handful of bytes, so a tiny repeat is a loss.
        let original = tool_body(&[("tool", "ab"), ("tool", "ab")]);
        let out = compact(
            &original,
            &DedupPolicy {
                enabled: true,
                min_repeat_bytes: 64,
            },
        );
        assert_eq!(out.disposition, DedupDisposition::NoExactRepeat);
        assert_eq!(out.bytes, original);
    }

    #[test]
    fn non_tool_messages_are_never_touched() {
        // Even byte-identical user/assistant content is left alone: this codec owns tool results.
        let repeated = "identical prose that appears in both a user and an assistant message";
        let original = tool_body(&[
            ("user", repeated),
            ("assistant", repeated),
            ("tool", repeated),
            ("tool", repeated),
        ]);
        let out = compact(&original, &policy());
        let parsed: Value = serde_json::from_slice(&out.bytes).unwrap();
        assert_eq!(parsed["messages"][0]["content"], json!(repeated));
        assert_eq!(parsed["messages"][1]["content"], json!(repeated));
        assert!(
            parsed["messages"][3]["content"]
                .as_str()
                .unwrap()
                .starts_with(MARKER_PREFIX)
        );
        assert_eq!(expand(&out.bytes).unwrap(), original);
    }

    #[test]
    fn a_non_chat_body_is_left_alone() {
        let body = serde_json::to_vec(&json!({"prompt": "hello"})).unwrap();
        let out = compact(&body, &policy());
        assert_eq!(out.disposition, DedupDisposition::NotAChatRequest);
        assert_eq!(out.bytes, body);
    }

    #[test]
    fn compaction_is_deterministic() {
        let repeated = "a repeated observation used to prove the codec never varies between runs";
        let original = tool_body(&[("tool", repeated), ("tool", repeated), ("tool", repeated)]);
        let first = compact(&original, &policy()).bytes;
        for _ in 0..5 {
            assert_eq!(compact(&original, &policy()).bytes, first);
        }
    }

    #[test]
    fn a_kept_baseline_is_always_the_input_bytes_exactly() {
        // Every non-compacting path must return the input untouched, not a re-serialization.
        let cases: Vec<Vec<u8>> = vec![
            b"not json at all".to_vec(),
            serde_json::to_vec(&json!({"messages": "not an array"})).unwrap(),
            tool_body(&[("tool", "unique content that never repeats here")]),
        ];
        for case in cases {
            let out = compact(&case, &policy());
            assert_eq!(out.bytes, case);
            assert!(!out.disposition.is_compacted());
        }
    }
}
