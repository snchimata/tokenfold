//! `json_value_dict` transform (canonical id `"json_value_dict"`, v1.0.0).
//!
//! Content-aware, **losslessly reversible** value deduplication for generic JSON data.
//! Where `json_field_fold` factors out repeated *keys*, this factors out repeated *values*:
//! the same large object/array/string appearing many times (a constant nested record, a
//! repeated timestamp, an identical config blob) is stored once in a dictionary and every
//! occurrence is replaced by a compact reference.
//!
//! ```text
//! {"a":{"x":1,"y":2,"z":3},"b":{"x":1,"y":2,"z":3}}
//!   -> {"__tf_dict__":[{"x":1,"y":2,"z":3}],
//!       "__tf_data__":{"a":{"__tf_ref__":0},"b":{"__tf_ref__":0}}}
//! ```
//!
//! It runs *after* `json_field_fold` in the pipeline, so it also collapses the repeated
//! nested values that folding surfaces across rows. Only values whose serialized form is
//! large enough that a reference is cheaper are dictionaried (see `MIN_VALUE_BYTES`), and the
//! pipeline's exact-token gate drops the whole transform if it fails to net a saving — so it
//! can never make a payload worse. Reversibility is guaranteed by `round_trips` (the pipeline
//! safety gate): a fold that wouldn't restore exactly is rolled back.

use std::collections::HashMap;

use serde_json::{Map, Value};

/// Canonical transform id, as registered with the pipeline.
pub const TRANSFORM_ID: &str = "json_value_dict";

/// Semantic version of this transform's output behavior.
pub const TRANSFORM_VERSION: &str = "1.0.0";

const DICT: &str = "__tf_dict__";
const DATA: &str = "__tf_data__";
const REF: &str = "__tf_ref__";

/// Minimum serialized byte length for a value to be worth dictionarying. A reference is
/// `{"__tf_ref__":N}` (~16 bytes), so shorter values would only grow; the exact-token gate is
/// the final arbiter, this is just a cheap pre-filter that keeps the transform net-positive.
const MIN_VALUE_BYTES: usize = 24;

/// A value must repeat at least this many times to be dictionaried.
const MIN_COUNT: usize = 2;

#[derive(Debug, thiserror::Error)]
pub enum JsonDictError {
    #[error("invalid json: {0}")]
    Invalid(#[from] serde_json::Error),
}

/// Replaces repeated large values in `input` with dictionary references. No-op-safe: empty
/// input returns empty; input with nothing worth dictionarying returns unchanged.
pub fn dict_json(input: &[u8]) -> Result<Vec<u8>, JsonDictError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(input)?;

    let mut counts: HashMap<String, usize> = HashMap::new();
    count_dictable(&value, &mut counts);

    let mut index: Vec<String> = Vec::new();
    let mut index_of: HashMap<String, usize> = HashMap::new();
    let data = replace(&value, &counts, &mut index, &mut index_of);

    if index.is_empty() {
        return Ok(input.to_vec());
    }
    let dict: Vec<Value> = index
        .iter()
        .map(|s| serde_json::from_str(s).expect("canonical string from our own to_string"))
        .collect();
    let mut wrapper = Map::new();
    wrapper.insert(DICT.to_string(), Value::Array(dict));
    wrapper.insert(DATA.to_string(), data);
    Ok(serde_json::to_vec(&Value::Object(wrapper))?)
}

/// Inverse of [`dict_json`]: expands every `{__tf_ref__: i}` back to `dict[i]`.
pub fn undict_json(input: &[u8]) -> Result<Vec<u8>, JsonDictError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = serde_json::from_slice(input)?;
    let restored = restore(&value);
    Ok(serde_json::to_vec(&restored)?)
}

/// True iff expanding `after` reproduces `before` exactly (as JSON values) — the pipeline's
/// safety gate for this transform.
pub fn round_trips(before: &[u8], after: &[u8]) -> bool {
    let (Ok(before_v), Ok(after_v)) = (
        serde_json::from_slice::<Value>(before),
        serde_json::from_slice::<Value>(after),
    ) else {
        return false;
    };
    restore(&after_v) == before_v
}

/// Canonical dictionary key for a value, or `None` if it isn't a dictionary candidate
/// (too small, or a scalar that a reference couldn't beat).
fn dict_key(v: &Value) -> Option<String> {
    if !matches!(v, Value::Object(_) | Value::Array(_) | Value::String(_)) {
        return None;
    }
    let key = serde_json::to_string(v).ok()?;
    (key.len() >= MIN_VALUE_BYTES).then_some(key)
}

fn count_dictable(v: &Value, counts: &mut HashMap<String, usize>) {
    if let Some(k) = dict_key(v) {
        *counts.entry(k).or_insert(0) += 1;
    }
    match v {
        Value::Object(m) => m.values().for_each(|val| count_dictable(val, counts)),
        Value::Array(a) => a.iter().for_each(|item| count_dictable(item, counts)),
        _ => {}
    }
}

/// Top-down replacement: the outermost repeated value at any position becomes a reference and
/// is not descended into (so a repeated parent object swallows its children rather than
/// leaving dead dictionary entries). Indices are assigned lazily, so only referenced values
/// end up in the dictionary.
fn replace(
    v: &Value,
    counts: &HashMap<String, usize>,
    index: &mut Vec<String>,
    index_of: &mut HashMap<String, usize>,
) -> Value {
    if let Some(k) = dict_key(v)
        && counts.get(&k).copied().unwrap_or(0) >= MIN_COUNT
    {
        let idx = *index_of.entry(k.clone()).or_insert_with(|| {
            index.push(k);
            index.len() - 1
        });
        let mut r = Map::new();
        r.insert(REF.to_string(), Value::from(idx as u64));
        return Value::Object(r);
    }
    match v {
        Value::Object(m) => {
            let mut o = Map::new();
            for (kk, val) in m {
                o.insert(kk.clone(), replace(val, counts, index, index_of));
            }
            Value::Object(o)
        }
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|it| replace(it, counts, index, index_of))
                .collect(),
        ),
        _ => v.clone(),
    }
}

/// Expands a whole document: if it's our `{__tf_dict__, __tf_data__}` wrapper, resolve every
/// reference in the data against the dictionary; otherwise return it unchanged.
fn restore(v: &Value) -> Value {
    let Some((dict, data)) = as_wrapper(v) else {
        return v.clone();
    };
    expand(data, dict)
}

fn as_wrapper(v: &Value) -> Option<(&Vec<Value>, &Value)> {
    let m = v.as_object()?;
    if m.len() != 2 {
        return None;
    }
    let dict = m.get(DICT)?.as_array()?;
    let data = m.get(DATA)?;
    Some((dict, data))
}

fn expand(v: &Value, dict: &[Value]) -> Value {
    if let Some(idx) = as_ref(v) {
        // Dictionary entries are stored ref-free, so this resolves in one step; out-of-range
        // indices are left as-is (the round-trip gate then rejects the fold).
        return dict.get(idx).cloned().unwrap_or_else(|| v.clone());
    }
    match v {
        Value::Object(m) => {
            let mut o = Map::new();
            for (k, val) in m {
                o.insert(k.clone(), expand(val, dict));
            }
            Value::Object(o)
        }
        Value::Array(a) => Value::Array(a.iter().map(|it| expand(it, dict)).collect()),
        _ => v.clone(),
    }
}

fn as_ref(v: &Value) -> Option<usize> {
    let m = v.as_object()?;
    if m.len() != 1 {
        return None;
    }
    Some(m.get(REF)?.as_u64()? as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(b: &[u8]) -> Value {
        serde_json::from_slice(b).unwrap()
    }

    #[test]
    fn dedups_a_repeated_large_object() {
        let obj = r#"{"kind":"widget","color":"blue","size":"large"}"#;
        let input = format!(r#"{{"a":{obj},"b":{obj},"c":{obj}}}"#);
        let out = dict_json(input.as_bytes()).unwrap();
        let s = String::from_utf8(out.clone()).unwrap();
        assert!(s.contains("__tf_dict__"), "expected dict form, got {s}");
        // the object literal appears once (in the dict), not three times
        assert_eq!(s.matches("widget").count(), 1);
        assert!(round_trips(input.as_bytes(), &out));
    }

    #[test]
    fn dict_then_undict_reproduces_original() {
        let obj = r#"{"kind":"widget","color":"blue","size":"large"}"#;
        let input = format!(r#"[{obj},{obj},{obj},{obj}]"#);
        let folded = dict_json(input.as_bytes()).unwrap();
        let back = undict_json(&folded).unwrap();
        assert_eq!(parse(&back), parse(input.as_bytes()));
    }

    #[test]
    fn short_repeated_values_are_left_alone() {
        // "member" is short; a reference would be longer, so it must NOT be dictionaried.
        let input = br#"{"a":"member","b":"member","c":"member"}"#;
        let out = dict_json(input).unwrap();
        assert_eq!(parse(&out), parse(input));
        assert!(!String::from_utf8(out).unwrap().contains("__tf_dict__"));
    }

    #[test]
    fn single_occurrence_is_not_dictionaried() {
        let input = br#"{"only":{"a":"quite a long value here indeed yes"}}"#;
        let out = dict_json(input).unwrap();
        assert!(!String::from_utf8(out).unwrap().contains("__tf_dict__"));
    }

    #[test]
    fn nested_repeats_do_not_create_dead_entries() {
        let inner = r#"{"deeply":"nested repeated value that is long"}"#;
        let parent = format!(r#"{{"p":{inner},"q":"x"}}"#);
        let input = format!(r#"[{parent},{parent}]"#);
        let out = dict_json(input.as_bytes()).unwrap();
        assert!(round_trips(input.as_bytes(), &out));
    }

    #[test]
    fn empty_and_invalid_inputs() {
        assert!(dict_json(b"").unwrap().is_empty());
        assert!(dict_json(b"{bad").is_err());
    }

    use proptest::prelude::*;

    fn arb_json() -> impl Strategy<Value = Value> {
        let leaf = prop_oneof![
            Just(Value::Null),
            any::<bool>().prop_map(Value::Bool),
            any::<i64>().prop_map(|n| Value::Number(n.into())),
            "[^\"\\\\]{0,20}".prop_map(Value::String),
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
        #[test]
        fn dict_then_undict_is_the_identity_on_arbitrary_json(v in arb_json()) {
            let bytes = serde_json::to_vec(&v).unwrap();
            let dicted = dict_json(&bytes).unwrap();
            prop_assert!(round_trips(&bytes, &dicted));
            let back: Value = serde_json::from_slice(&undict_json(&dicted).unwrap()).unwrap();
            prop_assert_eq!(back, v);
        }
    }
}
