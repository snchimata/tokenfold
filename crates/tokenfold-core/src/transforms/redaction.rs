//! `secret_redaction` (canonical transform id: `"secret_redaction"`): a mandatory safety
//! preprocessor that scans text for common enterprise secret shapes (JWTs, API keys, AWS
//! access key IDs, basic-auth-in-URL credentials, and bearer tokens) and replaces each match
//! with a fixed marker naming which kind of secret was found, without ever echoing the
//! secret's actual value back into the output.
//!
//! This module implements only the pure scan-and-replace behavior. The policy wiring that
//! makes `secret_redaction` non-disableable lives elsewhere (see `CompressionPolicyBuilder`
//! in `budget.rs`).
//!
//! All patterns run on Rust's `regex` crate, whose matching engine is provably linear-time in
//! the length of the input (no backtracking), which is why it is the only regex engine
//! permitted in this file's problem space — see `deny.toml` at the workspace root.

use regex::Regex;
use std::sync::OnceLock;

/// Result of running [`redact`] over a byte buffer.
pub struct RedactionOutcome {
    /// The redacted text, re-encoded as bytes.
    pub bytes: Vec<u8>,
    /// Total number of secret matches replaced across all patterns.
    pub redacted_count: usize,
}

fn jwt_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"eyJ[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+\.[A-Za-z0-9_-]+")
            .expect("jwt_pattern regex is a fixed valid literal")
    })
}

fn api_key_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"sk-[A-Za-z0-9]{20,}").expect("api_key_pattern regex is a fixed valid literal")
    })
}

fn aws_key_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"AKIA[0-9A-Z]{16}").expect("aws_key_pattern regex is a fixed valid literal")
    })
}

fn basic_auth_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(https?://)[^:/\s@]+:[^@/\s]+@")
            .expect("basic_auth_pattern regex is a fixed valid literal")
    })
}

fn bearer_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)Bearer\s+[A-Za-z0-9\-._~+/]+=*")
            .expect("bearer_pattern regex is a fixed valid literal")
    })
}

/// Cheaply tests whether any known secret pattern matches `input`, without allocating a
/// redacted copy. Used by `retrieval_store` to gate what may ever be persisted: nothing that
/// would be redacted by [`redact`] is eligible for storage.
pub fn contains_secret(input: &[u8]) -> bool {
    let text = String::from_utf8_lossy(input);
    jwt_pattern().is_match(&text)
        || api_key_pattern().is_match(&text)
        || aws_key_pattern().is_match(&text)
        || basic_auth_pattern().is_match(&text)
        || bearer_pattern().is_match(&text)
}

/// Scans `input` (treated as best-effort UTF-8 via lossy conversion) for known secret shapes
/// and replaces each match with a marker identifying the kind of secret found. Returns the
/// redacted bytes plus a count of total replacements made.
///
/// This is a best-effort safety net, not a strict parser: invalid UTF-8 in `input` is handled
/// via `String::from_utf8_lossy`, which substitutes the standard replacement character for any
/// invalid byte sequences before the patterns below are applied.
pub fn redact(input: &[u8]) -> RedactionOutcome {
    let mut text = String::from_utf8_lossy(input).into_owned();
    let mut redacted_count = 0usize;

    let re = jwt_pattern();
    redacted_count += re.find_iter(&text).count();
    text = re.replace_all(&text, "[REDACTED:jwt]").into_owned();

    let re = api_key_pattern();
    redacted_count += re.find_iter(&text).count();
    text = re.replace_all(&text, "[REDACTED:api_key]").into_owned();

    let re = aws_key_pattern();
    redacted_count += re.find_iter(&text).count();
    text = re.replace_all(&text, "[REDACTED:aws_key]").into_owned();

    let re = basic_auth_pattern();
    redacted_count += re.find_iter(&text).count();
    // No colon inside this marker: "scheme://[REDACTED:basic_auth]@" would itself re-match
    // `basic_auth_pattern` on a second pass (the colon makes it look like another
    // "user:pass" segment), which breaks redaction idempotency and the pipeline's
    // no-bypass safety check (`safety::no_redaction_bypass`, which re-runs `redact` over its
    // own output and expects zero further matches).
    text = re
        .replace_all(&text, "${1}[REDACTED-BASIC-AUTH]@")
        .into_owned();

    let re = bearer_pattern();
    redacted_count += re.find_iter(&text).count();
    text = re
        .replace_all(&text, "Bearer [REDACTED:bearer_token]")
        .into_owned();

    RedactionOutcome {
        bytes: text.into_bytes(),
        redacted_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contains_subslice(haystack: &[u8], needle: &[u8]) -> bool {
        !needle.is_empty()
            && haystack.len() >= needle.len()
            && haystack.windows(needle.len()).any(|w| w == needle)
    }

    #[test]
    fn jwt_token_is_redacted() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dQw4w9WgXcQ";
        let input = format!("Authorization header carried: {jwt} in the clear");
        let outcome = redact(input.as_bytes());
        let output = String::from_utf8(outcome.bytes).unwrap();

        assert!(!output.contains(jwt));
        assert!(output.contains("[REDACTED:jwt]"));
        assert!(outcome.redacted_count > 0);
    }

    #[test]
    fn openai_api_key_is_redacted() {
        let key = "sk-ABCDEFGHIJ1234567890abcdefgh";
        let input = format!("OPENAI_API_KEY={key}");
        let outcome = redact(input.as_bytes());
        let output = String::from_utf8(outcome.bytes).unwrap();

        assert!(!output.contains(key));
        assert!(output.contains("[REDACTED:api_key]"));
        assert!(outcome.redacted_count > 0);
    }

    #[test]
    fn aws_access_key_is_redacted() {
        // AKIAIOSFODNN7EXAMPLE is AWS's own published documentation placeholder key, not a
        // real credential: https://docs.aws.amazon.com/IAM/latest/UserGuide/id_credentials_access-keys.html
        let key = "AKIAIOSFODNN7EXAMPLE";
        let input = format!("AWS_ACCESS_KEY_ID={key}");
        let outcome = redact(input.as_bytes());
        let output = String::from_utf8(outcome.bytes).unwrap();

        assert!(!output.contains(key));
        assert!(output.contains("[REDACTED:aws_key]"));
        assert!(outcome.redacted_count > 0);
    }

    #[test]
    fn basic_auth_in_url_is_redacted() {
        let input = "see https://alice:hunter2@example.com/path for details";
        let outcome = redact(input.as_bytes());
        let output = String::from_utf8(outcome.bytes).unwrap();

        assert!(!output.contains("alice:hunter2"));
        assert!(output.contains("example.com/path"));
        assert!(output.contains("[REDACTED-BASIC-AUTH]"));
        assert!(outcome.redacted_count > 0);
    }

    #[test]
    fn redacting_already_redacted_basic_auth_output_finds_no_further_matches() {
        // Regression test for the marker self-match bug: the basic-auth marker must not
        // contain a colon, or "scheme://MARKER@" would look like another credential to the
        // same pattern on a second pass.
        let input = "see https://alice:hunter2@example.com/path for details";
        let once = redact(input.as_bytes());
        let twice = redact(&once.bytes);
        assert_eq!(twice.redacted_count, 0);
        assert_eq!(once.bytes, twice.bytes);
    }

    #[test]
    fn bearer_token_is_redacted() {
        let input = "Authorization: Bearer abcDEF123.token-value";
        let outcome = redact(input.as_bytes());
        let output = String::from_utf8(outcome.bytes).unwrap();

        assert!(!output.contains("abcDEF123.token-value"));
        assert!(output.contains("Bearer [REDACTED:bearer_token]"));
        assert!(outcome.redacted_count > 0);
    }

    /// Regression canary: this input is the kind of string that pathologically stalls a
    /// backtracking regex engine (e.g. `(a+)+` / naive nested-quantifier patterns) because it
    /// almost-but-never-quite matches, forcing exponential blowup. It is not a real
    /// vulnerability test — Rust's `regex` crate guarantees linear-time matching regardless of
    /// pattern shape — but it proves the redaction path stays on that guaranteed-linear-time
    /// engine and never regresses onto a backtracking one.
    #[test]
    fn redos_canary_completes_within_time_budget() {
        let long_run_of_a = "a".repeat(40_000);
        let start = std::time::Instant::now();
        let outcome = redact(long_run_of_a.as_bytes());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(outcome.redacted_count, 0);

        let nested_quantifier_bait = "a".repeat(20_000) + "!";
        let start = std::time::Instant::now();
        let outcome = redact(nested_quantifier_bait.as_bytes());
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
        assert_eq!(outcome.redacted_count, 0);
    }

    #[test]
    fn contains_secret_matches_the_same_inputs_redact_would_change() {
        assert!(contains_secret(b"AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE"));
        assert!(contains_secret(
            b"Authorization: Bearer abcDEF123.token-value"
        ));
        assert!(!contains_secret(
            b"just a normal log line, nothing sensitive"
        ));
    }

    #[test]
    fn text_with_no_secrets_is_returned_unchanged() {
        let input = b"just a normal log line with nothing sensitive in it at all.";
        let outcome = redact(input);

        assert_eq!(outcome.bytes, input.to_vec());
        assert_eq!(outcome.redacted_count, 0);
    }

    #[test]
    fn redacted_output_never_contains_literal_secret_bytes() {
        let jwt = "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dQw4w9WgXcQ";
        let api_key = "sk-ABCDEFGHIJ1234567890abcdefgh";
        let input = format!("jwt={jwt} key={api_key}");
        let outcome = redact(input.as_bytes());

        assert!(!contains_subslice(&outcome.bytes, jwt.as_bytes()));
        assert!(!contains_subslice(&outcome.bytes, api_key.as_bytes()));
    }
}
