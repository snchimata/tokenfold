//! `log_field_fold` transform (canonical id `"log_field_fold"`, v1.0.0).
//!
//! Content-aware, **losslessly reversible** structural compression for line-oriented text
//! (`InputFormat::PlainText` / `CommandOutput`). It is the log-line analogue of
//! [`json_field_fold`](super::json_fold): where that folds arrays of homogeneous *objects* by
//! emitting each repeated key once, this folds runs of *templated* log lines by emitting each
//! repeated line **template** once.
//!
//! Most log lines share a fixed skeleton and vary only in a few fields (timestamps, ids, counts):
//!
//! ```text
//! 2026-07-01T10:05:11Z req=req-0311 status=200 ms=41
//! 2026-07-01T10:05:12Z req=req-0312 status=200 ms=44
//! ```
//!
//! Replacing each variable token (a run of digits, or a `0x…` hex literal) with a placeholder
//! yields one shared template `2026-\x00-\x00T\x00:\x00:\x00Z req=req-\x00 status=\x00 ms=\x00`
//! plus a per-line tuple of the captured values. The fold emits every distinct template once and
//! a compact row per original line (`template_id` + its captured values), so the skeleton text is
//! paid for once instead of per line — a large win on repetitive logs, unlike
//! [`log_compaction`](super::logs) which only collapses *identical adjacent* lines.
//!
//! It is a pure structural rewrite: `unfold_log` reconstructs the original bytes exactly, and the
//! pipeline gates adoption on that round-trip ([`round_trips`]) — a fold that would ever lose data
//! (or a genuine input that happens to look like our framing) is rolled back rather than emitted.

/// Canonical transform id, as registered with the pipeline.
pub const TRANSFORM_ID: &str = "log_field_fold";

/// Semantic version of this transform's output behavior.
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// First line of the folded blob. Collision-unlikely in real logs; any actual collision is caught
/// by the pipeline's round-trip safety gate, so it never corrupts data.
const HEADER: &str = "__tf_logfold1__";

/// Placeholder standing in for a captured variable token inside a template.
const PH: char = '\u{0}';

/// Minimum number of lines worth folding, and the maximum distinct-template fraction below which a
/// fold can pay off. Both are cheap early-outs; the pipeline also rolls back any net token
/// regression, so these are heuristics, not the correctness boundary.
const MIN_LINES: usize = 3;

use regex::Regex;
use std::sync::OnceLock;

/// Matches a maximal variable token: a `0x…` hex literal or a run of decimal digits. Linear-time
/// (no backtracking); see `deny.toml`.
fn var_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"0x[0-9a-fA-F]+|[0-9]+").expect("var_pattern is a valid literal"))
}

/// Replaces each variable token in `segment` with [`PH`], returning the template and the captured
/// tokens in left-to-right order. `caps` never contain [`PH`], `\n`, or spaces (the pattern only
/// matches `[0-9a-fA-F]`/`x`), which keeps the row serialization below unambiguous.
fn templatize(segment: &str) -> (String, Vec<&str>) {
    let re = var_pattern();
    let mut template = String::with_capacity(segment.len());
    let mut caps = Vec::new();
    let mut last = 0;
    for m in re.find_iter(segment) {
        template.push_str(&segment[last..m.start()]);
        template.push(PH);
        caps.push(m.as_str());
        last = m.end();
    }
    template.push_str(&segment[last..]);
    (template, caps)
}

/// Folds runs of templated lines in `input` into a header + a JSON array of distinct templates +
/// one compact row (`template_id` then captured values) per original line. Returns `input`
/// unchanged when folding cannot help (too few lines, no shared templates, or a line contains the
/// placeholder char). Never panics.
pub fn fold_log(input: &str) -> String {
    if input.is_empty() || input.contains(PH) {
        return input.to_string();
    }
    // split_inclusive keeps each line's trailing '\n' as part of the segment, so reassembly is
    // byte-exact (CRLF, blank lines, and a missing final newline are all preserved).
    let segments: Vec<&str> = input.split_inclusive('\n').collect();
    if segments.len() < MIN_LINES {
        return input.to_string();
    }

    let mut templates: Vec<String> = Vec::new();
    let mut ids: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut rows: Vec<(usize, Vec<&str>)> = Vec::with_capacity(segments.len());
    for seg in &segments {
        let (template, caps) = templatize(seg);
        let id = *ids.entry(template.clone()).or_insert_with(|| {
            templates.push(template);
            templates.len() - 1
        });
        rows.push((id, caps));
    }

    // Only worth it if templates are actually shared (skeleton text amortizes). If every line is
    // its own template there is nothing to save; let the pipeline keep the original.
    if templates.len() >= segments.len() {
        return input.to_string();
    }

    let mut out = String::with_capacity(input.len() / 2);
    out.push_str(HEADER);
    out.push('\n');
    out.push_str(&serde_json::to_string(&templates).expect("Vec<String> always serializes"));
    for (id, caps) in &rows {
        out.push('\n');
        out.push_str(&id.to_string());
        for cap in caps {
            out.push(' ');
            out.push_str(cap);
        }
    }
    out
}

/// Inverse of [`fold_log`]: expands a folded blob back to the original text. Returns `input`
/// unchanged if it is not a well-formed folded blob (so a genuine log that merely starts with the
/// header is not mangled — the pipeline's [`round_trips`] gate makes that safe either way).
pub fn unfold_log(input: &str) -> String {
    match try_unfold(input) {
        Some(s) => s,
        None => input.to_string(),
    }
}

fn try_unfold(input: &str) -> Option<String> {
    let mut lines = input.split('\n');
    if lines.next()? != HEADER {
        return None;
    }
    let templates: Vec<String> = serde_json::from_str(lines.next()?).ok()?;
    let mut out = String::with_capacity(input.len() * 2);
    for row in lines {
        let mut fields = row.split(' ');
        let id: usize = fields.next()?.parse().ok()?;
        let template = templates.get(id)?;
        let mut caps = fields;
        // Interleave the template's constant segments with the captured values. The number of
        // placeholders must equal the number of captures, else this isn't our framing.
        let mut parts = template.split(PH);
        out.push_str(parts.next()?);
        for part in parts {
            out.push_str(caps.next()?);
            out.push_str(part);
        }
        if caps.next().is_some() {
            return None; // more captures than placeholders → malformed
        }
    }
    Some(out)
}

/// True iff unfolding `after` reproduces `before` exactly. The pipeline's safety gate for this
/// transform — folding is only adopted when this holds.
pub fn round_trips(before: &[u8], after: &[u8]) -> bool {
    let (Ok(before_s), Ok(after_s)) = (std::str::from_utf8(before), std::str::from_utf8(after))
    else {
        return false;
    };
    unfold_log(after_s) == before_s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_lossless(input: &str) {
        let folded = fold_log(input);
        assert!(
            round_trips(input.as_bytes(), folded.as_bytes()),
            "round_trips() rejected the fold of {input:?}"
        );
        assert_eq!(
            unfold_log(&folded),
            input,
            "unfold != original for {input:?}"
        );
    }

    #[test]
    fn folds_templated_log_and_emits_skeleton_once() {
        // A realistically-sized run of one skeleton, so the shared template amortizes: the header +
        // template JSON is paid once, the per-line skeleton text is not.
        let input: String = (0..40)
            .map(|i| format!("req=req-{i:04} status=200 ms={}\n", 30 + i % 20))
            .collect();
        let folded = fold_log(&input);
        assert!(
            folded.starts_with(HEADER),
            "expected folded form, got {folded:?}"
        );
        // the shared skeleton word "status" is emitted once (in the single template), not per line.
        assert_eq!(folded.matches("status").count(), 1);
        assert!(
            folded.len() < input.len(),
            "fold ({}) not smaller than input ({})",
            folded.len(),
            input.len()
        );
        assert_lossless(&input);
    }

    #[test]
    fn preserves_crlf_blank_lines_and_missing_final_newline() {
        assert_lossless("a=1\r\na=2\r\na=3\r\n");
        assert_lossless("x=1\n\nx=2\n\nx=3\n");
        assert_lossless("x=1\nx=2\nx=3"); // no trailing newline
    }

    #[test]
    fn no_shared_templates_is_left_unchanged() {
        // every line a distinct skeleton → nothing to fold.
        let input = "alpha\nbeta gamma\ndelta epsilon zeta\n";
        assert_eq!(fold_log(input), input);
    }

    #[test]
    fn fewer_than_min_lines_is_left_unchanged() {
        let input = "req=1 ok\nreq=2 ok\n";
        assert_eq!(fold_log(input), input);
    }

    #[test]
    fn empty_input_is_a_noop() {
        assert_eq!(fold_log(""), "");
        assert_eq!(unfold_log(""), "");
    }

    #[test]
    fn lines_with_no_variable_tokens_still_fold_when_identical() {
        // three identical constant lines share one (capture-less) template.
        let input = "heartbeat ok\nheartbeat ok\nheartbeat ok\n";
        assert_lossless(input);
    }

    #[test]
    fn hex_and_decimal_tokens_both_captured() {
        let input = "addr=0x1f val=10\naddr=0x2a val=20\naddr=0x3b val=30\n";
        assert_lossless(input);
        // the "addr=" / "val=" skeleton is shared → one template.
        assert!(fold_log(input).starts_with(HEADER));
    }

    #[test]
    fn input_containing_the_placeholder_char_is_not_folded() {
        let input = "a\u{0}b\na\u{0}c\na\u{0}d\n";
        assert_eq!(fold_log(input), input);
    }

    #[test]
    fn genuine_input_shaped_like_the_header_round_trips_safely() {
        // If real text starts with our header, unfold must not silently corrupt it: fold_log of a
        // 2-line input is a no-op (< MIN_LINES), and round_trips gates the pipeline regardless.
        let input = "__tf_logfold1__\n[\"x\"]\n0 boom";
        let folded = fold_log(input);
        // Either it wasn't folded (identity) or, if it were, the pipeline would only adopt it when
        // round_trips holds — so the original is always recoverable.
        assert_eq!(
            unfold_log(&folded),
            if folded == input { input } else { input }
        );
    }

    use proptest::prelude::*;

    // Lines built from a small alphabet of skeletons + digit/hex fields, so templates recur (the
    // regime the fold targets) while still exercising CRLF, blanks, and missing final newlines.
    fn arb_log() -> impl Strategy<Value = String> {
        let line = prop_oneof![
            (0u32..999u32).prop_map(|n| format!("req={n} status=200")),
            (0u32..999u32).prop_map(|n| format!("addr=0x{n:x} ok")),
            Just("heartbeat".to_string()),
            "[a-z ]{0,8}".prop_map(|s| s),
        ];
        (prop::collection::vec(line, 0..40), any::<bool>()).prop_map(|(lines, trailing)| {
            let mut s = lines.join("\n");
            if trailing && !s.is_empty() {
                s.push('\n');
            }
            s
        })
    }

    proptest! {
        // Core safety guarantee: folding never loses data. For ANY log-ish input, unfolding the
        // folded form reproduces it exactly, and round_trips() (the pipeline's gate) agrees.
        #[test]
        fn fold_then_unfold_is_the_identity(input in arb_log()) {
            let folded = fold_log(&input);
            prop_assert!(round_trips(input.as_bytes(), folded.as_bytes()));
            prop_assert_eq!(unfold_log(&folded), input);
        }
    }
}
