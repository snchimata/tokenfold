//! Canonical mode matrix: the single source of truth for which transforms run in which
//! mode, at what ratio cap, for which task scopes and input formats. `secret_redaction` is
//! deliberately absent from this table — it runs unconditionally before the pipeline and
//! cannot be disabled (see `budget::CompressionPolicyBuilder::build`).
//!
//! `tests/fixtures/mode_matrix.toml` mirrors this table for cross-surface testing
//! (INTERFACES.md Part 2 is the authoritative reference for both).

use crate::budget::{CompressionMode, TaskScope};
use crate::input::InputFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformId {
    JsonMinify,
    SchemaCompaction,
    LogCompaction,
    DiffCompaction,
}

impl TransformId {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransformId::JsonMinify => "json_minify",
            TransformId::SchemaCompaction => "schema_compaction",
            TransformId::LogCompaction => "log_compaction",
            TransformId::DiffCompaction => "diff_compaction",
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ModeEntry {
    pub transform_id: TransformId,
    pub version: &'static str,
    pub conservative_enabled: bool,
    pub balanced_enabled: bool,
    pub aggressive_enabled: bool,
    pub experimental: bool,
    pub max_ratio_conservative: f64,
    pub max_ratio_balanced: f64,
    pub max_ratio_aggressive: f64,
    pub task_scopes: &'static [TaskScope],
    pub applicable_formats: &'static [InputFormat],
}

impl ModeEntry {
    pub fn enabled_for(&self, mode: CompressionMode) -> bool {
        match mode {
            CompressionMode::Conservative => self.conservative_enabled,
            CompressionMode::Balanced => self.balanced_enabled,
            CompressionMode::Aggressive => self.aggressive_enabled,
        }
    }

    pub fn max_ratio_for(&self, mode: CompressionMode) -> f64 {
        match mode {
            CompressionMode::Conservative => self.max_ratio_conservative,
            CompressionMode::Balanced => self.max_ratio_balanced,
            CompressionMode::Aggressive => self.max_ratio_aggressive,
        }
    }

    fn applies_to_format(&self, format: InputFormat) -> bool {
        self.applicable_formats.contains(&format)
    }
}

// Canonical ordered table — order here IS the pipeline execution order (INTERFACES.md Part 2:
// lossless before lossy, higher-savings before lower-savings, within each mode).
//
// ponytail: `table_compaction` is intentionally omitted. The First Consumer worksheet
// (plan.md) doesn't name tables among the dominant payload types, so F-019 stays out of
// scope until a consumer worksheet asks for it (roadmap.md F-019 dependency).
pub static ALL_ENTRIES: &[ModeEntry] = &[
    ModeEntry {
        transform_id: TransformId::JsonMinify,
        version: "1.0.0",
        conservative_enabled: true,
        balanced_enabled: true,
        aggressive_enabled: true,
        experimental: false,
        max_ratio_conservative: 1.0,
        max_ratio_balanced: 1.0,
        max_ratio_aggressive: 1.0,
        task_scopes: &[TaskScope::All],
        applicable_formats: &[InputFormat::OpenAiJson, InputFormat::AnthropicJson],
    },
    ModeEntry {
        transform_id: TransformId::SchemaCompaction,
        version: "1.0.0",
        conservative_enabled: true,
        balanced_enabled: true,
        aggressive_enabled: true,
        experimental: false,
        max_ratio_conservative: 0.15,
        max_ratio_balanced: 0.30,
        max_ratio_aggressive: 0.50,
        task_scopes: &[TaskScope::All],
        applicable_formats: &[InputFormat::OpenAiJson, InputFormat::AnthropicJson],
    },
    ModeEntry {
        transform_id: TransformId::LogCompaction,
        version: "1.0.0",
        // Promoted out of --experimental (roadmap.md Phase 5 Task 9, 2026-07-12): the
        // full-lossy-promotion fidelity gate clears every D-005 draft threshold cleanly
        // (quality_retention=1.0, contrastive_failure_rate=0.0, critical_token_survival_rate=1.0).
        // conservative_enabled stays false — per plan.md's mode table, Conservative never runs
        // lossy-with-evidence transforms at all, same convention table_compaction documents.
        conservative_enabled: false,
        balanced_enabled: true,
        aggressive_enabled: true,
        experimental: false,
        max_ratio_conservative: 0.0,
        max_ratio_balanced: 0.65, // draft; updated after Phase 2 accuracy@ratio data
        max_ratio_aggressive: 0.75,
        task_scopes: &[TaskScope::General, TaskScope::ChangeSummary],
        applicable_formats: &[InputFormat::PlainText, InputFormat::CommandOutput],
    },
    ModeEntry {
        transform_id: TransformId::DiffCompaction,
        version: "1.0.0",
        // Stays --experimental (roadmap.md Phase 5 Task 9, 2026-07-12 re-investigation): the
        // full-lossy-promotion gate's per_variant breakdown checked the default (body-preserving,
        // task_scope != ChangeSummary) and header-only (TaskScope::ChangeSummary) forms
        // separately, as F-013 requires, and BOTH miss the D-005 draft thresholds on their own
        // (default: quality_retention=0.36, contrastive_failure_rate=0.5, critical_token_survival=
        // 0.5). Root cause: compact_diff has no fallback for non-diff-shaped input — it drops
        // everything, critical tokens included, when no line matches a unified-diff prefix. See
        // eval/tasks/FIXTURES.md's "Scorer status" section for the full measured breakdown.
        conservative_enabled: false,
        balanced_enabled: false,
        aggressive_enabled: false,
        experimental: true,
        max_ratio_conservative: 0.0,
        max_ratio_balanced: 0.60,
        max_ratio_aggressive: 0.70,
        task_scopes: &[TaskScope::CodeReview, TaskScope::ChangeSummary],
        applicable_formats: &[
            InputFormat::PlainText,
            InputFormat::CommandOutput,
            InputFormat::GitDiff,
        ],
    },
    // v0.2+ entries (table_compaction, prose_extraction, code_digest, conversation) added
    // here after their fidelity approval / D-002 scope decisions land.
];

/// Returns the ordered, applicable transform list for a given (mode, task_scope, format).
/// `secret_redaction` is not part of this table: the pipeline always runs it first,
/// unconditionally, before consulting this function.
pub fn pipeline_for(
    mode: CompressionMode,
    task_scope: TaskScope,
    format: InputFormat,
    experimental: bool,
    enabled_ids: &[String],
    disabled_ids: &[String],
) -> Vec<&'static ModeEntry> {
    ALL_ENTRIES
        .iter()
        .filter(|e| {
            let mode_enabled = e.enabled_for(mode);
            let experimentally_enabled = e.experimental && experimental;
            let explicitly_enabled = (!e.experimental || experimental)
                && enabled_ids.iter().any(|id| id == e.transform_id.as_str());
            mode_enabled || experimentally_enabled || explicitly_enabled
        })
        .filter(|e| !disabled_ids.iter().any(|id| id == e.transform_id.as_str()))
        .filter(|e| e.task_scopes.contains(&TaskScope::All) || e.task_scopes.contains(&task_scope))
        .filter(|e| e.applies_to_format(format))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transform_id_as_str_matches_canonical_ids() {
        assert_eq!(TransformId::JsonMinify.as_str(), "json_minify");
        assert_eq!(TransformId::SchemaCompaction.as_str(), "schema_compaction");
        assert_eq!(TransformId::LogCompaction.as_str(), "log_compaction");
        assert_eq!(TransformId::DiffCompaction.as_str(), "diff_compaction");
    }

    #[test]
    fn conservative_mode_never_includes_experimental_lossy_transforms() {
        let entries = pipeline_for(
            CompressionMode::Conservative,
            TaskScope::All,
            InputFormat::PlainText,
            /* experimental */ true,
            &[],
            &[],
        );
        assert!(
            entries
                .iter()
                .all(|e| e.transform_id != TransformId::LogCompaction
                    && e.transform_id != TransformId::DiffCompaction)
        );
    }

    #[test]
    fn balanced_mode_lossless_transforms_apply_to_openai_json() {
        let entries = pipeline_for(
            CompressionMode::Balanced,
            TaskScope::All,
            InputFormat::OpenAiJson,
            false,
            &[],
            &[],
        );
        let ids: Vec<_> = entries.iter().map(|e| e.transform_id).collect();
        assert!(ids.contains(&TransformId::JsonMinify));
        assert!(ids.contains(&TransformId::SchemaCompaction));
    }

    #[test]
    fn experimental_flag_enables_log_compaction_for_matching_task_scope() {
        let entries = pipeline_for(
            CompressionMode::Balanced,
            TaskScope::General,
            InputFormat::CommandOutput,
            true,
            &[],
            &[],
        );
        assert!(
            entries
                .iter()
                .any(|e| e.transform_id == TransformId::LogCompaction)
        );
    }

    #[test]
    fn log_compaction_skipped_for_non_applicable_format_even_when_experimental() {
        let entries = pipeline_for(
            CompressionMode::Balanced,
            TaskScope::General,
            InputFormat::OpenAiJson,
            true,
            &[],
            &[],
        );
        assert!(
            !entries
                .iter()
                .any(|e| e.transform_id == TransformId::LogCompaction)
        );
    }

    #[test]
    fn disabled_ids_remove_a_transform_even_when_otherwise_enabled() {
        let entries = pipeline_for(
            CompressionMode::Balanced,
            TaskScope::All,
            InputFormat::OpenAiJson,
            false,
            &[],
            &["json_minify".to_string()],
        );
        assert!(
            !entries
                .iter()
                .any(|e| e.transform_id == TransformId::JsonMinify)
        );
    }

    #[test]
    fn diff_compaction_requires_matching_task_scope_even_with_enable_flag() {
        // enable + experimental together still respect task_scope filtering.
        let entries = pipeline_for(
            CompressionMode::Balanced,
            TaskScope::Debugging,
            InputFormat::GitDiff,
            true,
            &["diff_compaction".to_string()],
            &[],
        );
        assert!(
            !entries
                .iter()
                .any(|e| e.transform_id == TransformId::DiffCompaction)
        );
    }
}
