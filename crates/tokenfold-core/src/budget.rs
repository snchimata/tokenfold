use std::path::PathBuf;

use crate::errors::TokenFoldError;
use crate::input::{CompressionInput, InputFormat};
use crate::token_estimator::TokenEstimator;

/// Placeholder floor for `retrieval_ttl_seconds` whenever `lossy` is set — see
/// `CompressionPolicyBuilder::build`'s lossy validation. The exact retention policy is still an
/// open owner decision, not yet confirmed; this value only needs to be "clearly more than
/// instant," not final.
const MIN_LOSSY_TTL_SECONDS: u64 = 86_400;

// NOT Eq: `lossy_ratio` holds an f64 (same precedent as `CompressionOutput`/`CompressionReport`).
#[derive(Debug, Clone, PartialEq)]
pub struct CompressionPolicy {
    pub target_tokens: Option<usize>,
    pub reserve_output_tokens: usize,
    pub mode: CompressionMode,
    pub task_scope: TaskScope,
    /// Reserved for a future prompt-cache preservation contract. Any non-`None` value is
    /// rejected by [`CompressionPolicy::validate`] so it can never silently do nothing.
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
    /// When true, and the full pre-transform input contains no secret-shaped content,
    /// `pipeline::compress_with_estimator` persists it to the reversible evidence store
    /// (`retrieval_backend`/`retrieval_store_path`) under its SHA-256 hash.
    pub store_originals: bool,
    /// The evidence-store namespace stored-original entries are keyed under (see
    /// `retrieval_store::RetrievalStore::store`).
    pub retrieval_namespace: String,
    /// TTL passed to `RetrievalStore::store` for newly stored originals. `None` means
    /// "use `retrieval_store::DEFAULT_TTL_SECONDS`" (this is a *default*, not "never expire" —
    /// that per-entry meaning belongs to `RetrievalStore::store`'s own `ttl_seconds` parameter).
    pub retrieval_ttl_seconds: Option<u64>,
    /// Backend name passed to `RetrievalStore::open` ("memory" | "filesystem" |
    /// "sqlite" — the latter fails clearly, handled as best-effort skip, see
    /// `pipeline::maybe_store_originals`).
    pub retrieval_backend: String,
    /// Filesystem backend root override. `None` means
    /// `retrieval_store::default_store_path()`.
    pub retrieval_store_path: Option<PathBuf>,
    /// Opt-in lossy JSON array-item selection: array items are dropped and replaced by a
    /// recoverable `$tf_ref` marker. `None` (the default) means the lossless pipeline is
    /// untouched — this field is set only by an explicit CLI flag, never derived from
    /// `mode`/`experimental`, and is deliberately NOT part of `modes.rs`/`ALL_ENTRIES`: it is a
    /// fundamentally different category (data-lossy, not just structurally-lossy-but-reversible)
    /// from every other transform in this crate. See `pipeline::apply_lossy_reduction`.
    pub lossy: Option<LossyPath>,
    /// BEST-EFFORT selection hint, not an enforced budget: how aggressively to prune, as the
    /// fraction (0.0..=1.0) of the prunable pool's own estimated token cost to keep when `lossy`
    /// is set. It parameterizes `transforms::json_prune`'s selection walk and is deliberately
    /// never re-checked against the final serialized document — the achieved whole-document ratio
    /// will differ, since the pool excludes preserved arrays, all non-array content, and items
    /// cheaper than the `$tf_ref` marker that would replace them, and since
    /// `pipeline::apply_lossy_reduction` discards a whole prune that fails to beat the lossless
    /// pipeline. `target_tokens` is the enforced ceiling; this is not. Ignored when `lossy` is
    /// `None`.
    pub lossy_ratio: f64,
    /// Dot-separated paths (see `transforms::json_prune::LossyOptions::preserve_paths`) whose
    /// arrays must never be pruned. Ignored when `lossy` is `None`.
    pub lossy_preserve: Vec<String>,
    /// True for a side-effect-free preview (`tokenfold inspect` / `compress --dry-run`): the
    /// projected output/savings are computed exactly as a real run would, but
    /// `pipeline::maybe_store_originals`/`apply_lossy_reduction` must not perform any real
    /// `RetrievalStore` write.
    ///
    /// `pub(crate)`, settable only via [`CompressionPolicyBuilder::preview`]. It was a plain
    /// `pub` field, which made it a footgun for library callers: a preview run's output can carry
    /// `$tf_ref` markers whose targets were deliberately never persisted, so flipping this on an
    /// otherwise ordinary policy and feeding `CompressionOutput::bytes` to a model yields
    /// references that resolve to nothing. Callers who want the projection must ask for it by
    /// name and are told, right here, that the bytes are a projection to measure — not to ship.
    pub(crate) preview: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompressionMode {
    Conservative,
    Balanced,
    Aggressive,
}

/// Selection backend for opt-in lossy JSON pruning. `Heuristic` is the only Phase 1
/// implementation; a future `Select` (Tokenfold Select as the scorer) is Phase 2 and not
/// implemented — deliberately a single-variant enum for now rather than a bare `bool`, since the
/// CLI already speaks of this as choosing an algorithm (`--lossy heuristic`), not toggling a flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LossyPath {
    Heuristic,
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

    /// Re-checks the same invariants `CompressionPolicyBuilder::build` enforces, against the
    /// concrete built struct rather than the builder's `Option` fields. Every field here is
    /// `pub`, so a caller can construct or mutate a `CompressionPolicy` directly without ever
    /// going through the builder -- `pipeline::compress_with_estimator` calls this on every
    /// policy it receives so a hand-built policy can't silently skip the same fail-closed
    /// guarantees a builder-built one gets for free.
    pub fn validate(&self) -> Result<(), TokenFoldError> {
        if self.cache_boundary.is_some() {
            return Err(TokenFoldError::ConfigError(
                "cache_boundary is reserved but not implemented; omit it".to_string(),
            ));
        }
        if self.disabled.iter().any(|id| id == "secret_redaction") {
            return Err(TokenFoldError::ConfigError(
                "secret_redaction cannot be disabled via CompressionPolicy.disabled".to_string(),
            ));
        }
        if !(0.0..=1.0).contains(&self.lossy_ratio) {
            return Err(TokenFoldError::ConfigError(format!(
                "lossy_ratio must be between 0.0 and 1.0, got {}",
                self.lossy_ratio
            )));
        }
        if self.lossy.is_some() {
            // Design doc §4/§8: "must refuse to run when retrieval_backend == Memory or the TTL
            // is below a floor" -- a lossy run with no durable receipt is real data loss, not
            // "lossy but recoverable".
            if self.retrieval_backend != "filesystem" {
                return Err(TokenFoldError::ConfigError(format!(
                    "lossy pruning requires a durable retrieval backend (\"filesystem\"); \
                     {:?} would make dropped items unrecoverable",
                    self.retrieval_backend
                )));
            }
            let effective_ttl = self
                .retrieval_ttl_seconds
                .unwrap_or(crate::retrieval_store::DEFAULT_TTL_SECONDS);
            if effective_ttl < MIN_LOSSY_TTL_SECONDS {
                return Err(TokenFoldError::ConfigError(format!(
                    "lossy pruning requires retrieval_ttl_seconds >= {MIN_LOSSY_TTL_SECONDS} \
                     (got {effective_ttl}); a near-immediate expiry has no real recoverability"
                )));
            }
        }
        Ok(())
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
    lossy: Option<LossyPath>,
    lossy_ratio: Option<f64>,
    lossy_preserve: Vec<String>,
    preview: bool,
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

    pub fn lossy(mut self, lossy: LossyPath) -> Self {
        self.lossy = Some(lossy);
        self
    }

    pub fn lossy_ratio(mut self, ratio: f64) -> Self {
        self.lossy_ratio = Some(ratio);
        self
    }

    pub fn lossy_preserve(mut self, path: impl Into<String>) -> Self {
        self.lossy_preserve.push(path.into());
        self
    }

    /// Opt into a side-effect-free preview: no real `RetrievalStore` write happens anywhere in
    /// the pipeline. The returned `CompressionOutput::bytes` are a PROJECTION of what a real run
    /// would emit — with a lossy policy they can contain `$tf_ref` markers pointing at content
    /// that was deliberately never stored, so they are for measuring savings, never for sending
    /// to a model. See `CompressionPolicy::preview`.
    pub fn preview(mut self, preview: bool) -> Self {
        self.preview = preview;
        self
    }

    pub fn build(self) -> Result<CompressionPolicy, TokenFoldError> {
        let policy = CompressionPolicy {
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
            lossy: self.lossy,
            lossy_ratio: self.lossy_ratio.unwrap_or(0.3),
            lossy_preserve: self.lossy_preserve,
            preview: self.preview,
        };
        policy.validate()?;
        Ok(policy)
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
    // Anthropic's `system` field is either a plain string OR a structured array of content
    // blocks (`[{"type":"text","text":"..."}, ...]`) -- the structured shape used to fall
    // through `.as_str()` as `None` and get zero protection. Mirrors `message_content_bytes`'s
    // existing string-or-structured handling for message `content`.
    match value.get("system") {
        Some(serde_json::Value::String(text)) => segments.push(text.as_bytes().to_vec()),
        Some(structured @ serde_json::Value::Array(_)) => {
            if let Ok(bytes) = serde_json::to_vec(structured) {
                segments.push(bytes);
            }
        }
        _ => {}
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

/// Keeps file names and hunk headers, matching the `diff_compaction` contract of what must
/// survive compaction. Each kept line is its own segment.
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
    fn cache_boundary_is_rejected_instead_of_silently_ignored() {
        let error = CompressionPolicy::builder()
            .cache_boundary(CacheBoundary::ByteOffset(10))
            .build()
            .unwrap_err();
        assert!(error.to_string().contains("not implemented"));
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
    fn floor_covers_structured_anthropic_system_content_not_just_a_plain_string() {
        // Round-4 external review: Anthropic's `system` field can be a structured array of
        // content blocks (`[{"type":"text","text":"..."}]`), not just a plain string --
        // `.as_str()` alone returned `None` for that shape, so a structured system prompt got
        // ZERO protection (silently prunable/rewritable like any other content).
        let payload = serde_json::json!({
            "system": [{"type": "text", "text": "structured system prompt"}],
            "messages": [
                {"role": "user", "content": "first"},
            ]
        });
        let input = CompressionInput::anthropic_json(serde_json::to_vec(&payload).unwrap());
        let policy = CompressionPolicy::builder()
            .preserve_latest_user_message(false)
            .build()
            .unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        assert!(
            floor > 0,
            "a structured Anthropic `system` array must contribute to the protected floor"
        );
        let segments = protected_segments(&input, &policy);
        let system_bytes = serde_json::to_vec(
            &serde_json::json!([{"type": "text", "text": "structured system prompt"}]),
        )
        .unwrap();
        assert!(
            segments.contains(&system_bytes),
            "the structured system content must be a protected segment, byte-for-byte"
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
    fn lossy_defaults_to_disabled_with_a_default_ratio() {
        let policy = CompressionPolicy::builder().build().unwrap();
        assert_eq!(policy.lossy, None);
        assert_eq!(policy.lossy_ratio, 0.3);
        assert!(policy.lossy_preserve.is_empty());
    }

    #[test]
    fn lossy_is_settable_via_the_builder() {
        let policy = CompressionPolicy::builder()
            .lossy(LossyPath::Heuristic)
            .lossy_ratio(0.5)
            .lossy_preserve("items")
            .lossy_preserve("data.results")
            .build()
            .unwrap();
        assert_eq!(policy.lossy, Some(LossyPath::Heuristic));
        assert_eq!(policy.lossy_ratio, 0.5);
        assert_eq!(policy.lossy_preserve, vec!["items", "data.results"]);
    }

    #[test]
    fn lossy_refuses_memory_retrieval_backend() {
        let err = CompressionPolicy::builder()
            .lossy(LossyPath::Heuristic)
            .retrieval_backend("memory")
            .build()
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn lossy_refuses_a_ttl_below_the_floor() {
        let err = CompressionPolicy::builder()
            .lossy(LossyPath::Heuristic)
            .retrieval_ttl_seconds(Some(60))
            .build()
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn lossy_with_default_retrieval_settings_is_accepted() {
        // Defaults (filesystem backend, 7-day TTL) already clear the floor -- a user shouldn't
        // need to configure retrieval explicitly just to use --lossy.
        let policy = CompressionPolicy::builder()
            .lossy(LossyPath::Heuristic)
            .build()
            .unwrap();
        assert_eq!(policy.lossy, Some(LossyPath::Heuristic));
    }

    #[test]
    fn lossy_ratio_outside_unit_interval_is_rejected() {
        let err = CompressionPolicy::builder()
            .lossy_ratio(1.5)
            .build()
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
        let err = CompressionPolicy::builder()
            .lossy_ratio(-0.1)
            .build()
            .unwrap_err();
        assert!(matches!(err, TokenFoldError::ConfigError(_)));
    }

    #[test]
    fn malformed_json_never_panics_and_yields_zero_floor() {
        let input = CompressionInput::openai_json(b"{not json".to_vec());
        let policy = CompressionPolicy::builder().build().unwrap();
        let floor = protected_floor(&input, &policy, &ByteHeuristicEstimator);
        assert_eq!(floor, 0);
    }
}
