//! Explicitly experimental generated text, never semantic admission or a default transform.
use crate::TokenFoldError;
use crate::token_estimator::TokenEstimator;
use crate::transforms::redaction::contains_secret;
use std::collections::HashSet;

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SourceGroup {
    pub id: String,
    pub text: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    #[serde(default)]
    pub prefix: String,
    pub groups: Vec<SourceGroup>,
    #[serde(default)]
    pub suffix: String,
}

impl Source {
    /// Validate decoded content before any model export, including split-secret bypasses.
    pub fn baseline(&self, query: &str) -> Result<String, TokenFoldError> {
        let mut ids = HashSet::new();
        if query.trim().is_empty()
            || query.len() > 4096
            || self.groups.is_empty()
            || self.groups.len() > 512
            || self
                .groups
                .iter()
                .any(|g| g.id.trim().is_empty() || g.id.len() > 256 || !ids.insert(&g.id))
        {
            return Err(TokenFoldError::InvalidInput(
                "invalid summarizer source or query".into(),
            ));
        }
        let mut text = self.prefix.clone();
        for g in &self.groups {
            text.push_str(&g.text);
        }
        text.push_str(&self.suffix);
        let encoded = serde_json::to_vec(self)
            .map_err(|_| TokenFoldError::InvalidInput("invalid summarizer source".into()))?;
        if text.len() > 1024 * 1024 || encoded.len() > 2 * 1024 * 1024 {
            return Err(TokenFoldError::InvalidInput(
                "summarizer source exceeds byte limit".into(),
            ));
        }
        if contains_secret(text.as_bytes())
            || contains_secret(query.as_bytes())
            || contains_secret(&encoded)
        {
            return Err(TokenFoldError::SafetyViolation(
                "secret-shaped summarizer input refused".into(),
            ));
        }
        Ok(text)
    }
}

/// Source-only admission avoids paying for a generation that cannot be useful.
/// Call after `Source::baseline` validation; fallback is always the exact baseline.
pub fn skip_generation(
    source: &Source,
    baseline: &str,
    target: usize,
    estimator: &dyn TokenEstimator,
) -> Option<&'static str> {
    if !source.groups.iter().any(|g| !g.required) {
        return Some("no_optional_groups");
    }
    let mut protected = source.prefix.clone();
    for group in source.groups.iter().filter(|g| g.required) {
        protected.push_str(&group.text);
    }
    protected.push_str(&source.suffix);
    if estimator.count_bytes(protected.as_bytes()) >= target {
        return Some("protected_budget");
    }
    if estimator.count_bytes(baseline.as_bytes()) <= target {
        return Some("already_within_budget");
    }
    None
}

/// Only fixed runtime failures may cross into receipts; rejected model text never does.
#[derive(Debug, Clone, Copy, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationFailure {
    IncompleteResponse,
    ModelMismatch,
    MalformedCandidate,
    HttpClientError,
    HttpServerError,
    HttpUnexpectedStatus,
    TransportFailed,
    RuntimeTimeout,
    InvalidRuntimeResponse,
}
impl GenerationFailure {
    pub fn reason(self) -> &'static str {
        match self {
            Self::IncompleteResponse => "incomplete_response",
            Self::ModelMismatch => "model_mismatch",
            Self::MalformedCandidate => "malformed_candidate",
            Self::HttpClientError => "http_client_error",
            Self::HttpServerError => "http_server_error",
            Self::HttpUnexpectedStatus => "http_unexpected_status",
            Self::TransportFailed => "transport_failed",
            Self::RuntimeTimeout => "runtime_timeout",
            Self::InvalidRuntimeResponse => "invalid_runtime_response",
        }
    }
}

/// Counters reported by the approved runtime, not the local estimator or verified billing.
#[derive(Debug, Clone, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct InferenceUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub schema_version: u32,
    pub model_revision: String,
    pub summary: String,
    pub source_ids: Vec<String>,
    #[serde(default)]
    pub inference_usage: Option<InferenceUsage>,
    #[serde(default)]
    pub generation_failure: Option<GenerationFailure>,
}

/// Explicit caller-owned replay artifact; not a cache of source text or a semantic proof.
#[derive(serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryArtifact {
    version: String,
    source_sha256: String,
    query_sha256: String,
    approval_sha256: String,
    target_tokens: usize,
    estimator: serde_json::Value,
    pub candidate: Candidate,
}

impl SummaryArtifact {
    pub fn new(
        source: &Source,
        query: &str,
        approval: &[u8],
        target: usize,
        estimator: &dyn TokenEstimator,
        candidate: Candidate,
    ) -> Self {
        use crate::retrieval_store::hex_sha256;
        Self {
            version: "tokenfold-generative-artifact-v1".into(),
            source_sha256: hex_sha256(&serde_json::to_vec(source).expect("source serializes")),
            query_sha256: hex_sha256(query.as_bytes()),
            approval_sha256: hex_sha256(approval),
            target_tokens: target,
            estimator: serde_json::to_value(estimator.info()).expect("estimator serializes"),
            candidate,
        }
    }

    /// Reuse only for exactly matching authorized inputs and accounting context.
    /// The caller must still run `compile` to apply CURRENT evidence/safety/budget gates.
    pub fn matches(
        &self,
        source: &Source,
        query: &str,
        approval: &[u8],
        target: usize,
        estimator: &dyn TokenEstimator,
    ) -> bool {
        use crate::retrieval_store::hex_sha256;
        self.version == "tokenfold-generative-artifact-v1"
            && self.source_sha256
                == hex_sha256(&serde_json::to_vec(source).expect("source serializes"))
            && self.query_sha256 == hex_sha256(query.as_bytes())
            && self.approval_sha256 == hex_sha256(approval)
            && self.target_tokens == target
            && self.estimator
                == serde_json::to_value(estimator.info()).expect("estimator serializes")
    }
}

/// Compile literal evidence in source order. Attribution does NOT prove entailment/completeness.
/// All protected text stays outside generation; the complete final payload is recounted.
pub fn compile(
    source: &Source,
    candidate: &Candidate,
    revision: &str,
    baseline: &str,
    target: usize,
    estimator: &dyn TokenEstimator,
) -> Result<String, &'static str> {
    if !source
        .baseline("validation")
        .is_ok_and(|text| text == baseline)
    {
        return Err("invalid_authorized_baseline");
    }
    if let Some(failure) = candidate.generation_failure {
        return Err(failure.reason());
    }
    if candidate.summary.len() > 65536 {
        return Err("oversized_candidate");
    }
    if candidate.schema_version != 1 || candidate.model_revision != revision {
        return Err("revision_or_schema_mismatch");
    }
    if candidate.summary.trim().is_empty()
        || candidate.source_ids.is_empty()
        || candidate.source_ids.len() > 512
    {
        return Err("empty_or_oversized_candidate");
    }
    let ids: HashSet<_> = candidate.source_ids.iter().map(String::as_str).collect();
    if ids
        .iter()
        .any(|id| !source.groups.iter().any(|g| !g.required && g.id == *id))
    {
        return Err("unauthorized_source_id");
    }
    let evidence: Vec<_> = source
        .groups
        .iter()
        .filter(|g| ids.contains(g.id.as_str()))
        .map(|g| serde_json::json!({"id":g.id,"text":g.text}))
        .collect();
    let wrapper =
        serde_json::json!({"summary_unverified":candidate.summary,"source_evidence":evidence})
            .to_string();
    let mut output = source.prefix.clone();
    for g in source.groups.iter().filter(|g| g.required) {
        output.push_str(&g.text);
    }
    output.push_str(&wrapper);
    output.push_str(&source.suffix);
    if output.len() > 1024 * 1024 || contains_secret(output.as_bytes()) {
        return Err("output_guard_failed");
    }
    let count = estimator.count_bytes(output.as_bytes());
    if count > target {
        return Err("over_budget");
    }
    if output.len() >= baseline.len() || count >= estimator.count_bytes(baseline.as_bytes()) {
        return Err("not_smaller");
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_estimator::ByteHeuristicEstimator;
    #[test]
    fn replay_artifact_is_bound_to_every_authorized_context() {
        let mut source = Source {
            prefix: String::new(),
            suffix: String::new(),
            groups: vec![
                SourceGroup {
                    id: "a".into(),
                    text: "A = 007".into(),
                    required: false,
                },
                SourceGroup {
                    id: "b".into(),
                    text: "noise ".repeat(100),
                    required: false,
                },
            ],
        };
        let candidate: Candidate=serde_json::from_value(serde_json::json!({"schema_version":1,"model_revision":"r1","summary":"A has code 007","source_ids":["a"]})).unwrap();
        let artifact = SummaryArtifact::new(
            &source,
            "A?",
            b"approval",
            100,
            &ByteHeuristicEstimator,
            candidate,
        );
        assert!(artifact.matches(&source, "A?", b"approval", 100, &ByteHeuristicEstimator));
        assert!(!artifact.matches(&source, "B?", b"approval", 100, &ByteHeuristicEstimator));
        assert!(!artifact.matches(&source, "A?", b"changed", 100, &ByteHeuristicEstimator));
        assert!(!artifact.matches(&source, "A?", b"approval", 101, &ByteHeuristicEstimator));
        source.groups[0].required = true;
        assert!(!artifact.matches(&source, "A?", b"approval", 100, &ByteHeuristicEstimator));
    }

    #[test]
    fn source_only_admission_and_fixed_generation_failures() {
        let mut source = Source {
            prefix: String::new(),
            suffix: String::new(),
            groups: vec![SourceGroup {
                id: "a".into(),
                text: "x".repeat(200),
                required: false,
            }],
        };
        let baseline = source.baseline("q").unwrap();
        let estimator = ByteHeuristicEstimator;
        assert_eq!(
            skip_generation(&source, &baseline, 1000, &estimator),
            Some("already_within_budget")
        );
        assert_eq!(skip_generation(&source, &baseline, 10, &estimator), None);
        source.prefix = "p".repeat(100);
        let baseline = source.baseline("q").unwrap();
        assert_eq!(
            skip_generation(&source, &baseline, 10, &estimator),
            Some("protected_budget")
        );
        source.groups[0].required = true;
        assert_eq!(
            skip_generation(&source, &baseline, 1000, &estimator),
            Some("no_optional_groups")
        );
        let candidate: Candidate=serde_json::from_value(serde_json::json!({"schema_version":1,"model_revision":"r1",
            "summary":"REJECTED MODEL TEXT","source_ids":[],"generation_failure":"incomplete_response",
            "inference_usage":{"input_tokens":100,"output_tokens":50}})).unwrap();
        assert_eq!(
            compile(&source, &candidate, "r1", &baseline, 1000, &estimator),
            Err("incomplete_response")
        );
        let invalid = serde_json::json!({"schema_version":1,"model_revision":"r1","summary":"x","source_ids":["a"],"generation_failure":"arbitrary model diagnostic"});
        assert!(serde_json::from_value::<Candidate>(invalid).is_err());
    }

    #[test]
    fn decoded_split_secrets_and_duplicate_ids_are_refused() {
        let mut source = Source {
            prefix: "sk-".into(),
            suffix: String::new(),
            groups: vec![SourceGroup {
                id: "a".into(),
                text: "x".repeat(25),
                required: false,
            }],
        };
        assert!(source.baseline("safe query").is_err());
        source.prefix.clear();
        assert!(source.baseline(&format!("sk-{}", "y".repeat(25))).is_err());
        source.groups.push(SourceGroup {
            id: "a".into(),
            text: "other".into(),
            required: false,
        });
        assert!(source.baseline("safe query").is_err());
    }

    #[test]
    fn generated_text_is_unverified_protected_evidenced_and_bounded() {
        let source = Source {
            prefix: "P".into(),
            suffix: "S".into(),
            groups: vec![
                SourceGroup {
                    id: "fixed".into(),
                    text: "MUST KEEP".into(),
                    required: true,
                },
                SourceGroup {
                    id: "a".into(),
                    text: "A = 007".into(),
                    required: false,
                },
                SourceGroup {
                    id: "b".into(),
                    text: "irrelevant ".repeat(300),
                    required: false,
                },
            ],
        };
        let baseline = source.baseline("A?").unwrap();
        let mut candidate = Candidate {
            schema_version: 1,
            model_revision: "r1".into(),
            summary: "A's code is 007".into(),
            source_ids: vec!["a".into(), "a".into()],
            inference_usage: None,
            generation_failure: None,
        };
        let output = compile(
            &source,
            &candidate,
            "r1",
            &baseline,
            1000,
            &ByteHeuristicEstimator,
        )
        .unwrap();
        assert!(output.starts_with("PMUST KEEP{"));
        assert!(output.ends_with("}S"));
        assert!(output.contains("summary_unverified") && output.contains("A = 007"));
        assert_eq!(
            compile(
                &source,
                &candidate,
                "r1",
                &baseline,
                1,
                &ByteHeuristicEstimator
            ),
            Err("over_budget")
        );
        candidate.source_ids = vec!["fixed".into()];
        assert_eq!(
            compile(
                &source,
                &candidate,
                "r1",
                &baseline,
                1000,
                &ByteHeuristicEstimator
            ),
            Err("unauthorized_source_id")
        );
        candidate.source_ids = vec!["invented".into()];
        assert!(
            compile(
                &source,
                &candidate,
                "r1",
                &baseline,
                1000,
                &ByteHeuristicEstimator
            )
            .is_err()
        );
        candidate.source_ids = vec!["a".into()];
        candidate.model_revision = "wrong".into();
        assert!(
            compile(
                &source,
                &candidate,
                "r1",
                &baseline,
                1000,
                &ByteHeuristicEstimator
            )
            .is_err()
        );
    }
}
