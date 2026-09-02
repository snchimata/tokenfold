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

    assert!(matches!(
        output.report.status,
        Status::Compressed | Status::Passthrough
    ));
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
    assert_eq!(output.report.status, Status::Passthrough);
    assert_eq!(
        output.report.budget.unwrap().status,
        crate::report::BudgetStatus::NotRequested
    );
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
fn irreversible_log_compaction_is_excluded_from_presets() {
    // log_compaction was promoted out of --experimental (Phase 5 fidelity gate,
    // 2026-07-12): it now applies under the default Balanced preset with no --experimental
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
            .all(|t| t.id != "log_compaction")
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
    let output = compress_with_estimator(input.clone(), &policy, &ByteHeuristicEstimator).unwrap();

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
    // `retrieval.marker_count` is still 1 here: that is the whole-payload evidence-store
    // receipt, which every lossy run on a supported format gets regardless of pruning -- a
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
    // whole-payload evidence receipt is legitimate here; every per-item blob would be orphaned.
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

    // The `quality` presence rule: `Some(...)` once a lossy transform really ran,
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
        .build()
        .unwrap();
    let output = compress_with_estimator(input, &policy, &ByteHeuristicEstimator).unwrap();
    let out_value: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
    let arr = out_value["items"].as_array().unwrap();
    assert!(arr.iter().all(|item| {
        item.get("$tf_ref").is_some() || !item.to_string().contains("AKIAIOSFODNN7EXAMPLE")
    }));

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
    // also forces the separate whole-payload evidence backup (`maybe_store_originals`), which
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
        .expect("the whole-payload backup is forced on whenever lossy is set");
    assert_eq!(
        r.marker_count, 1,
        "only the whole-payload marker should be reported, not any of json_prune's own"
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
    // preview) while performing ZERO real RetrievalStore writes -- neither the whole-payload
    // evidence backup nor json_prune's own per-item stores.
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

    let input = CompressionInput::plain_text(b"AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE".to_vec());
    let policy = CompressionPolicy::builder()
        .store_originals(true)
        .retrieval_namespace("pipeline-test")
        .build()
        .unwrap();
    let output = compress_with_estimator(input.clone(), &policy, &ByteHeuristicEstimator).unwrap();

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
