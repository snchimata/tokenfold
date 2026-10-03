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
    pub preset: String,
    pub format: String,
    pub output_encoding: String,
    pub task_scope: String,
    pub request_id: Option<String>,
    /// Staged `raw -> RTK -> tokenfold` accounting. `None` for the common
    /// single-stage path; populated only by RTK-composed `wrap --rtk` runs.
    #[serde(default)]
    pub pipeline: Option<PipelineReport>,
    pub quality: Option<QualityReport>,
    pub budget: Option<BudgetReport>,
    pub encoding: Option<EncodingReport>,
    pub pruning: Option<PruningReport>,
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
    /// Reads a receipt, refusing any `schema_version` this build does not understand.
    ///
    /// This is the versioned reader. Plain `serde_json::from_slice` is still available and still
    /// used on paths where the bytes came from this process, but a receipt from disk or from a
    /// network must go through here: an unrecognized version is a hard error, never a best-effort
    /// parse that could surface a missing number as a real one.
    pub fn parse_versioned(bytes: &[u8]) -> Result<Self, ReportParseError> {
        let mut value: serde_json::Value =
            serde_json::from_slice(bytes).map_err(|_| ReportParseError::Malformed)?;
        let version = value
            .get("schema_version")
            .and_then(serde_json::Value::as_str)
            .ok_or(ReportParseError::Malformed)?
            .to_string();
        if !SUPPORTED_SCHEMA_VERSIONS.contains(&version.as_str()) {
            return Err(ReportParseError::UnsupportedVersion { got: version });
        }
        // A known older shape is normalized here rather than refused: refusing a receipt we have
        // shipped and documented would make every archived receipt unreadable. An *unknown*
        // version is still refused, because we cannot know what it means.
        if version == "1.0" {
            value = upgrade_v1_receipt(value);
        }
        serde_json::from_value(value).map_err(|_| ReportParseError::Malformed)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        original_tokens: usize,
        compressed_tokens: usize,
        estimator: EstimatorInfo,
        status: Status,
        preset: String,
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
            schema_version: "2.0".to_string(),
            original_tokens,
            compressed_tokens,
            saved_tokens,
            savings_ratio,
            savings_pct,
            estimator,
            status,
            preset,
            format,
            output_encoding: "native".to_string(),
            task_scope,
            request_id: None,
            pipeline: None,
            quality: None,
            budget: None,
            encoding: None,
            pruning: None,
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

/// The schema version this build writes.
pub const CURRENT_SCHEMA_VERSION: &str = "2.0";

/// Receipt schema versions this build can read.
///
/// A reader must know which versions it understands *before* it starts handing numbers to a
/// caller. Accepting an unknown version and parsing it best-effort is how an unavailable value
/// quietly turns into a plausible-looking zero: v3 might make `original_tokens` optional, and a
/// reader that "handled" the absence would report `0` rather than admitting it does not know.
pub const SUPPORTED_SCHEMA_VERSIONS: &[&str] = &["1.0", "2.0"];

/// Why a receipt could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReportParseError {
    /// The bytes were not a JSON receipt at all.
    Malformed,
    /// The receipt declares a `schema_version` this build does not read.
    ///
    /// Refused rather than best-effort parsed. A caller that needs this receipt must be told the
    /// format is unknown, not handed numbers from a contract it is not reading.
    UnsupportedVersion { got: String },
}

impl std::fmt::Display for ReportParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReportParseError::Malformed => write!(f, "not a valid compression report"),
            ReportParseError::UnsupportedVersion { got } => write!(
                f,
                "unsupported compression report schema_version {got:?}; this build reads {:?}",
                SUPPORTED_SCHEMA_VERSIONS
            ),
        }
    }
}

impl std::error::Error for ReportParseError {}

/// Normalizes a v1 receipt into the v2 field set.
///
/// Every mapping here is a *rename or a documented absence*, never an invented measurement:
///
/// * v1 named the preset field `mode`; v2 renamed it to `preset`. Same value, renamed.
/// * v1 predates output re-encoding, so it always emitted the input's own encoding. `native` is
///   therefore the truthful value, not a default that hides a difference.
/// * v1 predates pruning accounting entirely, so it is `null` -- "not measured", which must not
///   become a zero.
fn upgrade_v1_receipt(mut value: serde_json::Value) -> serde_json::Value {
    let Some(object) = value.as_object_mut() else {
        return value;
    };
    if !object.contains_key("preset")
        && let Some(mode) = object.remove("mode")
    {
        object.insert("preset".to_string(), mode);
    }
    object
        .entry("output_encoding".to_string())
        .or_insert_with(|| serde_json::json!("native"));
    object
        .entry("pruning".to_string())
        .or_insert(serde_json::Value::Null);
    value
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EstimatorInfo {
    pub backend: String,
    pub model: Option<String>,
    pub is_exact: bool,
}

/// Separates savings and recoverability across composed stages (RTK then
/// tokenfold) so RTK's savings are never credited to tokenfold. The top-level
/// `original_tokens` keeps its v1 meaning — tokens *entering* `tokenfold_core`,
/// which is the post-RTK count when composed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipelineReport {
    /// Pre-RTK byte count. `Some` only when a complete raw capture was observed.
    pub raw_input_bytes: Option<usize>,
    /// Pre-RTK token count. `Some` only when raw capture is complete.
    pub raw_input_tokens: Option<usize>,
    pub final_output_bytes: usize,
    /// Equals top-level `compressed_tokens`.
    pub final_output_tokens: usize,
    /// Populated only when raw and final counts use the same estimator.
    pub total_saved_tokens: Option<usize>,
    /// `"complete"`, `"partial"`, `"unavailable"`, or `"not_applicable"`.
    pub raw_capture: String,
    /// `"full"`, `"tokenfold_only"`, `"none"`, or `"not_applicable"`.
    pub upstream_recoverability: String,
    pub stages: Vec<PipelineStageReport>,
}

/// One composed stage. Count fields are nullable because an unavailable
/// stage or missing pre-stage capture cannot be measured honestly.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PipelineStageReport {
    /// `"rtk"` or `"tokenfold"`.
    pub id: String,
    pub version: Option<String>,
    pub input_bytes: Option<usize>,
    pub output_bytes: Option<usize>,
    pub saved_bytes: Option<usize>,
    pub input_tokens: Option<usize>,
    pub output_tokens: Option<usize>,
    pub saved_tokens: Option<usize>,
    pub estimator: Option<EstimatorInfo>,
    /// `"applied"`, `"passthrough"`, `"unavailable"`, `"incompatible"`, or `"failed"`.
    pub status: String,
    pub duration_ms: Option<f64>,
    pub bypass_reason: Option<String>,
    /// e.g. `"external:rtk@0.4.1"` or `"tokenfold_core"`.
    pub provenance: String,
    /// `"full"`, `"partial"`, `"none"`, or `"not_applicable"`.
    pub recoverability: String,
    pub evidence_ref: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BudgetReport {
    pub status: BudgetStatus,
    pub target_tokens: Option<usize>,
    pub protected_floor: usize,
    pub achieved_tokens: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    NotRequested,
    Met,
    BestEffort,
    Unreachable,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EncodingReport {
    pub codec: String,
    pub version: String,
    pub roundtrip_verified: bool,
    pub tokens_before: usize,
    pub tokens_after: usize,
    pub token_delta: i64,
    pub warnings: Vec<Warning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PruningReport {
    pub requested: bool,
    pub applied: bool,
    pub preview: bool,
    pub candidate_items: usize,
    pub retained_items: usize,
    pub pruned_items: usize,
    pub evidence_refs: usize,
    pub preserve_paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QualityReport {
    pub eval_profile_id: String,
    pub task_scope: String,
    pub validated_ratio_band: Option<String>,
    /// `None` when a lossy transform ran but no fidelity-gate data was baked in at build time —
    /// the documented "early dev build, before any gate data exists" state. These were plain
    /// `f64` before, which forced that state to be reported as a fabricated `0.0` ("nothing was
    /// retained") — indistinguishable from a real, measured total-loss result. Absent data must
    /// read as absent, not as a measurement.
    #[serde(default)]
    pub quality_retention: Option<f64>,
    #[serde(default)]
    pub contrastive_failure_rate: Option<f64>,
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
    /// The transform is enabled for this preset/format, but a lossy run (`policy.lossy`) actually
    /// pruned the payload, and this transform restructures arrays in a way that would move a
    /// `lossy_preserve` path off the array it names. See `pipeline::apply_transforms`, which
    /// defers these until after the lossy stage and only skips them when pruning really applied.
    IncompatibleWithLossy,
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
    OutputEncodingIncreased,
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
        assert_eq!(report.schema_version, "2.0");
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
            Status::Compressed,
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
            Status::Compressed,
            "balanced".to_string(),
            "plain_text".to_string(),
            "general".to_string(),
            vec![],
            vec![],
        );
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["status"], "compressed");
        assert_eq!(json["estimator"]["backend"], "heuristic");
        assert_eq!(json["estimator"]["is_exact"], false);
    }

    #[test]
    fn quality_report_round_trips() {
        let quality = QualityReport {
            eval_profile_id: "smoke-first-consumer".to_string(),
            task_scope: "code_review".to_string(),
            validated_ratio_band: Some("0.6-0.8".to_string()),
            quality_retention: Some(0.975),
            contrastive_failure_rate: Some(0.0),
            gate_passed: true,
        };
        let json = serde_json::to_string(&quality).unwrap();
        let back: QualityReport = serde_json::from_str(&json).unwrap();
        assert_eq!(quality, back);
    }

    #[test]
    fn quality_report_without_baked_in_gate_data_round_trips_as_absent_not_zero() {
        let quality = QualityReport {
            eval_profile_id: "unvalidated".to_string(),
            task_scope: "all".to_string(),
            validated_ratio_band: None,
            quality_retention: None,
            contrastive_failure_rate: None,
            gate_passed: false,
        };
        let json = serde_json::to_value(&quality).unwrap();
        assert!(
            json["quality_retention"].is_null(),
            "absent must not serialize as 0.0"
        );
        let back: QualityReport = serde_json::from_value(json).unwrap();
        assert_eq!(quality, back);
    }

    #[test]
    fn canonical_v2_report_fixture_round_trips_without_schema_drift() {
        let expected: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/compression_report_v2.json"
        ))
        .unwrap();
        let report: CompressionReport = serde_json::from_value(expected.clone()).unwrap();
        assert_eq!(serde_json::to_value(report).unwrap(), expected);

        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/compression_report_v2.schema.json"
        ))
        .unwrap();
        let expected_keys = expected.as_object().unwrap().keys().collect::<Vec<_>>();
        let schema_keys = schema["properties"]
            .as_object()
            .unwrap()
            .keys()
            .collect::<Vec<_>>();
        assert_eq!(schema_keys, expected_keys);
        assert_eq!(
            schema["required"].as_array().unwrap().len(),
            expected_keys.len()
        );
    }

    // --- EP-02: the versioned reader, old receipts, and unavailable values --------
    //
    // These close the deliberately-open item: a reader must know which schema versions it
    // understands *before* it exposes numbers, and an unavailable value must never surface as a
    // plausible-looking zero.

    #[test]
    fn the_shipped_v1_receipt_fixture_is_still_readable() {
        // v1 predates several sections; every one of them is absent there, and absence must parse
        // as "unknown", not as a zero.
        let bytes = include_bytes!("../../../tests/fixtures/compression_report_v1.json");
        let report = CompressionReport::parse_versioned(bytes)
            .expect("the shipped v1 receipt must remain readable");

        assert_eq!(report.schema_version, "1.0");
        assert_eq!(report.original_tokens, 4);
        assert_eq!(report.savings_ratio, 0.0);
        assert!(
            report.estimator.model.is_none(),
            "v1 has no model; must not be invented"
        );
    }

    #[test]
    fn absent_v1_sections_parse_as_unknown_rather_than_as_zero() {
        // This is the property the item is really about: in a numeric-only schema, an absent
        // section must stay absent. A reader that defaulted these to 0 would report "we measured
        // zero" for something it never measured.
        let bytes = include_bytes!("../../../tests/fixtures/compression_report_v1.json");
        let report = CompressionReport::parse_versioned(bytes).unwrap();
        let json = serde_json::to_value(&report).unwrap();

        for key in [
            "request_id",
            "pipeline",
            "quality",
            "budget",
            "encoding",
            "pruning",
            "cache",
            "retrieval",
            "output_savings",
            "bypass",
            "command",
            "ledger",
        ] {
            assert!(
                json[key].is_null(),
                "absent section {key:?} was exposed as a value: {json:?}"
            );
        }
        assert_eq!(report.quality, None);
        assert_eq!(report.retrieval, None);
    }

    #[test]
    fn an_absent_nested_metric_does_not_become_a_number_after_a_round_trip() {
        // The numeric trap in its purest form: `Option<f64>` that serializes as `0.0` reads as a
        // real measurement of zero.
        let quality = QualityReport {
            eval_profile_id: "smoke".to_string(),
            task_scope: "all".to_string(),
            validated_ratio_band: None,
            quality_retention: None,
            contrastive_failure_rate: None,
            gate_passed: false,
        };
        let json = serde_json::to_value(&quality).unwrap();
        assert!(json["quality_retention"].is_null());

        let back: QualityReport = serde_json::from_value(json).unwrap();
        assert_eq!(back.quality_retention, None);
        assert!(serde_json::to_value(&back).unwrap()["quality_retention"].is_null());
    }

    #[test]
    fn a_v1_receipt_survives_read_then_write_without_gaining_or_losing_numbers() {
        // Reading an old receipt and writing it back must not silently upgrade it into a v2 claim.
        let bytes = include_bytes!("../../../tests/fixtures/compression_report_v1.json");
        let report = CompressionReport::parse_versioned(bytes).unwrap();
        let written = serde_json::to_vec(&report).unwrap();
        let reread = CompressionReport::parse_versioned(&written).unwrap();
        assert_eq!(reread, report, "a read/write cycle changed the receipt");
        assert_eq!(reread.schema_version, "1.0");
    }

    #[test]
    fn the_current_v2_receipt_fixture_is_readable_by_the_versioned_reader() {
        let bytes = include_bytes!("../../../tests/fixtures/compression_report_v2.json");
        let report = CompressionReport::parse_versioned(bytes).unwrap();
        assert_eq!(report.schema_version, CURRENT_SCHEMA_VERSION);
    }

    #[test]
    fn an_unknown_future_schema_version_is_refused_rather_than_best_effort_parsed() {
        // The whole point of versioning the reader. A v3 receipt may make numbers optional;
        // parsing it anyway would turn "absent" into a number this build cannot justify.
        let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
            "../../../tests/fixtures/compression_report_v2.json"
        ))
        .unwrap();
        value["schema_version"] = serde_json::json!("3.0");
        let bytes = serde_json::to_vec(&value).unwrap();

        assert_eq!(
            CompressionReport::parse_versioned(&bytes),
            Err(ReportParseError::UnsupportedVersion {
                got: "3.0".to_string()
            })
        );
    }

    #[test]
    fn a_receipt_with_no_schema_version_is_malformed_rather_than_assumed_current() {
        // Defaulting an absent version to "the one I write" would let an unrelated document be
        // read as a receipt.
        let value = serde_json::json!({"original_tokens": 1, "compressed_tokens": 1});
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(
            CompressionReport::parse_versioned(&bytes),
            Err(ReportParseError::Malformed)
        );
    }

    #[test]
    fn bytes_that_are_not_a_receipt_are_malformed() {
        assert_eq!(
            CompressionReport::parse_versioned(b"not json at all"),
            Err(ReportParseError::Malformed)
        );
    }

    #[test]
    fn every_supported_version_is_actually_supported_by_the_reader() {
        // Keeps the constant honest: a version listed as readable but not handled would be a
        // silent promise.
        for version in SUPPORTED_SCHEMA_VERSIONS {
            let mut value: serde_json::Value = serde_json::from_slice(include_bytes!(
                "../../../tests/fixtures/compression_report_v2.json"
            ))
            .unwrap();
            value["schema_version"] = serde_json::json!(version);
            let bytes = serde_json::to_vec(&value).unwrap();
            assert!(
                CompressionReport::parse_versioned(&bytes).is_ok(),
                "{version:?} is listed as supported but the reader rejected it"
            );
        }
    }
}

/// Signed measurements are separate from the legacy nonnegative savings receipt.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct OutputDelta {
    pub schema_version: String,
    pub baseline_tokens: usize,
    pub shaped_tokens: usize,
    pub delta_tokens: i64,
    pub estimator: EstimatorInfo,
}
