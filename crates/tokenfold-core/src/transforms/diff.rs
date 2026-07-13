//! `diff_compaction` (canonical id: `"diff_compaction"`, v1.0.0).
//!
//! Lossy-with-evidence transform operating on unified-diff text such as the output of
//! `git diff`. Ships behind `--experimental` until the fidelity gate is green for this
//! transform (see F-013 / F-016 in roadmap.md).
//!
//! This module is not yet wired into the crate (no `mod transforms;` declaration exists in
//! `lib.rs`); it implements the mechanical line-classification behavior only. The policy
//! decision of *when* the header-only form (`keep_line_bodies = false`) is allowed to run
//! (only for `TaskScope::ChangeSummary`) is made by the caller, not by this module.

/// Stable canonical transform id, for future `TransformReport.id` wiring.
pub const TRANSFORM_ID: &str = "diff_compaction";
/// Semantic version of this transform's behavior, for future `TransformReport.version` wiring.
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// How a single line of unified-diff input is classified by [`compact_diff`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LineKind {
    /// `diff --git ...`, `index ...`, `--- `, `+++ `, or `@@...` hunk header lines.
    /// Always kept verbatim, in order, regardless of `keep_line_bodies`.
    Structural,
    /// A changed line body: starts with `+` or `-` (but not the structural `--- `/`+++ `
    /// forms). Kept verbatim, in order, only when `keep_line_bodies` is `true`.
    ChangeBody,
    /// Everything else: an unchanged context line. Never kept in the output.
    Context,
}

/// Classifies a single line of unified-diff text. See [`LineKind`] for the rules.
fn classify_line(line: &str) -> LineKind {
    if line.starts_with("diff --git")
        || line.starts_with("index ")
        || line.starts_with("--- ")
        || line.starts_with("+++ ")
        || line.starts_with("@@")
    {
        LineKind::Structural
    } else if line.starts_with('+') || line.starts_with('-') {
        LineKind::ChangeBody
    } else {
        LineKind::Context
    }
}

/// Pushes a `[N <noun> dropped]` evidence marker onto `out` if any lines have been dropped
/// since the last flush, then resets the run counter. No-op when nothing was dropped.
fn flush_dropped_run(out: &mut Vec<String>, dropped_run: &mut usize, marker_noun: &str) {
    if *dropped_run > 0 {
        out.push(format!("[{dropped_run} {marker_noun} dropped]"));
        *dropped_run = 0;
    }
}

/// Compacts unified-diff text (e.g. `git diff` output) per the `diff_compaction` (F-013)
/// contract.
///
/// Processes `input` line by line (via [`str::lines`]) and classifies each line:
///
/// - **Structural** lines — `diff --git ...`, the git blob-hash `index ...` line, `--- `,
///   `+++ `, or `@@` hunk headers (e.g. `@@ -12,5 +12,7 @@ optional context`) — are always
///   kept verbatim, in order, no matter what.
/// - **Change-body** lines — lines starting with `+` or `-` that are not one of the
///   structural `--- `/`+++ ` forms — are kept verbatim, in order, only when
///   `keep_line_bodies` is `true`.
/// - Every other line is a **context** line (typically starting with a leading space) and is
///   never kept.
///
/// Lines that are dropped (context lines always; change-body lines too when
/// `keep_line_bodies` is `false`) collapse consecutive runs into a single evidence marker
/// line, so the marker is emitted once per run of dropped lines rather than once per line:
///
/// - `keep_line_bodies == true`: only context lines can ever be dropped in this mode, so the
///   marker reads `"[N context lines dropped]"`.
/// - `keep_line_bodies == false` (the header-only form — valid only when the caller's
///   `TaskScope` is `ChangeSummary`; that policy decision is made by the caller, not here):
///   both context lines and change-body lines are dropped, so the marker reads
///   `"[N lines dropped]"` to make clear it covers both.
///
/// Relative order of everything that survives is preserved. Empty input produces empty
/// output. Output lines are joined with `"\n"` with no trailing newline.
pub fn compact_diff(input: &str, keep_line_bodies: bool) -> String {
    let marker_noun = if keep_line_bodies {
        "context lines"
    } else {
        "lines"
    };

    let mut out: Vec<String> = Vec::new();
    let mut dropped_run: usize = 0;

    for line in input.lines() {
        match classify_line(line) {
            LineKind::Structural => {
                flush_dropped_run(&mut out, &mut dropped_run, marker_noun);
                out.push(line.to_string());
            }
            LineKind::ChangeBody => {
                if keep_line_bodies {
                    flush_dropped_run(&mut out, &mut dropped_run, marker_noun);
                    out.push(line.to_string());
                } else {
                    dropped_run += 1;
                }
            }
            LineKind::Context => {
                dropped_run += 1;
            }
        }
    }
    flush_dropped_run(&mut out, &mut dropped_run, marker_noun);

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small realistic unified diff for one file: header lines, 3 leading context lines,
    /// one removed line, one added line, then 3 trailing context lines. Matches
    /// `tests/golden/diff_compaction/small_diff.in.txt`.
    const SAMPLE_DIFF: &str = "diff --git a/f.rs b/f.rs\n\
        index 1234567..89abcde 100644\n\
        --- a/f.rs\n\
        +++ b/f.rs\n\
        @@ -1,7 +1,7 @@\n\
        \x20fn main() {\n\
        \x20    let x = 1;\n\
        \x20    let y = 2;\n\
        -    println!(\"{}\", x);\n\
        +    println!(\"{} {}\", x, y);\n\
        \x20    let z = 3;\n\
        \x20    let w = 4;\n\
        \x20    println!(\"done\");";

    #[test]
    fn hunk_headers_are_preserved() {
        let out = compact_diff(SAMPLE_DIFF, true);
        assert!(out.lines().any(|l| l == "@@ -1,7 +1,7 @@"));
    }

    #[test]
    fn change_body_lines_are_preserved_when_keep_line_bodies_true() {
        let out = compact_diff(SAMPLE_DIFF, true);
        assert!(out.lines().any(|l| l == "-    println!(\"{}\", x);"));
        assert!(out.lines().any(|l| l == "+    println!(\"{} {}\", x, y);"));
    }

    #[test]
    fn file_names_and_diff_git_header_are_preserved() {
        let out = compact_diff(SAMPLE_DIFF, true);
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.contains(&"diff --git a/f.rs b/f.rs"));
        assert!(lines.contains(&"index 1234567..89abcde 100644"));
        assert!(lines.contains(&"--- a/f.rs"));
        assert!(lines.contains(&"+++ b/f.rs"));
    }

    #[test]
    fn change_body_lines_are_dropped_when_keep_line_bodies_false_but_structural_survives() {
        let out = compact_diff(SAMPLE_DIFF, false);
        let lines: Vec<&str> = out.lines().collect();

        // Structural lines still survive.
        assert!(lines.contains(&"diff --git a/f.rs b/f.rs"));
        assert!(lines.contains(&"index 1234567..89abcde 100644"));
        assert!(lines.contains(&"--- a/f.rs"));
        assert!(lines.contains(&"+++ b/f.rs"));
        assert!(lines.contains(&"@@ -1,7 +1,7 @@"));

        // Change-body lines are gone, replaced by a "[N lines dropped]"-style marker.
        // (The structural "+++ b/f.rs" header also starts with '+' and must survive, so the
        // checks below exclude the structural "--- "/"+++ " forms explicitly.)
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with('+') && !l.starts_with("+++ "))
        );
        assert!(
            !lines
                .iter()
                .any(|l| l.starts_with('-') && !l.starts_with("--- "))
        );
        assert!(lines.iter().any(|l| l.ends_with("lines dropped]")));
    }

    #[test]
    fn evidence_marker_counts_consecutive_dropped_context_lines() {
        let out = compact_diff(SAMPLE_DIFF, true);
        let lines: Vec<&str> = out.lines().collect();

        // Exactly two markers (one per 3-line context run), each reporting 3, not one
        // marker per dropped line.
        let markers: Vec<&&str> = lines.iter().filter(|l| l.ends_with("dropped]")).collect();
        assert_eq!(markers.len(), 2);
        for marker in markers {
            assert_eq!(*marker, "[3 context lines dropped]");
        }
    }

    #[test]
    fn relative_ordering_of_surviving_lines_matches_input() {
        let out = compact_diff(SAMPLE_DIFF, true);
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(
            lines,
            vec![
                "diff --git a/f.rs b/f.rs",
                "index 1234567..89abcde 100644",
                "--- a/f.rs",
                "+++ b/f.rs",
                "@@ -1,7 +1,7 @@",
                "[3 context lines dropped]",
                "-    println!(\"{}\", x);",
                "+    println!(\"{} {}\", x, y);",
                "[3 context lines dropped]",
            ]
        );
    }

    #[test]
    fn empty_input_returns_empty_output() {
        assert_eq!(compact_diff("", true), "");
        assert_eq!(compact_diff("", false), "");
    }

    #[test]
    fn pure_context_input_collapses_to_a_single_marker() {
        let input = " line one\n line two\n line three";
        let out = compact_diff(input, true);
        assert_eq!(out, "[3 context lines dropped]");
    }

    #[test]
    fn header_only_form_marker_wording_covers_dropped_bodies() {
        let out = compact_diff(SAMPLE_DIFF, false);
        // The two context runs (3 each) plus the one removed + one added change-body line
        // in between collapse into a single 8-line run once bodies are dropped too.
        assert_eq!(
            out,
            "diff --git a/f.rs b/f.rs\n\
             index 1234567..89abcde 100644\n\
             --- a/f.rs\n\
             +++ b/f.rs\n\
             @@ -1,7 +1,7 @@\n\
             [8 lines dropped]"
        );
    }
}
