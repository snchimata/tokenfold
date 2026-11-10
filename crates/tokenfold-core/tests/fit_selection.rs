#![cfg(feature = "tiktoken")]
use tokenfold_core::fit::*;

struct Groups(Vec<SourceGroup>);
impl FitRequest for Groups {
    fn source_groups(&self) -> &[SourceGroup] {
        &self.0
    }
    fn scoring_query(&self) -> Option<&str> {
        Some("query")
    }
    fn fixed_guide_bytes(&self) -> Vec<u8> {
        vec![]
    }
    fn render(&self, ids: &[GroupId], _: RenderPass) -> Result<RenderedRequest, FitError> {
        validate_survivors(&self.0, ids)?;
        Ok(RenderedRequest {
            format: FitFormat::Context,
            scope: AccountingScope::RenderedText,
            payload: RenderedPayload::Text(ids.join(" ")),
            core_transforms: vec![],
        })
    }
    fn validate(&self, candidate: &RenderedRequest, ids: &[GroupId]) -> Result<(), FitError> {
        assert_eq!(candidate, &self.render(ids, RenderPass::Original)?);
        Ok(())
    }
}
fn selection_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/sdk-fit/selection.json"
    ))
    .unwrap()
}
fn groups() -> Groups {
    Groups(
        selection_fixture()["groups"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .map(|(i, g)| {
                let scoring_text = g["scoring_text"].as_str().unwrap().to_owned();
                assert_eq!(
                    guide_estimator()
                        .unwrap()
                        .count_bytes(scoring_text.as_bytes()),
                    g["guide_cost"].as_u64().unwrap() as usize
                );
                SourceGroup {
                    id: g["id"].as_str().unwrap().into(),
                    scoring_text,
                    required: g["required"].as_bool().unwrap(),
                    source_positions: vec![i],
                }
            })
            .collect(),
    )
}
fn counted(action: FitAction, count: TokenCount) -> FitReply {
    let FitAction::Count {
        candidate_id,
        request,
        ..
    } = action
    else {
        panic!("count required")
    };
    FitReply::Counted {
        candidate_id,
        counter_id: "test:selection:v1".into(),
        scope: request.scope,
        count,
    }
}
#[test]
fn selection_preflights_required_subset_before_scoring() {
    let mut s = FitSession::new(
        Box::new(groups()),
        FitOptions {
            target_tokens: 10,
            counter_id: "test:selection:v1".into(),
            allow_selection: true,
            ..FitOptions::default()
        },
    )
    .unwrap();
    let first = s.advance(None).unwrap();
    let action = s
        .advance(Some(counted(first, TokenCount::exact(14))))
        .unwrap();
    assert!(
        matches!(
            action,
            FitAction::Count {
                purpose: CountPurpose::Required,
                ..
            }
        ),
        "required preflight: {action:?}"
    );
}
fn pin() -> ScorerInfo {
    ScorerInfo {
        revision: "test-rev".into(),
        manifest_hash: "test-manifest".into(),
        runtime_id: "test-runtime".into(),
        device: "cpu".into(),
        precision: "fp32".into(),
        qualification: "unqualified".into(),
    }
}
fn ready() -> FitOptions {
    FitOptions {
        target_tokens: 10,
        counter_id: "test:selection:v1".into(),
        allow_selection: true,
        select: Some(pin()),
        ..FitOptions::default()
    }
}
fn run(
    input: Box<dyn FitRequest>,
    opts: FitOptions,
    mut count: impl FnMut(&str) -> TokenCount,
    mut score: impl FnMut(u64, Vec<SourceGroup>, ScorerInfo) -> FitReply,
) -> (FitOutput, Vec<String>) {
    let mut s = FitSession::new(input, opts).unwrap();
    let mut action = s.advance(None).unwrap();
    let mut counts = vec![];
    for _ in 0..520 {
        action = match action {
            FitAction::Count { ref request, .. } => {
                let RenderedPayload::Text(text) = &request.payload else {
                    panic!("text")
                };
                counts.push(text.clone());
                let measurement = count(text);
                s.advance(Some(counted(action, measurement))).unwrap()
            }
            FitAction::Score {
                action_id,
                groups,
                scorer,
                ..
            } => s.advance(Some(score(action_id, groups, scorer))).unwrap(),
            FitAction::Done(out) => {
                out.receipt.validate().unwrap();
                return (out, counts);
            }
        }
    }
    panic!("candidate bound")
}
fn scored(action_id: u64, groups: Vec<SourceGroup>, scorer: ScorerInfo) -> FitReply {
    assert_eq!(
        groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
        ["A", "B", "C"]
    );
    assert!(groups[0].required);
    FitReply::Scored {
        action_id,
        scorer,
        scores: groups
            .iter()
            .map(|g| GroupScore {
                id: g.id.clone(),
                score: match g.id.as_str() {
                    "B" => 2.,
                    "C" => 1.,
                    _ => -100.,
                },
            })
            .collect(),
    }
}
fn exact(text: &str) -> TokenCount {
    TokenCount::exact(match text {
        "A B C" => 14,
        "A B" => 10,
        "A" => 6,
        _ => panic!("unexpected {text}"),
    })
}
#[test]
fn exact_selection_and_nonadditive_trim_reuse_required_candidate() {
    for ab in [10, 11] {
        let (out, calls) = run(
            Box::new(groups()),
            ready(),
            |t| {
                if t == "A B" {
                    TokenCount::exact(ab)
                } else {
                    exact(t)
                }
            },
            scored,
        );
        assert!(out.receipt.selection_applied);
        assert_eq!(
            out.receipt.groups_kept,
            if ab == 10 { vec!["A", "B"] } else { vec!["A"] }
        );
        assert_eq!(
            out.payload,
            RenderedPayload::Text(out.receipt.groups_kept.join(" "))
        );
        assert_eq!(
            out.receipt.final_tokens,
            Some(if ab == 10 { 10 } else { 6 })
        );
        assert_eq!(calls, ["A B C", "A", "A B"]);
        assert!(out.require_met().is_ok());
    }
}
#[test]
fn conservative_required_preflight_never_claims_monotonic_impossibility() {
    let (out, calls) = run(
        Box::new(groups()),
        ready(),
        |t| {
            if t == "A" {
                TokenCount::exact(11)
            } else if t == "A B" {
                TokenCount::exact(9)
            } else {
                exact(t)
            }
        },
        |_, _, _| panic!("no score"),
    );
    assert_eq!(out.receipt.reason, Some(FitReason::RequiredOverBudget));
    assert_eq!(out.payload, RenderedPayload::Text("A B C".into()));
    assert_eq!(out.receipt.final_tokens, Some(14));
    assert_eq!(calls, ["A B C", "A"]);
}
#[test]
fn strict_estimates_unavailable_disabled_and_missing_runtime_never_infer() {
    let cases = [
        (
            BudgetMode::Strict,
            CountBasis::Estimated,
            true,
            true,
            FitReason::UnknownAccounting,
        ),
        (
            BudgetMode::BestEffort,
            CountBasis::Unavailable,
            true,
            true,
            FitReason::UnknownAccounting,
        ),
        (
            BudgetMode::Strict,
            CountBasis::Exact,
            false,
            true,
            FitReason::SelectionDisabled,
        ),
        (
            BudgetMode::Strict,
            CountBasis::Exact,
            true,
            false,
            FitReason::RuntimeUnavailable,
        ),
    ];
    for (mode, basis, consent, runtime, reason) in cases {
        let opts = FitOptions {
            budget_mode: mode,
            allow_selection: consent,
            select: runtime.then(pin),
            ..ready()
        };
        let (out, _) = run(
            Box::new(groups()),
            opts,
            |t| TokenCount {
                tokens: if basis == CountBasis::Unavailable {
                    None
                } else {
                    exact(t).tokens
                },
                basis,
            },
            |_, _, _| panic!("no inference"),
        );
        assert_eq!(out.receipt.reason, Some(reason));
        assert!(!out.receipt.scorer_attempted);
        assert_eq!(out.receipt.groups_kept, ["A", "B", "C"]);
    }
}
#[test]
fn best_effort_estimated_selection_remains_indeterminate() {
    let opts = FitOptions {
        budget_mode: BudgetMode::BestEffort,
        ..ready()
    };
    let (out, _) = run(
        Box::new(groups()),
        opts,
        |t| TokenCount::estimated(exact(t).tokens.unwrap()),
        scored,
    );
    assert_eq!(out.receipt.budget_status, BudgetStatus::Indeterminate);
    assert_eq!(out.receipt.reason, Some(FitReason::EstimatedFit));
    assert!(out.require_met().is_err());
    assert!(out.require_fit().is_ok());
}
#[test]
fn all_failed_candidates_restore_the_complete_baseline_measurement() {
    for basis in [CountBasis::Unavailable, CountBasis::Estimated] {
        let (out, _) = run(
            Box::new(groups()),
            ready(),
            |t| {
                if t == "A B" {
                    TokenCount {
                        tokens: (basis != CountBasis::Unavailable).then_some(1),
                        basis,
                    }
                } else {
                    exact(t)
                }
            },
            scored,
        );
        assert_eq!(out.payload, RenderedPayload::Text("A B C".into()));
        assert_eq!(out.receipt.final_tokens, Some(14));
        assert_eq!(out.receipt.accounting.final_basis, CountBasis::Exact);
        assert_eq!(out.receipt.reason, Some(FitReason::UnknownAccounting));
        assert!(out.receipt.scorer_succeeded);
        assert!(!out.receipt.selection_applied);
    }
}
#[test]
fn malformed_and_failed_scores_are_bounded_not_fallback_rankings() {
    for kind in 0..10 {
        let (out, _) = run(Box::new(groups()), ready(), exact, |id, groups, scorer| {
            if kind < 5 {
                return FitReply::ScoreFailed {
                    action_id: id,
                    failure: [
                        ScoreFailure::Failure,
                        ScoreFailure::Timeout,
                        ScoreFailure::ResponseInvalid,
                        ScoreFailure::QueueTimeout,
                        ScoreFailure::SelectionLimit,
                    ][kind],
                };
            }
            let FitReply::Scored {
                mut scores,
                mut scorer,
                ..
            } = scored(id, groups, scorer)
            else {
                unreachable!()
            };
            match kind {
                5 => {
                    scores.pop();
                }
                6 => scores[0].score = f64::NAN,
                7 => scores.swap(0, 1),
                8 => scorer.revision = "wrong".into(),
                _ => scores[1].id = "A".into(),
            }
            FitReply::Scored {
                action_id: id,
                scores,
                scorer,
            }
        });
        assert_eq!(out.payload, RenderedPayload::Text("A B C".into()));
        assert_eq!(out.receipt.final_tokens, Some(14));
        assert!(!out.receipt.scorer_succeeded);
        assert_eq!(out.receipt.scorer_attempted, kind != 3 && kind != 4);
        assert_eq!(
            out.receipt.scorer,
            if kind == 3 || kind == 4 {
                None
            } else {
                Some(pin())
            }
        );
    }
}
#[test]
fn tie_breaking_keeps_earlier_source_and_trims_later_first() {
    let (out, _) = run(Box::new(groups()), ready(), exact, |id, groups, pin| {
        FitReply::Scored {
            action_id: id,
            scorer: pin,
            scores: groups
                .iter()
                .map(|g| GroupScore {
                    id: g.id.clone(),
                    score: 1.,
                })
                .collect(),
        }
    });
    assert_eq!(out.receipt.groups_kept, ["A", "B"]);
}
#[test]
fn no_optionals_fitting_baselines_and_pins_are_validated_without_scoring() {
    let mut required = groups();
    for g in &mut required.0 {
        g.required = true;
    }
    let (out, calls) = run(Box::new(required), ready(), exact, |_, _, _| {
        panic!("no score")
    });
    assert_eq!(out.receipt.reason, Some(FitReason::NoOptionalGroups));
    assert_eq!(calls, ["A B C"]);
    let (out, calls) = run(
        Box::new(groups()),
        FitOptions {
            target_tokens: 20,
            ..ready()
        },
        exact,
        |_, _, _| panic!("already fits"),
    );
    assert_eq!(out.receipt.reason, None);
    assert!(out.receipt.warnings.is_empty());
    assert_eq!(calls.len(), 1);
    let mut bad = ready();
    bad.select.as_mut().unwrap().revision = "bad\nmetadata".into();
    assert!(matches!(
        FitSession::new(Box::new(groups()), bad),
        Err(FitError::Config(_))
    ));
}
#[test]
fn splice_secret_is_scanned_in_full_before_custom_counter() {
    use std::cell::Cell;
    let input = ContextRequest::new(
        "query".into(),
        vec![
            ContextGroup {
                id: "prefix".into(),
                content: GroupContent::Text(format!("Bearer{}", " ".repeat(300))),
                required: true,
            },
            ContextGroup {
                id: "separator".into(),
                content: GroupContent::Text("!".into()),
                required: false,
            },
            ContextGroup {
                id: "suffix".into(),
                content: GroupContent::Text("synthetic_token_1234567890".into()),
                required: true,
            },
        ],
    )
    .unwrap();
    struct Counter(Cell<usize>);
    impl TokenCounter for Counter {
        fn id(&self) -> &str {
            "test:selection:v1"
        }
        fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
            let RenderedPayload::Text(s) = &request.payload else {
                panic!()
            };
            assert!(s.contains('!'), "unsafe candidate reached counter");
            self.0.set(self.0.get() + 1);
            Ok(TokenCount::exact(20))
        }
    }
    let c = Counter(Cell::new(0));
    let mut session = FitSession::new(Box::new(input), ready()).unwrap();
    assert!(matches!(session.drive(&c, None), Err(FitError::Safety)));
    assert_eq!(c.0.get(), 1);
}
#[test]
fn numeric_zero_score_ties_trim_the_later_group_first() {
    let (out, _) = run(
        Box::new(groups()),
        FitOptions {
            target_tokens: 14,
            ..ready()
        },
        |t| {
            TokenCount::exact(match t {
                "A B C" => 20,
                "A" => 6,
                "A B" | "A C" => 10,
                _ => panic!(),
            })
        },
        |id, groups, scorer| FitReply::Scored {
            action_id: id,
            scorer,
            scores: groups
                .iter()
                .map(|g| GroupScore {
                    id: g.id.clone(),
                    score: if g.id == "B" { -0.0 } else { 0.0 },
                })
                .collect(),
        },
    );
    assert_eq!(out.receipt.groups_kept, ["A", "B"]);
}
#[test]
fn shared_selection_decisions_match_counts_and_receipts() {
    let f = selection_fixture();
    for case in f["cases"].as_array().unwrap() {
        let (out, _) = run(
            Box::new(groups()),
            ready(),
            |s| TokenCount::exact(case["counts"][s].as_u64().unwrap()),
            scored,
        );
        assert_eq!(
            serde_json::to_value(&out.receipt.groups_kept).unwrap(),
            case["groups_kept"]
        );
        assert_eq!(out.receipt.final_tokens, case["final_tokens"].as_u64());
        assert_eq!(
            serde_json::to_value(out.receipt.reason).unwrap(),
            case.get("reason")
                .cloned()
                .unwrap_or(serde_json::Value::Null)
        );
    }
}
#[test]
fn candidate_cache_reuses_preflight_validation_and_is_per_profile() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    struct Validations {
        groups: Groups,
        validations: Arc<AtomicUsize>,
    }
    impl FitRequest for Validations {
        fn source_groups(&self) -> &[SourceGroup] {
            self.groups.source_groups()
        }
        fn scoring_query(&self) -> Option<&str> {
            Some("query")
        }
        fn fixed_guide_bytes(&self) -> Vec<u8> {
            vec![]
        }
        fn render(&self, ids: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError> {
            self.groups.render(ids, pass)
        }
        fn validate(&self, request: &RenderedRequest, ids: &[GroupId]) -> Result<(), FitError> {
            self.validations.fetch_add(1, Ordering::SeqCst);
            self.groups.validate(request, ids)
        }
    }
    for _ in 0..2 {
        let validations = Arc::new(AtomicUsize::new(0));
        let (out, calls) = run(
            Box::new(Validations {
                groups: groups(),
                validations: validations.clone(),
            }),
            ready(),
            |s| {
                if s == "A B" {
                    TokenCount::exact(11)
                } else {
                    exact(s)
                }
            },
            scored,
        );
        assert_eq!(out.receipt.groups_kept, ["A"]);
        assert_eq!(calls, ["A B C", "A", "A B"]);
        assert_eq!(validations.load(Ordering::SeqCst), 3);
    }
    let mut session = FitSession::new(Box::new(groups()), ready()).unwrap();
    let action = session.advance(None).unwrap();
    let mut reply = counted(action, TokenCount::exact(14));
    let FitReply::Counted { counter_id, .. } = &mut reply else {
        panic!()
    };
    *counter_id = "changed-profile".into();
    assert!(matches!(
        session.advance(Some(reply)),
        Err(FitError::Accounting(_))
    ));
}
#[test]
fn selected_candidates_are_bounded_and_empty_output_never_commits() {
    let mut input = groups();
    input.0[0].required = false;
    let (out, calls) = run(
        Box::new(input),
        FitOptions {
            target_tokens: 100,
            ..ready()
        },
        |s| TokenCount::exact(if s.is_empty() { 0 } else { 200 }),
        |id, groups, scorer| FitReply::Scored {
            action_id: id,
            scorer,
            scores: groups
                .into_iter()
                .map(|g| GroupScore {
                    id: g.id,
                    score: 1.,
                })
                .collect(),
        },
    );
    assert_eq!(out.receipt.reason, Some(FitReason::NoValidFit));
    assert_eq!(out.payload, RenderedPayload::Text("A B C".into()));
    assert_eq!(out.receipt.final_tokens, Some(200));
    assert!(!out.receipt.selection_applied);
    assert_eq!(calls, ["A B C", "", "A B", "A"]);
}
#[test]
fn stale_and_duplicate_score_replies_remain_protocol_errors() {
    let mut session = FitSession::new(Box::new(groups()), ready()).unwrap();
    let action = session.advance(None).unwrap();
    let action = session
        .advance(Some(counted(action, TokenCount::exact(14))))
        .unwrap();
    let action = session
        .advance(Some(counted(action, TokenCount::exact(6))))
        .unwrap();
    let FitAction::Score {
        action_id,
        groups,
        scorer,
        ..
    } = action
    else {
        panic!()
    };
    assert!(matches!(
        session.advance(Some(scored(action_id + 1, groups.clone(), scorer.clone()))),
        Err(FitError::Protocol)
    ));
    let action = session
        .advance(Some(scored(action_id, groups.clone(), scorer.clone())))
        .unwrap();
    assert!(matches!(
        action,
        FitAction::Count {
            purpose: CountPurpose::Selected,
            ..
        }
    ));
    assert!(matches!(
        session.advance(Some(scored(action_id, groups, scorer))),
        Err(FitError::Protocol)
    ));
}
#[test]
fn required_and_final_accounting_changes_preserve_returned_baseline_basis() {
    for (mode, baseline, candidate, preflight) in [
        (
            BudgetMode::Strict,
            CountBasis::Exact,
            CountBasis::Unavailable,
            true,
        ),
        (
            BudgetMode::Strict,
            CountBasis::Exact,
            CountBasis::Estimated,
            true,
        ),
        (
            BudgetMode::BestEffort,
            CountBasis::Estimated,
            CountBasis::Exact,
            true,
        ),
        (
            BudgetMode::BestEffort,
            CountBasis::Estimated,
            CountBasis::Unavailable,
            false,
        ),
    ] {
        let (out, _) = run(
            Box::new(groups()),
            FitOptions {
                budget_mode: mode,
                ..ready()
            },
            |t| {
                let basis = if t == if preflight { "A" } else { "A B" } {
                    candidate
                } else {
                    baseline
                };
                TokenCount {
                    tokens: if basis == CountBasis::Unavailable {
                        None
                    } else {
                        exact(t).tokens
                    },
                    basis,
                }
            },
            |id, g, p| {
                assert!(!preflight);
                scored(id, g, p)
            },
        );
        assert_eq!(out.receipt.reason, Some(FitReason::UnknownAccounting));
        assert_eq!(out.receipt.final_tokens, Some(14));
        assert_eq!(out.receipt.accounting.final_basis, baseline);
        assert_eq!(out.receipt.scorer_succeeded, !preflight);
    }
}
#[test]
fn unsafe_selected_splice_is_rejected_after_scores_before_a_third_counter_call() {
    use std::cell::Cell;
    let prefix = format!("Bearer{}", " ".repeat(300));
    let suffix = "synthetic_token_1234567890";
    let guide = guide_estimator().unwrap();
    let target = (guide.count_bytes(b"q")
        + guide.count_bytes(prefix.as_bytes())
        + guide.count_bytes(suffix.as_bytes())) as u64;
    let input = ContextRequest::new(
        "q".into(),
        vec![
            ContextGroup {
                id: "prefix".into(),
                content: GroupContent::Text(prefix),
                required: false,
            },
            ContextGroup {
                id: "separator".into(),
                content: GroupContent::Text("!".into()),
                required: false,
            },
            ContextGroup {
                id: "suffix".into(),
                content: GroupContent::Text(suffix.into()),
                required: true,
            },
        ],
    )
    .unwrap();
    struct Counter(Cell<usize>);
    impl TokenCounter for Counter {
        fn id(&self) -> &str {
            "test:selection:v1"
        }
        fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
            let RenderedPayload::Text(text) = &request.payload else {
                panic!()
            };
            assert!(
                !text.contains("Bearer") || text.contains('!'),
                "unsafe selected splice entered counter"
            );
            self.0.set(self.0.get() + 1);
            Ok(TokenCount::exact(if text.contains('!') { 200 } else { 1 }))
        }
    }
    let counter = Counter(Cell::new(0));
    let mut session = FitSession::new(
        Box::new(input),
        FitOptions {
            target_tokens: target,
            ..ready()
        },
    )
    .unwrap();
    let FitAction::Score {
        action_id,
        groups,
        scorer,
        ..
    } = session.drive(&counter, None).unwrap()
    else {
        panic!()
    };
    let scores = groups
        .into_iter()
        .map(|g| GroupScore {
            score: if g.id == "prefix" { 2. } else { 1. },
            id: g.id,
        })
        .collect();
    assert!(matches!(
        session.drive(
            &counter,
            Some(FitReply::Scored {
                action_id,
                scores,
                scorer
            })
        ),
        Err(FitError::Safety)
    ));
    assert_eq!(counter.0.get(), 2);
}
