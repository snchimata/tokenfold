use serde::{Deserialize, Serialize};

use crate::status::Status;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressionReport {
    pub schema_version: String,
    pub original_tokens: usize,
    pub compressed_tokens: usize,
    pub saved_tokens: usize,
    pub savings_ratio: f64, // fraction: 0.353
    pub savings_pct: f64,   // positive percent: 35.3
    pub estimator: EstimatorInfo,
    pub status: Status,
    pub mode: String,
    pub format: String,
    pub task_scope: String,
    pub request_id: Option<String>,
    pub quality: Option<QualityReport>,
    pub budget: Option<BudgetReport>,
    pub cache: Option<CacheReport>,
    pub retrieval: Option<RetrievalReport>,
    pub output_savings: Option<OutputSavingsReport>,
    pub bypass: Option<BypassReport>,
    pub command: Option<CommandReport>,
    pub ledger: Option<LedgerReport>,
    pub transforms: Vec<TransformReport>,
    pub warnings: Vec<Warning>,
}

impl CompressionReport {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        original_tokens: usize,
        compressed_tokens: usize,
        estimator: EstimatorInfo,
        status: Status,
        mode: String,
        format: String,
        task_scope: String,
        transforms: Vec<TransformReport>,
        warnings: Vec<Warning>,
    ) -> Self {
        let saved_tokens = original_tokens.saturating_sub(compressed_tokens);
        let savings_ratio = if original_tokens == 0 {
            0.0
        } else {
            saved_tokens as f64 / original_tokens as f64
        };
        let savings_pct = savings_ratio * 100.0;
        Self {
            schema_version: "1.0".to_string(),
            original_tokens,
            compressed_tokens,
            saved_tokens,
            savings_ratio,
            savings_pct,
            estimator,
            status,
            mode,
            format,
            task_scope,
            request_id: None,
            quality: None,
            budget: None,
            cache: None,
            retrieval: None,
            output_savings: None,
            bypass: None,
            command: None,
            ledger: None,
            transforms,
            warnings,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EstimatorInfo {
    pub backend: String,
    pub model: Option<String>,
    pub is_exact: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetReport {
    pub target_tokens: Option<usize>,
    pub protected_floor: usize,
    pub achieved_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityReport {
    pub eval_profile_id: String,
    pub task_scope: String,
    pub validated_ratio_band: Option<String>,
    pub quality_retention: f64,
    pub contrastive_failure_rate: f64,
    pub gate_passed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TransformReport {
    pub id: String,
    pub version: String,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub saved_tokens: usize,
    pub savings_ratio: f64,
    pub elapsed_micros: Option<u64>,
    pub status: TransformStatus,
    pub skipped_reason: Option<SkippedReason>,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TransformStatus {
    Applied,
    NoOp,
    Skipped,
    RolledBack,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SkippedReason {
    TargetAlreadyMet,
    NotApplicableToFormat,
    NotEnabledInMode,
    ExperimentalFlagRequired,
    DisabledByUser,
    WouldIncreaseTokens,
    FilterUntrusted,
    FilterFailedVerify,
    BypassEnvSet,
    UnsupportedCommandShape,
    PipeOrHeredocNotRewritten,
    BinaryOutputDetected,
    UnsafeCommandPassthrough,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Warning {
    pub code: WarningCode,
    pub severity: Severity,
    pub transform: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Info,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WarningCode {
    UnreachableTarget,
    UnredactedContentPossible,
    SafetyDowngrade,
    SecurityFieldAltered,
    HeuristicBudgetUsed,
    PrefixModified,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CacheReport {
    pub boundary_kind: Option<String>,
    pub protected_bytes: usize,
    pub prefix_byte_identical: bool,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RetrievalReport {
    pub store_namespace: String,
    pub hash_algorithm: String,
    pub marker_count: usize,
    pub ttl_seconds: Option<u64>,
    pub persisted_original_bytes: usize,
    pub skipped_original_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OutputSavingsReport {
    pub profile: String,
    pub estimated_output_tokens_saved: Option<usize>,
    pub measured_output_tokens_saved: Option<usize>,
    pub provenance: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BypassReport {
    pub reason: String,
    pub source: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CommandReport {
    pub command_family: Option<String>,
    pub child_exit_code: Option<i32>,
    pub duration_ms: u64,
    pub raw_output_bytes: usize,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
    pub stderr_mode: String,
    pub stderr_truncated: bool,
    pub compressed_output_bytes: usize,
    pub filter_pack_id: Option<String>,
    pub filter_version: Option<String>,
    pub never_worse_applied: bool,
    pub bypass_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LedgerReport {
    pub recorded: bool,
    pub scope: Option<String>,
    pub project_hash: Option<String>,
    pub record_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn heuristic_estimator() -> EstimatorInfo {
        EstimatorInfo {
            backend: "heuristic".to_string(),
            model: None,
            is_exact: false,
        }
    }

    #[test]
    fn saved_tokens_and_ratio_are_derived_correctly() {
        let report = CompressionReport::new(
            18_400,
            11_900,
            heuristic_estimator(),
            Status::Compressed,
            "balanced".to_string(),
            "plain_text".to_string(),
            "general".to_string(),
            vec![],
            vec![],
        );
        assert_eq!(report.saved_tokens, 6_500);
        assert!((report.savings_ratio - 0.353_260_869_565_217_4).abs() < f64::EPSILON * 10.0);
        assert!((report.savings_pct - 35.326_086_956_521_74).abs() < 1e-9);
        assert_eq!(report.schema_version, "1.0");
    }

    #[test]
    fn zero_original_tokens_never_divides_by_zero() {
        let report = CompressionReport::new(
            0,
            0,
            heuristic_estimator(),
            Status::Passthrough,
            "balanced".to_string(),
            "plain_text".to_string(),
            "general".to_string(),
            vec![],
            vec![],
        );
        assert_eq!(report.saved_tokens, 0);
        assert_eq!(report.savings_ratio, 0.0);
        assert_eq!(report.savings_pct, 0.0);
    }

    #[test]
    fn compressed_never_exceeding_original_keeps_saved_tokens_nonnegative() {
        // saturating_sub guards against compressed_tokens > original_tokens (should never
        // happen, but the report must never panic or underflow if it does).
        let report = CompressionReport::new(
            10,
            15,
            heuristic_estimator(),
            Status::BestEffort,
            "balanced".to_string(),
            "plain_text".to_string(),
            "general".to_string(),
            vec![],
            vec![],
        );
        assert_eq!(report.saved_tokens, 0);
    }

    #[test]
    fn status_serializes_inside_report_as_snake_case() {
        let report = CompressionReport::new(
            100,
            80,
            heuristic_estimator(),
            Status::UnreachableTarget,
            "balanced".to_string(),
            "plain_text".to_string(),
            "general".to_string(),
            vec![],
            vec![],
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["status"], "unreachable_target");
        assert_eq!(json["estimator"]["backend"], "heuristic");
        assert_eq!(json["estimator"]["is_exact"], false);
    }

    #[test]
    fn quality_report_round_trips() {
        let quality = QualityReport {
            eval_profile_id: "smoke-first-consumer".to_string(),
            task_scope: "code_review".to_string(),
            validated_ratio_band: Some("0.6-0.8".to_string()),
            quality_retention: 0.975,
            contrastive_failure_rate: 0.0,
            gate_passed: true,
        };
        let json = serde_json::to_string(&quality).unwrap();
        let back: QualityReport = serde_json::from_str(&json).unwrap();
        assert_eq!(quality, back);
    }
}
