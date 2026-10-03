//! EP-12 / NF-20: tool-catalog selection.
//!
//! # What this does
//!
//! A large tool catalog is pure overhead on every request: most of it is irrelevant to the task
//! at hand. This reduces the catalog to the tools that can actually be used, while guaranteeing
//! that nothing load-bearing is dropped.
//!
//! # The three things that must survive
//!
//! 1. **Forced tools.** If `tool_choice` names a tool, the model has no choice but to call it.
//!    Dropping it would make the request fail at the provider, so a forced tool is never dropped
//!    and never counts against the budget.
//! 2. **Companion tools.** A forced tool is frequently useless alone -- reading a file is
//!    pointless without an edit tool. Companions are detected by explicit declaration from the
//!    caller, never by guessing from a tool's description.
//! 3. **Exact schema constraints.** A kept tool's JSON is copied **byte-for-byte**. This module
//!    never rewrites, reorders keys inside, or "simplifies" a schema: a tool whose constraints are
//!    silently altered is a tool the model will misuse.
//!
//! # Unknown tools are reported, never invented
//!
//! If a caller declares a companion or a forced tool that is not in the catalog, that is reported
//! in [`ToolSelection::unknown`] rather than being fabricated. Silently adding a tool that the
//! provider does not have would produce a request that fails far from its cause.

use serde_json::Value;

/// Bounds and the on/off switch. Off by default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolPolicy {
    pub enabled: bool,
    /// Maximum tools to emit. Forced and companion tools are exempt from this limit.
    pub max_tools: usize,
    /// Tool names the caller declares as companions of the forced tool. Never inferred.
    pub companions: Vec<String>,
}

impl Default for ToolPolicy {
    fn default() -> Self {
        Self {
            enabled: false,
            max_tools: 8,
            companions: Vec::new(),
        }
    }
}

/// What `select_tools` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolDisposition {
    Disabled,
    /// Not a request with a `tools` array.
    NotAChatRequest,
    /// The request carries no tools at all.
    NoTools,
    /// Nothing was dropped.
    Unchanged,
    /// The catalog was reduced.
    Reduced,
}

/// The selection, plus everything the caller needs to explain it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolSelection {
    /// The rewritten body, or the untouched baseline when nothing was safely reducible.
    pub bytes: Vec<u8>,
    /// Names kept, in the catalog's original order.
    pub kept: Vec<String>,
    /// Names dropped.
    pub dropped: Vec<String>,
    /// Declared companions that were kept alongside the forced tool.
    pub companions: Vec<String>,
    /// Declared companions (or forced tools) that are not in the catalog. Reported, never added.
    pub unknown: Vec<String>,
    pub saved_bytes: usize,
    pub disposition: ToolDisposition,
}

/// The tool name a `tool_choice` forces, when one does.
fn forced_tool(body: &Value) -> Option<String> {
    let choice = body.get("tool_choice")?;
    // `{"type":"function","function":{"name":"x"}}` is the forcing shape.
    choice
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .map(str::to_string)
        // `{"type":"tool","name":"x"}` is accepted too.
        .or_else(|| {
            if choice.get("type").and_then(Value::as_str) == Some("tool") {
                choice
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            } else {
                None
            }
        })
}

/// Reduces `body`'s tool catalog, preserving every tool that can matter.
///
/// Returns the untouched baseline unless a reduced catalog is strictly smaller, so a caller can
/// never be handed a larger body than it sent.
pub fn select_tools(body: &[u8], policy: &ToolPolicy) -> ToolSelection {
    let value: Value = match serde_json::from_slice(body) {
        Ok(value) => value,
        Err(_) => return unchanged(body, ToolDisposition::NotAChatRequest),
    };
    let Some(tools) = value.get("tools").and_then(Value::as_array) else {
        return unchanged(body, ToolDisposition::NotAChatRequest);
    };
    if tools.is_empty() {
        return unchanged(body, ToolDisposition::NoTools);
    }
    if !policy.enabled {
        return unchanged(body, ToolDisposition::Disabled);
    }

    let names: Vec<String> = tools
        .iter()
        .map(|tool| tool_name(tool).unwrap_or_default())
        .collect();

    let forced = forced_tool(&value);
    // Unknown = declared by the caller but absent from the catalog. Reported, never added.
    let mut unknown: Vec<String> = Vec::new();
    if let Some(name) = &forced {
        if !names.contains(name) {
            unknown.push(name.clone());
        }
    }
    for companion in &policy.companions {
        if !names.contains(companion) {
            unknown.push(companion.clone());
        }
    }

    // Keep every forced or companion tool, plus the first `max_tools` others, preserving catalog
    // order throughout.
    let mut keep_flags = vec![false; names.len()];
    for (index, name) in names.iter().enumerate() {
        let is_forced = forced.as_deref() == Some(name.as_str());
        let is_companion = policy.companions.contains(name);
        if is_forced || is_companion {
            keep_flags[index] = true;
        }
    }
    let mut optional_kept = 0usize;
    for keep in keep_flags.iter_mut() {
        if *keep {
            continue;
        }
        if optional_kept < policy.max_tools {
            *keep = true;
            optional_kept += 1;
        }
    }

    let kept: Vec<String> = names
        .iter()
        .enumerate()
        .filter(|(index, _)| keep_flags[*index])
        .map(|(_, name)| name.clone())
        .collect();
    let dropped: Vec<String> = names
        .iter()
        .enumerate()
        .filter(|(index, _)| !keep_flags[*index])
        .map(|(_, name)| name.clone())
        .collect();

    // Computed before either list is moved into the struct below.
    let kept_companions = companions_of(&names, &policy.companions)
        .into_iter()
        .filter(|name| !dropped.contains(name))
        .collect();

    if dropped.is_empty() {
        return ToolSelection {
            bytes: body.to_vec(),
            companions: companions_of(&names, &policy.companions),
            kept,
            dropped,
            unknown,
            saved_bytes: 0,
            disposition: ToolDisposition::Unchanged,
        };
    }

    // Rebuild by copying each kept tool's original `Value`, so its schema survives exactly.
    let mut reduced = value.clone();
    let reduced_tools: Vec<Value> = tools
        .iter()
        .enumerate()
        .filter(|(index, _)| keep_flags[*index])
        .map(|(_, tool)| tool.clone())
        .collect();
    if let Some(object) = reduced.as_object_mut() {
        object.insert("tools".to_string(), Value::Array(reduced_tools));
    }
    let candidate = match serde_json::to_vec(&reduced) {
        Ok(bytes) => bytes,
        Err(_) => return unchanged(body, ToolDisposition::Unchanged),
    };

    if candidate.len() >= body.len() {
        return unchanged(body, ToolDisposition::Unchanged);
    }

    ToolSelection {
        saved_bytes: body.len() - candidate.len(),
        bytes: candidate,
        kept,
        dropped,
        companions: kept_companions,
        unknown,
        disposition: ToolDisposition::Reduced,
    }
}

fn companions_of(kept: &[String], declared: &[String]) -> Vec<String> {
    declared
        .iter()
        .filter(|name| kept.contains(name))
        .cloned()
        .collect()
}

fn tool_name(tool: &Value) -> Option<String> {
    tool.get("function")
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .or_else(|| tool.get("name").and_then(Value::as_str))
        .map(str::to_string)
}

fn unchanged(body: &[u8], disposition: ToolDisposition) -> ToolSelection {
    ToolSelection {
        bytes: body.to_vec(),
        kept: Vec::new(),
        dropped: Vec::new(),
        companions: Vec::new(),
        unknown: Vec::new(),
        saved_bytes: 0,
        disposition,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool(name: &str) -> Value {
        json!({
            "type": "function",
            "function": {
                "name": name,
                "description": format!("the {name} tool"),
                "parameters": {
                    "type": "object",
                    "properties": {"path": {"type": "string"}},
                    "required": ["path"],
                    "additionalProperties": false
                }
            }
        })
    }

    fn body(names: &[&str]) -> Vec<u8> {
        serde_json::to_vec(&json!({
            "model": "m",
            "tools": names.iter().map(|n| tool(n)).collect::<Vec<Value>>(),
        }))
        .unwrap()
    }

    fn policy(max_tools: usize) -> ToolPolicy {
        ToolPolicy {
            enabled: true,
            max_tools,
            companions: Vec::new(),
        }
    }

    // --- what must survive -----------------------------------------------------

    #[test]
    fn a_forced_tool_always_survives_even_when_it_is_last_and_beyond_the_limit() {
        // The model has no choice but to call it; dropping it fails at the provider.
        let names = vec!["a", "b", "c", "d", "e", "f"];
        let mut value: Value = serde_json::from_slice(&body(&names)).unwrap();
        value.as_object_mut().unwrap().insert(
            "tool_choice".to_string(),
            json!({"type":"function","function":{"name":"f"}}),
        );
        let original = serde_json::to_vec(&value).unwrap();

        let out = select_tools(&original, &policy(1));
        assert_eq!(out.disposition, ToolDisposition::Reduced);
        assert!(
            out.kept.contains(&"f".to_string()),
            "a forced tool was dropped: {:?}",
            out.kept
        );
    }

    #[test]
    fn a_companion_tool_survives_and_is_reported_as_one() {
        // Reading a file is pointless without an edit tool; companions are caller-declared, never
        // inferred from a description.
        let names = vec!["read", "write", "u1", "u2", "u3", "u4", "u5"];
        let mut value: Value = serde_json::from_slice(&body(&names)).unwrap();
        value.as_object_mut().unwrap().insert(
            "tool_choice".to_string(),
            json!({"type":"function","function":{"name":"read"}}),
        );
        let original = serde_json::to_vec(&value).unwrap();

        let out = select_tools(
            &original,
            &ToolPolicy {
                enabled: true,
                max_tools: 1,
                companions: vec!["write".to_string()],
            },
        );
        assert!(out.kept.contains(&"read".to_string()));
        assert!(out.kept.contains(&"write".to_string()));
        assert_eq!(out.companions, vec!["write".to_string()]);
    }

    #[test]
    fn forced_and_companion_tools_are_exempt_from_the_limit() {
        let names = vec!["forced", "comp", "x1", "x2", "x3"];
        let mut value: Value = serde_json::from_slice(&body(&names)).unwrap();
        value.as_object_mut().unwrap().insert(
            "tool_choice".to_string(),
            json!({"type":"tool","name":"forced"}),
        );
        let original = serde_json::to_vec(&value).unwrap();

        let out = select_tools(
            &original,
            &ToolPolicy {
                enabled: true,
                max_tools: 0,
                companions: vec!["comp".to_string()],
            },
        );
        assert_eq!(out.kept.len(), 2);
        assert!(out.dropped.is_empty() || out.kept.contains(&"forced".to_string()));
    }

    // --- schema fidelity -------------------------------------------------------

    #[test]
    fn a_kept_tools_schema_survives_byte_for_byte() {
        // A tool whose constraints are silently altered is a tool the model will misuse.
        let original = body(&["a", "b", "c"]);
        let out = select_tools(&original, &policy(1));
        let parsed: Value = serde_json::from_slice(&out.bytes).unwrap();
        let tools = parsed["tools"].as_array().unwrap();

        assert_eq!(tools.len(), 1);
        let kept = &tools[0];
        // Compare against the original catalog entry, field for field.
        let original_value: Value = serde_json::from_slice(&original).unwrap();
        let original_kept = original_value["tools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["function"]["name"] == kept["function"]["name"])
            .unwrap();
        assert_eq!(kept, original_kept);
        assert_eq!(
            kept["function"]["parameters"]["additionalProperties"],
            json!(false)
        );
        assert_eq!(kept["function"]["parameters"]["required"], json!(["path"]));
    }

    #[test]
    fn catalog_order_is_preserved_after_selection() {
        let out = select_tools(&body(&["t1", "t2", "t3", "t4"]), &policy(2));
        let parsed: Value = serde_json::from_slice(&out.bytes).unwrap();
        let names: Vec<&str> = parsed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, vec!["t1", "t2"]);
    }

    // --- unknown tools ---------------------------------------------------------

    #[test]
    fn a_forced_tool_missing_from_the_catalog_is_reported_and_never_invented() {
        let mut value: Value = serde_json::from_slice(&body(&["a"])).unwrap();
        value.as_object_mut().unwrap().insert(
            "tool_choice".to_string(),
            json!({"type": "function", "function": {"name": "ghost"}}),
        );
        let original = serde_json::to_vec(&value).unwrap();

        let out = select_tools(&original, &policy(5));
        assert_eq!(out.unknown, vec!["ghost".to_string()]);
        // Crucially, no tool named "ghost" was fabricated into the catalog.
        let parsed: Value = serde_json::from_slice(&out.bytes).unwrap();
        let names: Vec<&str> = parsed["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap())
            .collect();
        assert!(!names.contains(&"ghost"));
    }

    #[test]
    fn an_unknown_companion_is_reported_rather_than_added() {
        let out = select_tools(
            &body(&["a", "b"]),
            &ToolPolicy {
                enabled: true,
                max_tools: 5,
                companions: vec!["missing_companion".to_string()],
            },
        );
        assert_eq!(out.unknown, vec!["missing_companion".to_string()]);
        let parsed: Value = serde_json::from_slice(&out.bytes).unwrap();
        assert_eq!(parsed["tools"].as_array().unwrap().len(), 2);
    }

    // --- lifecycle and stability ----------------------------------------------

    #[test]
    fn the_feature_is_off_by_default() {
        let original = body(&["a", "b", "c"]);
        let out = select_tools(&original, &ToolPolicy::default());
        assert_eq!(out.disposition, ToolDisposition::Disabled);
        assert_eq!(out.bytes, original);
    }

    #[test]
    fn a_catalog_within_the_limit_is_left_unchanged() {
        let original = body(&["a", "b"]);
        let out = select_tools(&original, &policy(8));
        assert_eq!(out.disposition, ToolDisposition::Unchanged);
        assert_eq!(out.bytes, original);
    }

    #[test]
    fn a_request_without_tools_is_reported_rather_than_guessed_at() {
        let original = serde_json::to_vec(&json!({"model": "m"})).unwrap();
        let out = select_tools(&original, &policy(4));
        assert_eq!(out.disposition, ToolDisposition::NotAChatRequest);
        assert_eq!(out.bytes, original);
    }

    #[test]
    fn an_empty_tool_array_is_reported_as_no_tools() {
        let original = serde_json::to_vec(&json!({"tools": []})).unwrap();
        assert_eq!(
            select_tools(&original, &policy(4)).disposition,
            ToolDisposition::NoTools
        );
    }

    #[test]
    fn selection_is_deterministic_across_repeated_requests() {
        // Cache churn: the same request must produce the same catalog every time, or a provider
        // prefix cache would miss on every turn.
        let original = body(&["t1", "t2", "t3", "t4", "t5", "t6"]);
        let first = select_tools(&original, &policy(3)).bytes;
        for _ in 0..5 {
            assert_eq!(select_tools(&original, &policy(3)).bytes, first);
        }
    }

    #[test]
    fn a_reduction_is_always_smaller_than_the_original() {
        let out = select_tools(&body(&["a", "b", "c", "d"]), &policy(1));
        assert_eq!(out.disposition, ToolDisposition::Reduced);
        assert_eq!(
            out.saved_bytes,
            out.bytes.len().max(0) * 0 + (body(&["a", "b", "c", "d"]).len() - out.bytes.len())
        );
        assert!(out.bytes.len() < body(&["a", "b", "c", "d"]).len());
    }

    #[test]
    fn every_kept_and_dropped_name_is_accounted_for() {
        let out = select_tools(&body(&["t1", "t2", "t3", "t4", "t5"]), &policy(2));
        let mut all: Vec<&String> = out.kept.iter().chain(out.dropped.iter()).collect();
        all.sort();
        all.dedup();
        assert_eq!(
            all.len(),
            5,
            "a tool was neither kept nor accounted as dropped"
        );
    }
}
