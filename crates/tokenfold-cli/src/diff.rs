//! `tokenfold diff`: a compression-aware line diff between a raw and a compressed payload.
//!
//! ponytail: classic O(n*m) LCS dynamic programming, no external diff crate. Fine for the
//! request/response/log-sized payloads tokenfold targets; if huge multi-MB inputs become
//! common, swap in a linear-space Myers diff (e.g. the `similar` crate) instead of hand-rolling
//! one.

use crate::render::{Colors, thousands};

pub enum Tag {
    Equal,
    Delete,
    Insert,
}

pub struct DiffLine {
    pub tag: Tag,
    pub text: String,
}

pub fn diff_lines(raw: &str, compressed: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = raw.lines().collect();
    let b: Vec<&str> = compressed.lines().collect();
    let (n, m) = (a.len(), b.len());

    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if a[i] == b[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if a[i] == b[j] {
            out.push(DiffLine {
                tag: Tag::Equal,
                text: a[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            out.push(DiffLine {
                tag: Tag::Delete,
                text: a[i].to_string(),
            });
            i += 1;
        } else {
            out.push(DiffLine {
                tag: Tag::Insert,
                text: b[j].to_string(),
            });
            j += 1;
        }
    }
    while i < n {
        out.push(DiffLine {
            tag: Tag::Delete,
            text: a[i].to_string(),
        });
        i += 1;
    }
    while j < m {
        out.push(DiffLine {
            tag: Tag::Insert,
            text: b[j].to_string(),
        });
        j += 1;
    }
    out
}

pub fn render_header(
    orig_tokens: usize,
    comp_tokens: usize,
    savings_pct: f64,
    is_exact: bool,
) -> String {
    let t = if is_exact { "" } else { "~" };
    format!(
        "raw {t}{} \u{2192} compressed {t}{} est. tokens ({savings_pct:.1}% reduction)",
        thousands(orig_tokens),
        thousands(comp_tokens)
    )
}

pub fn render_body(lines: &[DiffLine], colors: &Colors) -> String {
    let mut out = String::new();
    for l in lines {
        let line = match l.tag {
            Tag::Equal => format!("  {}", l.text),
            Tag::Delete => colors.dim(&format!("- {}", l.text)),
            Tag::Insert => format!("+ {}", l.text),
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

pub fn to_json(lines: &[DiffLine]) -> serde_json::Value {
    serde_json::Value::Array(
        lines
            .iter()
            .map(|l| {
                let tag = match l.tag {
                    Tag::Equal => "equal",
                    Tag::Delete => "delete",
                    Tag::Insert => "insert",
                };
                serde_json::json!({ "tag": tag, "text": l.text })
            })
            .collect(),
    )
}
