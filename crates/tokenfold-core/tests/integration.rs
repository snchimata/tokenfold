//! End-to-end pipeline scenarios (ENGINEERING.md "Integration Tests"). Unlike `golden.rs`
//! (byte-exact single-transform fixtures) and the inline unit tests (isolated module
//! behavior), these exercise `compress`/`compress_with_estimator` as a whole.

use tokenfold_core::report::{TransformStatus, WarningCode};
use tokenfold_core::status::Status;
use tokenfold_core::{CompressionInput, CompressionMode, CompressionPolicy};

#[test]
fn multi_transform_pipeline_applies_json_minify_and_schema_compaction() {
    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "Looks something up.",
                "parameters": {"type": "object", "properties": {"q": {"type": "string"}}},
                "examples": ["a", "b", "c", "d", "e"]
            }
        }]
    });
    let bytes = format!("{payload:#}").into_bytes(); // pretty-printed, so json_minify has work to do
    let input = CompressionInput::openai_json(bytes);
    let policy = CompressionPolicy::builder().build().unwrap();

    let output = tokenfold_core::compress(input, &policy).unwrap();

    let ids: Vec<&str> = output
        .report
        .transforms
        .iter()
        .map(|t| t.id.as_str())
        .collect();
    assert!(ids.contains(&"json_minify"));
    assert!(ids.contains(&"schema_compaction"));

    let value: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
    let examples = value["tools"][0]["function"]["examples"]
        .as_array()
        .unwrap();
    assert_eq!(examples.len(), 1, "schema_compaction should cap examples");
}

#[test]
fn conservative_mode_preserves_tool_description_byte_for_byte() {
    let description = "Looks up account balance by customer ID — never guess the balance.";
    let payload = serde_json::json!({
        "messages": [{"role": "user", "content": "hi"}],
        "tools": [{
            "type": "function",
            "function": {
                "name": "lookup_balance",
                "description": description,
                "parameters": {"type": "object"},
                "examples": ["a", "b", "c"]
            }
        }]
    });
    let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
    let policy = CompressionPolicy::builder()
        .mode(CompressionMode::Conservative)
        .build()
        .unwrap();

    let output = tokenfold_core::compress(input, &policy).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
    assert_eq!(
        value["tools"][0]["function"]["description"].as_str(),
        Some(description)
    );
}

#[test]
fn protected_content_survives_a_multiturn_conversation() {
    let early_fact = "the account ID is ACC-88213";
    let payload = serde_json::json!({
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": early_fact},
            {"role": "assistant", "content": "Got it, noted."},
            {"role": "user", "content": "What was the account ID again?"},
        ]
    });
    let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
    let policy = CompressionPolicy::builder().build().unwrap();

    let output = tokenfold_core::compress(input, &policy).unwrap();
    let text = String::from_utf8_lossy(&output.bytes);
    assert!(
        text.contains("You are a helpful assistant."),
        "system turn must survive"
    );
    assert!(
        text.contains("What was the account ID again?"),
        "latest user turn must survive"
    );
}

#[test]
fn unreachable_target_below_floor_keeps_protected_content_and_reports_status() {
    let payload = serde_json::json!({
        "messages": [
            {"role": "system", "content": "You must always answer in French."},
            {"role": "user", "content": "bonjour"},
        ]
    });
    let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
    let policy = CompressionPolicy::builder()
        .target_tokens(1)
        .build()
        .unwrap();

    let output = tokenfold_core::compress(input, &policy).unwrap();
    assert_eq!(output.report.status, Status::UnreachableTarget);
    let text = String::from_utf8_lossy(&output.bytes);
    assert!(text.contains("You must always answer in French."));
    let budget = output.report.budget.expect("budget populated");
    assert!(budget.achieved_tokens >= budget.protected_floor);
}

#[test]
fn schema_compaction_is_rolled_back_when_it_would_alter_protected_system_content() {
    // The system message's content is a *structured* JSON value (not a plain string) that
    // itself contains an "examples" array. schema_compaction shortens every "examples" array
    // in the document, including this one — which would corrupt protected system content.
    // The pipeline must roll that specific application back rather than ship a corrupted
    // protected segment.
    let payload = serde_json::json!({
        "messages": [
            {
                "role": "system",
                "content": {"policy": "always cite sources", "examples": [1, 2, 3, 4, 5]}
            },
            {"role": "user", "content": "question"},
        ]
    });
    let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
    let policy = CompressionPolicy::builder().build().unwrap();

    let output = tokenfold_core::compress(input, &policy).unwrap();

    let schema_report = output
        .report
        .transforms
        .iter()
        .find(|t| t.id == "schema_compaction")
        .expect("schema_compaction attempted");
    assert_eq!(schema_report.status, TransformStatus::RolledBack);
    assert!(
        output
            .report
            .warnings
            .iter()
            .any(|w| w.code == WarningCode::SafetyDowngrade)
    );

    // The protected system content's full original "examples" array must survive untouched.
    let value: serde_json::Value = serde_json::from_slice(&output.bytes).unwrap();
    let system_examples = value["messages"][0]["content"]["examples"]
        .as_array()
        .unwrap();
    assert_eq!(system_examples.len(), 5);
}

#[test]
fn redaction_runs_before_any_report_field_is_populated() {
    let secret = "sk-abcdEFGH1234567890123456";
    let input = CompressionInput::plain_text(format!("api key: {secret}").into_bytes());
    let policy = CompressionPolicy::builder().build().unwrap();

    let output = tokenfold_core::compress(input, &policy).unwrap();

    assert!(!contains(&output.bytes, secret.as_bytes()));
    for warning in &output.report.warnings {
        assert!(!warning.message.contains(secret));
    }
    let debug_repr = format!("{:?}", output.report);
    assert!(!debug_repr.contains(secret));
}

#[test]
fn mode_matrix_fixture_mirrors_the_rust_source_of_truth() {
    use tokenfold_core::InputFormat;
    use tokenfold_core::modes::ALL_ENTRIES;

    #[derive(serde::Deserialize)]
    struct MatrixFile {
        transforms: Vec<MatrixEntry>,
    }

    #[derive(serde::Deserialize)]
    struct MatrixEntry {
        id: String,
        version: String,
        conservative_enabled: bool,
        balanced_enabled: bool,
        aggressive_enabled: bool,
        experimental: bool,
        max_ratio_conservative: f64,
        max_ratio_balanced: f64,
        max_ratio_aggressive: f64,
        applicable_formats: Vec<String>,
    }

    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("tests/fixtures/mode_matrix.toml");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!("failed to read {}: {e}", path.display());
    });
    let matrix: MatrixFile = toml::from_str(&raw).expect("valid mode_matrix.toml");

    assert_eq!(matrix.transforms.len(), ALL_ENTRIES.len());
    for fixture in &matrix.transforms {
        let entry = ALL_ENTRIES
            .iter()
            .find(|e| e.transform_id.as_str() == fixture.id)
            .unwrap_or_else(|| panic!("modes.rs has no entry for fixture id {}", fixture.id));
        assert_eq!(entry.version, fixture.version, "{}: version", fixture.id);
        assert_eq!(
            entry.conservative_enabled, fixture.conservative_enabled,
            "{}: conservative_enabled",
            fixture.id
        );
        assert_eq!(
            entry.balanced_enabled, fixture.balanced_enabled,
            "{}: balanced_enabled",
            fixture.id
        );
        assert_eq!(
            entry.aggressive_enabled, fixture.aggressive_enabled,
            "{}: aggressive_enabled",
            fixture.id
        );
        assert_eq!(
            entry.experimental, fixture.experimental,
            "{}: experimental",
            fixture.id
        );
        assert_eq!(
            entry.max_ratio_conservative, fixture.max_ratio_conservative,
            "{}: max_ratio_conservative",
            fixture.id
        );
        assert_eq!(
            entry.max_ratio_balanced, fixture.max_ratio_balanced,
            "{}: max_ratio_balanced",
            fixture.id
        );
        assert_eq!(
            entry.max_ratio_aggressive, fixture.max_ratio_aggressive,
            "{}: max_ratio_aggressive",
            fixture.id
        );

        let format_from_str = |s: &str| match s {
            "openai_json" => InputFormat::OpenAiJson,
            "anthropic_json" => InputFormat::AnthropicJson,
            "plain_text" => InputFormat::PlainText,
            "command_output" => InputFormat::CommandOutput,
            "git_diff" => InputFormat::GitDiff,
            other => panic!("unknown format {other} in fixture"),
        };
        for format_str in &fixture.applicable_formats {
            let format = format_from_str(format_str);
            assert!(
                entry.applicable_formats.contains(&format),
                "{}: modes.rs is missing applicable_format {format_str}",
                fixture.id
            );
        }
        assert_eq!(
            entry.applicable_formats.len(),
            fixture.applicable_formats.len(),
            "{}: applicable_formats length mismatch",
            fixture.id
        );
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
