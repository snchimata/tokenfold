//! `json_minify` transform (canonical id `"json_minify"`, v1.0.0).
//!
//! Strips insignificant JSON whitespace (spaces, tabs, `\n`, `\r` that fall *outside*
//! string literals) without ever:
//! - reordering object keys,
//! - changing string content or escape sequences, or
//! - renormalizing number spelling (`1.0` must stay `1.0`, `1e10` must stay `1e10`).
//!
//! This is deliberately **not** implemented as parse-then-reserialize through
//! `serde_json::Value`. Re-serializing a `Value` would risk exactly the failure modes
//! above: `serde_json`'s number type does not remember whether `1.0` was spelled with a
//! trailing `.0` or an exponent, and losing that spelling would change the byte identity
//! of prompts that providers may cache on. Key order is only preserved through
//! `Value` if the `preserve_order` feature is enabled crate-wide, which is a global,
//! easy-to-silently-break invariant; recovering that in a `Value` layer would be nice, but the
//! two problems above (numbers, and depending on a global feature flag for correctness)
//! are still true.
//!
//! Instead, this transform:
//! 1. Validates the input by parsing it into a `serde_json::Value` and immediately
//!    discarding it. This enforces valid UTF-8 and valid JSON grammar (rejecting things
//!    like unterminated strings) without ever using the parsed value for output.
//! 2. Performs a single lexical pass over the original bytes, tracking whether the
//!    cursor is inside a string literal (so whitespace inside strings is never touched)
//!    and whether the current byte is escaped (so an escaped quote `\"` does not
//!    prematurely end the string).

/// Canonical transform id, as registered with the pipeline.
pub const TRANSFORM_ID: &str = "json_minify";

/// Semantic version of this transform's output behavior.
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// Strips insignificant JSON whitespace from `input`, preserving key order, string
/// content/escapes, and number spelling exactly.
///
/// Empty input is a special case: `serde_json` treats `b""` as invalid JSON, but this
/// transform must be a no-op (not an error) on empty input, so it short-circuits before
/// validation.
pub fn minify_json(input: &[u8]) -> Result<Vec<u8>, JsonMinifyError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }

    // Validate only; the parsed `Value` is discarded. This is what rejects invalid
    // UTF-8, malformed JSON, and unterminated strings before the lexical pass below
    // ever runs.
    serde_json::from_slice::<serde_json::Value>(input)?;

    let mut out = Vec::with_capacity(input.len());
    let mut in_string = false;
    let mut escaped = false;

    for &byte in input {
        if in_string {
            out.push(byte);
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                in_string = false;
            }
        } else {
            match byte {
                b'"' => {
                    in_string = true;
                    out.push(byte);
                }
                b' ' | b'\n' | b'\r' | b'\t' => {}
                _ => out.push(byte),
            }
        }
    }

    Ok(out)
}

#[derive(Debug, thiserror::Error)]
pub enum JsonMinifyError {
    #[error("invalid json: {0}")]
    Invalid(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minified_output_is_still_valid_json() {
        let input = br#"{ "a" : [1, 2, 3], "b" : { "c" : null } }"#;
        let out = minify_json(input).unwrap();

        let original: serde_json::Value = serde_json::from_slice(input).unwrap();
        let minified: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(original, minified);
    }

    #[test]
    fn key_order_is_preserved_byte_for_byte() {
        let input = b"{\"b\": 1, \"a\": 2}";
        let out = minify_json(input).unwrap();
        // Must be exactly this - NOT reordered to {"a":2,"b":1}.
        assert_eq!(out, b"{\"b\":1,\"a\":2}");
    }

    #[test]
    fn duplicate_keys_are_not_deduplicated() {
        // `minify_json` is a lexical whitespace strip, not a parser: it does not
        // deduplicate repeated keys. Collapsing duplicate keys into a single entry is
        // the responsibility of whoever later parses these bytes into a map.
        let input = b"{\"a\": 1, \"a\": 2}";
        let out = minify_json(input).unwrap();
        assert_eq!(out, b"{\"a\":1,\"a\":2}");
    }

    #[test]
    fn string_content_and_escapes_survive_byte_for_byte() {
        // `br#"..."#` is a raw byte string literal: every backslash below is a literal
        // source byte (a JSON newline escape, an escaped quote, an escaped backslash,
        // and a six-character JSON unicode escape sequence), not a Rust string escape.
        // They must reach the output completely unchanged - the transform never
        // decodes or re-encodes them.
        let input: &[u8] = br#"{"s": "line1\nline2 \"quoted\" back\\slash unicode\u00e9"}"#;
        let out = minify_json(input).unwrap();
        assert_eq!(
            out,
            br#"{"s":"line1\nline2 \"quoted\" back\\slash unicode\u00e9"}"#.to_vec()
        );
    }

    #[test]
    fn number_spelling_is_never_renormalized() {
        let input = b"{ \"x\" : 1.0 , \"y\" : 1e10 }";
        let out = minify_json(input).unwrap();
        let out_str = std::str::from_utf8(&out).unwrap();
        assert_eq!(out_str, "{\"x\":1.0,\"y\":1e10}");
        assert!(out_str.contains("1.0"));
        assert!(out_str.contains("1e10"));
        assert!(!out_str.contains("10000000000"));
    }

    #[test]
    fn non_utf8_input_is_rejected() {
        let input: &[u8] = &[0xFF, 0xFE, 0x00];
        let err = minify_json(input).unwrap_err();
        assert!(matches!(err, JsonMinifyError::Invalid(_)));
    }

    #[test]
    fn empty_input_returns_empty_output_not_an_error() {
        let out = minify_json(b"").unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn unterminated_string_is_rejected() {
        let input = b"{\"a\": \"b";
        let err = minify_json(input).unwrap_err();
        assert!(matches!(err, JsonMinifyError::Invalid(_)));
    }

    #[test]
    fn minify_is_idempotent_on_whitespace_heavy_nested_input() {
        let input =
            b"{\n  \"outer\" : {\n    \"list\" : [ 1,  2,\t3 ],\n    \"s\" : \"a\\tb\"\n  }\n}\n";
        let once = minify_json(input).unwrap();
        let twice = minify_json(&once).unwrap();
        assert_eq!(once, twice);
    }
}
