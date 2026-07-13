//! Cross-platform byte-exact golden tests (ENGINEERING.md "Golden Tests"). Fixtures live at
//! the repo root under `tests/golden/{transform_id}/` so CLI/proxy/Python surfaces can share
//! them later; this file is the Rust runner against `tokenfold-core`'s transform functions.

use std::fs;
use std::path::PathBuf;

use tokenfold_core::transforms::{diff, json, logs, schema};

fn fixture(relative: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", relative]
        .iter()
        .collect();
    fs::read(&path).unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()))
}

#[test]
fn golden_json_minify_simple_object() {
    let input = fixture("tests/golden/json_minify/simple_object.in.json");
    let expected = fixture("tests/golden/json_minify/simple_object.out.json");
    let actual = json::minify_json(&input).expect("valid json fixture");
    assert_eq!(actual, expected);
}

#[test]
fn golden_json_minify_nested_array() {
    let input = fixture("tests/golden/json_minify/nested_array.in.json");
    let expected = fixture("tests/golden/json_minify/nested_array.out.json");
    let actual = json::minify_json(&input).expect("valid json fixture");
    assert_eq!(actual, expected);
}

#[test]
fn golden_json_minify_string_escapes() {
    let input = fixture("tests/golden/json_minify/string_escapes.in.json");
    let expected = fixture("tests/golden/json_minify/string_escapes.out.json");
    let actual = json::minify_json(&input).expect("valid json fixture");
    assert_eq!(actual, expected);
}

#[test]
fn golden_log_compaction_adjacent_duplicates() {
    let input = fixture("tests/golden/log_compaction/adjacent_duplicates.in.txt");
    let expected = fixture("tests/golden/log_compaction/adjacent_duplicates.out.txt");
    let input_text = String::from_utf8(input).expect("utf8 fixture");
    let actual = logs::compact(&input_text, false);
    assert_eq!(actual.into_bytes(), expected);
}

#[test]
fn golden_log_compaction_no_duplicates() {
    let input = fixture("tests/golden/log_compaction/no_duplicates.in.txt");
    let expected = fixture("tests/golden/log_compaction/no_duplicates.out.txt");
    let input_text = String::from_utf8(input).expect("utf8 fixture");
    let actual = logs::compact(&input_text, false);
    assert_eq!(actual.into_bytes(), expected);
}

#[test]
fn golden_schema_compaction_openai_tool_schema() {
    let input = fixture("tests/golden/schema_compaction/openai_tool_schema.in.json");
    let expected = fixture("tests/golden/schema_compaction/openai_tool_schema.out.json");
    let actual = schema::compact_schema(&input, 1).expect("valid json fixture");
    assert_eq!(actual, expected);
}

#[test]
fn golden_diff_compaction_small_diff() {
    let input = fixture("tests/golden/diff_compaction/small_diff.in.txt");
    let expected = fixture("tests/golden/diff_compaction/small_diff.out.txt");
    let input_text = String::from_utf8(input).expect("utf8 fixture");
    let actual = diff::compact_diff(&input_text, true);
    assert_eq!(actual.into_bytes(), expected);
}
