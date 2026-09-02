//! Canonical preset matrix: the single source of truth for which transforms run in which
//! preset, at what ratio cap, for which task scopes and input formats. `secret_redaction` is
//! deliberately absent from this table — it runs unconditionally before the pipeline and
//! cannot be disabled (see `budget::CompressionPolicyBuilder::build`).
//!
//! `tests/fixtures/mode_matrix.toml` mirrors this table for cross-surface testing
//! (this table is authoritative; the fixture must be kept in sync with it).

use crate::budget::{Preset, TaskScope};
use crate::input::InputFormat;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransformId {
    JsonMinify,
    JsonFieldFold,
    JsonValueDict,
    SchemaCompaction,
    LogFieldFold,
    LogCompaction,
    DiffCompaction,
}

impl TransformId {
    pub fn as_str(&self) -> &'static str {
        match self {
            TransformId::JsonMinify => "json_minify",
            TransformId::JsonFieldFold => "json_field_fold",
            TransformId::JsonValueDict => "json_value_dict",
            TransformId::SchemaCompaction => "schema_compaction",
            TransformId::LogFieldFold => "log_field_fold",
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
    pub fn enabled_for(&self, preset: Preset) -> bool {
        match preset {
            Preset::Conservative => self.conservative_enabled,
            Preset::Balanced => self.balanced_enabled,
            Preset::Aggressive => self.aggressive_enabled,
        }
    }

    pub fn max_ratio_for(&self, preset: Preset) -> f64 {
        match preset {
            Preset::Conservative => self.max_ratio_conservative,
            Preset::Balanced => self.max_ratio_balanced,
            Preset::Aggressive => self.max_ratio_aggressive,
        }
    }

    fn applies_to_format(&self, format: InputFormat) -> bool {
        self.applicable_formats.contains(&format)
    }
}

// Canonical ordered table — order here IS the pipeline execution order (lossless before lossy,
// higher-savings before lower-savings, within each preset).
//
// ponytail: `table_compaction` is intentionally omitted. Tabular payloads aren't among this
// project's dominant input types, so a table transform stays out of scope until a real consumer
// asks for it.
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
        applicable_formats: &[
            InputFormat::OpenAiJson,
            InputFormat::AnthropicJson,
            InputFormat::Json,
        ],
    },
    // json_field_fold (v0.2): reversible columnar fold of arrays of homogeneous objects.
    // Lossless (round-trip gated in the pipeline), so max_ratio is unrestricted (1.0), but
    // it restructures what the model sees, so it stays out of Conservative (same convention
    // as log_compaction) and only runs on generic Json data, never on OpenAI/Anthropic
    // message bodies (whose API shape must not change).
    ModeEntry {
        transform_id: TransformId::JsonFieldFold,
        version: "1.0.0",
        conservative_enabled: false,
        balanced_enabled: true,
        aggressive_enabled: true,
        experimental: false,
        max_ratio_conservative: 0.0,
        max_ratio_balanced: 1.0,
        max_ratio_aggressive: 1.0,
        task_scopes: &[TaskScope::All],
        applicable_formats: &[InputFormat::Json],
    },
    // json_value_dict (v0.2): reversible value deduplication. Runs AFTER json_field_fold so it
    // also collapses the repeated nested values folding surfaces across rows. Lossless
    // (round-trip gated), unrestricted ratio, out of Conservative, generic Json only.
    ModeEntry {
        transform_id: TransformId::JsonValueDict,
        version: "1.0.0",
        conservative_enabled: false,
        balanced_enabled: true,
        aggressive_enabled: true,
        experimental: false,
        max_ratio_conservative: 0.0,
        max_ratio_balanced: 1.0,
        max_ratio_aggressive: 1.0,
        task_scopes: &[TaskScope::All],
        applicable_formats: &[InputFormat::Json],
    },
    ModeEntry {
        transform_id: TransformId::SchemaCompaction,
        version: "1.0.0",
        conservative_enabled: false,
        balanced_enabled: false,
        aggressive_enabled: false,
        experimental: false,
        max_ratio_conservative: 0.15,
        max_ratio_balanced: 0.30,
        max_ratio_aggressive: 0.50,
        task_scopes: &[TaskScope::All],
        applicable_formats: &[InputFormat::OpenAiJson, InputFormat::AnthropicJson],
    },
    // log_field_fold (v0.4): reversible columnar fold of TEMPLATED log lines — the log-line
    // analogue of json_field_fold (emit each shared line skeleton once + per-line captured fields).
    // Lossless (round-trip gated in the pipeline), so max_ratio is unrestricted (1.0), but like
    // log_compaction it restructures what the model sees, so it stays out of Conservative and ships
    // behind --experimental until its fidelity gate is green (the same path json_field_fold and
    // log_compaction took). Runs before the lossy log_compaction (lossless-before-lossy ordering).
    ModeEntry {
        transform_id: TransformId::LogFieldFold,
        version: "1.0.0",
        conservative_enabled: false,
        balanced_enabled: true,
        aggressive_enabled: true,
        experimental: false,
        max_ratio_conservative: 0.0,
        max_ratio_balanced: 1.0,
        max_ratio_aggressive: 1.0,
        task_scopes: &[TaskScope::All],
        applicable_formats: &[InputFormat::PlainText, InputFormat::CommandOutput],
    },
    ModeEntry {
        transform_id: TransformId::LogCompaction,
        version: "1.0.0",
        // Promoted out of --experimental (Phase 5 fidelity gate, 2026-07-12): the
        // full-lossy-promotion gate profile clears every draft fidelity threshold cleanly
        // (quality_retention=1.0, contrastive_failure_rate=0.0, critical_token_survival_rate=1.0).
        // conservative_enabled stays false — Conservative never runs lossy-with-evidence
        // transforms at all, same convention table_compaction documents.
        conservative_enabled: false,
        balanced_enabled: false,
        aggressive_enabled: false,
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
        // Stays --experimental (Phase 5 fidelity gate, 2026-07-12 re-investigation): the
        // full-lossy-promotion gate's per_variant breakdown checked the default (body-preserving,
        // task_scope != ChangeSummary) and header-only (TaskScope::ChangeSummary) forms
        // separately, as diff_compaction's two documented forms require, and BOTH miss the draft
        // fidelity thresholds on their own — the bar is quality_retention >= 0.95,
        // contrastive_failure_rate <= 0.005, critical_token_survival >= 0.99, and the default form
        // measured quality_retention=0.36, contrastive_failure_rate=0.5, critical_token_survival=
        // 0.5. Root cause: compact_diff has no fallback for non-diff-shaped input — it drops
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
    // here after their fidelity approval / scope decisions land.
];

/// Returns the ordered, applicable transform list for a given (preset, task_scope, format).
/// `secret_redaction` is not part of this table: the pipeline always runs it first,
/// unconditionally, before consulting this function.
pub fn pipeline_for(
    preset: Preset,
    task_scope: TaskScope,
    format: InputFormat,
    experimental: bool,
    enabled_ids: &[String],
    disabled_ids: &[String],
) -> Vec<&'static ModeEntry> {
    ALL_ENTRIES
        .iter()
        .filter(|e| {
            let mode_enabled = e.enabled_for(preset);
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
            Preset::Conservative,
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
            Preset::Balanced,
            TaskScope::All,
            InputFormat::OpenAiJson,
            false,
            &[],
            &[],
        );
        let ids: Vec<_> = entries.iter().map(|e| e.transform_id).collect();
        assert!(ids.contains(&TransformId::JsonMinify));
        assert!(!ids.contains(&TransformId::SchemaCompaction));
    }

    #[test]
    fn irreversible_log_compaction_stays_out_of_presets() {
        let entries = pipeline_for(
            Preset::Balanced,
            TaskScope::General,
            InputFormat::CommandOutput,
            true,
            &[],
            &[],
        );
        assert!(
            entries
                .iter()
                .all(|e| e.transform_id != TransformId::LogCompaction)
        );
    }

    #[test]
    fn log_compaction_skipped_for_non_applicable_format_even_when_experimental() {
        let entries = pipeline_for(
            Preset::Balanced,
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
            Preset::Balanced,
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
            Preset::Balanced,
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
