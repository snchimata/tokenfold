#![cfg(feature = "tiktoken")]
use tokenfold_core::fit::*;
#[test]
fn integral_json_numbers_and_signed_zero_have_one_semantic_spelling() {
    let request = DataRequest::new(
        serde_json::json!({"z":[1.0,-0.0,1.25],"a":true}),
        DataRepresentation::Json,
    )
    .unwrap();
    assert_eq!(
        request.render(&[], RenderPass::Original).unwrap().payload,
        RenderedPayload::Text("{\"z\":[1,0,1.25],\"a\":true}".into())
    );
}
#[test]
fn real_pinned_tokenizer_failure_is_accounting_not_an_unwind() {
    let counter = EncodingCounter::new("o200k_base").unwrap();
    let request = RenderedRequest {
        format: FitFormat::Text,
        payload: RenderedPayload::Text("x".repeat(MAX_SOURCE_BYTES)),
        scope: AccountingScope::RenderedText,
        core_transforms: vec![],
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| counter.count(&request)));
    assert!(matches!(result, Ok(Err(FitError::Accounting(_)))));
}
#[test]
fn guide_and_folded_core_contain_the_same_real_tokenizer_failure() {
    let text = "x".repeat(MAX_SOURCE_BYTES - 2);
    assert!(matches!(
        guide_count(text.as_bytes()),
        Err(FitError::Accounting(_))
    ));
    let request = DataRequest::new(text.clone().into(), DataRepresentation::Folded).unwrap();
    assert!(matches!(
        request.render(&[], RenderPass::CoreCompatible),
        Err(FitError::Accounting(_))
    ));
    // Size acceptance is independent of the built-in tokenizer's regex limitation.
    let request = DataRequest::new(text.into(), DataRepresentation::Json).unwrap();
    assert_eq!(
        request
            .render(&[], RenderPass::Original)
            .unwrap()
            .payload
            .bytes()
            .unwrap()
            .len(),
        MAX_SOURCE_BYTES
    );
}

#[test]
fn shared_number_fixture_has_identical_payloads_and_refuses_unsafe_values() {
    let fixture: serde_json::Value =
        serde_json::from_str(include_str!("../../../tests/fixtures/sdk-fit/numbers.json")).unwrap();
    for input in fixture["equivalent_inputs"].as_array().unwrap() {
        let request = DataRequest::new(input.clone(), DataRepresentation::Json).unwrap();
        assert_eq!(
            request.render(&[], RenderPass::Original).unwrap().payload,
            RenderedPayload::Text(fixture["canonical_json"].as_str().unwrap().into())
        );
    }
    for input in fixture["unsafe_values"].as_array().unwrap() {
        assert!(matches!(
            DataRequest::new(input.clone(), DataRepresentation::Json),
            Err(FitError::InvalidInput(_))
        ));
    }
}
