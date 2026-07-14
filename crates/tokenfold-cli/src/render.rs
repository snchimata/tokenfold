//! Human-readable report rendering per PLAN.md "CLI Output & UX Spec" and INTERFACES.md §1.7.

use std::io::IsTerminal;

use tokenfold_core::report::{
    CompressionReport, Severity, SkippedReason, TransformStatus, Warning,
};
use tokenfold_core::{InputFormat, Status};

pub struct Colors {
    enabled: bool,
}

impl Colors {
    pub fn new(no_color: bool, stream_is_tty: bool) -> Self {
        Colors {
            enabled: !no_color && stream_is_tty,
        }
    }

    fn wrap(&self, code: &str, s: &str) -> String {
        if self.enabled {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }

    pub fn green(&self, s: &str) -> String {
        self.wrap("32", s)
    }
    pub fn yellow(&self, s: &str) -> String {
        self.wrap("33", s)
    }
    pub fn red(&self, s: &str) -> String {
        self.wrap("31", s)
    }
    pub fn blue(&self, s: &str) -> String {
        self.wrap("34", s)
    }
    pub fn dim(&self, s: &str) -> String {
        self.wrap("2", s)
    }
}

pub fn stderr_colors(no_color: bool) -> Colors {
    Colors::new(no_color, std::io::stderr().is_terminal())
}

pub fn stdout_colors(no_color: bool) -> Colors {
    Colors::new(no_color, std::io::stdout().is_terminal())
}

pub(crate) fn thousands(n: usize) -> String {
    let s = n.to_string();
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len() + bytes.len() / 3);
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 && (bytes.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(*b as char);
    }
    out
}

fn tilde(is_exact: bool) -> &'static str {
    if is_exact { "" } else { "~" }
}

/// Verdict line. `target_tokens` is the value the caller resolved and passed to
/// `CompressionPolicy`, not `report.budget` — the latter is only populated on some status
/// paths (see `pipeline::compress_with_estimator`), so the caller's own resolved value is the
/// reliable source for "was a target requested at all".
pub fn render_verdict(
    report: &CompressionReport,
    target_tokens: Option<usize>,
    is_inspect: bool,
    colors: &Colors,
) -> String {
    let t = tilde(report.estimator.is_exact);
    let orig = thousands(report.original_tokens);
    let comp = thousands(report.compressed_tokens);
    match (target_tokens, &report.status) {
        (Some(target), Status::Passthrough) => colors.dim(&format!(
            "UNDER budget: {t}{orig} est. tokens \u{2264} target {} \u{2014} nothing to compress",
            thousands(target)
        )),
        (Some(target), Status::Compressed) => colors.green(&format!(
            "OVER budget: {t}{orig} \u{2192} {t}{comp} est. tokens ({:.1}% reduction, target {}) \u{2713} reachable",
            report.savings_pct,
            thousands(target)
        )),
        (Some(target), Status::BestEffort) => colors.yellow(&format!(
            "OVER budget: {t}{orig} \u{2192} {t}{comp} est. tokens ({:.1}% reduction, target {}) ~ best effort (target not fully reached)",
            report.savings_pct,
            thousands(target)
        )),
        (Some(target), Status::UnreachableTarget) => {
            let floor = report
                .budget
                .as_ref()
                .map(|b| format!(", protected floor {}", thousands(b.protected_floor)))
                .unwrap_or_default();
            colors.yellow(&format!(
                "OVER budget: {t}{orig} \u{2192} {t}{comp} est. tokens (target {}{floor}) ! unreachable: target is below the protected floor",
                thousands(target)
            ))
        }
        (None, _) if is_inspect => {
            colors.dim("No target set \u{2014} showing max achievable savings per transform")
        }
        (None, _) => colors.green(&format!(
            "Compressed to safe floor: {t}{orig} \u{2192} {t}{comp} est. tokens ({:.1}% reduction, no target set)",
            report.savings_pct
        )),
    }
}

fn truncate(s: &str, max: usize, no_truncate: bool) -> String {
    if no_truncate || s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{head}\u{2026}")
}

fn mode_label_for(id: &str) -> &'static str {
    if id == "secret_redaction" {
        return "all";
    }
    for entry in tokenfold_core::modes::ALL_ENTRIES {
        if entry.transform_id.as_str() == id {
            if entry.experimental {
                return "experimental";
            }
            if entry.conservative_enabled {
                return "all";
            }
            if entry.balanced_enabled {
                return "balanced+";
            }
            if entry.aggressive_enabled {
                return "aggressive+";
            }
            return "off";
        }
    }
    "?"
}

fn skipped_reason_label(r: &SkippedReason) -> &'static str {
    match r {
        SkippedReason::TargetAlreadyMet => "target_already_met",
        SkippedReason::NotApplicableToFormat => "not_applicable_to_format",
        SkippedReason::NotEnabledInMode => "not_enabled_in_mode",
        SkippedReason::ExperimentalFlagRequired => "experimental_flag_required",
        SkippedReason::DisabledByUser => "disabled_by_user",
        SkippedReason::WouldIncreaseTokens => "would_increase_tokens",
        SkippedReason::FilterUntrusted => "filter_untrusted",
        SkippedReason::FilterFailedVerify => "filter_failed_verify",
        SkippedReason::BypassEnvSet => "bypass_env_set",
        SkippedReason::UnsupportedCommandShape => "unsupported_command_shape",
        SkippedReason::PipeOrHeredocNotRewritten => "pipe_or_heredoc_not_rewritten",
        SkippedReason::BinaryOutputDetected => "binary_output_detected",
        SkippedReason::UnsafeCommandPassthrough => "unsafe_command_passthrough",
    }
}

fn status_label(status: &TransformStatus, reason: &Option<SkippedReason>) -> String {
    match status {
        TransformStatus::Applied => "applied".to_string(),
        TransformStatus::NoOp => "no_op".to_string(),
        TransformStatus::RolledBack => "rolled_back".to_string(),
        TransformStatus::Skipped => match reason {
            Some(r) => format!("skipped ({})", skipped_reason_label(r)),
            None => "skipped".to_string(),
        },
    }
}

pub fn render_transform_table(
    report: &CompressionReport,
    colors: &Colors,
    no_truncate: bool,
) -> String {
    let mut out = format!(
        "{:<22} {:<12} {:>24} {:>10} {:>7}  STATUS\n",
        "TRANSFORM", "MODE", "EST TOKENS BEFORE\u{2192}AFTER", "SAVED", "%"
    );
    for t in &report.transforms {
        let name = truncate(&t.id, 22, no_truncate);
        let mode = mode_label_for(&t.id);
        let before_after = format!(
            "{}\u{2192}{}",
            thousands(t.tokens_before),
            thousands(t.tokens_after)
        );
        let saved = thousands(t.saved_tokens);
        let pct = format!("{:.1}%", t.savings_ratio * 100.0);
        let status = status_label(&t.status, &t.skipped_reason);
        let row =
            format!("{name:<22} {mode:<12} {before_after:>24} {saved:>10} {pct:>7}  {status}");
        let row = if matches!(
            t.status,
            TransformStatus::Skipped | TransformStatus::RolledBack
        ) {
            colors.dim(&row)
        } else {
            row
        };
        out.push_str(&row);
        out.push('\n');
    }
    out.push_str(
        "(est. = bytes/4 heuristic unless the estimator is exact; see the EST prefix above)\n",
    );
    out
}

pub fn render_totals(report: &CompressionReport) -> String {
    format!(
        "TOTAL  {} \u{2192} {} tokens   saved {} ({:.1}% reduction)",
        thousands(report.original_tokens),
        thousands(report.compressed_tokens),
        thousands(report.saved_tokens),
        report.savings_pct
    )
}

fn severity_rank(s: &Severity) -> u8 {
    match s {
        Severity::Critical => 0,
        Severity::Warn => 1,
        Severity::Info => 2,
    }
}

pub fn render_warnings(report: &CompressionReport, colors: &Colors) -> String {
    if report.warnings.is_empty() {
        return String::new();
    }
    let mut warnings: Vec<&Warning> = report.warnings.iter().collect();
    warnings.sort_by_key(|w| severity_rank(&w.severity));

    let mut out = String::from("WARNINGS:\n");
    for w in warnings {
        let (glyph, colored): (&str, String) = {
            let prefix = w
                .transform
                .as_deref()
                .map(|t| format!("[{t}] "))
                .unwrap_or_default();
            let line = format!("{prefix}{:?}: {}", w.code, w.message);
            match w.severity {
                Severity::Critical => ("\u{274c}", colors.red(&line)),
                Severity::Warn => ("\u{26a0}\u{fe0f}", colors.yellow(&line)),
                Severity::Info => ("\u{1f7e6}", colors.blue(&line)),
            }
        };
        out.push_str(&format!("  {glyph} {colored}\n"));
    }
    out
}

fn format_label(format: InputFormat) -> &'static str {
    match format {
        InputFormat::Auto => "auto",
        InputFormat::OpenAiJson => "openai_json",
        InputFormat::AnthropicJson => "anthropic_json",
        InputFormat::Json => "json",
        InputFormat::PlainText => "plain_text",
        InputFormat::CommandOutput => "command_output",
        InputFormat::GitDiff => "git_diff",
    }
}

pub fn render_transform_list() -> String {
    let mut out = format!(
        "{:<20} {:<12} {:<12} FORMAT\n",
        "TRANSFORM", "MODE", "STATUS"
    );
    out.push_str(&format!(
        "{:<20} {:<12} {:<12} {}\n",
        "secret_redaction",
        "all",
        "enabled",
        "openai_json, anthropic_json, plain_text, command_output, git_diff"
    ));
    for entry in tokenfold_core::modes::ALL_ENTRIES {
        let id = entry.transform_id.as_str();
        let mode = mode_label_for(id);
        let status = if entry.experimental {
            "experimental"
        } else if entry.conservative_enabled || entry.balanced_enabled || entry.aggressive_enabled {
            "enabled"
        } else {
            "disabled"
        };
        let formats = entry
            .applicable_formats
            .iter()
            .copied()
            .map(format_label)
            .collect::<Vec<_>>()
            .join(", ");
        out.push_str(&format!("{id:<20} {mode:<12} {status:<12} {formats}\n"));
    }
    out
}

pub fn render_transform_list_json() -> serde_json::Value {
    let mut rows = vec![serde_json::json!({
        "id": "secret_redaction",
        "mode": "all",
        "status": "enabled",
        "formats": ["openai_json", "anthropic_json", "plain_text", "command_output", "git_diff"],
    })];
    for entry in tokenfold_core::modes::ALL_ENTRIES {
        let id = entry.transform_id.as_str();
        let status = if entry.experimental {
            "experimental"
        } else if entry.conservative_enabled || entry.balanced_enabled || entry.aggressive_enabled {
            "enabled"
        } else {
            "disabled"
        };
        rows.push(serde_json::json!({
            "id": id,
            "mode": mode_label_for(id),
            "status": status,
            "formats": entry.applicable_formats.iter().copied().map(format_label).collect::<Vec<_>>(),
        }));
    }
    serde_json::Value::Array(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_color_flag_disables_regardless_of_tty() {
        assert!(!Colors::new(true, true).enabled);
    }

    #[test]
    fn color_enabled_only_when_tty_and_not_suppressed() {
        assert!(Colors::new(false, true).enabled);
        assert!(!Colors::new(false, false).enabled);
    }

    #[test]
    fn colorize_wraps_in_ansi_only_when_enabled() {
        let on = Colors::new(false, true);
        let off = Colors::new(false, false);
        assert_eq!(on.green("x"), "\x1b[32mx\x1b[0m");
        assert_eq!(off.green("x"), "x");
    }

    #[test]
    fn thousands_inserts_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1_000), "1,000");
        assert_eq!(thousands(18_400), "18,400");
        assert_eq!(thousands(1_234_567), "1,234,567");
    }

    #[test]
    fn truncate_respects_max_and_no_truncate_override() {
        assert_eq!(
            truncate("schema_compaction_with_a_long_name", 22, false)
                .chars()
                .count(),
            22
        );
        assert!(truncate("schema_compaction_with_a_long_name", 22, false).ends_with('\u{2026}'));
        assert_eq!(
            truncate("schema_compaction_with_a_long_name", 22, true),
            "schema_compaction_with_a_long_name"
        );
        assert_eq!(truncate("short", 22, false), "short");
    }
}
