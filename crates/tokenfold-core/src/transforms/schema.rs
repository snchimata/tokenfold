//! `schema_compaction` (canonical transform id: `"schema_compaction"`, version `1.0.0`): a
//! semantics-preserving transform for JSON payloads containing JSON-Schema-shaped fragments
//! and/or OpenAI-style tool/function definitions.
//!
//! It never drops required semantic information — `description`, `required`, `enum`, `type`,
//! `default`, and `name` fields (and every other field) survive byte-for-byte. The only thing
//! this transform ever shortens is illustrative `"examples"` arrays, which are truncated down
//! to a configured maximum length wherever they appear in the document.

use serde_json::Value;

/// Canonical stable id for this transform, as used in reports and `--disable`.
pub const TRANSFORM_ID: &str = "schema_compaction";
/// Semantic version of this transform's behavior (see `crate::modes` for the pipeline-wide
/// version table this must stay in sync with).
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// Errors produced while compacting a schema/tool-definition payload.
#[derive(Debug, thiserror::Error)]
pub enum SchemaCompactionError {
    #[error("invalid json: {0}")]
    Invalid(#[from] serde_json::Error),
}

/// Parses `input` as JSON, truncates every `"examples"` array found anywhere in the document
/// (object or array nesting, at any depth) down to at most `max_examples` elements — keeping
/// the first `max_examples` elements in their original order — and re-serializes the result in
/// compact form.
///
/// Every other key and value in the document (including `description`, `required`, `enum`,
/// `type`, `default`, and tool/function `name` fields) is left exactly as parsed: this function
/// never renames, removes, or reorders anything other than shortening `"examples"` arrays.
///
/// Returns [`SchemaCompactionError::Invalid`] if `input` is not valid JSON.
pub fn compact_schema(input: &[u8], max_examples: usize) -> Result<Vec<u8>, SchemaCompactionError> {
    let mut value: Value = serde_json::from_slice(input)?;
    shorten_examples(&mut value, max_examples);
    let bytes = serde_json::to_vec(&value)?;
    Ok(bytes)
}

/// Recursively walks `value`, truncating any `"examples"` array (on any object, at any depth)
/// to `max_examples` elements. Objects without an `"examples"` key, and every scalar
/// (string/number/bool/null), are left untouched.
fn shorten_examples(value: &mut Value, max_examples: usize) {
    match value {
        Value::Object(map) => {
            if let Some(Value::Array(examples)) = map.get_mut("examples") {
                examples.truncate(max_examples);
            }
            for v in map.values_mut() {
                shorten_examples(v, max_examples);
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                shorten_examples(v, max_examples);
            }
        }
        Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn description_field_preserved_byte_for_byte() {
        let input = json!({
            "name": "get_weather",
            "description": "Fetches the current weather conditions for a named city or postal code.",
            "parameters": {
                "type": "object",
                "properties": {
                    "location": { "type": "string" }
                }
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 3).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            parsed["description"],
            json!("Fetches the current weather conditions for a named city or postal code.")
        );
    }

    #[test]
    fn required_array_preserved() {
        let input = json!({
            "parameters": {
                "type": "object",
                "properties": {
                    "a": { "type": "string" },
                    "b": { "type": "number" }
                }
            },
            "required": ["a", "b"]
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 2).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["required"], json!(["a", "b"]));
    }

    #[test]
    fn enum_array_preserved() {
        let input = json!({
            "parameters": {
                "properties": {
                    "unit": { "type": "string", "enum": ["celsius", "fahrenheit"] }
                }
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 1).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            parsed["parameters"]["properties"]["unit"]["enum"],
            json!(["celsius", "fahrenheit"])
        );
    }

    #[test]
    fn type_field_preserved() {
        let input = json!({
            "parameters": {
                "type": "object",
                "properties": {}
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 1).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["parameters"]["type"], json!("object"));
    }

    #[test]
    fn default_field_preserved() {
        let input = json!({
            "parameters": {
                "properties": {
                    "count": { "type": "integer", "default": 10 }
                }
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 1).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(
            parsed["parameters"]["properties"]["count"]["default"],
            json!(10)
        );
    }

    #[test]
    fn examples_array_shortened_to_max_examples_keeping_first_elements() {
        let input = json!({
            "parameters": {
                "examples": ["one", "two", "three", "four", "five"]
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 1).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["parameters"]["examples"], json!(["one"]));
    }

    #[test]
    fn examples_array_shorter_than_max_is_left_unchanged() {
        let input = json!({
            "examples": ["only-one"]
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 5).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["examples"], json!(["only-one"]));
    }

    #[test]
    fn tool_name_field_preserved() {
        let input = json!({
            "name": "search_flights",
            "parameters": {
                "type": "object",
                "properties": {}
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 1).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["name"], json!("search_flights"));
    }

    #[test]
    fn invalid_json_input_returns_error() {
        let result = compact_schema(b"{not json", 1);

        assert!(matches!(result, Err(SchemaCompactionError::Invalid(_))));
    }

    #[test]
    fn document_with_no_examples_key_round_trips_without_panic() {
        let input = json!({
            "name": "lookup_stock_price",
            "description": "Looks up the latest known stock price for a given ticker symbol.",
            "parameters": {
                "type": "object",
                "properties": {
                    "ticker": { "type": "string" }
                },
                "required": ["ticker"]
            }
        });
        let bytes = serde_json::to_vec(&input).unwrap();

        let out = compact_schema(&bytes, 3).unwrap();
        let parsed: Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed, input);
    }
}
