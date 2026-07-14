//! `json_field_fold` transform (canonical id `"json_field_fold"`, v1.0.0).
//!
//! Content-aware, **losslessly reversible** structural compression for generic JSON data
//! (`InputFormat::Json`). Where `json_minify` only removes whitespace, this transform removes
//! the dominant source of token waste in data-JSON: **repeated object keys**. An array of N
//! objects that all share the same keys emits each key N times; folding rewrites it to a
//! columnar form that emits each key exactly once.
//!
//! ```text
//! [ {"id":1,"role":"member"}, {"id":2,"role":"member"} ]
//!   -> {"__tf_cols__":["id","role"],"__tf_rows__":[[1,"member"],[2,"member"]]}
//! ```
//!
//! The fold is applied recursively (values are folded before their enclosing array is), so
//! nested arrays-of-objects fold too. It is a pure structural rewrite — every value is
//! preserved — so `unfold_json` reconstructs the original data exactly (as a `serde_json`
//! value; number *spelling* like `1e10` may be renormalized by re-serialization, but the
//! numeric value is unchanged). The pipeline gates adoption on that round-trip
//! (`round_trips`), so a fold that would ever lose data is rolled back rather than emitted.

use serde_json::{Map, Value};

/// Canonical transform id, as registered with the pipeline.
pub const TRANSFORM_ID: &str = "json_field_fold";

/// Semantic version of this transform's output behavior.
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// Reserved keys marking a folded array. Chosen to be collision-unlikely in real data; any
/// actual collision is caught by the pipeline's round-trip safety gate, never corrupts data.
const COLS: &str = "__tf_cols__";
const ROWS: &str = "__tf_rows__";

/// Minimum array length worth folding. Below this the `{cols,rows}` framing overhead can
/// exceed the saved keys; the pipeline also rolls back any net regression, so this is only a
/// cheap early-out, not the correctness boundary.
const MIN_ROWS: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum JsonFoldError {
    #[error("invalid json: {0}")]
    Invalid(#[from] serde_json::Error),
}

/// Folds arrays of homogeneous objects in `input` into columnar `{__tf_cols__, __tf_rows__}`
/// form. No-op-safe: empty input returns empty; input with no foldable arrays returns a
/// (compact) re-serialization with the same data.
pub fn fold_json(input: &[u8]) -> Result<Vec<u8>, JsonFoldError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(input)?;
    let folded = fold_value(&value);
    Ok(serde_json::to_vec(&folded)?)
}

/// Inverse of [`fold_json`]: expands every `{__tf_cols__, __tf_rows__}` node back into an
/// array of objects. This is the reversible half that makes the transform safe.
pub fn unfold_json(input: &[u8]) -> Result<Vec<u8>, JsonFoldError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(input)?;
    let unfolded = unfold_value(&value);
    Ok(serde_json::to_vec(&unfolded)?)
}

/// True iff unfolding `after` reproduces `before` exactly (as JSON values). The pipeline's
/// safety gate for this transform — folding is only adopted when this holds.
pub fn round_trips(before: &[u8], after: &[u8]) -> bool {
    let (Ok(before_v), Ok(after_v)) = (
        serde_json::from_slice::<Value>(before),
        serde_json::from_slice::<Value>(after),
    ) else {
        return false;
    };
    unfold_value(&after_v) == before_v
}

fn fold_value(v: &Value) -> Value {
    match v {
        Value::Array(items) => {
            let folded: Vec<Value> = items.iter().map(fold_value).collect();
            try_fold_array(&folded).unwrap_or(Value::Array(folded))
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, val) in map {
                out.insert(k.clone(), fold_value(val));
            }
            Value::Object(out)
        }
        _ => v.clone(),
    }
}

/// Returns the columnar node if `items` is a foldable array (>= MIN_ROWS objects sharing the
/// same key set, none empty, none already using the reserved markers), else `None`.
fn try_fold_array(items: &[Value]) -> Option<Value> {
    if items.len() < MIN_ROWS {
        return None;
    }
    let first = items[0].as_object()?;
    if first.is_empty() || first.contains_key(COLS) || first.contains_key(ROWS) {
        return None;
    }
    // Column order is the first object's key order; membership is a set check so objects that
    // carry the same keys in a different order still fold (values are placed by key, so this
    // stays lossless).
    let cols: Vec<String> = first.keys().cloned().collect();
    for item in items {
        let obj = item.as_object()?;
        if obj.len() != cols.len() || !cols.iter().all(|k| obj.contains_key(k)) {
            return None;
        }
    }
    let rows: Vec<Value> = items
        .iter()
        .map(|item| {
            let obj = item.as_object().expect("checked above");
            Value::Array(cols.iter().map(|k| obj[k].clone()).collect())
        })
        .collect();
    let mut folded = Map::new();
    folded.insert(
        COLS.to_string(),
        Value::Array(cols.into_iter().map(Value::String).collect()),
    );
    folded.insert(ROWS.to_string(), Value::Array(rows));
    Some(Value::Object(folded))
}

fn unfold_value(v: &Value) -> Value {
    match v {
        Value::Object(map) => {
            if let Some(arr) = try_unfold_node(map) {
                arr
            } else {
                let mut out = Map::new();
                for (k, val) in map {
                    out.insert(k.clone(), unfold_value(val));
                }
                Value::Object(out)
            }
        }
        Value::Array(items) => Value::Array(items.iter().map(unfold_value).collect()),
        _ => v.clone(),
    }
}

/// Expands one `{__tf_cols__, __tf_rows__}` node back to an array of objects, or `None` if
/// `map` isn't a well-formed folded node (wrong keys, wrong types, or a row whose width
/// doesn't match the column count — all of which mean it's real data, not our framing).
fn try_unfold_node(map: &Map<String, Value>) -> Option<Value> {
    if map.len() != 2 {
        return None;
    }
    let cols = map.get(COLS)?.as_array()?;
    let rows = map.get(ROWS)?.as_array()?;
    let col_names: Vec<&str> = cols.iter().map(|c| c.as_str()).collect::<Option<_>>()?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let vals = row.as_array()?;
        if vals.len() != col_names.len() {
            return None;
        }
        let mut obj = Map::new();
        for (name, val) in col_names.iter().zip(vals) {
            obj.insert((*name).to_string(), unfold_value(val));
        }
        out.push(Value::Object(obj));
    }
    Some(Value::Array(out))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    #[test]
    fn folds_homogeneous_array_and_removes_repeated_keys() {
        let input = br#"[{"id":1,"role":"member"},{"id":2,"role":"member"}]"#;
        let out = fold_json(input).unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        assert!(s.contains("__tf_cols__"), "expected folded form, got {s}");
        // "role" appears once as a column, not once per row.
        assert_eq!(s.matches("role").count(), 1);
        // and it round-trips
        assert!(round_trips(input, &out));
    }

    #[test]
    fn fold_then_unfold_reproduces_original_value() {
        let input = br#"{"count":2,"results":[{"a":1,"b":2},{"a":3,"b":4}]}"#;
        let folded = fold_json(input).unwrap();
        let unfolded = unfold_json(&folded).unwrap();
        assert_eq!(parse(&unfolded), parse(input));
    }

    #[test]
    fn nested_arrays_of_objects_fold_too() {
        let input = br#"{"groups":[{"users":[{"id":1},{"id":2}]},{"users":[{"id":3},{"id":4}]}]}"#;
        let out = fold_json(input).unwrap();
        assert!(round_trips(input, &out));
        // both the outer "groups" array and each inner "users" array are homogeneous → folded.
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("__tf_cols__"));
    }

    #[test]
    fn heterogeneous_array_is_left_alone() {
        // different key sets → not foldable, must stay a plain array (still round-trips).
        let input = br#"[{"a":1},{"b":2}]"#;
        let out = fold_json(input).unwrap();
        assert_eq!(parse(&out), parse(input));
        assert!(!String::from_utf8(out).unwrap().contains("__tf_cols__"));
    }

    #[test]
    fn array_of_non_objects_is_left_alone() {
        let input = br#"[1,2,3,4]"#;
        let out = fold_json(input).unwrap();
        assert_eq!(parse(&out), parse(input));
    }

    #[test]
    fn single_object_array_is_not_folded() {
        let input = br#"[{"a":1,"b":2}]"#;
        let out = fold_json(input).unwrap();
        assert!(!String::from_utf8(out).unwrap().contains("__tf_cols__"));
    }

    #[test]
    fn same_keys_different_order_still_folds_losslessly() {
        let input = br#"[{"a":1,"b":2},{"b":4,"a":3}]"#;
        let out = fold_json(input).unwrap();
        assert!(round_trips(input, &out));
    }

    #[test]
    fn source_data_using_reserved_markers_does_not_round_trip_falsely() {
        // If real data already looks like our framing, folding must not silently corrupt it.
        // Here the array isn't foldable anyway; the point is unfold doesn't misread real data
        // that happens to be a genuine 2-key object with other names.
        let input = br#"{"__tf_cols__":["x"],"__tf_rows__":[[1]]}"#;
        // This *is* a valid folded node shape; unfold would expand it. round_trips guards the
        // pipeline: folding this input produces identical bytes (nothing foldable), and
        // unfold(after) != before, so the pipeline would not adopt a fold here.
        let folded = fold_json(input).unwrap();
        assert!(!round_trips(input, &folded));
    }

    #[test]
    fn empty_input_is_a_noop() {
        assert!(fold_json(b"").unwrap().is_empty());
        assert!(unfold_json(b"").unwrap().is_empty());
    }

    #[test]
    fn invalid_json_is_rejected() {
        assert!(fold_json(b"{not json").is_err());
    }

    #[test]
    fn numbers_and_nulls_survive_the_round_trip() {
        let input = br#"[{"n":1.5,"z":null,"b":true},{"n":2.5,"z":null,"b":false}]"#;
        let out = fold_json(input).unwrap();
        assert!(round_trips(input, &out));
    }

    use proptest::prelude::*;

    // Arbitrary JSON (integer numbers only, to keep value-equality exact — float spelling is
    // the one thing re-serialization may renormalize, and it's covered by the unit test above).
    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| Value::Number(n.into())),
            "[^\"\\\\]{0,12}".prop_map(Value::String),
        ];
        leaf.prop_recursive(4, 48, 6, |inner| {
            prop_oneof![
                prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
                prop::collection::hash_map("[a-z]{1,6}", inner, 0..6)
                    .prop_map(|m| Value::Object(m.into_iter().collect())),
            ]
        })
    }

    proptest! {
        // The core safety guarantee: folding never loses data. For ANY JSON value, unfolding
        // the folded form reproduces it exactly, and round_trips() (the pipeline's gate) agrees.
        #[test]
        fn fold_then_unfold_is_the_identity_on_arbitrary_json(v in arb_json()) {
            let bytes = serde_json::to_vec(&v).unwrap();
            let folded = fold_json(&bytes).unwrap();
            prop_assert!(round_trips(&bytes, &folded), "round_trips() rejected a valid fold");
            let unfolded = unfold_json(&folded).unwrap();
            let back: Value = serde_json::from_slice(&unfolded).unwrap();
            prop_assert_eq!(back, v);
        }
    }
}
