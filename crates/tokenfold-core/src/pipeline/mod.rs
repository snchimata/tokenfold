use std::collections::HashMap;
#[cfg(feature = "tiktoken")]
use std::sync::OnceLock;

use serde_json::Value;

use crate::budget::{CompressionMode, CompressionPolicy, TaskScope, protected_segments};
use crate::errors::TokenFoldError;
use crate::input::{CompressionInput, CompressionOutput, InputFormat};
use crate::modes::{self, ModeEntry, TransformId};
use crate::report::{
    BudgetReport, CompressionReport, QualityReport, RetrievalReport, Severity, SkippedReason,
    TransformReport, TransformStatus, Warning, WarningCode,
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
        static ESTIMATOR: OnceLock<Option<crate::token_estimator::TiktokenEstimator>> =
            OnceLock::new();
        if let Some(estimator) =
            ESTIMATOR.get_or_init(|| crate::token_estimator::TiktokenEstimator::o200k_base().ok())
        {
            return compress_with_estimator(input, policy, estimator);
        }
    }
    compress_with_estimator(input, policy, &ByteHeuristicEstimator)
}

pub fn compress_with_estimator(
    input: CompressionInput,
    policy: &CompressionPolicy,
    estimator: &dyn TokenEstimator,
) -> Result<CompressionOutput, TokenFoldError> {
    // Every `CompressionPolicy` field is `pub`, so a caller can build or mutate one without
    // ever going through `CompressionPolicyBuilder::build` -- re-validate here so a hand-built
    // policy can't silently bypass the same invariants (e.g. lossy-without-durable-retrieval)
    // the builder enforces for free.
    policy.validate()?;
    let original_tokens = estimator.count_bytes(&input.bytes);
    let target = policy.target_tokens;
    let estimator_info = estimator.info();

    // Whole-payload reversible evidence store, best-effort. Runs against the full pre-transform
    // input regardless of which status path below is taken, so it must be computed up front.
    let retrieval = maybe_store_originals(&input.bytes, input.format, policy);

    // Passthrough is checked before any transform (including redaction) runs: the budget planner
    // contract requires input bytes to stay byte-for-byte unchanged when the input already fits.
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

/// When `policy.store_originals` is set, persists the full pre-transform input to the
/// configured reversible evidence store (`policy.retrieval_backend`/`retrieval_store_path`)
/// under its SHA-256 hash, unless it contains secret-shaped content (`RetrievalStore::store`'s
/// own unconditional gate — never bypassable from here). Best-effort: any storage failure
/// (an unopenable store, e.g. the documented `backend = "sqlite"` scope cut, or the secret
/// gate) is reported as `skipped_original_bytes`, never as a compression error.
fn maybe_store_originals(
    input_bytes: &[u8],
    format: InputFormat,
    policy: &CompressionPolicy,
) -> Option<RetrievalReport> {
    // A preview (`tokenfold inspect` / `compress --dry-run`) must never write anything real to
    // disk, no matter what store_originals/lossy say -- report nothing rather than either a
    // fake success or a misleading "skipped" (which would otherwise read as "we tried and
    // couldn't", not "we deliberately didn't try").
    if policy.preview {
        return None;
    }
    // Design doc §4: a lossy run always gets a durable receipt for the pre-transform input too,
    // regardless of whether the user separately asked for `--store-originals`. That receipt is
    // owed only where the lossy stage can actually run, though: `apply_lossy_reduction` skips
    // every format but generic `Json` (`NotApplicableToFormat`), so on an OpenAI/Anthropic
    // payload -- or an unresolved `Auto`, which core never sniffs -- `--lossy` used to persist
    // the whole unmodified input to disk (a live repro: `json_prune: skipped`, yet
    // `persisted_original_bytes: 22586` and a freshly created store directory) in exchange for
    // nothing. Writing a user's full payload to durable storage is exactly the side effect that
    // must not happen as an accident of an inapplicable flag; `--store-originals` still requests
    // it independently, on any format.
    let lossy_can_run = policy.lossy.is_some() && format == InputFormat::Json;
    if !policy.store_originals && !lossy_can_run {
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
    // target is met (the pipeline's "early exit" rule).
    //
    // When `--lossy` is set, `json_field_fold`/`json_value_dict` are DEFERRED past the lossy
    // stage rather than run in place -- both restructure an eligible array (columnar folding /
    // value dictionary references), which can silently move a user's `--lossy-preserve` path to
    // a different array path than the one they named (a real, reproduced gap: preserving
    // `"items"` did nothing once `json_field_fold` had already turned it into per-field
    // sub-arrays), and can just as easily confuse json_prune's own per-item structural scoring on
    // the restructured shape.
    //
    // Deferred, NOT disabled: the conflict only exists when pruning actually happens. A round-5
    // external review measured what unconditional disabling cost -- a `--lossy-ratio 0.25` run
    // over a fixture where pruning turned out to be a no-op emitted 1,834 bytes against plain
    // lossless's 644, ~3x WORSE while dropping nothing at all, because the two transforms that
    // would have done the real work had been switched off for a stage that never ran. So they are
    // held back here and replayed below through the identical gates whenever the lossy stage ends
    // up NoOp/Skipped/RolledBack.
    let defer_for_lossy = |entry: &ModeEntry| {
        policy.lossy.is_some()
            && matches!(
                entry.transform_id,
                TransformId::JsonFieldFold | TransformId::JsonValueDict
            )
    };
    let entries = modes::pipeline_for(
        policy.mode,
        policy.task_scope,
        input.format,
        policy.experimental,
        &policy.enable,
        &policy.disabled,
    );
    let mut deferred: Vec<&ModeEntry> = Vec::new();
    for entry in entries {
        if defer_for_lossy(entry) {
            deferred.push(entry);
            continue;
        }
        run_transform_entry(
            entry,
            policy,
            input.format,
            estimator,
            &protected,
            target,
            &mut bytes,
            &mut current_tokens,
            &mut transform_reports,
            &mut warnings,
        );
    }

    // Terminal, opt-in lossy stage — strictly after the lossless loop above and its own safety
    // gates, never gated by `modes.rs`/`ALL_ENTRIES`. `retrieval` may already hold the
    // whole-payload evidence-store report computed up front; lossy's own per-item stores merge
    // into it rather than replacing it.
    //
    // Both branches are computed and the better one adopted. Merely deferring the two array-
    // restructuring transforms past the lossy stage (rather than disabling them outright) fixes
    // only the case where pruning turns out to be a no-op; it does NOT cover a prune that
    // succeeds and is still beaten by folding. That case is real and was measured: 30 identical
    // large rows fold/dictionary down to 1,186 bytes, while a successful `--lossy-ratio 0.25`
    // prune of the same document emits 7,589 — 6.4x worse, with four items genuinely dropped.
    // Accepting data loss is only ever justified by an output the lossless pipeline could not
    // produce, so the lossless branch is what the lossy branch has to beat.
    let mut retrieval = retrieval;
    let mut lossy_applied = false;
    if policy.lossy.is_some() {
        // The lossless branch, computed on a clone: exactly what a run without `--lossy` would
        // have emitted from this point on.
        let mut lossless_bytes = bytes.clone();
        let mut lossless_tokens = current_tokens;
        let mut lossless_reports = Vec::new();
        let mut lossless_warnings = Vec::new();
        for entry in &deferred {
            run_transform_entry(
                entry,
                policy,
                input.format,
                estimator,
                &protected,
                target,
                &mut lossless_bytes,
                &mut lossless_tokens,
                &mut lossless_reports,
                &mut lossless_warnings,
            );
        }

        // The "early exit" rule applies to the lossy stage too, and it is where the
        // rule matters most: every other transform is checked against the target before it runs
        // (`run_transform_entry`), but this one used to run unconditionally, so a target the
        // LOSSLESS pipeline could already hit still cost the caller real data. Measured: with
        // `--target-tokens 1462`, which `json_minify` alone reaches, `json_prune` ran anyway and
        // replaced 17 items with markers. The bar is the lossless branch's own result, not
        // `current_tokens` — if compression without data loss meets the target, data loss is never
        // warranted, whichever deferred transform got it there.
        let target_met_losslessly = target.is_some_and(|t| lossless_tokens <= t);
        let lossy_report = if target_met_losslessly {
            skipped_at(
                transforms::json_prune::TRANSFORM_ID,
                transforms::json_prune::TRANSFORM_VERSION,
                current_tokens,
                SkippedReason::TargetAlreadyMet,
            )
        } else {
            let (new_bytes, new_tokens, lossy_report) = apply_lossy_reduction(
                &bytes,
                current_tokens,
                lossless_tokens,
                policy,
                input.format,
                estimator,
                &protected,
                &mut retrieval,
            )?;
            lossy_applied = lossy_report.status == TransformStatus::Applied;
            if lossy_applied {
                bytes = new_bytes;
                current_tokens = new_tokens;
            }
            lossy_report
        };
        transform_reports.push(lossy_report);

        if lossy_applied {
            // The deferred transforms stay off: their restructuring would rewrite an array that
            // now carries `$tf_ref` markers, and the preserve paths the user named still describe
            // the pre-fold shape. Reported explicitly rather than vanishing from the report, so
            // "why didn't json_field_fold run?" has an answer.
            for entry in deferred {
                transform_reports.push(skipped(
                    entry,
                    current_tokens,
                    SkippedReason::IncompatibleWithLossy,
                ));
            }
        } else {
            // Pruning was skipped, was a no-op, was rolled back, or simply lost to folding: take
            // the lossless branch wholesale, reports and all, so `--lossy` is never worse than
            // omitting it.
            bytes = lossless_bytes;
            current_tokens = lossless_tokens;
            transform_reports.extend(lossless_reports);
            warnings.extend(lossless_warnings);
        }
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
    // The `quality` presence rule: `None` iff no lossy transform ran, `Some` with a
    // `validated_ratio_band: None` / metrics-absent body when one did but no fidelity-gate data
    // was baked in at build time. Phase 1 is exactly that second case — there is no baked gate
    // for `json_prune` yet — so the honest report is "a lossy transform ran, and nothing here has
    // been validated", never a fabricated retention number and never a silent `None` that makes a
    // pruned payload indistinguishable from a lossless one.
    if lossy_applied {
        report.quality = Some(QualityReport {
            eval_profile_id: "unvalidated".to_string(),
            task_scope: task_scope_label(policy.task_scope).to_string(),
            validated_ratio_band: None,
            quality_retention: None,
            contrastive_failure_rate: None,
            gate_passed: false,
        });
    }
    report.retrieval = retrieval;
    Ok(CompressionOutput { bytes, report })
}

/// One iteration of the mode-matrix transform loop: budget early-exit, run, regression check,
/// mode ratio cap, safety validation, and the matching `TransformReport`. Extracted so the
/// deferred lossy-safe entries (see `apply_transforms`) replay through the exact same gates
/// instead of a second copy that could drift from this one.
#[allow(clippy::too_many_arguments)]
fn run_transform_entry(
    entry: &ModeEntry,
    policy: &CompressionPolicy,
    format: InputFormat,
    estimator: &dyn TokenEstimator,
    protected: &[Vec<u8>],
    target: Option<usize>,
    bytes: &mut Vec<u8>,
    current_tokens: &mut usize,
    transform_reports: &mut Vec<TransformReport>,
    warnings: &mut Vec<Warning>,
) {
    if let Some(t) = target
        && *current_tokens <= t
    {
        transform_reports.push(skipped(
            entry,
            *current_tokens,
            SkippedReason::TargetAlreadyMet,
        ));
        return;
    }

    let tokens_before = *current_tokens;
    let max_ratio = entry.max_ratio_for(policy.mode);

    let candidate = match apply_single_transform(entry.transform_id, bytes, policy) {
        Ok(candidate) => candidate,
        Err(_) => {
            transform_reports.push(skipped(
                entry,
                tokens_before,
                SkippedReason::NotApplicableToFormat,
            ));
            return;
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
        return;
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
        return;
    }
    let ratio_used = 1.0 - (tokens_after_candidate as f64 / tokens_before.max(1) as f64);
    if ratio_used > max_ratio {
        transform_reports.push(skipped(
            entry,
            tokens_before,
            SkippedReason::NotEnabledInMode,
        ));
        return;
    }

    if !validate_safety(entry.transform_id, format, bytes, &candidate, protected) {
        transform_reports.push(rolled_back(entry, tokens_before));
        warnings.push(safety_downgrade_warning(entry.transform_id.as_str()));
        return;
    }

    *bytes = candidate;
    *current_tokens = tokens_after_candidate;
    transform_reports.push(TransformReport {
        id: entry.transform_id.as_str().to_string(),
        version: entry.version.to_string(),
        tokens_before,
        tokens_after: *current_tokens,
        saved_tokens: tokens_before.saturating_sub(*current_tokens),
        savings_ratio: ratio(tokens_before, *current_tokens),
        elapsed_micros: None,
        status: TransformStatus::Applied,
        skipped_reason: None,
        warnings: Vec::new(),
    });
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
        TransformId::JsonValueDict => {
            transforms::json_dict::dict_json(bytes).map_err(|e| e.to_string())
        }
        TransformId::SchemaCompaction => {
            // ponytail: a fixed example cap for now; per-mode example counts are a future
            // config knob (schema compaction only requires the kept-example count be
            // controlled by mode config, not that distinct values ship per mode yet).
            transforms::schema::compact_schema(bytes, 1).map_err(|e| e.to_string())
        }
        TransformId::LogFieldFold => {
            let text = std::str::from_utf8(bytes).map_err(|e| e.to_string())?;
            Ok(transforms::log_fold::fold_log(text).into_bytes())
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

/// Design doc §4: the terminal, opt-in lossy stage. Fail-closed — an item `transforms::json_prune`
/// proposed dropping is only actually removed if `RetrievalStore::store` returns `Ok` for it; a
/// store failure (including the secret-shaped-content refusal every call goes through
/// unconditionally) puts that item back rather than losing it silently. Always returns a
/// `TransformReport` (NoOp/Skipped/Applied), matching how every other transform is accounted for.
#[allow(clippy::too_many_arguments)]
fn apply_lossy_reduction(
    bytes: &[u8],
    tokens_before: usize,
    must_beat_tokens: usize,
    policy: &CompressionPolicy,
    format: InputFormat,
    estimator: &dyn TokenEstimator,
    protected: &[Vec<u8>],
    retrieval: &mut Option<RetrievalReport>,
) -> Result<(Vec<u8>, usize, TransformReport), TokenFoldError> {
    let noop = |status, reason| TransformReport {
        id: transforms::json_prune::TRANSFORM_ID.to_string(),
        version: transforms::json_prune::TRANSFORM_VERSION.to_string(),
        tokens_before,
        tokens_after: tokens_before,
        saved_tokens: 0,
        savings_ratio: 0.0,
        elapsed_micros: None,
        status,
        skipped_reason: reason,
        warnings: Vec::new(),
    };

    // json_prune has no concept of message roles: it treats a `messages`/`system` array the
    // same as any other JSON array, so on OpenAI/Anthropic payloads it can nominate a protected
    // message as a droppable candidate. `protected_segments_present` (below) only checks that
    // protected BYTES appear somewhere in the final output, not that the specific protected
    // MESSAGE survived -- a real gap when protected content is byte-identical to surviving,
    // unprotected content elsewhere in the document (e.g. templated/duplicated text), which a
    // live repro confirmed defeats the check entirely. Until real role-aware protection exists,
    // the only fail-closed option is to keep lossy pruning off every format where "protected
    // segments" is a real, non-empty concept -- i.e. run it only for generic `Json`, exactly
    // like every other JSON-data-only transform's `NotApplicableToFormat` path.
    if format != InputFormat::Json {
        return Ok((
            bytes.to_vec(),
            tokens_before,
            noop(
                TransformStatus::Skipped,
                Some(SkippedReason::NotApplicableToFormat),
            ),
        ));
    }

    let options = transforms::json_prune::LossyOptions {
        preserve_paths: policy.lossy_preserve.clone(),
        ratio: policy.lossy_ratio,
        namespace: policy.retrieval_namespace.clone(),
    };
    let outcome = match transforms::json_prune::prune(bytes, &options, estimator) {
        Ok(Some(outcome)) => outcome,
        Ok(None) => {
            return Ok((
                bytes.to_vec(),
                tokens_before,
                noop(TransformStatus::NoOp, None),
            ));
        }
        // Not JSON-shaped input (or empty): json_prune is only applicable to JSON, exactly like
        // every other JSON-only transform's NotApplicableToFormat path.
        Err(_) => {
            return Ok((
                bytes.to_vec(),
                tokens_before,
                noop(
                    TransformStatus::Skipped,
                    Some(SkippedReason::NotApplicableToFormat),
                ),
            ));
        }
    };

    // Preflight, BEFORE the store is so much as opened. `outcome.json` is this stage's best
    // possible result: every proposed drop storing successfully. The fail-closed path below can
    // only put items BACK, never remove more, so this token count is a hard lower bound on what
    // the stage can achieve. If even the best case can't beat the lossless branch, the stage is
    // going to be rolled back no matter what happens next -- so decide it here, while deciding it
    // is still free of side effects.
    //
    // Without this, a losing branch still ran every `store()` call first and left the persisted
    // bytes behind when its output was discarded: measured on a document that folds well, a
    // rolled-back prune left 4 per-item blobs on disk that no marker in the output referenced and
    // no field of the report counted. Storage the caller can neither see nor reach is not a
    // harmless leftover -- it is their data, written to disk as a side effect of an operation that
    // was reported as having been rolled back.
    let best_case_bytes = serde_json::to_vec(&outcome.json).map_err(|e| {
        TokenFoldError::InternalError(format!("failed to serialize pruned json: {e}"))
    })?;
    if estimator.count_bytes(&best_case_bytes) >= must_beat_tokens {
        return Ok((
            bytes.to_vec(),
            tokens_before,
            noop(TransformStatus::RolledBack, None),
        ));
    }

    let ttl_seconds = policy
        .retrieval_ttl_seconds
        .unwrap_or(retrieval_store::DEFAULT_TTL_SECONDS);
    // A preview must never write anything real to the retrieval store -- `store` stays `None`
    // unconditionally, never even opened, so a preview can't create so much as an empty
    // directory. An unopenable store for a REAL run (e.g. the documented `backend = "sqlite"`
    // scope cut) is folded into the same fail-closed path as an individual `store()` call
    // failing, matching how `maybe_store_originals` treats the identical failure class as a
    // soft skip rather than aborting compression — `--lossy` proposing zero recoverable drops
    // is a degraded but valid outcome, not a hard error.
    let store = if policy.preview {
        None
    } else {
        RetrievalStore::open(
            &policy.retrieval_backend,
            "sha256",
            policy.retrieval_store_path.clone(),
        )
        .ok()
    };

    // Preview never performs a REAL store — but it DOES run every dropped item through the same
    // store() safety checks (secret-shaped-content refusal, namespace validation) against a
    // throwaway, disk-free `RetrievalStore::memory()`, so a projected drop a real run would
    // actually refuse to persist gets put back here too. Without this, preview unconditionally
    // assumed every drop would succeed, so its projected savings could overstate what a real run
    // (which fails closed on a refused store()) would actually achieve.
    let preview_probe = policy.preview.then(RetrievalStore::memory);

    let mut restore: HashMap<String, Value> = HashMap::new();
    let mut persisted_bytes = 0usize;
    let mut skipped_bytes = 0usize;
    let mut marker_count = 0usize;
    if let Some(probe) = &preview_probe {
        for item in &outcome.dropped {
            let would_store = probe
                .store(&item.bytes, &policy.retrieval_namespace, Some(ttl_seconds))
                .is_ok();
            if !would_store {
                // Mirrors the real fail-closed branch below: put the item back rather than
                // leave its marker in a projection that a real run would never produce.
                let original: Value = serde_json::from_slice(&item.bytes).map_err(|e| {
                    TokenFoldError::InternalError(format!(
                        "json_prune produced a dropped item that isn't valid JSON: {e}"
                    ))
                })?;
                restore.insert(item.pointer.clone(), original);
            }
            // Whether or not the probe succeeded, `marker_count`/`persisted_bytes` stay at 0 —
            // nothing was REALLY stored, so the report must still honestly show `retrieval:
            // None` for a preview run; the probe only ever gates the projected OUTPUT/savings.
        }
    } else {
        let entries: Vec<_> = outcome
            .dropped
            .iter()
            .map(|item| {
                (
                    item.bytes.as_slice(),
                    policy.retrieval_namespace.as_str(),
                    Some(ttl_seconds),
                )
            })
            .collect();
        let stored_all = store
            .as_ref()
            .is_some_and(|store| store.store_batch(&entries).is_ok());
        if stored_all {
            persisted_bytes = outcome.dropped.iter().map(|item| item.bytes.len()).sum();
            marker_count = outcome.dropped.len();
        } else {
            for item in &outcome.dropped {
                let original: Value = serde_json::from_slice(&item.bytes).map_err(|e| {
                    TokenFoldError::InternalError(format!(
                        "json_prune produced a dropped item that isn't valid JSON: {e}"
                    ))
                })?;
                restore.insert(item.pointer.clone(), original);
                skipped_bytes += item.bytes.len();
            }
        }
    }

    let final_json = transforms::json_prune::revert_markers(&outcome.json, &restore);
    let final_bytes = serde_json::to_vec(&final_json).map_err(|e| {
        TokenFoldError::InternalError(format!("failed to serialize pruned json: {e}"))
    })?;
    let tokens_after_candidate = estimator.count_bytes(&final_bytes);

    // A regression here means every proposed drop failed to store (fail-closed put everything
    // back) while the marker scaffolding still added overhead -- report it plainly rather than
    // silently emitting a larger payload than we started with. Status is decided BEFORE the
    // retrieval report is touched (below): on rollback, `bytes_out` reverts to the original
    // plaintext with zero `$tf_ref` markers in it, so the report must not claim any markers
    // exist either. Batched publication above ensures a refused set commits no new entries.
    //
    // `json_prune` has no concept of message roles/protected content -- it happily nominates a
    // system message or the latest user turn in an OpenAI/Anthropic `messages` array as prunable
    // like any other array item. This check is what actually enforces the "system + latest-user
    // survive every transform byte-for-byte" invariant for the lossy stage, mirroring exactly
    // how `validate_safety()` gates every lossless transform in the loop above via
    // `safety::protected_segments_present` -- a protected-segment violation is treated identically
    // to a token regression: roll the whole stage back, never partially apply it.
    let violates_protected_segments = !safety::protected_segments_present(protected, &final_bytes);
    // `must_beat_tokens` is the LOSSLESS branch's own result (see `apply_transforms`), which is
    // always <= `tokens_before`, so this subsumes the plain token-regression check. `>=`, not
    // `>`: an output that merely ties the lossless one while having thrown items away is
    // strictly worse, not a wash -- the caller paid in data for nothing.
    let status = if final_bytes == bytes {
        TransformStatus::NoOp
    } else if tokens_after_candidate >= must_beat_tokens || violates_protected_segments {
        TransformStatus::RolledBack
    } else {
        TransformStatus::Applied
    };
    let bytes_out = if status == TransformStatus::RolledBack {
        bytes.to_vec()
    } else {
        final_bytes
    };
    let tokens_after = if status == TransformStatus::RolledBack {
        tokens_before
    } else {
        tokens_after_candidate
    };

    if status != TransformStatus::RolledBack {
        match retrieval {
            Some(existing) => {
                existing.marker_count += marker_count;
                existing.persisted_original_bytes += persisted_bytes;
                existing.skipped_original_bytes += skipped_bytes;
                // The whole-payload receipt can legitimately have been refused (e.g. secret-shaped
                // input, which `RetrievalStore::store` rejects unconditionally), leaving
                // `ttl_seconds: None` on the report it produced. These per-item entries, though,
                // were really written WITH `ttl_seconds` -- the redaction pass runs before them, so
                // they can succeed where the raw payload could not. Reporting a null TTL beside a
                // positive `marker_count`/`persisted_original_bytes` misdescribes what is on disk
                // and, worse, reads as "these never expire" (live repro: 18 markers, 15,840
                // persisted bytes, `ttl_seconds: null`, while every entry on disk carried 604800).
                if marker_count > 0 {
                    existing.ttl_seconds = Some(ttl_seconds);
                }
            }
            None if marker_count > 0 || skipped_bytes > 0 => {
                *retrieval = Some(RetrievalReport {
                    store_namespace: policy.retrieval_namespace.clone(),
                    hash_algorithm: "sha256".to_string(),
                    marker_count,
                    ttl_seconds: Some(ttl_seconds),
                    persisted_original_bytes: persisted_bytes,
                    skipped_original_bytes: skipped_bytes,
                });
            }
            None => {}
        }
    }

    let report_warnings = if status == TransformStatus::RolledBack && violates_protected_segments {
        vec![safety_downgrade_warning(
            transforms::json_prune::TRANSFORM_ID,
        )]
    } else {
        Vec::new()
    };

    Ok((
        bytes_out,
        tokens_after,
        TransformReport {
            id: transforms::json_prune::TRANSFORM_ID.to_string(),
            version: transforms::json_prune::TRANSFORM_VERSION.to_string(),
            tokens_before,
            tokens_after,
            saved_tokens: tokens_before.saturating_sub(tokens_after),
            savings_ratio: ratio(tokens_before, tokens_after),
            elapsed_micros: None,
            status,
            skipped_reason: None,
            warnings: report_warnings,
        },
    ))
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
        // json_value_dict replaces repeated values with dictionary references — also a
        // reversible restructure, gated on exact round-trip reconstruction.
        TransformId::JsonValueDict => {
            if !safety::json_still_valid(after) {
                return false;
            }
            if !transforms::json_dict::round_trips(before, after) {
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
        // log_field_fold restructures templated log lines into a columnar form, so its safety
        // invariant is exact reversibility: unfolding the output must reproduce the input bytes.
        TransformId::LogFieldFold => {
            if !transforms::log_fold::round_trips(before, after) {
                return false;
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
mod tests;
