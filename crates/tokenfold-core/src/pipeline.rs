use crate::budget::{CompressionMode, CompressionPolicy, TaskScope, protected_segments};
use crate::errors::TokenFoldError;
use crate::input::{CompressionInput, CompressionOutput, InputFormat};
use crate::modes::{self, ModeEntry, TransformId};
use crate::report::{
    BudgetReport, CompressionReport, RetrievalReport, Severity, SkippedReason, TransformReport,
    TransformStatus, Warning, WarningCode,
};
use crate::retrieval_store::{self, RetrievalStore};
use crate::safety;
use crate::status::Status;
use crate::token_estimator::{ByteHeuristicEstimator, TokenEstimator};
use crate::transforms;

/// Compresses `input` under `policy` using the best available estimator (exact `tiktoken`
/// when the feature is compiled in and its data is reachable, heuristic otherwise).
pub fn compress(
    input: CompressionInput,
    policy: &CompressionPolicy,
) -> Result<CompressionOutput, TokenFoldError> {
    #[cfg(feature = "tiktoken")]
    {
        if let Ok(estimator) = crate::token_estimator::TiktokenEstimator::o200k_base() {
            return compress_with_estimator(input, policy, &estimator);
        }
    }
    compress_with_estimator(input, policy, &ByteHeuristicEstimator)
}

pub fn compress_with_estimator(
    input: CompressionInput,
    policy: &CompressionPolicy,
    estimator: &dyn TokenEstimator,
) -> Result<CompressionOutput, TokenFoldError> {
    let original_tokens = estimator.count_bytes(&input.bytes);
    let target = policy.target_tokens;
    let estimator_info = estimator.info();

    // F-045: whole-payload evidence store, best-effort. Runs against the full pre-transform
    // input regardless of which status path below is taken, so it must be computed up front.
    let retrieval = maybe_store_originals(&input.bytes, policy);

    // Passthrough is checked before any transform (including redaction) runs: F-001 requires
    // input bytes to stay byte-for-byte unchanged in this case.
    if let Some(t) = target
        && original_tokens <= t
    {
        let mut warnings = Vec::new();
        if !estimator_info.is_exact {
            warnings.push(heuristic_budget_warning());
        }
        let mut report = CompressionReport::new(
            original_tokens,
            original_tokens,
            estimator_info,
            Status::Passthrough,
            mode_label(policy.mode).to_string(),
            format_label(input.format).to_string(),
            task_scope_label(policy.task_scope).to_string(),
            Vec::new(),
            warnings,
        );
        report.retrieval = retrieval;
        return Ok(CompressionOutput {
            bytes: input.bytes,
            report,
        });
    }

    apply_transforms(input, policy, estimator, original_tokens, target, retrieval)
}

/// F-045: when `policy.store_originals` is set, persists the full pre-transform input to the
/// configured reversible evidence store (`policy.retrieval_backend`/`retrieval_store_path`)
/// under its SHA-256 hash, unless it contains secret-shaped content (`RetrievalStore::store`'s
/// own unconditional gate — never bypassable from here). Best-effort: any storage failure
/// (an unopenable store, e.g. the documented `backend = "sqlite"` scope cut, or the secret
/// gate) is reported as `skipped_original_bytes`, never as a compression error.
fn maybe_store_originals(
    input_bytes: &[u8],
    policy: &CompressionPolicy,
) -> Option<RetrievalReport> {
    if !policy.store_originals {
        return None;
    }
    let ttl_seconds = policy
        .retrieval_ttl_seconds
        .unwrap_or(retrieval_store::DEFAULT_TTL_SECONDS);
    let skipped = || RetrievalReport {
        store_namespace: policy.retrieval_namespace.clone(),
        hash_algorithm: "sha256".to_string(),
        marker_count: 0,
        ttl_seconds: None,
        persisted_original_bytes: 0,
        skipped_original_bytes: input_bytes.len(),
    };
    let Ok(store) = RetrievalStore::open(
        &policy.retrieval_backend,
        "sha256",
        policy.retrieval_store_path.clone(),
    ) else {
        return Some(skipped());
    };
    Some(
        match store.store(input_bytes, &policy.retrieval_namespace, Some(ttl_seconds)) {
            Ok(_marker) => RetrievalReport {
                store_namespace: policy.retrieval_namespace.clone(),
                hash_algorithm: "sha256".to_string(),
                marker_count: 1,
                ttl_seconds: Some(ttl_seconds),
                persisted_original_bytes: input_bytes.len(),
                skipped_original_bytes: 0,
            },
            Err(_) => skipped(),
        },
    )
}

fn apply_transforms(
    input: CompressionInput,
    policy: &CompressionPolicy,
    estimator: &dyn TokenEstimator,
    original_tokens: usize,
    target: Option<usize>,
    retrieval: Option<RetrievalReport>,
) -> Result<CompressionOutput, TokenFoldError> {
    let estimator_info = estimator.info();
    let mut warnings = Vec::new();
    let mut transform_reports = Vec::new();
    if !estimator_info.is_exact {
        warnings.push(heuristic_budget_warning());
    }

    // Step 1: secret_redaction — mandatory, always first, cannot be disabled via `disabled`
    // (CompressionPolicyBuilder::build rejects that). The only bypass is the CLI-only
    // `unsafe_disable_redaction` escape hatch, which emits a Critical warning instead.
    let mut bytes;
    if policy.unsafe_disable_redaction {
        bytes = input.bytes.clone();
        warnings.push(Warning {
            code: WarningCode::UnredactedContentPossible,
            severity: Severity::Critical,
            transform: Some("secret_redaction".to_string()),
            message: "redaction was disabled via unsafe_disable_redaction; output may contain unredacted secrets".to_string(),
        });
        transform_reports.push(skipped_at(
            "secret_redaction",
            "1.0.0",
            original_tokens,
            SkippedReason::DisabledByUser,
        ));
    } else {
        let outcome = transforms::redaction::redact(&input.bytes);
        let tokens_after = estimator.count_bytes(&outcome.bytes);
        warnings.push(Warning {
            code: WarningCode::UnredactedContentPossible,
            severity: Severity::Info,
            transform: Some("secret_redaction".to_string()),
            message: "redaction is best-effort; it is not a guarantee that no secret survives"
                .to_string(),
        });
        transform_reports.push(TransformReport {
            id: "secret_redaction".to_string(),
            version: "1.0.0".to_string(),
            tokens_before: original_tokens,
            tokens_after,
            saved_tokens: original_tokens.saturating_sub(tokens_after),
            savings_ratio: ratio(original_tokens, tokens_after),
            elapsed_micros: None,
            status: if outcome.redacted_count > 0 {
                TransformStatus::Applied
            } else {
                TransformStatus::NoOp
            },
            skipped_reason: None,
            warnings: Vec::new(),
        });
        bytes = outcome.bytes;
    }

    // Protected content is computed against the POST-redaction view: redaction may
    // legitimately alter protected content that itself contained a secret, so later
    // transforms are held to "survives redaction", not "survives the original bytes".
    let working_input = CompressionInput {
        format: input.format,
        bytes: bytes.clone(),
    };
    let protected = protected_segments(&working_input, policy);
    let floor = estimator.count_bytes(&protected.concat());
    let mut current_tokens = estimator.count_bytes(&bytes);

    if let Some(t) = target
        && t < floor
    {
        warnings.push(Warning {
            code: WarningCode::UnreachableTarget,
            severity: Severity::Warn,
            transform: None,
            message: format!("target {t} tokens is below the protected floor of {floor} tokens"),
        });
        let mut report = CompressionReport::new(
            original_tokens,
            current_tokens,
            estimator_info,
            Status::UnreachableTarget,
            mode_label(policy.mode).to_string(),
            format_label(input.format).to_string(),
            task_scope_label(policy.task_scope).to_string(),
            transform_reports,
            warnings,
        );
        report.budget = Some(BudgetReport {
            target_tokens: target,
            protected_floor: floor,
            achieved_tokens: current_tokens,
        });
        report.retrieval = retrieval;
        return Ok(CompressionOutput { bytes, report });
    }

    // Step 2: mode-matrix-selected transforms, in canonical order, stopping early once the
    // target is met (INTERFACES.md Part 2 "Early Exit").
    let entries = modes::pipeline_for(
        policy.mode,
        policy.task_scope,
        input.format,
        policy.experimental,
        &policy.enable,
        &policy.disabled,
    );
    for entry in entries {
        if let Some(t) = target
            && current_tokens <= t
        {
            transform_reports.push(skipped(
                entry,
                current_tokens,
                SkippedReason::TargetAlreadyMet,
            ));
            continue;
        }

        let tokens_before = current_tokens;
        let before_bytes = bytes.clone();
        let max_ratio = entry.max_ratio_for(policy.mode);

        let candidate = match apply_single_transform(entry.transform_id, &bytes, policy) {
            Ok(candidate) => candidate,
            Err(_) => {
                transform_reports.push(skipped(
                    entry,
                    tokens_before,
                    SkippedReason::NotApplicableToFormat,
                ));
                continue;
            }
        };

        let tokens_after_candidate = estimator.count_bytes(&candidate);
        if tokens_after_candidate > tokens_before {
            // A genuine regression: never adopt a transform that costs more tokens than it saves.
            transform_reports.push(skipped(
                entry,
                tokens_before,
                SkippedReason::WouldIncreaseTokens,
            ));
            continue;
        }
        if tokens_after_candidate == tokens_before {
            // The transform ran (unlike the cases above/below, which never call it) but had no
            // measurable effect — that's NoOp, not Skipped, per the TransformStatus contract.
            transform_reports.push(TransformReport {
                id: entry.transform_id.as_str().to_string(),
                version: entry.version.to_string(),
                tokens_before,
                tokens_after: tokens_before,
                saved_tokens: 0,
                savings_ratio: 0.0,
                elapsed_micros: None,
                status: TransformStatus::NoOp,
                skipped_reason: None,
                warnings: Vec::new(),
            });
            continue;
        }
        let ratio_used = 1.0 - (tokens_after_candidate as f64 / tokens_before.max(1) as f64);
        if ratio_used > max_ratio {
            transform_reports.push(skipped(
                entry,
                tokens_before,
                SkippedReason::NotEnabledInMode,
            ));
            continue;
        }

        if !validate_safety(
            entry.transform_id,
            input.format,
            &before_bytes,
            &candidate,
            &protected,
        ) {
            transform_reports.push(rolled_back(entry, tokens_before));
            warnings.push(safety_downgrade_warning(entry.transform_id.as_str()));
            continue;
        }

        bytes = candidate;
        current_tokens = tokens_after_candidate;
        transform_reports.push(TransformReport {
            id: entry.transform_id.as_str().to_string(),
            version: entry.version.to_string(),
            tokens_before,
            tokens_after: current_tokens,
            saved_tokens: tokens_before.saturating_sub(current_tokens),
            savings_ratio: ratio(tokens_before, current_tokens),
            elapsed_micros: None,
            status: TransformStatus::Applied,
            skipped_reason: None,
            warnings: Vec::new(),
        });
    }

    let status = match target {
        None => Status::BestEffort,
        Some(t) if current_tokens <= t => Status::Compressed,
        Some(_) => Status::BestEffort,
    };

    let mut report = CompressionReport::new(
        original_tokens,
        current_tokens,
        estimator_info,
        status,
        mode_label(policy.mode).to_string(),
        format_label(input.format).to_string(),
        task_scope_label(policy.task_scope).to_string(),
        transform_reports,
        warnings,
    );
    report.budget = Some(BudgetReport {
        target_tokens: target,
        protected_floor: floor,
        achieved_tokens: current_tokens,
    });
    report.retrieval = retrieval;
    Ok(CompressionOutput { bytes, report })
}

fn apply_single_transform(
    transform_id: TransformId,
    bytes: &[u8],
    policy: &CompressionPolicy,
) -> Result<Vec<u8>, String> {
    match transform_id {
        TransformId::JsonMinify => transforms::json::minify_json(bytes).map_err(|e| e.to_string()),
        TransformId::JsonFieldFold => {
            transforms::json_fold::fold_json(bytes).map_err(|e| e.to_string())
        }
        TransformId::SchemaCompaction => {
            // ponytail: a fixed example cap for now; per-mode example counts are a future
            // config knob (F-011 acceptance criteria only requires the count be configurable,
            // not that Phase 2 ship distinct values per mode).
            transforms::schema::compact_schema(bytes, 1).map_err(|e| e.to_string())
        }
        TransformId::LogCompaction => {
            let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            Ok(transforms::logs::compact(text, false).into_bytes())
        }
        TransformId::DiffCompaction => {
            let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            let keep_line_bodies = policy.task_scope != TaskScope::ChangeSummary;
            Ok(transforms::diff::compact_diff(text, keep_line_bodies).into_bytes())
        }
    }
}

fn validate_safety(
    transform_id: TransformId,
    format: InputFormat,
    before: &[u8],
    after: &[u8],
    protected: &[Vec<u8>],
) -> bool {
    match transform_id {
        // json_field_fold intentionally restructures JSON (arrays of objects -> columnar
        // form), so key-order preservation does NOT apply. Its safety invariant is instead
        // exact reversibility: unfolding the output must reproduce the input's data.
        TransformId::JsonFieldFold => {
            if !safety::json_still_valid(after) {
                return false;
            }
            if !transforms::json_fold::round_trips(before, after) {
                return false;
            }
        }
        // json_minify / schema_compaction on any JSON-family format: output must stay valid
        // JSON with byte-for-byte key order preserved.
        TransformId::JsonMinify | TransformId::SchemaCompaction => {
            let is_json_format = matches!(
                format,
                InputFormat::OpenAiJson | InputFormat::AnthropicJson | InputFormat::Json
            );
            if is_json_format {
                if !safety::json_still_valid(after) {
                    return false;
                }
                if !safety::json_key_order_preserved(before, after) {
                    return false;
                }
            }
        }
        TransformId::LogCompaction | TransformId::DiffCompaction => {}
    }
    safety::protected_segments_present(protected, after)
}

fn skipped(entry: &ModeEntry, tokens: usize, reason: SkippedReason) -> TransformReport {
    skipped_at(entry.transform_id.as_str(), entry.version, tokens, reason)
}

fn skipped_at(id: &str, version: &str, tokens: usize, reason: SkippedReason) -> TransformReport {
    TransformReport {
        id: id.to_string(),
        version: version.to_string(),
        tokens_before: tokens,
        tokens_after: tokens,
        saved_tokens: 0,
        savings_ratio: 0.0,
        elapsed_micros: None,
        status: TransformStatus::Skipped,
        skipped_reason: Some(reason),
        warnings: Vec::new(),
    }
}

fn rolled_back(entry: &ModeEntry, tokens: usize) -> TransformReport {
    TransformReport {
        id: entry.transform_id.as_str().to_string(),
        version: entry.version.to_string(),
        tokens_before: tokens,
        tokens_after: tokens,
        saved_tokens: 0,
        savings_ratio: 0.0,
        elapsed_micros: None,
        status: TransformStatus::RolledBack,
        skipped_reason: None,
        warnings: Vec::new(),
    }
}

fn safety_downgrade_warning(transform_id: &str) -> Warning {
    Warning {
        code: WarningCode::SafetyDowngrade,
        severity: Severity::Warn,
        transform: Some(transform_id.to_string()),
        message: format!(
            "{transform_id} was rolled back: a safety invariant would have been violated"
        ),
    }
}

fn heuristic_budget_warning() -> Warning {
    Warning {
        code: WarningCode::HeuristicBudgetUsed,
        severity: Severity::Info,
        transform: None,
        message: "token counts are heuristic estimates (~bytes/4), not exact".to_string(),
    }
}

fn ratio(before: usize, after: usize) -> f64 {
    if before == 0 {
        0.0
    } else {
        before.saturating_sub(after) as f64 / before as f64
    }
}

fn mode_label(mode: CompressionMode) -> &'static str {
    match mode {
        CompressionMode::Conservative => "conservative",
        CompressionMode::Balanced => "balanced",
        CompressionMode::Aggressive => "aggressive",
    }
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

fn task_scope_label(scope: TaskScope) -> &'static str {
    match scope {
        TaskScope::All => "all",
        TaskScope::General => "general",
        TaskScope::CodeReview => "code_review",
        TaskScope::ChangeSummary => "change_summary",
        TaskScope::Debugging => "debugging",
        TaskScope::Generation => "generation",
        TaskScope::ApiOverview => "api_overview",
        TaskScope::RetrievalQa => "retrieval_qa",
        TaskScope::AgentHistory => "agent_history",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::budget::CompressionPolicy;

    struct MockEstimator(usize);

    impl TokenEstimator for MockEstimator {
        fn info(&self) -> crate::report::EstimatorInfo {
            crate::report::EstimatorInfo {
                backend: "mock".to_string(),
                model: Some("mock-1".to_string()),
                is_exact: true,
            }
        }

        fn count_bytes(&self, _bytes: &[u8]) -> usize {
            self.0
        }
    }

    #[test]
    fn compress_with_estimator_accepts_a_mock_backend() {
        let input = CompressionInput::plain_text(b"hello".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress_with_estimator(input, &policy, &MockEstimator(42)).unwrap();
        assert_eq!(output.report.original_tokens, 42);
        assert_eq!(output.report.estimator.backend, "mock");
    }

    #[test]
    fn passthrough_when_input_is_already_under_target() {
        let input = CompressionInput::plain_text(b"hi".to_vec());
        let policy = CompressionPolicy::builder()
            .target_tokens(1_000)
            .build()
            .unwrap();
        let output = compress_with_estimator(input.clone(), &policy, &MockEstimator(5)).unwrap();
        assert_eq!(output.report.status, Status::Passthrough);
        assert_eq!(output.bytes, input.bytes);
    }

    #[test]
    fn unreachable_target_returns_best_effort_bytes_and_never_panics() {
        let payload = serde_json::json!({
            "messages": [{"role": "system", "content": "a fairly long system prompt here"}]
        });
        let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
        let policy = CompressionPolicy::builder()
            .target_tokens(1)
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        assert_eq!(output.report.status, Status::UnreachableTarget);
        assert!(!output.bytes.is_empty());
        let budget = output.report.budget.expect("budget report populated");
        assert_eq!(budget.target_tokens, Some(1));
        assert!(budget.protected_floor > 1);
        assert_eq!(budget.achieved_tokens, output.report.compressed_tokens);
    }

    #[test]
    fn no_target_set_runs_pipeline_and_reports_estimator_provenance() {
        let input = CompressionInput::plain_text(b"no target here".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert_eq!(output.report.estimator.backend, "heuristic");
        assert_eq!(output.report.status, Status::BestEffort);
    }

    #[test]
    fn public_compress_seam_never_panics_on_empty_input() {
        let input = CompressionInput::plain_text(Vec::new());
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress(input, &policy).unwrap();
        assert_eq!(output.report.original_tokens, 0);
    }

    #[test]
    fn json_minify_actually_applies_and_reduces_tokens_for_openai_json() {
        let payload =
            b"{\n  \"messages\": [\n    {\"role\": \"user\", \"content\": \"hi\"}\n  ]\n}".to_vec();
        let input = CompressionInput::openai_json(payload);
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        let applied = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_minify")
            .expect("json_minify report present");
        assert_eq!(applied.status, TransformStatus::Applied);
        assert!(applied.saved_tokens > 0 || applied.tokens_after <= applied.tokens_before);
        assert!(serde_json::from_slice::<serde_json::Value>(&output.bytes).is_ok());
    }

    #[test]
    fn secret_redaction_warning_always_present_when_redaction_runs() {
        let input = CompressionInput::plain_text(b"nothing secret here".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert!(
            output
                .report
                .warnings
                .iter()
                .any(|w| w.code == WarningCode::UnredactedContentPossible)
        );
    }

    #[test]
    fn secret_redaction_removes_a_fake_bearer_token_before_any_other_transform() {
        let input = CompressionInput::plain_text(
            b"Authorization: Bearer sk-abcdEFGH1234567890123456\nother text".to_vec(),
        );
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert!(!contains(&output.bytes, b"sk-abcdEFGH1234567890123456"));
    }

    #[test]
    fn log_compaction_applies_by_default_after_promotion() {
        // log_compaction was promoted out of --experimental (roadmap.md Phase 5 Task 9,
        // 2026-07-12): it now applies under the default Balanced mode with no --experimental
        // flag needed, unlike diff_compaction below (which stays gated). Ten adjacent repeats
        // of a realistic log line (not a two-byte "a") so the collapsed evidence marker is a
        // genuine net token saving, not swamped by its own overhead.
        let mut text = String::from("Starting server on port 8080\n");
        for _ in 0..10 {
            text.push_str("Connecting to database...\n");
        }
        text.push_str("Database connection established");
        let input = CompressionInput::command_output(text.into_bytes());
        let policy = CompressionPolicy::builder()
            .task_scope(TaskScope::General)
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert!(
            output
                .report
                .transforms
                .iter()
                .any(|t| t.id == "log_compaction" && t.status == TransformStatus::Applied)
        );
    }

    #[test]
    fn diff_compaction_never_applies_without_experimental_flag() {
        let input = CompressionInput::plain_text(
            b"diff --git a/f.rs b/f.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n context\n context\n context"
                .to_vec(),
        );
        let policy = CompressionPolicy::builder()
            .task_scope(TaskScope::CodeReview)
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert!(
            !output
                .report
                .transforms
                .iter()
                .any(|t| t.id == "diff_compaction" && t.status == TransformStatus::Applied)
        );
    }

    fn contains(haystack: &[u8], needle: &[u8]) -> bool {
        haystack.windows(needle.len()).any(|w| w == needle)
    }

    // `XDG_DATA_HOME` is process-global; serialize the store_originals tests below so parallel
    // `cargo test` threads don't race each other's overrides (same pattern as
    // `tokenfold-cli::config`'s `ENV_LOCK`).
    static RETRIEVAL_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_retrieval_env() -> std::sync::MutexGuard<'static, ()> {
        RETRIEVAL_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn store_originals_false_leaves_retrieval_report_absent() {
        let input = CompressionInput::plain_text(b"anything".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert!(output.report.retrieval.is_none());
    }

    #[test]
    fn store_originals_persists_full_payload_and_populates_retrieval_report() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_store_originals_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        let input = CompressionInput::plain_text(b"nothing secret in here at all".to_vec());
        let policy = CompressionPolicy::builder()
            .store_originals(true)
            .retrieval_namespace("pipeline-test")
            .build()
            .unwrap();
        let output =
            compress_with_estimator(input.clone(), &policy, &ByteHeuristicEstimator).unwrap();

        let retrieval = output.report.retrieval.expect("retrieval report populated");
        assert_eq!(retrieval.marker_count, 1);
        assert_eq!(retrieval.persisted_original_bytes, input.bytes.len());
        assert_eq!(retrieval.skipped_original_bytes, 0);
        assert_eq!(retrieval.store_namespace, "pipeline-test");
        assert_eq!(retrieval.hash_algorithm, "sha256");
        assert_eq!(
            retrieval.ttl_seconds,
            Some(crate::retrieval_store::DEFAULT_TTL_SECONDS)
        );

        let hash = crate::retrieval_store::hex_sha256(&input.bytes);
        let store = crate::retrieval_store::RetrievalStore::default_filesystem();
        assert_eq!(
            store.retrieve(&hash, "pipeline-test"),
            crate::retrieval_store::RetrievalOutcome::Found(input.bytes.clone())
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn store_originals_skips_secret_bearing_payloads_without_erroring_the_compression() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_store_originals_secret_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        let input =
            CompressionInput::plain_text(b"AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE".to_vec());
        let policy = CompressionPolicy::builder()
            .store_originals(true)
            .retrieval_namespace("pipeline-test")
            .build()
            .unwrap();
        let output =
            compress_with_estimator(input.clone(), &policy, &ByteHeuristicEstimator).unwrap();

        let retrieval = output.report.retrieval.expect("retrieval report populated");
        assert_eq!(retrieval.marker_count, 0);
        assert_eq!(retrieval.persisted_original_bytes, 0);
        assert_eq!(retrieval.skipped_original_bytes, input.bytes.len());

        let hash = crate::retrieval_store::hex_sha256(&input.bytes);
        let store = crate::retrieval_store::RetrievalStore::default_filesystem();
        assert_eq!(
            store.retrieve(&hash, "pipeline-test"),
            crate::retrieval_store::RetrievalOutcome::Missing
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
