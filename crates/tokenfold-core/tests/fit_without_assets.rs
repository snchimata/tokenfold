#![cfg(not(feature = "tiktoken"))]
use tokenfold_core::fit::*;

#[test]
fn missing_assets_raise_accounting_error_without_a_heuristic() {
    assert!(matches!(
        EncodingCounter::new("o200k_base"),
        Err(FitError::Accounting(_))
    ));
    let request = TextRequest::new("ordinary text".into()).unwrap();
    assert!(matches!(
        FitSession::new(Box::new(request), FitOptions::default()),
        Err(FitError::Accounting(_))
    ));
}
