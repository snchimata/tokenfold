use serde_json::{Value, json};
use tokenfold_adapters::fit_request::{MessageGroup, PreparedRequest, ProviderCounter};
use tokenfold_core::fit::*;
fn fixture(name: &str) -> Value {
    serde_json::from_str(
        &std::fs::read_to_string(format!("../../tests/fixtures/sdk-fit/{name}.json")).unwrap(),
    )
    .unwrap()
}
#[test]
fn provider_source_projections_and_survivors_are_pinned() {
    for name in ["openai", "anthropic"] {
        let f = fixture(name);
        let payload = f["payload"].clone();
        let groups = vec![MessageGroup {
            id: "history".into(),
            indices: vec![1, 2],
            required: false,
        }];
        let request = if name == "openai" {
            PreparedRequest::openai(payload.clone(), Some("active".into()), groups)
        } else {
            PreparedRequest::anthropic(payload.clone(), Some("active".into()), groups)
        }
        .unwrap();
        let source = request.source_groups();
        assert_eq!(
            source.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
            f["source_ids"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            source
                .iter()
                .map(|g| g.scoring_text.clone())
                .collect::<Vec<_>>(),
            f["scoring_texts"]
                .as_array()
                .unwrap()
                .iter()
                .map(|s| s.as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        );
        let ids = source
            .iter()
            .filter(|g| g.required)
            .map(|g| g.id.clone())
            .collect::<Vec<_>>();
        let candidate = request.render(&ids, RenderPass::CoreCompatible).unwrap();
        request.validate(&candidate, &ids).unwrap();
        assert_eq!(
            candidate.payload,
            RenderedPayload::Json(f["required_payload"].clone())
        );
        assert_eq!(payload, f["payload"]);
    }
}
#[test]
fn profile_is_estimated_and_scope_is_closed() {
    let f = fixture("provider_estimates");
    let c = ProviderCounter::openai("gpt-4o-2024-08-06").unwrap();
    assert_eq!(c.id(), f["counter_id"].as_str().unwrap());
    for case in f["cases"].as_array().unwrap() {
        let request = RenderedRequest {
            format: FitFormat::OpenaiChat,
            scope: AccountingScope::ProviderRequest,
            payload: RenderedPayload::Json(case["payload"].clone()),
            core_transforms: vec![],
        };
        let count = c.count(&request).unwrap();
        assert_eq!(serde_json::to_value(count).unwrap(), case["count"]);
    }
    let mut text = TextRequest::new("hi".into())
        .unwrap()
        .render(&[], RenderPass::Original)
        .unwrap();
    assert_eq!(c.count(&text).unwrap(), TokenCount::unavailable());
    text.scope = AccountingScope::ProviderRequest;
    assert_eq!(c.count(&text).unwrap(), TokenCount::unavailable());
    assert!(ProviderCounter::openai("unverified-model").is_err());
}
#[test]
fn message_declarations_reject_protected_overlap_interleaving_and_reserved_ids() {
    let f = fixture("openai");
    for groups in [
        vec![MessageGroup {
            id: "protected".into(),
            indices: vec![0],
            required: false,
        }],
        vec![MessageGroup {
            id: "latest".into(),
            indices: vec![3],
            required: false,
        }],
        vec![MessageGroup {
            id: "gap".into(),
            indices: vec![1, 3],
            required: true,
        }],
        vec![MessageGroup {
            id: "reversed".into(),
            indices: vec![2, 1],
            required: true,
        }],
        vec![
            MessageGroup {
                id: "one".into(),
                indices: vec![1, 2],
                required: false,
            },
            MessageGroup {
                id: "two".into(),
                indices: vec![2],
                required: false,
            },
        ],
        vec![MessageGroup {
            id: "__tf_forbidden".into(),
            indices: vec![1],
            required: false,
        }],
        vec![MessageGroup {
            id: "empty".into(),
            indices: vec![],
            required: true,
        }],
        vec![MessageGroup {
            id: "outside".into(),
            indices: vec![4],
            required: true,
        }],
        vec![
            MessageGroup {
                id: "dup".into(),
                indices: vec![1],
                required: false,
            },
            MessageGroup {
                id: "dup".into(),
                indices: vec![2],
                required: false,
            },
        ],
    ] {
        assert!(matches!(
            PreparedRequest::openai(f["payload"].clone(), Some("active".into()), groups),
            Err(FitError::InvalidInput(_))
        ));
    }
    let optional: MessageGroup =
        serde_json::from_value(json!({"id":"history","indices":[1,2]})).unwrap();
    assert!(!optional.required);
    let protected: MessageGroup =
        serde_json::from_value(json!({"id":"rules","indices":[0]})).unwrap();
    assert!(PreparedRequest::openai(f["payload"].clone(), None, vec![protected]).is_err());
    let request = PreparedRequest::openai(f["payload"].clone(), None, vec![optional]).unwrap();
    let ids = request
        .source_groups()
        .iter()
        .filter(|g| g.required)
        .map(|g| g.id.clone())
        .collect::<Vec<_>>();
    let mut candidate = request.render(&ids, RenderPass::Original).unwrap();
    let RenderedPayload::Json(payload) = &mut candidate.payload else {
        panic!()
    };
    payload["messages"][0]["content"] = json!("changed");
    assert!(request.validate(&candidate, &ids).is_err());
    assert!(
        request
            .render(&["history".into()], RenderPass::Original)
            .is_err()
    );
}
fn pin() -> ScorerInfo {
    ScorerInfo {
        revision: "revision".into(),
        manifest_hash: "manifest".into(),
        runtime_id: "runtime".into(),
        device: "cpu".into(),
        precision: "fp32".into(),
        qualification: "unqualified".into(),
    }
}
#[test]
fn every_provider_candidate_counter_receives_the_complete_object() {
    let f = fixture("openai");
    let original = f["payload"].clone();
    let input = PreparedRequest::openai(
        original.clone(),
        Some("active".into()),
        vec![MessageGroup {
            id: "history".into(),
            indices: vec![1, 2],
            required: false,
        }],
    )
    .unwrap();
    struct Counter(Value);
    impl TokenCounter for Counter {
        fn id(&self) -> &str {
            "provider:whole:v1"
        }
        fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
            assert_eq!(request.scope, AccountingScope::ProviderRequest);
            let RenderedPayload::Json(payload) = &request.payload else {
                panic!()
            };
            for (k, v) in self.0.as_object().unwrap() {
                if k != "messages" {
                    assert_eq!(&payload[k], v);
                }
            }
            assert_eq!(payload["messages"][0], self.0["messages"][0]);
            assert_eq!(
                payload["messages"].as_array().unwrap().last(),
                self.0["messages"].as_array().unwrap().last()
            );
            Ok(TokenCount::exact(
                if payload["messages"].as_array().unwrap().len() == 4 {
                    100
                } else {
                    10
                },
            ))
        }
    }
    let counter = Counter(original.clone());
    let mut session = FitSession::new(
        Box::new(input),
        FitOptions {
            target_tokens: 10,
            counter_id: counter.id().into(),
            allow_selection: true,
            select: Some(pin()),
            ..FitOptions::default()
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
        panic!("score")
    };
    assert_eq!(
        groups.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(),
        ["__tf_message_0", "history", "__tf_message_3"]
    );
    let scores = groups
        .iter()
        .map(|g| GroupScore {
            id: g.id.clone(),
            score: 1.,
        })
        .collect();
    let FitAction::Done(out) = session
        .drive(
            &counter,
            Some(FitReply::Scored {
                action_id,
                scores,
                scorer,
            }),
        )
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(
        out.payload,
        RenderedPayload::Json(f["required_payload"].clone())
    );
    assert_eq!(out.receipt.final_tokens, Some(10));
    assert!(out.receipt.selection_applied);
    assert_eq!(original, f["payload"]);
}
#[test]
fn tools_and_media_return_baseline_and_never_score() {
    for name in ["openai", "anthropic"] {
        for media in [false, true] {
            let mut payload = fixture(name)["payload"].clone();
            if media {
                payload["messages"][2]["content"] = json!([{"type":"text","text":"block"}]);
            } else {
                payload["tools"] = json!([]);
            }
            let decl = vec![MessageGroup {
                id: "old".into(),
                indices: vec![1],
                required: false,
            }];
            let input = if name == "openai" {
                PreparedRequest::openai(payload.clone(), Some("query".into()), decl)
            } else {
                PreparedRequest::anthropic(payload.clone(), Some("query".into()), decl)
            }
            .unwrap();
            assert!(!input.selection_supported());
            let mut session = FitSession::new(
                Box::new(input),
                FitOptions {
                    target_tokens: 0,
                    counter_id: "test".into(),
                    allow_selection: true,
                    select: Some(pin()),
                    ..FitOptions::default()
                },
            )
            .unwrap();
            let FitAction::Count {
                candidate_id,
                request,
                ..
            } = session.advance(None).unwrap()
            else {
                panic!()
            };
            let FitAction::Done(out) = session
                .advance(Some(FitReply::Counted {
                    candidate_id,
                    counter_id: "test".into(),
                    scope: request.scope,
                    count: TokenCount::exact(100),
                }))
                .unwrap()
            else {
                panic!()
            };
            assert_eq!(out.payload, RenderedPayload::Json(payload));
            assert_eq!(out.receipt.reason, Some(FitReason::UnsupportedSelection));
            assert!(!out.receipt.scorer_attempted);
        }
    }
}
#[test]
fn provider_profile_field_allowlists_are_pinned_and_unknown_prompt_fields_fail_closed() {
    use tokenfold_adapters::fit_request::{OPENAI_MESSAGE_FIELDS, OPENAI_REQUEST_FIELDS};
    let f = fixture("provider_estimates");
    assert_eq!(
        serde_json::to_value(OPENAI_REQUEST_FIELDS).unwrap(),
        f["supported_request_fields"]
    );
    assert_eq!(
        serde_json::to_value(OPENAI_MESSAGE_FIELDS).unwrap(),
        f["supported_message_fields"]
    );
    let counter = ProviderCounter::openai("gpt-4o-2024-08-06").unwrap();
    for field in OPENAI_REQUEST_FIELDS
        .iter()
        .filter(|f| !["model", "messages"].contains(f))
    {
        let mut payload = f["cases"][0]["payload"].clone();
        payload[*field] = Value::Null;
        let request = RenderedRequest {
            format: FitFormat::OpenaiChat,
            scope: AccountingScope::ProviderRequest,
            payload: RenderedPayload::Json(payload),
            core_transforms: vec![],
        };
        assert_eq!(counter.count(&request).unwrap(), TokenCount::estimated(8));
    }
}
#[test]
fn provider_requires_query_only_when_selection_is_needed_and_respects_consent() {
    let f = fixture("openai");
    for (allow, target, expected) in [
        (false, 10, Some(FitReason::SelectionDisabled)),
        (true, 100, None),
    ] {
        let input = PreparedRequest::openai(
            f["payload"].clone(),
            None,
            vec![MessageGroup {
                id: "history".into(),
                indices: vec![1, 2],
                required: false,
            }],
        )
        .unwrap();
        let mut session = FitSession::new(
            Box::new(input),
            FitOptions {
                target_tokens: target,
                counter_id: "test".into(),
                allow_selection: allow,
                select: Some(pin()),
                ..FitOptions::default()
            },
        )
        .unwrap();
        let FitAction::Count {
            candidate_id,
            request,
            ..
        } = session.advance(None).unwrap()
        else {
            panic!()
        };
        let FitAction::Done(out) = session
            .advance(Some(FitReply::Counted {
                candidate_id,
                counter_id: "test".into(),
                scope: request.scope,
                count: TokenCount::exact(100),
            }))
            .unwrap()
        else {
            panic!()
        };
        assert_eq!(out.receipt.reason, expected);
        assert_eq!(out.payload, RenderedPayload::Json(f["payload"].clone()));
        assert!(out.receipt.warnings.is_empty());
    }
    let input = PreparedRequest::openai(
        f["payload"].clone(),
        None,
        vec![MessageGroup {
            id: "history".into(),
            indices: vec![1, 2],
            required: false,
        }],
    )
    .unwrap();
    let mut session = FitSession::new(
        Box::new(input),
        FitOptions {
            target_tokens: 10,
            counter_id: "test".into(),
            allow_selection: true,
            select: Some(pin()),
            ..FitOptions::default()
        },
    )
    .unwrap();
    let FitAction::Count {
        candidate_id,
        request,
        ..
    } = session.advance(None).unwrap()
    else {
        panic!()
    };
    assert!(matches!(
        session.advance(Some(FitReply::Counted {
            candidate_id,
            counter_id: "test".into(),
            scope: request.scope,
            count: TokenCount::exact(100)
        })),
        Err(FitError::InvalidInput(_))
    ));
}
#[test]
fn native_provider_profile_driver_never_requires_a_host_count_callback() {
    let counter = ProviderCounter::openai("gpt-4o-2024-08-06").unwrap();
    let payload = fixture("provider_estimates")["cases"][0]["payload"].clone();
    let input = PreparedRequest::openai(payload, None, vec![]).unwrap();
    let mut session = FitSession::new(
        Box::new(input),
        FitOptions {
            target_tokens: 8,
            budget_mode: BudgetMode::BestEffort,
            counter_id: counter.id().into(),
            ..FitOptions::default()
        },
    )
    .unwrap();
    let FitAction::Done(out) = session.drive(&counter, None).unwrap() else {
        panic!("native counts must not yield Count")
    };
    assert_eq!(out.receipt.final_tokens, Some(8));
    assert_eq!(out.receipt.reason, Some(FitReason::EstimatedFit));
    assert_eq!(out.receipt.budget_status, BudgetStatus::Indeterminate);
}
#[test]
fn malformed_provider_structure_and_non_text_system_are_not_silently_flattened() {
    let mut payload = fixture("anthropic")["payload"].clone();
    payload["system"] = json!([{"type":"text","text":"rules"}]);
    let prepared = PreparedRequest::anthropic(payload, None, vec![]).unwrap();
    assert!(!prepared.selection_supported());
    for value in [
        Value::Null,
        json!([]),
        json!({}),
        json!({"model":"m","messages":[]}),
        json!({"model":"m","messages":[{"role":"invalid","content":"x"}]}),
    ] {
        assert!(PreparedRequest::openai(value, None, vec![]).is_err());
    }
    let mut payload = fixture("anthropic")["payload"].clone();
    payload["system"] = json!(7);
    assert!(matches!(
        PreparedRequest::anthropic(payload, None, vec![]),
        Err(FitError::InvalidInput(_))
    ));
}

fn audio_payload(content: Value) -> Value {
    json!({
        "model":"gpt-4o-2024-08-06",
        "messages":[
            {"role":"assistant","content":content,"audio":{"id":"audio_previous"}},
            {"role":"assistant","content":"optional text"},
            {"role":"user","content":"latest question"}
        ],
        "unknown_field":"preserved"
    })
}
#[test]
fn message_audio_with_string_content_cannot_be_declared_optional() {
    let result = PreparedRequest::openai(
        audio_payload(json!("spoken answer")),
        Some("active query".into()),
        vec![MessageGroup {
            id: "audio_history".into(),
            indices: vec![0],
            required: false,
        }],
    );
    assert!(matches!(result, Err(FitError::InvalidInput(_))));
}
fn assert_audio_baseline(content: Value) {
    use std::cell::Cell;
    let payload = audio_payload(content);
    let request = PreparedRequest::openai(
        payload.clone(),
        Some("active query".into()),
        vec![MessageGroup {
            id: "text_history".into(),
            indices: vec![1],
            required: false,
        }],
    )
    .unwrap();
    struct Counter {
        expected: Value,
        calls: Cell<usize>,
    }
    impl TokenCounter for Counter {
        fn id(&self) -> &str {
            "audio:complete:v1"
        }
        fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
            assert_eq!(request.scope, AccountingScope::ProviderRequest);
            assert_eq!(
                request.payload,
                RenderedPayload::Json(self.expected.clone())
            );
            self.calls.set(self.calls.get() + 1);
            Ok(TokenCount::exact(100))
        }
    }
    let counter = Counter {
        expected: payload.clone(),
        calls: Cell::new(0),
    };
    let mut session = FitSession::new(
        Box::new(request),
        FitOptions {
            target_tokens: 1,
            counter_id: counter.id().into(),
            allow_selection: true,
            select: Some(pin()),
            ..FitOptions::default()
        },
    )
    .unwrap();
    let FitAction::Done(output) = session.drive(&counter, None).unwrap() else {
        panic!("audio must return its baseline without Score")
    };
    assert_eq!(output.payload, RenderedPayload::Json(payload));
    assert_eq!(output.receipt.reason, Some(FitReason::UnsupportedSelection));
    assert_eq!(output.receipt.final_tokens, Some(100));
    assert_eq!(output.receipt.accounting.final_basis, CountBasis::Exact);
    assert_eq!(
        output.receipt.groups_kept,
        ["__tf_message_0", "text_history", "__tf_message_2"]
    );
    assert!(output.receipt.groups_dropped.is_empty());
    assert!(!output.receipt.selection_applied);
    assert!(!output.receipt.scorer_attempted);
    assert_eq!(counter.calls.get(), 1);
}
#[test]
fn required_message_audio_disables_selection_of_other_optional_text() {
    assert_audio_baseline(json!("spoken answer"));
}
#[test]
fn nullable_message_audio_survives_complete_object_accounting() {
    assert_audio_baseline(Value::Null);
}
