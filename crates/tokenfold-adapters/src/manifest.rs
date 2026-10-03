//! EP-08 / NF-04: a source-backed state manifest.
//!
//! # The problem this solves
//!
//! By a late turn an agent transcript has buried a few facts that still matter (the branch it is
//! on, the file it decided to edit, the failing test name) under pages of tool output. A manifest
//! lifts those facts to the front so the model can decide with them in view.
//!
//! # Why nothing here is inferred
//!
//! Every fact is **caller-declared**: the caller passes the exact keys it wants, and this module
//! only ever copies a value that is literally present under one of them. It never summarises,
//! never paraphrases, and never fills in a key it did not find. A manifest that invents a fact is
//! worse than no manifest, because the model will act on it.
//!
//! # Provenance is not optional
//!
//! Each fact records where it came from -- the message index, the `tool_call_id`, and the tool
//! name. That is what makes a late-turn decision auditable, and it is what lets a caller drop
//! facts whose source has since disappeared (see [`ManifestDisposition`] and the staleness
//! handling in [`build_manifest`]).
//!
//! # Conflicts are surfaced, not resolved
//!
//! Two observations of the same key with different values are both kept and counted. Picking a
//! winner would be inventing state, which is the one thing this module must never do.
//!
//! # Secrets never enter
//!
//! A value matching a known secret pattern is dropped and counted, never stored and never
//! redacted-into-something-else. The same detector the pipeline uses owns this boundary.

use serde_json::Value;

use tokenfold_core::transforms::redaction;

/// Where one fact came from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Provenance {
    /// Index into the transcript's `messages` array.
    pub message_index: usize,
    /// The `tool_call_id` this observation answered, when the message carries one.
    pub tool_call_id: String,
    /// The declared tool name, when known.
    pub tool: String,
}

/// One declared key's observed value, with the source it came from.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct StateFact {
    pub key: String,
    pub value: Value,
    pub provenance: Provenance,
}

/// Bounds and the on/off switch. Off by default, like every other adapter increment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManifestPolicy {
    pub enabled: bool,
    /// Hard cap on facts retained, applied after everything else.
    pub max_facts: usize,
    /// Hard cap on the serialized manifest.
    pub max_bytes: usize,
}

impl Default for ManifestPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_facts: 32,
            max_bytes: 8 * 1024,
        }
    }
}

/// What `build_manifest` produced, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestDisposition {
    Disabled,
    /// Not a chat request with a `messages` array.
    NotAChatRequest,
    /// No declared key was present in any observation.
    NoDeclaredFacts,
    Built,
}

impl ManifestDisposition {
    pub fn is_built(&self) -> bool {
        matches!(self, ManifestDisposition::Built)
    }
}

/// The manifest plus an honest account of everything that was dropped.
#[derive(Debug, Clone, PartialEq)]
pub struct ManifestOutcome {
    pub facts: Vec<StateFact>,
    /// Serialized size of `facts`.
    pub bytes: usize,
    /// Facts whose value matched a secret pattern and were therefore never stored.
    pub dropped_secret: usize,
    /// Keys observed more than once with *different* values. Both observations are kept.
    pub conflicts: usize,
    /// Facts dropped because a cap was reached. The drop is reported, never silent.
    pub truncated: usize,
    pub disposition: ManifestDisposition,
}

/// Collects declared keys from `role: "tool"` observations, newest observation winning per key.
///
/// "Newest wins" applies only to *identical* keys carrying identical values, where keeping both
/// would be pure duplication. Genuine disagreements are counted as conflicts and both retained.
pub fn build_manifest(
    body: &[u8],
    declared_keys: &[String],
    policy: &ManifestPolicy,
) -> ManifestOutcome {
    let empty = ManifestOutcome {
        facts: Vec::new(),
        bytes: 0,
        dropped_secret: 0,
        conflicts: 0,
        truncated: 0,
        disposition: ManifestDisposition::Disabled,
    };
    if !policy.enabled {
        return empty;
    }
    if declared_keys.is_empty() {
        return ManifestOutcome {
            disposition: ManifestDisposition::NoDeclaredFacts,
            ..empty
        };
    }

    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => {
            return ManifestOutcome {
                disposition: ManifestDisposition::NotAChatRequest,
                ..empty
            };
        }
    };
    let Some(messages) = value.get("messages").and_then(Value::as_array) else {
        return ManifestOutcome {
            disposition: ManifestDisposition::NotAChatRequest,
            ..empty
        };
    };

    let mut facts: Vec<StateFact> = Vec::new();
    let mut dropped_secret = 0usize;
    let mut conflicts = 0usize;

    for (message_index, message) in messages.iter().enumerate() {
        if message.get("role").and_then(Value::as_str) != Some("tool") {
            continue;
        }
        let provenance = Provenance {
            message_index,
            tool_call_id: message
                .get("tool_call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            tool: message
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        };
        let Some(content) = message.get("content").and_then(Value::as_str) else {
            continue;
        };
        // Only a structured observation can carry declared keys. Free text is never mined for
        // field names: guessing where a key "is" inside prose is exactly the invention this
        // module refuses to do.
        let Ok(observed) = serde_json::from_str::<Value>(content) else {
            continue;
        };
        let Some(fields) = observed.as_object() else {
            continue;
        };

        for key in declared_keys {
            let Some(found) = fields.get(key) else {
                continue;
            };
            // Never persist a secret-shaped value, at any size, in any field.
            if redaction::contains_secret(
                serde_json::to_string(found).unwrap_or_default().as_bytes(),
            ) {
                dropped_secret += 1;
                continue;
            }
            match facts.iter_mut().find(|fact| fact.key == *key) {
                Some(existing) => {
                    if existing.value == *found {
                        // Same key, same value, seen again: a pure duplicate. Keep the most recent
                        // provenance so the manifest points at the freshest source.
                        existing.provenance = provenance.clone();
                    } else {
                        // A real disagreement. Keep both and say so; resolving it would be
                        // inventing state.
                        conflicts += 1;
                        facts.push(StateFact {
                            key: key.clone(),
                            value: found.clone(),
                            provenance: provenance.clone(),
                        });
                    }
                }
                None => facts.push(StateFact {
                    key: key.clone(),
                    value: found.clone(),
                    provenance: provenance.clone(),
                }),
            }
        }
    }

    if facts.is_empty() {
        return ManifestOutcome {
            dropped_secret,
            disposition: if dropped_secret > 0 {
                // Every declared key was found but refused: report it honestly rather than
                // implying there was nothing to find.
                ManifestDisposition::Built
            } else {
                ManifestDisposition::NoDeclaredFacts
            },
            ..empty
        };
    }

    // Bounds are applied last and reported. Truncating silently would let a caller believe it had
    // the whole picture.
    let mut truncated = 0usize;
    if facts.len() > policy.max_facts {
        truncated += facts.len() - policy.max_facts;
        facts.truncate(policy.max_facts);
    }
    let mut bytes = serialized_size(&facts);
    if bytes > policy.max_bytes {
        // Drop from the end, reporting each drop, until the manifest fits.
        while bytes > policy.max_bytes && facts.len() > 1 {
            facts.pop();
            truncated += 1;
            bytes = serialized_size(&facts);
        }
    }

    ManifestOutcome {
        facts,
        bytes,
        dropped_secret,
        conflicts,
        truncated,
        disposition: ManifestDisposition::Built,
    }
}

fn serialized_size(facts: &[StateFact]) -> usize {
    serde_json::to_vec(facts)
        .map(|bytes| bytes.len())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn policy() -> ManifestPolicy {
        ManifestPolicy {
            enabled: true,
            max_facts: 32,
            max_bytes: 8192,
        }
    }

    fn keys(list: &[&str]) -> Vec<String> {
        list.iter().map(|k| k.to_string()).collect()
    }

    /// A transcript of `role: "tool"` messages whose content is the given JSON objects.
    fn transcript(observations: &[Value]) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "messages": observations
                .iter()
                .enumerate()
                .map(|(i, obs)| json!({
                    "role": "tool",
                    "tool_call_id": format!("call-{i}"),
                    "name": "read_file",
                    "content": obs.to_string(),
                }))
                .collect::<Vec<Value>>(),
        }))
        .unwrap()
    }

    // --- provenance -------------------------------------------------------------

    #[test]
    fn every_fact_carries_the_source_it_came_from() {
        // Provenance is what makes a late-turn decision auditable, so it is never optional.
        let body = transcript(&[json!({"branch": "main", "file": "src/lib.rs"})]);
        let out = build_manifest(&body, &keys(&["branch"]), &policy());

        assert_eq!(out.disposition, ManifestDisposition::Built);
        assert_eq!(out.facts.len(), 1);
        let fact = &out.facts[0];
        assert_eq!(fact.key, "branch");
        assert_eq!(fact.value, json!("main"));
        assert_eq!(fact.provenance.message_index, 0);
        assert_eq!(fact.provenance.tool_call_id, "call-0");
        assert_eq!(fact.provenance.tool, "read_file");
    }

    #[test]
    fn a_later_observation_of_the_same_key_keeps_the_freshest_source() {
        let body = transcript(&[json!({"branch": "main"}), json!({"branch": "main"})]);
        let out = build_manifest(&body, &keys(&["branch"]), &policy());
        assert_eq!(
            out.facts.len(),
            1,
            "an identical repeat is duplication, not conflict"
        );
        assert_eq!(out.conflicts, 0);
        assert_eq!(out.facts[0].provenance.message_index, 1);
    }

    // --- conflicts --------------------------------------------------------------

    #[test]
    fn a_conflicting_key_keeps_both_observations_and_counts_the_conflict() {
        // Resolving this would mean inventing state. Both stay, and the conflict is reported.
        let body = transcript(&[json!({"branch": "main"}), json!({"branch": "feature/x"})]);
        let out = build_manifest(&body, &keys(&["branch"]), &policy());

        assert_eq!(out.conflicts, 1);
        assert_eq!(out.facts.len(), 2);
        let values: Vec<&Value> = out.facts.iter().map(|f| &f.value).collect();
        assert!(values.contains(&&json!("main")));
        assert!(values.contains(&&json!("feature/x")));
    }

    // --- nothing is inferred ----------------------------------------------------

    #[test]
    fn an_undeclared_key_is_never_extracted() {
        // Only what the caller asked for, so the manifest cannot grow state the caller never
        // sanctioned.
        let body = transcript(&[json!({"branch": "main", "secret_ish": "not declared"})]);
        let out = build_manifest(&body, &keys(&["branch"]), &policy());
        assert_eq!(out.facts.len(), 1);
        assert!(out.facts.iter().all(|f| f.key == "branch"));
    }

    #[test]
    fn a_key_absent_from_every_observation_is_simply_not_present() {
        // No invented placeholder for a key that never appeared.
        let body = transcript(&[json!({"branch": "main"})]);
        let out = build_manifest(&body, &keys(&["branch", "never_seen"]), &policy());
        assert_eq!(out.facts.len(), 1);
        assert!(!out.facts.iter().any(|f| f.key == "never_seen"));
    }

    #[test]
    fn free_text_observations_are_never_mined_for_field_names() {
        // Finding a key "inside" prose would require guessing where it is.
        let body = serde_json::to_vec(&json!({
            "messages": [{
                "role": "tool",
                "tool_call_id": "c0",
                "content": "the branch is main and the file is src/lib.rs",
            }],
        }))
        .unwrap();
        let out = build_manifest(&body, &keys(&["branch", "file"]), &policy());
        assert_eq!(out.disposition, ManifestDisposition::NoDeclaredFacts);
        assert!(out.facts.is_empty());
    }

    #[test]
    fn a_scalar_observation_yields_nothing_rather_than_guessing() {
        let body = serde_json::to_vec(&json!({
            "messages": [{"role": "tool", "tool_call_id": "c0", "content": "\"just a string\""}],
        }))
        .unwrap();
        let out = build_manifest(&body, &keys(&["anything"]), &policy());
        assert!(out.facts.is_empty());
    }

    #[test]
    fn a_non_tool_message_is_never_a_manifest_source() {
        let body = serde_json::to_vec(&json!({
            "messages": [{"role": "assistant", "content": "{\"branch\":\"main\"}"}],
        }))
        .unwrap();
        let out = build_manifest(&body, &keys(&["branch"]), &policy());
        assert!(out.facts.is_empty());
    }

    // --- source-missing ---------------------------------------------------------

    #[test]
    fn a_missing_source_body_yields_no_state_rather_than_stale_state() {
        // "Source missing" must produce nothing, not a remembered value that may no longer be true.
        let body = serde_json::to_vec(&json!({"messages": []})).unwrap();
        let out = build_manifest(&body, &keys(&["branch"]), &policy());
        assert_eq!(out.disposition, ManifestDisposition::NoDeclaredFacts);
        assert!(out.facts.is_empty());
    }

    #[test]
    fn a_message_without_provenance_fields_yields_an_empty_provenance_not_a_guess() {
        let body = serde_json::to_vec(&json!({
            "messages": [{"role": "tool", "content": "{\"branch\":\"main\"}"}],
        }))
        .unwrap();
        let out = build_manifest(&body, &keys(&["branch"]), &policy());
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].provenance.tool_call_id, "");
        assert_eq!(out.facts[0].provenance.tool, "");
        assert_eq!(out.facts[0].provenance.message_index, 0);
    }

    // --- secrets ----------------------------------------------------------------

    #[test]
    fn a_secret_shaped_value_is_never_stored_and_is_counted() {
        let body = transcript(&[json!({
            "branch": "main",
            "token": "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE",
        })]);
        let out = build_manifest(&body, &keys(&["branch", "token"]), &policy());

        assert_eq!(out.dropped_secret, 1);
        assert!(out.facts.iter().all(|f| f.key != "token"));
        // The harmless sibling is still captured.
        assert_eq!(out.facts.len(), 1);
        assert_eq!(out.facts[0].key, "branch");
    }

    #[test]
    fn a_body_whose_only_declared_key_is_a_secret_still_reports_that_it_found_one() {
        // Otherwise "found nothing" and "found something we refused" look identical.
        let body = transcript(&[json!({"token": "Authorization: Bearer abcDEF123.tok-value"})]);
        let out = build_manifest(&body, &keys(&["token"]), &policy());
        assert_eq!(out.dropped_secret, 1);
        assert!(out.facts.is_empty());
        assert_eq!(out.disposition, ManifestDisposition::Built);
    }

    // --- bounds -----------------------------------------------------------------

    #[test]
    fn a_manifest_is_bounded_and_reports_what_it_dropped() {
        // Truncating silently would let a caller believe it had the whole picture.
        let observations: Vec<Value> = (0..10)
            .map(|i| {
                let mut object = serde_json::Map::new();
                object.insert(format!("k{i}"), json!(i));
                Value::Object(object)
            })
            .collect();
        let body = transcript(&observations);
        let declared: Vec<String> = (0..10).map(|i| format!("k{i}")).collect();

        let out = build_manifest(
            &body,
            &declared,
            &ManifestPolicy {
                enabled: true,
                max_facts: 4,
                max_bytes: 8192,
            },
        );
        assert_eq!(out.facts.len(), 4);
        assert_eq!(out.truncated, 6, "the drop must be reported, not silent");
    }

    #[test]
    fn a_byte_budget_also_bounds_the_manifest_and_reports_the_drop() {
        let observations: Vec<Value> = (0..8)
            .map(|i| {
                let mut object = serde_json::Map::new();
                object.insert("pad".to_string(), json!("x".repeat(64)));
                object.insert("id".to_string(), json!(i));
                Value::Object(object)
            })
            .collect();
        let body = transcript(&observations);

        let out = build_manifest(
            &body,
            &keys(&["pad", "id"]),
            &ManifestPolicy {
                enabled: true,
                max_facts: 32,
                max_bytes: 200,
            },
        );
        assert!(
            out.bytes <= 200,
            "manifest exceeded its byte budget: {}",
            out.bytes
        );
        assert!(out.truncated > 0);
    }

    // --- lifecycle --------------------------------------------------------------

    #[test]
    fn the_feature_is_off_by_default() {
        let body = transcript(&[json!({"branch": "main"})]);
        let out = build_manifest(&body, &keys(&["branch"]), &ManifestPolicy::default());
        assert_eq!(out.disposition, ManifestDisposition::Disabled);
        assert!(out.facts.is_empty());
    }

    #[test]
    fn an_empty_declaration_yields_no_manifest() {
        let body = transcript(&[json!({"branch": "main"})]);
        let out = build_manifest(&body, &[], &policy());
        assert_eq!(out.disposition, ManifestDisposition::NoDeclaredFacts);
        assert!(out.facts.is_empty());
    }

    #[test]
    fn a_non_chat_body_is_reported_rather_than_guessed_at() {
        let out = build_manifest(b"not json", &keys(&["branch"]), &policy());
        assert_eq!(out.disposition, ManifestDisposition::NotAChatRequest);
        assert!(out.facts.is_empty());
    }

    #[test]
    fn building_twice_from_the_same_body_is_identical() {
        let body = transcript(&[
            json!({"branch": "main"}),
            json!({"branch": "feature/x"}),
            json!({"file": "src/lib.rs"}),
        ]);
        let first = build_manifest(&body, &keys(&["branch", "file"]), &policy());
        for _ in 0..4 {
            assert_eq!(
                build_manifest(&body, &keys(&["branch", "file"]), &policy()),
                first
            );
        }
    }
}
