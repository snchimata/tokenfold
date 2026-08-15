use std::collections::HashMap;

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
    // Every `CompressionPolicy` field is `pub`, so a caller can build or mutate one without
    // ever going through `CompressionPolicyBuilder::build` -- re-validate here so a hand-built
    // policy can't silently bypass the same invariants (e.g. lossy-without-durable-retrieval)
    // the builder enforces for free.
    policy.validate()?;
    let original_tokens = estimator.count_bytes(&input.bytes);
    let target = policy.target_tokens;
    let estimator_info = estimator.info();

    // F-045: whole-payload evidence store, best-effort. Runs against the full pre-transform
    // input regardless of which status path below is taken, so it must be computed up front.
    let retrieval = maybe_store_originals(&input.bytes, input.format, policy);

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
    // target is met (INTERFACES.md Part 2 "Early Exit").
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

    // Terminal, opt-in lossy stage (design doc §4) — strictly after the lossless loop above and
    // its own safety gates, never gated by `modes.rs`/`ALL_ENTRIES`. `retrieval` may already
    // hold the whole-payload F-045 report computed up front; lossy's own per-item stores merge
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

        // INTERFACES.md Part 2 "Early Exit" applies to the lossy stage too, and it is where the
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
    // INTERFACES.md §"`quality` presence rule": `None` iff no lossy transform ran, `Some` with a
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
            // config knob (F-011 acceptance criteria only requires the count be configurable,
            // not that Phase 2 ship distinct values per mode).
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
    for item in &outcome.dropped {
        if let Some(probe) = &preview_probe {
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
                restore.insert(item.hash.clone(), original);
            }
            // Whether or not the probe succeeded, `marker_count`/`persisted_bytes` stay at 0 —
            // nothing was REALLY stored, so the report must still honestly show `retrieval:
            // None` for a preview run; the probe only ever gates the projected OUTPUT/savings.
            continue;
        }
        let stored = store.as_ref().and_then(|s| {
            s.store(&item.bytes, &policy.retrieval_namespace, Some(ttl_seconds))
                .ok()
        });
        match stored {
            Some(_marker) => {
                persisted_bytes += item.bytes.len();
                marker_count += 1;
            }
            None => {
                let original: Value = serde_json::from_slice(&item.bytes).map_err(|e| {
                    TokenFoldError::InternalError(format!(
                        "json_prune produced a dropped item that isn't valid JSON: {e}"
                    ))
                })?;
                restore.insert(item.hash.clone(), original);
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
    // exist either, even though the underlying `store()` calls above already physically
    // succeeded — those bytes are real but orphaned (nothing in the output references them),
    // not a lie, but reporting them as live markers would be.
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
    fn json_data_transforms_never_regress_token_count() {
        // The "exact-token chooser" property: each JSON-data stage (minify -> fold -> dict) is
        // adopted only if it lowers the exact token count, so no input can come out larger —
        // including the shapes that sink naive TOON/CSV-ization: ragged, scalar, already-compact.
        let cases: &[&[u8]] = &[
            br#"[{"a":1,"b":2},{"a":3,"b":4},{"a":5,"b":6}]"#, // foldable
            br#"[{"a":1},{"b":2},{"c":3}]"#,                   // ragged, heterogeneous
            br#"{"x":[1,2,3],"y":"already compact scalar"}"#,  // no repeated structure
            br#"[1,2,3,4,5,6,7,8,9,10]"#,                      // scalars only
            br#"{}"#,                                          // trivial
        ];
        for case in cases {
            let input = CompressionInput::json(case.to_vec());
            let policy = CompressionPolicy::builder().build().unwrap();
            let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
            assert!(
                output.report.compressed_tokens <= output.report.original_tokens,
                "regressed on {}: {} -> {}",
                String::from_utf8_lossy(case),
                output.report.original_tokens,
                output.report.compressed_tokens,
            );
            // and every adopted transform round-trips is guaranteed by the pipeline safety gate;
            // here we just assert the output is still valid JSON.
            assert!(serde_json::from_slice::<serde_json::Value>(&output.bytes).is_ok());
        }
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
    fn omitting_lossy_reproduces_todays_output_byte_for_byte() {
        // Regression test per the design doc: `policy.lossy == None` must be code-for-code the
        // existing lossless path -- this compares against a policy that never even mentions
        // lossy fields, proving the new code is additive-only when not opted into.
        let payload = serde_json::json!({"items": (0..12).map(|i| serde_json::json!({"n": i})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes);
        let policy = CompressionPolicy::builder().build().unwrap();
        assert_eq!(policy.lossy, None);
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        assert!(
            !output
                .report
                .transforms
                .iter()
                .any(|t| t.id == "json_prune"),
            "json_prune must not even appear in the report when lossy is unset"
        );
    }

    #[test]
    fn lossy_prunes_a_large_array_and_reports_json_prune_applied() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // Each item carries enough padding to clear a `$tf_ref` marker's own overhead (so
        // dropping it is actually worth something). `json_field_fold`/`json_value_dict` are
        // disabled so `items` stays a plain array of literal objects for this test's assertions
        // -- lossy pruning runs strictly after the lossless stage per the design, so in real use
        // it would legitimately see an already-folded/dictionaried document instead; that
        // interaction is real but out of scope here, this test isolates json_prune's own
        // behavior.
        let padding = "x".repeat(150);
        let payload = serde_json::json!({
            "items": (0..30).map(|i| serde_json::json!({"id": i, "note": padding})).collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes);
        let policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.2)
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        let report = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(report.status, TransformStatus::Applied);
        assert!(report.tokens_after < report.tokens_before);
        assert!(serde_json::from_slice::<serde_json::Value>(&output.bytes).is_ok());

        // Every dropped item is actually retrievable -- the recoverability half of the contract.
        let out_value: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
        let arr = out_value["items"].as_array().unwrap();
        let store = crate::retrieval_store::RetrievalStore::default_filesystem();
        for item in arr {
            if let Some(hash) = item
                .get("$tf_ref")
                .and_then(|r| r.get("hash"))
                .and_then(|h| h.as_str())
            {
                assert!(matches!(
                    store.retrieve(hash, "default"),
                    crate::retrieval_store::RetrievalOutcome::Found(_)
                ));
            }
        }

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_that_prunes_nothing_is_never_worse_than_a_plain_lossless_run() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_noop_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // Round-5 external review, measured live: `--lossy-ratio 0.25` over a fixture whose
        // items are all far too small to be worth replacing with a `$tf_ref` marker emitted
        // 1,834 bytes where plain lossless emitted 644 -- ~3x WORSE while dropping nothing --
        // because `json_field_fold`/`json_value_dict` had been switched off up front for a
        // pruning stage that then never applied. Identical rows keep this document firmly in
        // that regime: nothing is prunable, so the deferred lossless transforms must run and the
        // two outputs must match exactly.
        let payload = serde_json::json!({
            "events": (0..12)
                .map(|i| serde_json::json!({"seq": i, "retries": 0, "note": "queue drain cycle completed normally"}))
                .collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();

        let lossless = compress_with_estimator(
            CompressionInput::json(bytes.clone()),
            &CompressionPolicy::builder().build().unwrap(),
            &ByteHeuristicEstimator,
        )
        .unwrap();
        let lossy = compress_with_estimator(
            CompressionInput::json(bytes),
            &CompressionPolicy::builder()
                .lossy(crate::budget::LossyPath::Heuristic)
                .lossy_ratio(0.25)
                .build()
                .unwrap(),
            &ByteHeuristicEstimator,
        )
        .unwrap();

        let prune = lossy
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_ne!(
            prune.status,
            TransformStatus::Applied,
            "fixture must stay in the nothing-was-pruned regime for this test to mean anything"
        );
        assert_eq!(
            lossy.bytes, lossless.bytes,
            "a --lossy run that pruned nothing must fall back to the exact lossless output"
        );
        // ...and the deferred transforms must really have run, not merely have been absent from
        // both sides: `json_field_fold` is what does the work on this shape.
        assert!(
            lossy
                .report
                .transforms
                .iter()
                .any(|t| t.id == "json_field_fold" && t.status == TransformStatus::Applied),
            "the deferred lossless transforms must be replayed once pruning turns out to be a no-op"
        );
        // No lossy transform ended up applying, so there is nothing for `quality` to describe.
        assert!(lossy.report.quality.is_none());

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_successful_prune_that_folding_would_have_beaten_is_rolled_back() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_loses_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // Deferring the array-restructuring transforms past the lossy stage only covers a prune
        // that does NOTHING. This is the other half, measured live: 30 identical large rows
        // dictionary/fold down to 1,186 bytes, while a genuinely successful `--lossy-ratio 0.25`
        // prune of the same document emits 7,589 -- 6.4x worse, with four items really dropped
        // and really persisted. Data loss is only ever justified by an output the lossless
        // pipeline could not produce, so this must lose to the lossless branch.
        let note = "routine health check completed without incident, all subsystems reported \
                    green, disk and memory pressure nominal, no retries were required, the \
                    scheduler handed off cleanly and downstream consumers acknowledged";
        let payload = serde_json::json!({
            "tasks": (0..30)
                .map(|i| serde_json::json!({"id": i, "worker": format!("w-{i}"), "note": note}))
                .collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();

        let lossless = compress_with_estimator(
            CompressionInput::json(bytes.clone()),
            &CompressionPolicy::builder().build().unwrap(),
            &ByteHeuristicEstimator,
        )
        .unwrap();
        let lossy = compress_with_estimator(
            CompressionInput::json(bytes),
            &CompressionPolicy::builder()
                .lossy(crate::budget::LossyPath::Heuristic)
                .lossy_ratio(0.25)
                .build()
                .unwrap(),
            &ByteHeuristicEstimator,
        )
        .unwrap();

        assert_eq!(
            lossy.bytes, lossless.bytes,
            "a prune that loses to folding must be rolled back in favor of the lossless output"
        );
        let prune = lossy
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_ne!(prune.status, TransformStatus::Applied);
        // Nothing pruned was adopted, so no `$tf_ref` may survive into the output. (The report's
        // `retrieval.marker_count` is still 1 here: that is F-045's whole-payload receipt, which
        // every lossy run on a supported format gets regardless of what pruning decided -- a
        // different thing from a per-item marker. `lossy_rollback_never_reports_retrieval_markers_
        // absent_from_the_output` covers the per-item accounting.)
        assert!(!String::from_utf8_lossy(&lossy.bytes).contains("$tf_ref"));
        assert!(lossy.report.quality.is_none());

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_is_skipped_when_the_lossless_pipeline_already_meets_the_target() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_target_met_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // Round-6 external review, measured live: every lossless transform is checked against the
        // target before it runs, but the lossy stage was not -- with `--target-tokens 1462`, a
        // figure `json_minify` alone already reached, `json_prune` ran anyway and replaced 17
        // items with markers. Destroying data to reach a target already reached is never right.
        let padding = "x".repeat(150);
        let payload = serde_json::json!({
            "items": (0..30).map(|i| serde_json::json!({"id": i, "note": padding})).collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();

        // Establish what the lossless pipeline achieves, then ask for exactly that.
        let lossless = compress_with_estimator(
            CompressionInput::json(bytes.clone()),
            &CompressionPolicy::builder().build().unwrap(),
            &ByteHeuristicEstimator,
        )
        .unwrap();
        let target = lossless.report.compressed_tokens;

        let lossy = compress_with_estimator(
            CompressionInput::json(bytes),
            &CompressionPolicy::builder()
                .target_tokens(target)
                .lossy(crate::budget::LossyPath::Heuristic)
                .lossy_ratio(0.1)
                .build()
                .unwrap(),
            &ByteHeuristicEstimator,
        )
        .unwrap();

        let prune = lossy
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(prune.status, TransformStatus::Skipped);
        assert_eq!(prune.skipped_reason, Some(SkippedReason::TargetAlreadyMet));
        assert!(!String::from_utf8_lossy(&lossy.bytes).contains("$tf_ref"));
        assert_eq!(lossy.report.status, Status::Compressed);

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_rejected_lossy_branch_leaves_no_orphaned_blobs_on_disk() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_orphans_{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        let store_dir = dir.join("store");

        // Round-6 external review, measured live: dropped items were persisted BEFORE the pipeline
        // decided whether the lossy branch beat the lossless one, so a losing branch left per-item
        // blobs behind that no marker in the output referenced and no report field counted --
        // the caller's data written to disk as a side effect of an operation reported as rolled
        // back. Same fold-friendly document as
        // `a_successful_prune_that_folding_would_have_beaten_is_rolled_back`.
        let note = "routine health check completed without incident, all subsystems reported \
                    green, disk and memory pressure nominal, no retries were required, the \
                    scheduler handed off cleanly and downstream consumers acknowledged";
        let payload = serde_json::json!({
            "tasks": (0..30)
                .map(|i| serde_json::json!({"id": i, "worker": format!("w-{i}"), "note": note}))
                .collect::<Vec<_>>()
        });
        let policy = CompressionPolicy::builder()
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.25)
            .retrieval_store_path(Some(store_dir.clone()))
            .build()
            .unwrap();
        let output = compress_with_estimator(
            CompressionInput::json(serde_json::to_vec(&payload).unwrap()),
            &policy,
            &ByteHeuristicEstimator,
        )
        .unwrap();

        let prune = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(prune.status, TransformStatus::RolledBack);

        // Whatever is physically on disk must equal what the report says was persisted. Only the
        // whole-payload F-045 receipt is legitimate here; every per-item blob would be an orphan.
        let blobs = std::fs::read_dir(store_dir.join("default"))
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| e.path().extension().is_some_and(|x| x == "bin"))
                    .count()
            })
            .unwrap_or(0);
        let reported = output
            .report
            .retrieval
            .as_ref()
            .map_or(0, |r| r.marker_count);
        assert_eq!(
            blobs, reported,
            "{blobs} blobs on disk vs {reported} reported -- a rejected branch persisted orphans"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn per_item_stores_report_their_real_ttl_even_when_the_whole_payload_was_refused() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_ttl_{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        let store_dir = dir.join("store");

        // Round-6 external review, measured live: a secret-shaped payload makes the whole-payload
        // receipt fail (`RetrievalStore::store` refuses it unconditionally), producing a report
        // with `ttl_seconds: None`. The per-item stores that follow run on POST-redaction content,
        // so they succeed -- and merging them left `marker_count: 18` / `persisted_original_bytes:
        // 15840` sitting next to `ttl_seconds: null`, which reads as "these never expire" while
        // every entry on disk actually carried the policy TTL.
        // Varied per-row filler, so pruning genuinely wins against folding here (identical rows
        // dictionary down to nothing and the stage is rolled back before any store runs -- see
        // `a_rejected_lossy_branch_leaves_no_orphaned_blobs_on_disk`).
        let items: Vec<Value> = (0..20)
            .map(|i| {
                serde_json::json!({
                    "id": i,
                    "note": format!("row {i}: {}", (0..40).map(|j| format!("field{}", (i * 7 + j) % 23)).collect::<Vec<_>>().join(" ")),
                })
            })
            .collect();
        let payload = serde_json::json!({
            "api_key": format!("sk-{}", "A".repeat(40)),
            "items": items,
        });
        let policy = CompressionPolicy::builder()
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.1)
            .retrieval_store_path(Some(store_dir))
            .build()
            .unwrap();
        let output = compress_with_estimator(
            CompressionInput::json(serde_json::to_vec(&payload).unwrap()),
            &policy,
            &ByteHeuristicEstimator,
        )
        .unwrap();

        let retrieval = output.report.retrieval.expect("retrieval report present");
        assert!(
            retrieval.skipped_original_bytes > 0,
            "the whole-payload receipt must have been refused for this test to mean anything"
        );
        assert!(
            retrieval.marker_count > 0,
            "per-item stores must have succeeded"
        );
        assert_eq!(
            retrieval.ttl_seconds,
            Some(retrieval_store::DEFAULT_TTL_SECONDS),
            "a positive marker_count must report the TTL those entries were really stored with"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_real_lossy_prune_reports_quality_as_present_but_unvalidated() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_quality_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // INTERFACES.md's `quality` presence rule: `Some(...)` once a lossy transform really ran,
        // with `validated_ratio_band: None` and absent metrics while no fidelity gate is baked in.
        // It used to stay `None` after a successful prune, leaving a JSON caller no field at all
        // to distinguish a pruned payload from a lossless one.
        let padding = "x".repeat(150);
        let payload = serde_json::json!({
            "items": (0..30).map(|i| serde_json::json!({"id": i, "note": padding})).collect::<Vec<_>>()
        });
        let policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.2)
            .build()
            .unwrap();
        let output = compress_with_estimator(
            CompressionInput::json(serde_json::to_vec(&payload).unwrap()),
            &policy,
            &ByteHeuristicEstimator,
        )
        .unwrap();

        let quality = output
            .report
            .quality
            .expect("quality present after a lossy run");
        assert!(!quality.gate_passed);
        assert_eq!(quality.validated_ratio_band, None);
        assert_eq!(quality.quality_retention, None);
        assert_eq!(quality.contrastive_failure_rate, None);

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_on_a_format_it_cannot_run_on_never_persists_the_input() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_wrong_format_{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&dir).ok();
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // Round-5 external review, measured live: an OpenAI payload compressed with `--lossy`
        // reported `json_prune: skipped / not_applicable_to_format` and STILL wrote 23,487 bytes
        // of the user's unmodified input to a freshly created retrieval directory. `--lossy`
        // implies a durable receipt only where the lossy stage can actually run.
        let payload = serde_json::json!({
            "model": "gpt-4o",
            "messages": (0..8)
                .map(|i| serde_json::json!({"role": "user", "content": format!("question {i} {}", "lorem ipsum dolor sit amet ".repeat(20))}))
                .collect::<Vec<_>>()
        });
        let policy = CompressionPolicy::builder()
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.2)
            .build()
            .unwrap();
        let output = compress_with_estimator(
            CompressionInput {
                format: InputFormat::OpenAiJson,
                bytes: serde_json::to_vec(&payload).unwrap(),
            },
            &policy,
            &ByteHeuristicEstimator,
        )
        .unwrap();

        assert_eq!(
            output.report.retrieval, None,
            "nothing may be persisted when the lossy stage cannot run and --store-originals \
             was never asked for"
        );
        assert!(
            !dir.exists(),
            "no retrieval directory may be created either"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_never_drops_a_secret_shaped_item_fail_closed() {
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_secret_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        // Every item is secret-shaped and big enough to otherwise be worth dropping, so every
        // store() call must fail -- fail-closed means all of them stay in the output verbatim,
        // none become a $tf_ref marker, and compression still succeeds rather than erroring.
        let padding = "y".repeat(150);
        let payload = serde_json::json!({
            "items": (0..10).map(|_| serde_json::json!({
                "key": "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE",
                "note": padding,
            })).collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes);
        let policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.0)
            .unsafe_disable_redaction(true) // isolate json_prune's own secret gate, not the earlier redaction stage
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        let out_value: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
        let arr = out_value["items"].as_array().unwrap();
        assert_eq!(arr.len(), 10);
        assert!(
            arr.iter().all(|item| item.get("$tf_ref").is_none()),
            "every item must survive fail-closed since none could be stored"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_rollback_never_reports_retrieval_markers_absent_from_the_output() {
        // Regression test: an adversarial review caught that apply_lossy_reduction merged
        // per-item RetrievalStore::store() outcomes into the shared retrieval report BEFORE
        // deciding NoOp/RolledBack/Applied, and never undid that merge on rollback -- so a
        // rolled-back run (output reverts to the original, zero $tf_ref markers) could still
        // report a nonzero marker_count/persisted_original_bytes claiming markers exist that
        // don't. This estimator deliberately penalizes any assembled document containing a
        // marker (simulating a real non-additive-tokenization blowup Tier 3 exists to catch),
        // to reliably force a rollback despite items looking individually droppable in
        // isolation.
        struct JointPenaltyEstimator;
        impl TokenEstimator for JointPenaltyEstimator {
            fn info(&self) -> crate::report::EstimatorInfo {
                crate::report::EstimatorInfo {
                    backend: "test-joint-penalty".to_string(),
                    model: None,
                    is_exact: true,
                }
            }
            fn count_bytes(&self, bytes: &[u8]) -> usize {
                let has_marker = bytes.windows(7).any(|w| w == b"$tf_ref");
                if has_marker && bytes.len() > 300 {
                    bytes.len() * 3
                } else {
                    bytes.len()
                }
            }
        }

        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_rollback_report_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        let padding = "x".repeat(150);
        let payload = serde_json::json!({
            "items": (0..10).map(|i| serde_json::json!({"n": i, "pad": padding})).collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes.clone());
        let policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .disable("json_minify")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.3)
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &JointPenaltyEstimator).unwrap();

        let jp = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(
            jp.status,
            TransformStatus::RolledBack,
            "test fixture must actually trigger a rollback to exercise the fix; got {:?}",
            jp.status
        );
        // The core regression this guards against: json_prune's own per-item drops must not be
        // reported once the output has reverted to plain (marker-free) content. Note `--lossy`
        // also forces the separate F-045 whole-payload backup (`maybe_store_originals`), which
        // legitimately succeeds regardless of json_prune's own rollback -- so the correct
        // expectation is "exactly that one whole-payload marker, nothing extra from json_prune's
        // own (discarded) per-item drops", not "zero markers total".
        let s = String::from_utf8_lossy(&output.bytes);
        assert!(
            !s.contains("$tf_ref"),
            "rolled-back output must contain no markers"
        );
        let r = output
            .report
            .retrieval
            .as_ref()
            .expect("F-045 whole-payload backup is forced on whenever lossy is set");
        assert_eq!(
            r.marker_count, 1,
            "only the F-045 whole-payload marker should be reported, not any of json_prune's own"
        );
        assert_eq!(
            r.persisted_original_bytes,
            bytes.len(),
            "persisted bytes must be exactly the whole-payload backup, nothing extra from \
             json_prune's own rolled-back per-item drops"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_never_drops_the_system_message_or_latest_user_message_for_openai_format() {
        // Regression test for a real, confirmed safety gap: json_prune has no concept of
        // message roles, so without this check it would happily nominate a system message (or
        // any other message) in an OpenAI `messages` array as a droppable candidate like any
        // other array item -- silently violating tokenfold's core "system + latest-user survive
        // every transform byte-for-byte" invariant, with no warning and exit 0.
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_protected_openai_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        let padding = "x".repeat(300);
        let system_content = format!("SYSTEM_PROMPT_MUST_SURVIVE: {padding}");
        let payload = serde_json::json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": system_content},
                {"role": "user", "content": format!("turn 1: {padding}")},
                {"role": "assistant", "content": format!("turn 2: {padding}")},
                {"role": "user", "content": format!("turn 3: {padding}")},
                {"role": "assistant", "content": format!("turn 4: {padding}")},
            ]
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::openai_json(bytes);
        let policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.05) // aggressive -- maximizes the chance the system message looks droppable
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        let s = String::from_utf8_lossy(&output.bytes);
        assert!(
            s.contains("SYSTEM_PROMPT_MUST_SURVIVE"),
            "system message must survive lossy pruning byte-for-byte; output was: {s}"
        );

        // Superseded by the format-gate fix below: json_prune now never even attempts OpenAI
        // payloads, so it's always Skipped/NotApplicableToFormat here, never RolledBack.
        let jp = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(jp.status, TransformStatus::Skipped);
        assert_eq!(
            jp.skipped_reason,
            Some(SkippedReason::NotApplicableToFormat)
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_only_ever_runs_for_generic_json_never_openai_or_anthropic_format() {
        // Round-4 external review, live-repro-verified: `protected_segments_present` is a pure
        // substring-presence check across the WHOLE final output, not a per-message identity
        // check. If a dropped protected message's content is byte-identical to a surviving,
        // UNPROTECTED message's content (a realistic case for templated/duplicated text), the
        // check passes even though the real protected message was replaced by a marker. Real
        // repro that reproduced this before the fix: an assistant message with a
        // `success: false` failure signal (so json_prune ranks it highest-keep) carries the
        // SAME content as the system message and the latest user message; both of the latter
        // get dropped, but the surviving assistant message's identical bytes satisfy the
        // substring check for both, so json_prune reported `status: applied` with zero warnings
        // and exit 0 while the real system/user messages were gone. The only fail-closed fix
        // (until real role-aware protection exists) is to keep lossy off every format where
        // "protected segments" is a real concept at all -- this test proves that gate holds
        // even in the exact duplicate-content shape that defeated the substring check.
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_duplicate_content_bypass_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        let padding = "x".repeat(150);
        let content_x = format!("content X {padding}");
        let content_y = format!("content Y {padding}");
        let payload = serde_json::json!({
            "model": "gpt-4",
            "messages": [
                {"role": "assistant", "content": content_x, "success": false},
                {"role": "system", "content": content_x},
                {"role": "assistant", "content": content_y},
                {"role": "user", "content": content_x},
            ]
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::openai_json(bytes);
        let policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.35)
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        assert!(
            !String::from_utf8_lossy(&output.bytes).contains("$tf_ref"),
            "no message may be replaced by a marker on a message-format payload"
        );
        let jp = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(jp.status, TransformStatus::Skipped);
        assert_eq!(
            jp.skipped_reason,
            Some(SkippedReason::NotApplicableToFormat)
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_preview_shows_projected_savings_without_any_real_storage_write() {
        // Regression test: `compress --dry-run`/`inspect` route through `policy.preview = true`.
        // A preview must show accurate projected lossy savings (so it's actually useful as a
        // preview) while performing ZERO real RetrievalStore writes -- neither the F-045
        // whole-payload backup nor json_prune's own per-item stores.
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_preview_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }
        let store_root = dir.join("tokenfold").join("retrieve");

        let padding = "x".repeat(300);
        let payload = serde_json::json!({
            "items": (0..20).map(|i| serde_json::json!({"id": i, "note": padding})).collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes);
        let mut policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.2)
            .build()
            .unwrap();
        policy.preview = true;
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        let jp = output
            .report
            .transforms
            .iter()
            .find(|t| t.id == "json_prune")
            .expect("json_prune report present");
        assert_eq!(
            jp.status,
            TransformStatus::Applied,
            "preview must still show projected savings, not silently no-op"
        );
        assert!(jp.tokens_after < jp.tokens_before);
        assert!(
            output.report.retrieval.is_none(),
            "a preview must not claim anything was persisted"
        );
        assert!(
            !store_root.exists(),
            "a preview must never create the retrieval store directory at all"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_preview_puts_back_items_a_real_store_would_refuse_over_an_unsafe_namespace() {
        // Round-4 external review: preview previously assumed every proposed drop would
        // succeed, so its projected savings/output couldn't reflect a real run's fail-closed
        // rollback of an item `RetrievalStore::store` refuses to persist. Preview now probes the
        // same refusal via a throwaway in-memory store, so items a real run would put back stay
        // in the PROJECTED output too, matching what a real (non-preview) run would produce.
        //
        // Uses an unsafe (path-traversal-shaped) `retrieval_namespace` rather than secret-shaped
        // item content: the mandatory `secret_redaction` stage runs BEFORE json_prune and would
        // have already scrubbed a secret pattern out of the item bytes by the time `store()` (or
        // this probe) ever sees them, so that refusal path can never actually be reached from
        // here -- an unsafe namespace is a real `store()` refusal reason that content-scrubbing
        // upstream can't interfere with, and isn't validated anywhere before this point either.
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_preview_namespace_refusal_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }

        let padding = "x".repeat(300);
        let items: Vec<Value> = (0..10)
            .map(|i| serde_json::json!({"id": i, "note": "boring", "pad": padding}))
            .collect();
        let payload = serde_json::json!({"items": items});
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes);
        let mut policy = CompressionPolicy::builder()
            .disable("json_field_fold")
            .disable("json_value_dict")
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.0) // maximizes drop pressure
            .retrieval_namespace("../escape") // RetrievalStore::store refuses this unconditionally
            .build()
            .unwrap();
        policy.preview = true;
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();

        let s = String::from_utf8_lossy(&output.bytes);
        assert!(
            !s.contains("$tf_ref"),
            "every proposed drop must be put back once probed against a namespace a real \
             store() call would refuse; output was: {s}"
        );
        assert!(
            output.report.retrieval.is_none(),
            "a preview must still never claim anything was really persisted"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_preserve_holds_even_when_the_caller_never_disabled_json_field_fold() {
        // Real, reproduced gap found while testing the round-4 fixes: `json_field_fold`/
        // `json_value_dict` run BEFORE the terminal lossy stage and can restructure a
        // homogeneous array-of-objects (e.g. into columnar sub-arrays at DIFFERENT paths), which
        // silently moved `--lossy-preserve "items"` off the array it was meant to protect --
        // every existing lossy test in this codebase had been manually disabling both transforms
        // (masking the gap), so a real CLI user who never thought to do that got no protection at
        // all despite passing `--lossy-preserve`. Fixed by excluding both transforms from the
        // lossless loop whenever `policy.lossy` is set, so json_prune always sees (and
        // `--lossy-preserve` always names paths against) the same array-of-objects shape the
        // caller wrote. This test deliberately does NOT disable either transform.
        let _g = lock_retrieval_env();
        let dir = std::env::temp_dir().join(format!(
            "tokenfold_pipeline_test_lossy_preserve_survives_field_fold_{}",
            std::process::id()
        ));
        unsafe {
            std::env::set_var("XDG_DATA_HOME", &dir);
        }
        // Homogeneous key set (`id`/`note`) so json_field_fold would normally have a real
        // restructuring incentive to fold this array, unlike the uniquely-keyed workaround shape
        // used elsewhere in this file to sidestep folding.
        let items: Vec<Value> = (0..30)
            .map(|i| serde_json::json!({"id": i, "note": "x".repeat(300)}))
            .collect();
        let payload = serde_json::json!({"items": items});
        let bytes = serde_json::to_vec(&payload).unwrap();
        let input = CompressionInput::json(bytes);
        let policy = CompressionPolicy::builder()
            .lossy(crate::budget::LossyPath::Heuristic)
            .lossy_ratio(0.1)
            .lossy_preserve("items")
            .build()
            .unwrap();
        let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
        let s = String::from_utf8_lossy(&output.bytes);
        assert!(
            !s.contains("$tf_ref"),
            "preserve must hold; output was: {s}"
        );

        unsafe {
            std::env::remove_var("XDG_DATA_HOME");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn lossy_requires_a_non_memory_backend() {
        let err = CompressionPolicy::builder()
            .lossy(crate::budget::LossyPath::Heuristic)
            .retrieval_backend("memory")
            .build();
        assert!(
            err.is_err(),
            "policy construction itself must reject this combination"
        );
    }

    #[test]
    fn compress_rejects_a_hand_mutated_policy_that_bypasses_builder_validation() {
        // Every `CompressionPolicy` field is `pub`, so a caller can mutate a builder-built
        // policy (or construct one via struct literal) into a state `build()` would have
        // refused. `compress_with_estimator` must re-validate, not just trust the builder.
        let mut policy = CompressionPolicy::builder().build().unwrap();
        policy.lossy_ratio = 5.0; // out of range; build() would have rejected this outright
        let input = CompressionInput::plain_text(b"hello".to_vec());
        let err = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
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
