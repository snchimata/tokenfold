#![cfg(feature = "tiktoken")]
use serde_json::{Value, json};
use tokenfold_core::fit::*;

fn options(target: u64) -> FitOptions {
    FitOptions {
        target_tokens: target,
        counter_id: "test:whole-request:v1".into(),
        ..FitOptions::default()
    }
}

// A trusted renderer can propose a smaller guide form without knowing custom costs.
struct Proposal;
impl FitRequest for Proposal {
    fn source_groups(&self) -> &[SourceGroup] {
        &[]
    }
    fn scoring_query(&self) -> Option<&str> {
        None
    }
    fn fixed_guide_bytes(&self) -> Vec<u8> {
        vec![]
    }
    fn render(&self, _: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError> {
        Ok(RenderedRequest {
            format: FitFormat::Text,
            scope: AccountingScope::RenderedText,
            payload: RenderedPayload::Text(match pass {
                RenderPass::Original => "raw ".repeat(19) + "raw",
                RenderPass::CoreCompatible => "proposal ".repeat(11) + "proposal",
            }),
            core_transforms: if pass == RenderPass::Original {
                vec![]
            } else {
                vec!["test_proposal".into()]
            },
        })
    }
    fn validate(&self, _: &RenderedRequest, _: &[GroupId]) -> Result<(), FitError> {
        Ok(())
    }
}

fn complete(original: TokenCount, proposal: TokenCount, mode: BudgetMode) -> FitOutput {
    let mut session = FitSession::new(
        Box::new(Proposal),
        FitOptions {
            budget_mode: mode,
            ..options(12)
        },
    )
    .unwrap();
    let FitAction::Count {
        candidate_id,
        request,
        ..
    } = session.advance(None).unwrap()
    else {
        panic!("original count")
    };
    let reply = FitReply::Counted {
        candidate_id,
        counter_id: options(12).counter_id,
        scope: request.scope,
        count: original,
    };
    let FitAction::Count {
        candidate_id,
        request,
        ..
    } = session.advance(Some(reply)).unwrap()
    else {
        panic!("proposal count")
    };
    let reply = FitReply::Counted {
        candidate_id,
        counter_id: options(12).counter_id,
        scope: request.scope,
        count: proposal,
    };
    let FitAction::Done(output) = session.advance(Some(reply)).unwrap() else {
        panic!("done")
    };
    output
}

#[test]
fn core_fit() {
    let output = complete(
        TokenCount::exact(20),
        TokenCount::exact(12),
        BudgetMode::Strict,
    );
    assert_eq!(output.receipt.budget_status, BudgetStatus::Met);
    assert_eq!(output.receipt.original_tokens, Some(20));
    assert_eq!(output.receipt.baseline_tokens, Some(12));
    assert_eq!(output.receipt.final_tokens, Some(12));
    assert_eq!(output.receipt.tokens_saved(), Some(8));
    assert_eq!(output.require_met().unwrap(), &output.payload);
    assert_eq!(output.require_fit().unwrap(), &output.payload);
    assert_eq!(output.receipt.core_transforms, ["test_proposal"]);
}

#[test]
fn unknown_accounting() {
    for mode in [BudgetMode::Strict, BudgetMode::BestEffort] {
        let output = complete(TokenCount::estimated(20), TokenCount::estimated(12), mode);
        assert_eq!(output.receipt.budget_status, BudgetStatus::Indeterminate);
        assert!(matches!(
            output.require_met(),
            Err(FitError::BudgetIndeterminate(_))
        ));
        assert_eq!(output.require_fit().is_ok(), mode == BudgetMode::BestEffort);
        assert_eq!(output.receipt.estimated_fit(), Some(true));
        assert!(
            output
                .receipt
                .warnings
                .iter()
                .any(|w| w.code == "estimated_accounting")
        );
        let unknown = complete(TokenCount::unavailable(), TokenCount::exact(1), mode);
        assert_eq!(unknown.receipt.original_tokens, None);
        assert_eq!(unknown.receipt.final_tokens, None);
        assert_eq!(unknown.receipt.tokens_saved(), None);
        assert!(unknown.require_fit().is_err());
        assert!(unknown.require_met().is_err());
        assert!(unknown.receipt.core_transforms.is_empty());
        assert_eq!(
            unknown.payload,
            Proposal.render(&[], RenderPass::Original).unwrap().payload
        );
    }
    let changed = complete(
        TokenCount::exact(20),
        TokenCount::unavailable(),
        BudgetMode::Strict,
    );
    assert_eq!(changed.receipt.final_tokens, Some(20));
    assert_eq!(changed.receipt.budget_status, BudgetStatus::Unmet);
    assert_eq!(changed.receipt.reason, Some(FitReason::UnknownAccounting));
    assert!(changed.receipt.core_transforms.is_empty());
    let changed = complete(
        TokenCount::exact(20),
        TokenCount::estimated(12),
        BudgetMode::BestEffort,
    );
    assert_eq!(changed.receipt.final_tokens, Some(20));
    assert_eq!(changed.receipt.accounting.final_basis, CountBasis::Exact);
}

#[test]
fn signed_savings() {
    let output = complete(
        TokenCount::exact(10),
        TokenCount::exact(15),
        BudgetMode::Strict,
    );
    assert_eq!(output.receipt.final_tokens, Some(10));
    assert!(output.receipt.core_transforms.is_empty());
    let mut receipt = output.receipt;
    receipt.original_tokens = Some(8);
    receipt.baseline_tokens = Some(9);
    receipt.final_tokens = Some(10);
    let read = Receipt::from_json(&receipt.to_json().unwrap()).unwrap();
    assert_eq!(read.tokens_saved(), Some(-2));
    assert_eq!(read.selection_tokens_saved(), Some(-1));
}

#[test]
fn invalid_replies() {
    let mut session = FitSession::new(Box::new(Proposal), options(12)).unwrap();
    assert!(
        session
            .advance(Some(FitReply::Counted {
                candidate_id: 1,
                counter_id: options(12).counter_id,
                scope: AccountingScope::RenderedText,
                count: TokenCount::exact(20)
            }))
            .is_err()
    );
    let FitAction::Count { candidate_id, .. } = session.advance(None).unwrap() else {
        panic!()
    };
    assert!(session.advance(None).is_err());
    for (id, counter_id, scope, count) in [
        (
            candidate_id + 1,
            options(12).counter_id,
            AccountingScope::RenderedText,
            TokenCount::exact(20),
        ),
        (
            candidate_id,
            "changed".into(),
            AccountingScope::RenderedText,
            TokenCount::exact(20),
        ),
        (
            candidate_id,
            options(12).counter_id,
            AccountingScope::ProviderRequest,
            TokenCount::exact(20),
        ),
        (
            candidate_id,
            options(12).counter_id,
            AccountingScope::RenderedText,
            TokenCount {
                tokens: Some(1),
                basis: CountBasis::Unavailable,
            },
        ),
        (
            candidate_id,
            options(12).counter_id,
            AccountingScope::RenderedText,
            TokenCount::exact(MAX_SAFE_INTEGER + 1),
        ),
    ] {
        assert!(
            session
                .advance(Some(FitReply::Counted {
                    candidate_id: id,
                    counter_id,
                    scope,
                    count
                }))
                .is_err()
        );
    }
    let FitAction::Count {
        candidate_id: next, ..
    } = session
        .advance(Some(FitReply::Counted {
            candidate_id,
            counter_id: options(12).counter_id,
            scope: AccountingScope::RenderedText,
            count: TokenCount::exact(20),
        }))
        .unwrap()
    else {
        panic!()
    };
    assert!(
        session
            .advance(Some(FitReply::Counted {
                candidate_id,
                counter_id: options(12).counter_id,
                scope: AccountingScope::RenderedText,
                count: TokenCount::exact(20)
            }))
            .is_err()
    );
    assert!(matches!(
        session
            .advance(Some(FitReply::Counted {
                candidate_id: next,
                counter_id: options(12).counter_id,
                scope: AccountingScope::RenderedText,
                count: TokenCount::exact(12)
            }))
            .unwrap(),
        FitAction::Done(_)
    ));
    assert!(session.advance(None).is_err());
}

#[test]
fn receipt_roundtrip() {
    let output = complete(
        TokenCount::exact(20),
        TokenCount::exact(12),
        BudgetMode::Strict,
    );
    let wire = output.receipt.to_json().unwrap();
    assert_eq!(Receipt::from_json(&wire).unwrap(), output.receipt);
    let mut value: Value = serde_json::from_str(&wire).unwrap();
    for (key, invalid) in [
        ("schema_version", json!("2.0")),
        ("kind", json!("other")),
        ("final_tokens", json!(-1)),
        ("target_tokens", json!(MAX_SAFE_INTEGER + 1)),
        ("budget_status", json!("indeterminate")),
        ("scorer_succeeded", json!(true)),
        ("selection_applied", json!(true)),
    ] {
        let previous = value[key].clone();
        value[key] = invalid;
        assert!(Receipt::from_json(&value.to_string()).is_err(), "{key}");
        value[key] = previous;
    }
    assert!(
        Receipt::from_json(&wire.replacen("\"kind\":", "\"kind\":\"tokenfold.fit\",\"kind\":", 1))
            .is_err()
    );
    assert!(
        Receipt::from_json(&wire.replacen(
            "\"counter_id\":",
            "\"counter_id\":\"duplicate\",\"counter_id\":",
            1
        ))
        .is_err()
    );
}

#[test]
fn no_side_effects() {
    let source = json!([{ "long_field_name": 1, "other_name": 2 }, { "long_field_name": 3, "other_name": 4 }, { "long_field_name": 5, "other_name": 6 }, { "long_field_name": 7, "other_name": 8 }]);
    let original = source.clone();
    let request = DataRequest::new(source, DataRepresentation::Folded).unwrap();
    let counter = EncodingCounter::new("o200k_base").unwrap();
    let mut session = FitSession::new(
        Box::new(request),
        FitOptions {
            counter_id: counter.id().into(),
            ..options(1000)
        },
    )
    .unwrap();
    let FitAction::Done(result) = session.drive(&counter, None).unwrap() else {
        panic!("native counts must not escape to host")
    };
    let RenderedPayload::Text(payload) = result.payload else {
        panic!()
    };
    let decoded =
        tokenfold_core::decode(payload.as_bytes(), tokenfold_core::DecodeFormat::Json).unwrap();
    assert_eq!(serde_json::from_slice::<Value>(&decoded).unwrap(), original);
    assert!(!payload.contains("$tf_ref"));
    assert!(!result.receipt.scorer_attempted);
    assert!(result.receipt.scorer.is_none());
    assert!(result.receipt.groups_dropped.is_empty());
}

#[test]
fn authoritative_counter_rolls_back_a_smaller_guide_proposal() {
    let guide = EncodingCounter::new("o200k_base").unwrap();
    assert_eq!(
        guide
            .count(&Proposal.render(&[], RenderPass::Original).unwrap())
            .unwrap(),
        TokenCount::exact(20)
    );
    assert_eq!(
        guide
            .count(&Proposal.render(&[], RenderPass::CoreCompatible).unwrap())
            .unwrap(),
        TokenCount::exact(12)
    );
    let output = complete(
        TokenCount::exact(10),
        TokenCount::exact(15),
        BudgetMode::Strict,
    );
    assert_eq!(
        output.payload,
        Proposal.render(&[], RenderPass::Original).unwrap().payload
    );
    assert_eq!(output.receipt.budget_status, BudgetStatus::Met);
    assert_eq!(output.receipt.final_tokens, Some(10));
    assert!(output.receipt.core_transforms.is_empty());
}

#[test]
fn text_and_json_contracts_are_not_guessed() {
    let text = "  {\"b\": 1, \"a\": 2}\n\t😀é\0";
    let request = TextRequest::new(text.into()).unwrap();
    for pass in [RenderPass::Original, RenderPass::CoreCompatible] {
        assert_eq!(
            request.render(&[], pass).unwrap().payload,
            RenderedPayload::Text(text.into())
        );
    }
    let request = DataRequest::from_json(" {\"b\":1,\"a\":2} ", DataRepresentation::Json).unwrap();
    assert_eq!(
        request
            .render(&[], RenderPass::CoreCompatible)
            .unwrap()
            .payload,
        RenderedPayload::Text("{\"b\":1,\"a\":2}".into())
    );
    let request = DataRequest::new(
        json!({"__tf_cols__":["key"],"__tf_rows__":[[1]]}),
        DataRepresentation::Json,
    )
    .unwrap();
    let original = request.render(&[], RenderPass::Original).unwrap();
    request.validate(&original, &[]).unwrap();
}

#[test]
fn folded_codec_collisions_fail_as_invalid_input_not_internal_errors() {
    assert!(matches!(
        DataRequest::new(
            json!({"__tf_cols__":["key"],"__tf_rows__":[[1]]}),
            DataRepresentation::Folded
        ),
        Err(FitError::InvalidInput(_))
    ));
}

#[test]
fn invalid_receipt_combinations_are_rejected() {
    let receipt = complete(
        TokenCount::exact(20),
        TokenCount::exact(12),
        BudgetMode::Strict,
    )
    .receipt;
    let wire = receipt.to_json().unwrap();
    let value: Value = serde_json::from_str(&wire).unwrap();
    for key in ["reason", "scorer", "original_tokens", "timings_ms"] {
        let mut bad = value.clone();
        bad.as_object_mut().unwrap().remove(key);
        assert!(
            Receipt::from_json(&bad.to_string()).is_err(),
            "missing {key}"
        );
    }
    for (pointer, invalid) in [
        ("/accounting/final_basis", json!("unavailable")),
        ("/accounting/counter_id", json!("")),
        ("/timings_ms/core", json!(-0.1)),
        ("/groups_dropped", json!(["lost"])),
        ("/groups_kept", json!(["same", "same"])),
        ("/reason", json!("estimated_fit")),
        ("/scorer_attempted", json!(true)),
    ] {
        let mut bad = value.clone();
        *bad.pointer_mut(pointer).unwrap() = invalid;
        assert!(Receipt::from_json(&bad.to_string()).is_err(), "{pointer}");
    }
    let mut bad = receipt;
    bad.timings_ms.total = Some(f64::NAN);
    assert!(bad.to_json().is_err());
}

#[test]
fn receipt_timing_numbers_are_not_token_counts() {
    let mut receipt = complete(
        TokenCount::exact(20),
        TokenCount::exact(12),
        BudgetMode::Strict,
    )
    .receipt;
    receipt.timings_ms.total = Some(1e20);
    assert_eq!(
        Receipt::from_json(&receipt.to_json().unwrap()).unwrap(),
        receipt
    );
}

fn assert_invalid_receipt(receipt: &Receipt) {
    assert!(receipt.to_json().is_err(), "writer accepted {receipt:?}");
    let wire = serde_json::to_string(receipt).unwrap();
    assert!(
        Receipt::from_json(&wire).is_err(),
        "reader accepted {receipt:?}"
    );
}

fn attempted_scorer() -> ScorerInfo {
    ScorerInfo {
        revision: "pinned-test-revision".into(),
        manifest_hash: "test-manifest-hash".into(),
        runtime_id: "test-runtime".into(),
        device: "cpu".into(),
        precision: "fp32".into(),
        qualification: "unqualified".into(),
    }
}

#[test]
fn receipt_policy_reason_invariants() {
    for (mode, count, expected) in [
        (
            BudgetMode::Strict,
            TokenCount::estimated(10),
            FitReason::UnknownAccounting,
        ),
        (
            BudgetMode::Strict,
            TokenCount::estimated(20),
            FitReason::UnknownAccounting,
        ),
        (
            BudgetMode::Strict,
            TokenCount::unavailable(),
            FitReason::UnknownAccounting,
        ),
        (
            BudgetMode::BestEffort,
            TokenCount::unavailable(),
            FitReason::UnknownAccounting,
        ),
        (
            BudgetMode::BestEffort,
            TokenCount::estimated(10),
            FitReason::EstimatedFit,
        ),
    ] {
        let receipt = complete(count, count, mode).receipt;
        assert_eq!(receipt.reason, Some(expected));
        assert_eq!(
            Receipt::from_json(&receipt.to_json().unwrap()).unwrap(),
            receipt
        );
        if expected == FitReason::UnknownAccounting {
            let mut bad = receipt.clone();
            bad.scorer_attempted = true;
            bad.scorer_succeeded = true;
            bad.scorer = Some(attempted_scorer());
            assert_invalid_receipt(&bad);
        }
        for reason in [
            FitReason::SelectionDisabled,
            FitReason::NoGain,
            FitReason::ScorerFailure,
        ] {
            let mut bad = receipt.clone();
            bad.reason = Some(reason);
            assert_invalid_receipt(&bad);
        }
        let mut bad = receipt;
        bad.reason = Some(if expected == FitReason::EstimatedFit {
            FitReason::UnknownAccounting
        } else {
            FitReason::EstimatedFit
        });
        assert_invalid_receipt(&bad);
    }
}

#[test]
fn receipt_scorer_outcome_invariants() {
    let baseline = complete(
        TokenCount::exact(20),
        TokenCount::exact(15),
        BudgetMode::Strict,
    )
    .receipt;
    for reason in [
        FitReason::ScorerFailure,
        FitReason::ScorerTimeout,
        FitReason::ScorerResponseInvalid,
    ] {
        let mut receipt = baseline.clone();
        receipt.reason = Some(reason);
        assert_invalid_receipt(&receipt);
        receipt.scorer_attempted = true;
        receipt.scorer = Some(attempted_scorer());
        assert_eq!(
            Receipt::from_json(&receipt.to_json().unwrap()).unwrap(),
            receipt
        );
        receipt.scorer_succeeded = true;
        assert_invalid_receipt(&receipt);
    }
    for reason in [
        FitReason::SelectionDisabled,
        FitReason::RuntimeUnavailable,
        FitReason::NoOptionalGroups,
        FitReason::RequiredOverBudget,
        FitReason::UnsupportedSelection,
        FitReason::SelectionLimit,
        FitReason::ScorerQueueTimeout,
    ] {
        let mut receipt = baseline.clone();
        receipt.reason = Some(reason);
        assert_eq!(
            Receipt::from_json(&receipt.to_json().unwrap()).unwrap(),
            receipt
        );
        receipt.scorer_attempted = true;
        receipt.scorer = Some(attempted_scorer());
        assert_invalid_receipt(&receipt);
        receipt.scorer_succeeded = true;
        assert_invalid_receipt(&receipt);
    }
    for reason in [FitReason::NoGain, FitReason::NoValidFit] {
        let mut receipt = baseline.clone();
        receipt.reason = Some(reason);
        assert_invalid_receipt(&receipt);
        receipt.scorer_attempted = true;
        receipt.scorer_succeeded = true;
        receipt.scorer = Some(attempted_scorer());
        assert_eq!(
            Receipt::from_json(&receipt.to_json().unwrap()).unwrap(),
            receipt
        );
    }
    let mut unknown_after_scoring = baseline;
    unknown_after_scoring.reason = Some(FitReason::UnknownAccounting);
    unknown_after_scoring.scorer_attempted = true;
    unknown_after_scoring.scorer_succeeded = true;
    unknown_after_scoring.scorer = Some(attempted_scorer());
    assert_eq!(
        Receipt::from_json(&unknown_after_scoring.to_json().unwrap()).unwrap(),
        unknown_after_scoring
    );
    unknown_after_scoring.scorer_succeeded = false;
    assert_invalid_receipt(&unknown_after_scoring);
    for (mode, original, final_count) in [
        (
            BudgetMode::Strict,
            TokenCount::exact(20),
            TokenCount::exact(10),
        ),
        (
            BudgetMode::BestEffort,
            TokenCount::estimated(20),
            TokenCount::estimated(10),
        ),
    ] {
        let mut selected = complete(original, final_count, mode).receipt;
        selected.baseline_tokens = Some(20);
        selected.selection_applied = true;
        selected.groups_kept = vec!["required".into()];
        selected.groups_dropped = vec!["optional".into()];
        selected.scorer_attempted = true;
        selected.scorer_succeeded = true;
        selected.scorer = Some(attempted_scorer());
        assert_eq!(
            Receipt::from_json(&selected.to_json().unwrap()).unwrap(),
            selected
        );
    }
}

#[test]
fn a_discarded_unavailable_proposal_does_not_invalidate_an_exact_original_fit() {
    let output = complete(
        TokenCount::exact(10),
        TokenCount::unavailable(),
        BudgetMode::Strict,
    );
    assert_eq!(output.receipt.final_tokens, Some(10));
    assert_eq!(output.receipt.budget_status, BudgetStatus::Met);
    assert_eq!(output.receipt.reason, None);
    assert!(output.require_met().is_ok());
}

#[test]
fn rolled_back_best_effort_estimate_reports_the_returned_fit() {
    for proposed in [TokenCount::unavailable(), TokenCount::exact(8)] {
        let output = complete(TokenCount::estimated(10), proposed, BudgetMode::BestEffort);
        assert_eq!(output.receipt.reason, Some(FitReason::EstimatedFit));
        assert_eq!(output.receipt.budget_status, BudgetStatus::Indeterminate);
        assert_eq!(output.receipt.original_tokens, Some(10));
        assert_eq!(output.receipt.baseline_tokens, Some(10));
        assert_eq!(output.receipt.final_tokens, Some(10));
        assert_eq!(output.receipt.accounting.final_basis, CountBasis::Estimated);
        assert_eq!(
            output.payload,
            Proposal.render(&[], RenderPass::Original).unwrap().payload
        );
        assert!(output.receipt.core_transforms.is_empty());
        assert!(
            output
                .receipt
                .warnings
                .iter()
                .any(|w| w.code == "estimated_accounting")
        );
        assert!(output.require_fit().is_ok());
        assert!(matches!(
            output.require_met(),
            Err(FitError::BudgetIndeterminate(_))
        ));
        let strict = complete(TokenCount::estimated(10), proposed, BudgetMode::Strict);
        assert_eq!(strict.receipt.reason, Some(FitReason::UnknownAccounting));
        assert!(strict.require_fit().is_err());
        let overflow = complete(TokenCount::estimated(20), proposed, BudgetMode::BestEffort);
        assert_eq!(overflow.receipt.reason, Some(FitReason::UnknownAccounting));
        assert!(overflow.require_fit().is_err());
    }
}

#[test]
fn context_rendering_is_ordered_and_query_is_required() {
    let request = ContextRequest::new(
        "Query\n".into(),
        vec![
            ContextGroup {
                id: "first".into(),
                content: GroupContent::Text(" α ".into()),
                required: false,
            },
            ContextGroup {
                id: "required".into(),
                content: GroupContent::Data(
                    DataRequest::new(json!({"z":1,"a":2}), DataRepresentation::Json).unwrap(),
                ),
                required: true,
            },
        ],
    )
    .unwrap();
    assert_eq!(request.source_groups()[0].scoring_text, " α ");
    assert_eq!(request.source_groups()[1].scoring_text, "{\"z\":1,\"a\":2}");
    assert_eq!(
        request
            .render(&["first".into(), "required".into()], RenderPass::Original)
            .unwrap()
            .payload,
        RenderedPayload::Text("Query\n\n\n α \n\n{\"z\":1,\"a\":2}".into())
    );
    assert_eq!(
        request
            .render(&["required".into()], RenderPass::Original)
            .unwrap()
            .payload,
        RenderedPayload::Text("Query\n\n\n{\"z\":1,\"a\":2}".into())
    );
    assert!(
        request
            .render(&["first".into()], RenderPass::Original)
            .is_err()
    );
    assert!(
        request
            .render(&["required".into(), "first".into()], RenderPass::Original)
            .is_err()
    );
    assert!(ContextRequest::new("".into(), vec![]).is_err());
}

#[test]
fn input_limits_and_decoded_safety() {
    for source in [
        "{\"a\":1,\"\\u0061\":2}",
        "{\"x\":{\"a\":1,\"a\":2}}",
        "9007199254740992",
        "-9007199254740992",
        "9007199254740992.0",
        "NaN",
        "1e999",
        "\"\\ud800\"",
    ] {
        assert!(
            DataRequest::from_json(source, DataRepresentation::Json).is_err(),
            "{source}"
        );
    }
    assert!(DataRequest::from_json("9007199254740991", DataRepresentation::Json).is_ok());
    assert!(DataRequest::from_json("-9007199254740991", DataRepresentation::Json).is_ok());
    assert!(
        DataRequest::from_json(
            &("[".repeat(64) + "0" + &"]".repeat(64)),
            DataRepresentation::Json
        )
        .is_ok()
    );
    assert!(
        DataRequest::from_json(
            &("[".repeat(65) + "0" + &"]".repeat(65)),
            DataRepresentation::Json
        )
        .is_err()
    );
    assert!(TextRequest::new("x".repeat(MAX_SOURCE_BYTES)).is_ok());
    assert!(TextRequest::new("x".repeat(MAX_SOURCE_BYTES + 1)).is_err());
    assert!(matches!(
        DataRequest::from_json(
            "{\"x\":\"sk-\\u0041BCDEFGHIJ1234567890abcdefgh\"}",
            DataRepresentation::Json
        ),
        Err(FitError::Safety)
    ));
    assert!(matches!(
        TextRequest::new("sk-ABCDEFGHIJ1234567890abcdefgh".into()),
        Err(FitError::Safety)
    ));
    assert!(ContextRequest::new("q".repeat(4097), vec![]).is_err());
    for id in ["", "__tf_reserved", "\n"] {
        assert!(
            ContextRequest::new(
                "q".into(),
                vec![ContextGroup {
                    id: id.into(),
                    content: GroupContent::Text("text".into()),
                    required: false
                }]
            )
            .is_err()
        );
    }
    let group = |id: &str| ContextGroup {
        id: id.into(),
        content: GroupContent::Text("text".into()),
        required: false,
    };
    assert!(ContextRequest::new("q".into(), vec![group("same"), group("same")]).is_err());
    assert!(ContextRequest::new("q".into(), vec![group(&"é".repeat(129))]).is_err());
    assert!(
        ContextRequest::new(
            "q".into(),
            (0..513).map(|i| group(&i.to_string())).collect()
        )
        .is_err()
    );
    assert!(FitSession::new(Box::new(Proposal), options(MAX_SAFE_INTEGER + 1)).is_err());
}

#[test]
fn zero_empty_native_counts_and_cache_isolation() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct Counter {
        calls: AtomicUsize,
        id: String,
        tokens: u64,
    }
    impl TokenCounter for Counter {
        fn id(&self) -> &str {
            &self.id
        }
        fn count(&self, _: &RenderedRequest) -> Result<TokenCount, FitError> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(TokenCount::exact(self.tokens))
        }
    }
    for (id, tokens) in [("a:v1", 0), ("a:v2", 1), ("b:v1", 2)] {
        let counter = Counter {
            calls: AtomicUsize::new(0),
            id: id.into(),
            tokens,
        };
        for _ in 0..2 {
            let mut session = FitSession::new(
                Box::new(TextRequest::new("".into()).unwrap()),
                FitOptions {
                    counter_id: id.into(),
                    ..options(0)
                },
            )
            .unwrap();
            let FitAction::Done(output) = session.drive(&counter, None).unwrap() else {
                panic!()
            };
            assert_eq!(output.receipt.final_tokens, Some(tokens));
            assert_eq!(
                output.receipt.budget_status,
                if tokens == 0 {
                    BudgetStatus::Met
                } else {
                    BudgetStatus::Unmet
                }
            );
        }
        assert_eq!(
            counter.calls.load(Ordering::Relaxed),
            2,
            "identical original/baseline is counted once per fit, never cross-fit"
        );
    }
    for encoding in ["o200k_base", "cl100k_base"] {
        let counter = EncodingCounter::new(encoding).unwrap();
        let mut session = FitSession::new(
            Box::new(TextRequest::new("".into()).unwrap()),
            FitOptions {
                counter_id: counter.id().into(),
                ..options(0)
            },
        )
        .unwrap();
        let FitAction::Done(output) = session.drive(&counter, None).unwrap() else {
            panic!()
        };
        assert_eq!(output.receipt.final_tokens, Some(0));
        assert!(output.require_met().is_ok());
        let provider = RenderedRequest {
            format: FitFormat::OpenaiChat,
            scope: AccountingScope::ProviderRequest,
            payload: RenderedPayload::Json(json!({"messages":[]})),
            core_transforms: vec![],
        };
        assert_eq!(counter.count(&provider).unwrap(), TokenCount::unavailable());
    }
    assert!(matches!(
        EncodingCounter::new("unknown"),
        Err(FitError::Config(_))
    ));
    std::thread::scope(|scope| {
        for i in 0..8 {
            scope.spawn(move || {
                let counter = Counter {
                    calls: AtomicUsize::new(0),
                    id: format!("concurrent:{i}"),
                    tokens: i,
                };
                let mut session = FitSession::new(
                    Box::new(TextRequest::new("same".into()).unwrap()),
                    FitOptions {
                        counter_id: counter.id().into(),
                        ..options(100)
                    },
                )
                .unwrap();
                let FitAction::Done(output) = session.drive(&counter, None).unwrap() else {
                    panic!()
                };
                assert_eq!(output.receipt.final_tokens, Some(i));
                assert_eq!(counter.calls.load(Ordering::Relaxed), 1);
            });
        }
    });
}

#[test]
fn cancellation_and_callback_errors_are_not_outcomes() {
    struct Broken;
    impl TokenCounter for Broken {
        fn id(&self) -> &str {
            "test:whole-request:v1"
        }
        fn count(&self, _: &RenderedRequest) -> Result<TokenCount, FitError> {
            Err(FitError::Accounting("callback failed".into()))
        }
    }
    let mut session = FitSession::new(Box::new(Proposal), options(12)).unwrap();
    assert!(matches!(
        session.drive(&Broken, None),
        Err(FitError::Accounting(_))
    ));
    let mut session = FitSession::new(Box::new(Proposal), options(12)).unwrap();
    session.cancellation_token().cancel();
    assert!(matches!(session.advance(None), Err(FitError::Cancelled)));
}

#[test]
fn every_distinct_candidate_is_scanned_before_a_counter_sees_it() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct UnsafeProposal;
    impl FitRequest for UnsafeProposal {
        fn source_groups(&self) -> &[SourceGroup] {
            &[]
        }
        fn scoring_query(&self) -> Option<&str> {
            None
        }
        fn fixed_guide_bytes(&self) -> Vec<u8> {
            vec![]
        }
        fn render(&self, _: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError> {
            Ok(RenderedRequest {
                format: FitFormat::Text,
                scope: AccountingScope::RenderedText,
                payload: RenderedPayload::Text(
                    if pass == RenderPass::Original {
                        "clean source"
                    } else {
                        "sk-ABCDEFGHIJ1234567890abcdefgh"
                    }
                    .into(),
                ),
                core_transforms: vec![],
            })
        }
        fn validate(&self, _: &RenderedRequest, _: &[GroupId]) -> Result<(), FitError> {
            Ok(())
        }
    }
    struct Counter(AtomicUsize);
    impl TokenCounter for Counter {
        fn id(&self) -> &str {
            "test:whole-request:v1"
        }
        fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
            self.0.fetch_add(1, Ordering::Relaxed);
            assert_eq!(
                request.payload,
                RenderedPayload::Text("clean source".into())
            );
            Ok(TokenCount::exact(20))
        }
    }
    let counter = Counter(AtomicUsize::new(0));
    let mut session = FitSession::new(Box::new(UnsafeProposal), options(12)).unwrap();
    assert!(matches!(
        session.drive(&counter, None),
        Err(FitError::Safety)
    ));
    assert_eq!(counter.0.load(Ordering::Relaxed), 1);
}

#[test]
fn shared_core_fixture_and_native_driver_use_identical_decisions() {
    let fixture: Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/sdk-fit/core.json")).unwrap();
    let request = || {
        DataRequest::new(
            fixture["request"]["value"].clone(),
            DataRepresentation::Folded,
        )
        .unwrap()
    };
    assert_eq!(
        request().render(&[], RenderPass::Original).unwrap().payload,
        RenderedPayload::Text(fixture["original_payload"].as_str().unwrap().into())
    );
    assert_eq!(
        request()
            .render(&[], RenderPass::CoreCompatible)
            .unwrap()
            .payload,
        RenderedPayload::Text(fixture["proposal_payload"].as_str().unwrap().into())
    );
    struct FixtureCounter<'a>(&'a Value);
    impl TokenCounter for FixtureCounter<'_> {
        fn id(&self) -> &str {
            self.0["counter_id"].as_str().unwrap()
        }
        fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
            let RenderedPayload::Text(text) = &request.payload else {
                panic!()
            };
            let field = if text == self.0["original_payload"].as_str().unwrap() {
                "original_count"
            } else {
                assert_eq!(text, self.0["proposal_payload"].as_str().unwrap());
                "proposal_count"
            };
            Ok(serde_json::from_value(self.0[field].clone()).unwrap())
        }
    }
    let counter = FixtureCounter(&fixture);
    let opts = FitOptions {
        counter_id: counter.id().into(),
        ..options(12)
    };
    let mut native = FitSession::new(Box::new(request()), opts.clone()).unwrap();
    let FitAction::Done(expected) = native.drive(&counter, None).unwrap() else {
        panic!()
    };
    let expected_receipt =
        Receipt::from_json(include_str!("../../../tests/fixtures/sdk-fit/receipt.json")).unwrap();
    assert_eq!(expected.receipt, expected_receipt);
    let mut host = FitSession::new(Box::new(request()), opts).unwrap();
    let mut action = host.advance(None).unwrap();
    loop {
        match action {
            FitAction::Count {
                candidate_id,
                request,
                ..
            } => {
                let count = counter.count(&request).unwrap();
                action = host
                    .advance(Some(FitReply::Counted {
                        candidate_id,
                        counter_id: counter.id().into(),
                        scope: request.scope,
                        count,
                    }))
                    .unwrap();
            }
            FitAction::Done(actual) => {
                assert_eq!(actual, expected);
                break;
            }
            FitAction::Score { .. } => panic!("Core-only fixture must never score"),
        }
    }
    let context = &fixture["context"];
    let groups = context["groups"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| ContextGroup {
            id: g["id"].as_str().unwrap().into(),
            required: g["required"].as_bool().unwrap(),
            content: match &g["content"] {
                Value::String(s) => GroupContent::Text(s.clone()),
                v => GroupContent::Data(
                    DataRequest::new(v.clone(), DataRepresentation::Json).unwrap(),
                ),
            },
        })
        .collect();
    let context_request =
        ContextRequest::new(context["query"].as_str().unwrap().into(), groups).unwrap();
    assert_eq!(
        context_request
            .source_groups()
            .iter()
            .map(|g| g.scoring_text.as_str())
            .collect::<Vec<_>>(),
        context["scorer_projections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect::<Vec<_>>()
    );
    assert_eq!(
        context_request
            .render(
                &["optional".into(), "required".into()],
                RenderPass::Original
            )
            .unwrap()
            .payload,
        RenderedPayload::Text(context["rendered"].as_str().unwrap().into())
    );
}
