use std::path::PathBuf;

use crate::errors::TokenFoldError;
use crate::input::{CompressionInput, InputFormat};
use crate::token_estimator::TokenEstimator;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompressionPolicy {
    pub target_tokens: Option<usize>,
    pub reserve_output_tokens: usize,
    pub mode: CompressionMode,
    pub task_scope: TaskScope,
    pub cache_boundary: Option<CacheBoundary>,
    pub preserve_latest_user_message: bool,
    pub disabled: Vec<String>,
    pub unsafe_disable_redaction: bool,
    /// CLI `--experimental`: enables transforms with `ModeEntry.experimental == true`
    /// (currently `diff_compaction`; `log_compaction` was promoted out of `--experimental`
    /// after the Phase 5 fidelity gate, see `modes::ALL_ENTRIES`) at their validated ratio band.
    pub experimental: bool,
    /// CLI `--enable <id>`: force-enable a specific transform ID even though its mode-matrix
    /// entry doesn't enable it for the current mode. Still requires `experimental` for any
    /// transform whose `ModeEntry.experimental == true` (see `modes::pipeline_for`).
    pub enable: Vec<String>,
    /// F-045: when true, and the full pre-transform input contains no secret-shaped content,
    /// `pipeline::compress_with_estimator` persists it to the reversible evidence store
    /// (`retrieval_backend`/`retrieval_store_path`) under its SHA-256 hash.
    pub store_originals: bool,
    /// F-045: the namespace stored-original entries are keyed under (see
    /// `retrieval_store::RetrievalStore::store`).
    pub retrieval_namespace: String,
    /// F-045: TTL passed to `RetrievalStore::store` for newly stored originals. `None` means
    /// "use `retrieval_store::DEFAULT_TTL_SECONDS`" (this is a *default*, not "never expire" —
    /// that per-entry meaning belongs to `RetrievalStore::store`'s own `ttl_seconds` parameter).
    pub retrieval_ttl_seconds: Option<u64>,
    /// F-045: backend name passed to `RetrievalStore::open` ("memory" | "filesystem" |
    /// "sqlite" — the latter fails clearly, handled as best-effort skip, see
    /// `pipeline::maybe_store_originals`).
    pub retrieval_backend: String,
    /// F-045: filesystem backend root override. `None` means
    /// `retrieval_store::default_store_path()`.
    pub retrieval_store_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    Conservative,
    Balanced,
    Aggressive,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskScope {
    All,
    General,
    CodeReview,
    ChangeSummary,
    Debugging,
    Generation,
    ApiOverview,
    RetrievalQa,
    AgentHistory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheBoundary {
    ByteOffset(usize),
    TurnIndex(usize),
}

impl CompressionPolicy {
    pub fn builder() -> CompressionPolicyBuilder {
        CompressionPolicyBuilder::default()
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompressionPolicyBuilder {
    target_tokens: Option<usize>,
    reserve_output_tokens: Option<usize>,
    mode: Option<CompressionMode>,
    task_scope: Option<TaskScope>,
    cache_boundary: Option<CacheBoundary>,
    preserve_latest_user_message: Option<bool>,
    disabled: Vec<String>,
    unsafe_disable_redaction: bool,
    experimental: bool,
    enable: Vec<String>,
    store_originals: bool,
    retrieval_namespace: Option<String>,
    retrieval_ttl_seconds: Option<u64>,
    retrieval_backend: Option<String>,
    retrieval_store_path: Option<PathBuf>,
}

impl CompressionPolicyBuilder {
    pub fn target_tokens(mut self, target_tokens: usize) -> Self {
        self.target_tokens = Some(target_tokens);
        self
    }

    pub fn reserve_output_tokens(mut self, reserve_output_tokens: usize) -> Self {
        self.reserve_output_tokens = Some(reserve_output_tokens);
        self
    }

    pub fn mode(mut self, mode: CompressionMode) -> Self {
        self.mode = Some(mode);
        self
    }

    pub fn task_scope(mut self, task_scope: TaskScope) -> Self {
        self.task_scope = Some(task_scope);
        self
    }

    pub fn cache_boundary(mut self, cache_boundary: CacheBoundary) -> Self {
        self.cache_boundary = Some(cache_boundary);
        self
    }

    pub fn preserve_latest_user_message(mut self, preserve: bool) -> Self {
        self.preserve_latest_user_message = Some(preserve);
        self
    }

    pub fn disable(mut self, transform_id: impl Into<String>) -> Self {
        self.disabled.push(transform_id.into());
        self
    }

    pub fn unsafe_disable_redaction(mut self, unsafe_disable: bool) -> Self {
        self.unsafe_disable_redaction = unsafe_disable;
        self
    }

    pub fn experimental(mut self, experimental: bool) -> Self {
        self.experimental = experimental;
        self
    }

    pub fn enable(mut self, transform_id: impl Into<String>) -> Self {
        self.enable.push(transform_id.into());
        self
    }

    pub fn store_originals(mut self, store_originals: bool) -> Self {
        self.store_originals = store_originals;
        self
    }

    pub fn retrieval_namespace(mut self, namespace: impl Into<String>) -> Self {
        self.retrieval_namespace = Some(namespace.into());
        self
    }

    pub fn retrieval_ttl_seconds(mut self, ttl_seconds: Option<u64>) -> Self {
        self.retrieval_ttl_seconds = ttl_seconds;
        self
    }

    pub fn retrieval_backend(mut self, backend: impl Into<String>) -> Self {
        self.retrieval_backend = Some(backend.into());
        self
    }

    pub fn retrieval_store_path(mut self, store_path: Option<PathBuf>) -> Self {
        self.retrieval_store_path = store_path;
        self
    }

    pub fn build(self) -> Result<CompressionPolicy, TokenFoldError> {
        if self.disabled.iter().any(|id| id == "secret_redaction") {
            return Err(TokenFoldError::ConfigError(
                "secret_redaction cannot be disabled via CompressionPolicy.disabled".to_string(),
            ));
        }
        Ok(CompressionPolicy {
            target_tokens: self.target_tokens,
            reserve_output_tokens: self.reserve_output_tokens.unwrap_or(0),
            mode: self.mode.unwrap_or(CompressionMode::Balanced),
            task_scope: self.task_scope.unwrap_or(TaskScope::All),
            cache_boundary: self.cache_boundary,
            preserve_latest_user_message: self.preserve_latest_user_message.unwrap_or(true),
            disabled: self.disabled,
            unsafe_disable_redaction: self.unsafe_disable_redaction,
            experimental: self.experimental,
            enable: self.enable,
            store_originals: self.store_originals,
            retrieval_namespace: self
                .retrieval_namespace
                .unwrap_or_else(|| "default".to_string()),
            retrieval_ttl_seconds: self.retrieval_ttl_seconds,
            retrieval_backend: self
                .retrieval_backend
                .unwrap_or_else(|| "filesystem".to_string()),
            retrieval_store_path: self.retrieval_store_path,
        })
    }
}

/// tokens(protected + structurally-required content). Used to detect `Status::UnreachableTarget`.
pub fn protected_floor(
    input: &CompressionInput,
    policy: &CompressionPolicy,
    estimator: &dyn TokenEstimator,
) -> usize {
    estimator.count_bytes(&protected_segments(input, policy).concat())
}

/// The individual protected-content segments (one per system message, the latest user
/// message, each diff header/hunk line, …) that must each survive byte-for-byte after any
/// transform. Kept as separate segments (rather than one flattened blob) so `safety.rs` can
/// check each one independently — concatenated messages are rarely contiguous in the
/// original document, so a single substring check across the whole blob would be meaningless.
pub fn protected_segments(input: &CompressionInput, policy: &CompressionPolicy) -> Vec<Vec<u8>> {
    match input.format {
        InputFormat::OpenAiJson => extract_openai_protected(&input.bytes, policy),
        InputFormat::AnthropicJson => extract_anthropic_protected(&input.bytes, policy),
        InputFormat::GitDiff => extract_diff_protected(&input.bytes),
        // ponytail: no transform touches plain text/command output structure yet beyond
        // log/diff compaction (task-scope gated), so nothing is unconditionally protected.
        // Generic Json has no "protected" sub-segment either — json_field_fold's own
        // round-trip safety gate is what guarantees its data is preserved.
        InputFormat::PlainText
        | InputFormat::CommandOutput
        | InputFormat::Json
        | InputFormat::Auto => Vec::new(),
    }
}

fn extract_openai_protected(bytes: &[u8], policy: &CompressionPolicy) -> Vec<Vec<u8>> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };
    let Some(messages) = value.get("messages").and_then(|m| m.as_array()) else {
        return Vec::new();
    };

    let mut segments = Vec::new();
    for message in messages {
        if message.get("role").and_then(|r| r.as_str()) == Some("system")
            && let Some(bytes) = message_content_bytes(message)
        {
            segments.push(bytes);
        }
    }
    if policy.preserve_latest_user_message
        && let Some(last_user) = messages
            .iter()
            .rev()
            .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
        && let Some(bytes) = message_content_bytes(last_user)
    {
        segments.push(bytes);
    }
    segments
}

fn extract_anthropic_protected(bytes: &[u8], policy: &CompressionPolicy) -> Vec<Vec<u8>> {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return Vec::new();
    };

    let mut segments = Vec::new();
    if let Some(system) = value.get("system").and_then(|s| s.as_str()) {
        segments.push(system.as_bytes().to_vec());
    }
    if policy.preserve_latest_user_message
        && let Some(last_user) =
            value
                .get("messages")
                .and_then(|m| m.as_array())
                .and_then(|messages| {
                    messages
                        .iter()
                        .rev()
                        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))
                })
        && let Some(bytes) = message_content_bytes(last_user)
    {
        segments.push(bytes);
    }
    segments
}

fn message_content_bytes(message: &serde_json::Value) -> Option<Vec<u8>> {
    match message.get("content") {
        Some(serde_json::Value::String(text)) => Some(text.as_bytes().to_vec()),
        Some(structured) => serde_json::to_vec(structured).ok(),
        None => None,
    }
}

/// Keeps file names and hunk headers, matching the `diff_compaction` (F-013) contract of what
/// must survive compaction. Each kept line is its own segment.
fn extract_diff_protected(bytes: &[u8]) -> Vec<Vec<u8>> {
    let text = String::from_utf8_lossy(bytes);
    let mut segments = Vec::new();
    for line in text.lines() {
        if line.starts_with("diff --git")
            || line.starts_with("--- ")
            || line.starts_with("+++ ")
            || line.starts_with("@@")
        {
            let mut segment = line.as_bytes().to_vec();
            segment.push(b'\n');
            segments.push(segment);
        }
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_estimator::ByteHeuristicEstimator;

    #[test]
    fn default_mode_is_balanced() {
        let policy = CompressionPolicy::builder().build().unwrap();
        assert_eq!(policy.mode, CompressionMode::Balanced);
    }

    #[test]
    fn store_originals_defaults_to_false_with_a_default_namespace() {
        let policy = CompressionPolicy::builder().build().unwrap();
        assert!(!policy.store_originals);
        assert_eq!(policy.retrieval_namespace, "default");
    }

    #[test]
    fn store_originals_and_namespace_are_settable_via_the_builder() {
        let policy = CompressionPolicy::builder()
            .store_originals(true)
            .retrieval_namespace("project-x")
            .retrieval_ttl_seconds(Some(60))
            .retrieval_backend("memory")
            .retrieval_store_path(Some(std::path::PathBuf::from("/tmp/custom")))
            .build()
            .unwrap();
        assert!(policy.store_originals);
        assert_eq!(policy.retrieval_namespace, "project-x");
        assert_eq!(policy.retrieval_ttl_seconds, Some(60));
        assert_eq!(policy.retrieval_backend, "memory");
        assert_eq!(
            policy.retrieval_store_path,
            Some(std::path::PathBuf::from("/tmp/custom"))
        );
    }

    #[test]
    fn retrieval_defaults_are_none_ttl_and_filesystem_backend() {
        let policy = CompressionPolicy::builder().build().unwrap();
        assert_eq!(policy.retrieval_ttl_seconds, None);
        assert_eq!(policy.retrieval_backend, "filesystem");
        assert_eq!(policy.retrieval_store_path, None);
    }

    #[test]
    fn secret_redaction_cannot_be_disabled_through_policy() {
        let err = CompressionPolicy::builder()
            .disable("secret_redaction")
            .build()
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn disabling_other_transforms_is_allowed() {
        let policy = CompressionPolicy::builder()
            .disable("json_minify")
            .build()
            .unwrap();
        assert_eq!(policy.disabled, vec!["json_minify".to_string()]);
    }

    #[test]
    fn floor_is_zero_for_plain_text() {
        let input = CompressionInput::plain_text(b"just some plain text".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        assert_eq!(floor, 0);
    }

    #[test]
    fn floor_covers_system_and_latest_user_message_for_openai_json() {
        let payload = serde_json::json!({
            "model": "gpt-4",
            "messages": [
                {"role": "system", "content": "You are a helpful assistant."},
                {"role": "user", "content": "first question"},
                {"role": "assistant", "content": "first answer"},
                {"role": "user", "content": "second question"},
            ]
        });
        let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
        let policy = CompressionPolicy::builder().build().unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);

        let expected_bytes = "You are a helpful assistant.".len() + "second question".len();
        assert_eq!(
            floor,
            ByteHeuristicEstimator.count_bytes(&vec![0u8; expected_bytes])
        );
        // The earlier "first question" turn must NOT be counted as protected.
        assert!(floor < ByteHeuristicEstimator.count_bytes(input.bytes.as_slice()));
    }

    #[test]
    fn floor_excludes_latest_user_message_when_policy_disables_preservation() {
        let payload = serde_json::json!({
            "messages": [
                {"role": "system", "content": "system prompt"},
                {"role": "user", "content": "question"},
            ]
        });
        let input = CompressionInput::openai_json(serde_json::to_vec(&payload).unwrap());
        let policy = CompressionPolicy::builder()
            .preserve_latest_user_message(false)
            .build()
            .unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        assert_eq!(floor, ByteHeuristicEstimator.count_bytes(b"system prompt"));
    }

    #[test]
    fn floor_covers_system_and_latest_user_message_for_anthropic_json() {
        let payload = serde_json::json!({
            "system": "system prompt",
            "messages": [
                {"role": "user", "content": "first"},
                {"role": "assistant", "content": "reply"},
                {"role": "user", "content": "second"},
            ]
        });
        let input = CompressionInput::anthropic_json(serde_json::to_vec(&payload).unwrap());
        let policy = CompressionPolicy::builder().build().unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        let expected_bytes = "system prompt".len() + "second".len();
        assert_eq!(
            floor,
            ByteHeuristicEstimator.count_bytes(&vec![0u8; expected_bytes])
        );
    }

    #[test]
    fn floor_keeps_diff_headers_and_hunk_markers_only() {
        let diff =
            b"diff --git a/f.rs b/f.rs\n--- a/f.rs\n+++ b/f.rs\n@@ -1,2 +1,2 @@\n-old\n+new\n";
        let input = CompressionInput::git_diff(diff.to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        assert!(floor > 0);
        assert!(floor < ByteHeuristicEstimator.count_bytes(diff));
    }

    #[test]
    fn malformed_json_never_panics_and_yields_zero_floor() {
        let input = CompressionInput::openai_json(b"{not json".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        assert_eq!(floor, 0);
    }
}
