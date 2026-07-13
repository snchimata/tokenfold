//! `log_compaction` transform (canonical id `"log_compaction"`, v1.0.0).
//!
//! **Mode:** lossy-with-evidence. Ships behind `--experimental` until the fidelity gate
//! (roadmap.md F-016) is green for this transform; see `crate::modes::ALL_ENTRIES`.
//!
//! Collapses runs of three or more **adjacent** identical lines into three output lines:
//! the first occurrence, an evidence marker `[repeated Nx]` recording exactly how many
//! copies were dropped, and the last occurrence. This preserves enough evidence to tell a
//! reader "this line repeated N times here" without paying the token cost of every copy.
//!
//! Deliberate behavioral contract (see `port_spec` in `eval/transforms/log_compaction.py`
//! and roadmap.md F-012):
//! - **Threshold is 3, not 2.** Runs of exactly one or exactly two adjacent identical lines
//!   are left completely untouched — no marker, every line kept. Only a run of three or
//!   more collapses to the three-line evidence form above, regardless of how long the run
//!   actually is (`[repeated 2000x]` is still just three output lines).
//! - **Adjacent-only.** Only *consecutive* identical lines count as a run. Non-adjacent
//!   duplicates (e.g. `[A, B, A]`) are never collapsed, even though `A` appears twice
//!   overall. This is a documented, tested limitation, not a bug — interleaved log
//!   deduplication is explicitly out of scope for this transform.
//! - Relative line ordering is always preserved.
//! - Empty input produces empty output; a single-line input is returned unchanged.
//! - Timestamp removal is opt-in only (default off, via `remove_timestamps: bool`). When
//!   off, two lines that differ only by timestamp are distinct strings and are never
//!   collapsed. When on, a recognized leading timestamp is stripped from every line before
//!   comparison, and the stripped form is what appears in the output — the timestamp is
//!   genuinely removed from surviving lines, not just ignored for comparison purposes.
//!
//! Timestamp patterns run on Rust's `regex` crate, whose matching engine is provably
//! linear-time in the length of the input (no catastrophic backtracking) — see `deny.toml`
//! at the workspace root.

use regex::Regex;
use std::sync::OnceLock;

/// Canonical transform id, as registered with the pipeline.
pub const TRANSFORM_ID: &str = "log_compaction";

/// Semantic version of this transform's output behavior.
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// Minimum length of an adjacent run of identical lines that gets collapsed into the
/// `[repeated Nx]` evidence form. Runs shorter than this are left completely unchanged.
/// This is intentionally 3, not 2 — see the module-level doc comment.
const MIN_RUN_LEN: usize = 3;

/// Matches a leading ISO-8601 timestamp, e.g. `2026-07-11T10:22:03Z `,
/// `2026-07-11T10:22:03.123456Z `, or `2026-07-11T10:22:03+00:00 `, including the
/// trailing whitespace that separates it from the rest of the line.
fn iso8601_timestamp_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})[ \t]+")
            .expect("iso8601_timestamp_pattern regex is a fixed valid literal")
    })
}

/// Matches a leading syslog-style timestamp, e.g. `Jul 11 10:22:03 `, including the
/// trailing whitespace that separates it from the rest of the line.
fn syslog_timestamp_pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"^[A-Z][a-z]{2}[ \t]+\d{1,2}[ \t]+\d{2}:\d{2}:\d{2}[ \t]+")
            .expect("syslog_timestamp_pattern regex is a fixed valid literal")
    })
}

/// Strips a recognized leading timestamp from a single line, if present. Lines that don't
/// match any recognized pattern are returned untouched.
fn strip_timestamp(line: &str) -> &str {
    if let Some(m) = iso8601_timestamp_pattern().find(line) {
        return &line[m.end()..];
    }
    if let Some(m) = syslog_timestamp_pattern().find(line) {
        return &line[m.end()..];
    }
    line
}

/// Compacts a log by collapsing runs of three or more adjacent identical lines into
/// `first-occurrence` / `[repeated Nx]` / `last-occurrence` (three output lines total,
/// regardless of run length). See the module-level doc comment for the full behavioral
/// contract, including the adjacent-only limitation and the >=3 threshold.
///
/// # Parameters
/// - `input`: raw log text as `\n`-separated lines (a trailing newline is not required).
/// - `remove_timestamps`: when `true`, a recognized leading timestamp (ISO-8601 or
///   syslog-style) is stripped from every line before run-detection, and the stripped form
///   is what appears in the output. When `false` (the default), timestamps are never
///   touched, so two lines identical except for their timestamp are treated as distinct.
pub fn compact(input: &str, remove_timestamps: bool) -> String {
    if input.is_empty() {
        return String::new();
    }

    let lines: Vec<&str> = input.lines().collect();

    if remove_timestamps {
        let stripped: Vec<&str> = lines.iter().map(|line| strip_timestamp(line)).collect();
        collapse_adjacent_runs(&stripped)
    } else {
        collapse_adjacent_runs(&lines)
    }
}

/// Walks `lines` once, grouping adjacent identical lines into runs and handing each
/// complete run to [`push_run`]. See [`compact`] for the full behavioral contract.
fn collapse_adjacent_runs(lines: &[&str]) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut iter = lines.iter();
    let Some(&first) = iter.next() else {
        return String::new();
    };
    let mut current = first;
    let mut count = 1usize;

    for &line in iter {
        if line == current {
            count += 1;
        } else {
            push_run(&mut out, current, count);
            current = line;
            count = 1;
        }
    }
    push_run(&mut out, current, count);

    out.join("\n")
}

/// Emits one run of `count` adjacent copies of `line` into `out`: collapsed into the
/// `first` / `[repeated Nx]` / `last` evidence form when `count >= MIN_RUN_LEN`, or kept
/// verbatim (every copy, unchanged) otherwise.
fn push_run(out: &mut Vec<String>, line: &str, count: usize) {
    if count >= MIN_RUN_LEN {
        out.push(line.to_string());
        out.push(format!("[repeated {count}x]"));
        out.push(line.to_string());
    } else {
        for _ in 0..count {
            out.push(line.to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_of_four_collapses_to_first_marker_last() {
        let input = "A\nA\nA\nA";
        assert_eq!(compact(input, false), "A\n[repeated 4x]\nA");
    }

    #[test]
    fn run_of_exactly_three_collapses_boundary_case() {
        // Boundary: the threshold is >= 3, so exactly three must collapse.
        let input = "A\nA\nA";
        assert_eq!(compact(input, false), "A\n[repeated 3x]\nA");
    }

    #[test]
    fn run_of_exactly_two_does_not_collapse_boundary_case() {
        // Boundary: the threshold is 3, NOT 2 - exactly two adjacent identical lines must
        // be left completely unchanged, with no marker inserted.
        let input = "A\nA";
        assert_eq!(compact(input, false), "A\nA");
    }

    #[test]
    fn non_adjacent_duplicates_are_never_collapsed() {
        // Documented, intentional limitation: [A, B, A] has "A" appearing twice overall,
        // but the two occurrences are not adjacent, so no collapse happens at all.
        let input = "A\nB\nA";
        assert_eq!(compact(input, false), "A\nB\nA");
    }

    #[test]
    fn single_line_input_is_unchanged() {
        let input = "only line";
        assert_eq!(compact(input, false), "only line");
    }

    #[test]
    fn empty_input_produces_empty_output() {
        assert_eq!(compact("", false), "");
    }

    #[test]
    fn mix_of_runs_and_singletons_preserves_relative_ordering() {
        let input = "start\nA\nA\nA\nA\nA\nmiddle\nB\nB\nend";
        let expected = "start\nA\n[repeated 5x]\nA\nmiddle\nB\nB\nend";
        assert_eq!(compact(input, false), expected);
    }

    #[test]
    fn remove_timestamps_true_collapses_lines_that_differ_only_by_timestamp() {
        // Collapsing requires a run of >= 3 (MIN_RUN_LEN), so three lines are used here -
        // not two - to actually trigger a collapse under the documented >=3 threshold,
        // while still demonstrating that timestamp-only differences are ignored once
        // remove_timestamps strips them.
        let input = "2026-07-11T10:22:03Z connection reset\n\
                     2026-07-11T10:22:04.123456Z connection reset\n\
                     2026-07-11T10:22:05+00:00 connection reset";
        let output = compact(input, true);
        assert_eq!(output, "connection reset\n[repeated 3x]\nconnection reset");
    }

    #[test]
    fn remove_timestamps_true_strips_syslog_style_prefix_too() {
        let input = "Jul 11 10:22:03 connection reset\nJul 11 10:22:04 connection reset\nJul 11 10:22:05 connection reset";
        let output = compact(input, true);
        assert_eq!(output, "connection reset\n[repeated 3x]\nconnection reset");
    }

    #[test]
    fn remove_timestamps_false_keeps_timestamp_differing_lines_distinct() {
        // Default (off): timestamps are never touched, so these two lines - identical
        // except for their timestamp - are treated as distinct and are not collapsed.
        let input = "2026-07-11T10:22:03Z connection reset\n2026-07-11T10:22:04Z connection reset";
        assert_eq!(compact(input, false), input);
    }

    #[test]
    fn remove_timestamps_true_leaves_lines_without_a_recognized_timestamp_untouched() {
        let input = "no timestamp here\nno timestamp here\nno timestamp here";
        let output = compact(input, true);
        assert_eq!(
            output,
            "no timestamp here\n[repeated 3x]\nno timestamp here"
        );
    }
}
