//! Shared, owned fitting protocol. No counters, inference or I/O run in renderers.
mod receipt;
mod request;
pub use receipt::*;
pub use request::*;

use crate::token_estimator::TokenEstimator;
use serde::{Deserialize, Serialize};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;
pub const MAX_SOURCE_BYTES: usize = 2 * 1024 * 1024;
pub type GroupId = String;
pub type FitInput = Box<dyn FitRequest>;

#[derive(Debug, thiserror::Error)]
pub enum FitError {
    #[error("invalid fit input: {0}")]
    InvalidInput(String),
    #[error("invalid fit configuration: {0}")]
    Config(String),
    #[error("secret-shaped content is not accepted by fit")]
    Safety,
    #[error("accounting failed: {0}")]
    Accounting(String),
    #[error("invalid fit action reply")]
    Protocol,
    #[error("fit cancelled")]
    Cancelled,
    #[error("budget unmet")]
    BudgetUnmet(Box<Receipt>),
    #[error("budget indeterminate")]
    BudgetIndeterminate(Box<Receipt>),
    #[error("fit internal error")]
    Internal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitFormat {
    Text,
    JsonData,
    Context,
    OpenaiChat,
    AnthropicMessages,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccountingScope {
    RenderedText,
    ProviderRequest,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CountBasis {
    Exact,
    Estimated,
    Unavailable,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TokenCount {
    pub tokens: Option<u64>,
    pub basis: CountBasis,
}
impl TokenCount {
    pub fn exact(tokens: u64) -> Self {
        Self {
            tokens: Some(tokens),
            basis: CountBasis::Exact,
        }
    }
    pub fn estimated(tokens: u64) -> Self {
        Self {
            tokens: Some(tokens),
            basis: CountBasis::Estimated,
        }
    }
    pub fn unavailable() -> Self {
        Self {
            tokens: None,
            basis: CountBasis::Unavailable,
        }
    }
    pub fn validate(&self) -> Result<(), FitError> {
        if (self.basis == CountBasis::Unavailable) != self.tokens.is_none()
            || self.tokens.is_some_and(|n| n > MAX_SAFE_INTEGER)
        {
            return Err(FitError::Accounting("invalid token count".into()));
        }
        Ok(())
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMode {
    #[default]
    Strict,
    BestEffort,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetStatus {
    Met,
    Unmet,
    Indeterminate,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitReason {
    SelectionDisabled,
    RuntimeUnavailable,
    NoOptionalGroups,
    RequiredOverBudget,
    UnsupportedSelection,
    SelectionLimit,
    ScorerFailure,
    ScorerTimeout,
    ScorerQueueTimeout,
    ScorerResponseInvalid,
    NoValidFit,
    NoGain,
    UnknownAccounting,
    EstimatedFit,
}
#[derive(Debug, Clone, PartialEq)]
pub enum RenderedPayload {
    Text(String),
    Json(serde_json::Value),
}
impl RenderedPayload {
    pub fn bytes(&self) -> Result<Vec<u8>, FitError> {
        match self {
            Self::Text(text) => Ok(text.as_bytes().to_vec()),
            Self::Json(value) => serde_json::to_vec(value).map_err(|_| FitError::Internal),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct RenderedRequest {
    pub format: FitFormat,
    pub payload: RenderedPayload,
    pub scope: AccountingScope,
    pub core_transforms: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceGroup {
    pub id: GroupId,
    pub scoring_text: String,
    pub required: bool,
    pub source_positions: Vec<usize>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPass {
    Original,
    CoreCompatible,
}
pub trait FitRequest: Send {
    fn source_groups(&self) -> &[SourceGroup];
    fn scoring_query(&self) -> Option<&str>;
    fn fixed_guide_bytes(&self) -> Vec<u8>;
    fn render(&self, kept_ids: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError>;
    fn validate(&self, candidate: &RenderedRequest, kept_ids: &[GroupId]) -> Result<(), FitError>;
}
#[derive(Debug, Clone)]
pub struct FitOptions {
    pub target_tokens: u64,
    pub budget_mode: BudgetMode,
    pub allow_selection: bool,
    pub counter_id: String,
}
impl Default for FitOptions {
    fn default() -> Self {
        Self {
            target_tokens: 0,
            budget_mode: BudgetMode::Strict,
            allow_selection: false,
            counter_id: "encoding:o200k_base:v1".into(),
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CountPurpose {
    Original,
    Baseline,
    Required,
    Selected,
}
#[derive(Debug, Clone, PartialEq)]
// Done owns the receipt once; keep the protocol inline rather than allocate every result.
#[allow(clippy::large_enum_variant)]
pub enum FitAction {
    Count {
        candidate_id: u64,
        request: RenderedRequest,
        purpose: CountPurpose,
    },
    Score {
        action_id: u64,
        query: String,
        groups: Vec<SourceGroup>,
    },
    Done(FitOutput),
}
#[derive(Debug, Clone, PartialEq)]
pub struct GroupScore {
    pub id: GroupId,
    pub score: f64,
}
#[derive(Debug, Clone, PartialEq)]
pub enum FitReply {
    Counted {
        candidate_id: u64,
        counter_id: String,
        scope: AccountingScope,
        count: TokenCount,
    },
    Scored {
        action_id: u64,
        scores: Vec<GroupScore>,
        scorer: ScorerInfo,
    },
}
#[derive(Debug, Clone, PartialEq)]
pub struct FitOutput {
    pub payload: RenderedPayload,
    pub receipt: Receipt,
}
impl FitOutput {
    pub fn require_met(&self) -> Result<&RenderedPayload, FitError> {
        match self.receipt.budget_status {
            BudgetStatus::Met => Ok(&self.payload),
            BudgetStatus::Unmet => Err(FitError::BudgetUnmet(Box::new(self.receipt.clone()))),
            BudgetStatus::Indeterminate => Err(FitError::BudgetIndeterminate(Box::new(
                self.receipt.clone(),
            ))),
        }
    }
    pub fn require_fit(&self) -> Result<&RenderedPayload, FitError> {
        if self.receipt.budget_mode == BudgetMode::BestEffort
            && self.receipt.estimated_fit() == Some(true)
        {
            Ok(&self.payload)
        } else {
            self.require_met()
        }
    }
}
pub trait TokenCounter {
    fn id(&self) -> &str;
    fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError>;
}
pub struct EncodingCounter {
    id: String,
    #[cfg(feature = "tiktoken")]
    estimator: crate::token_estimator::TiktokenEstimator,
}
impl EncodingCounter {
    pub fn new(encoding: &str) -> Result<Self, FitError> {
        if !matches!(encoding, "o200k_base" | "cl100k_base") {
            return Err(FitError::Config("unsupported encoding".into()));
        }
        #[cfg(feature = "tiktoken")]
        {
            let estimator = match encoding {
                "o200k_base" => crate::token_estimator::TiktokenEstimator::o200k_base(),
                _ => crate::token_estimator::TiktokenEstimator::cl100k_base(),
            }
            .map_err(|_| FitError::Accounting("bundled encoding unavailable".into()))?;
            Ok(Self {
                id: format!("encoding:{encoding}:v1"),
                estimator,
            })
        }
        #[cfg(not(feature = "tiktoken"))]
        {
            Err(FitError::Accounting("bundled encoding unavailable".into()))
        }
    }
}
impl TokenCounter for EncodingCounter {
    fn id(&self) -> &str {
        &self.id
    }
    fn count(&self, request: &RenderedRequest) -> Result<TokenCount, FitError> {
        if request.scope != AccountingScope::RenderedText {
            return Ok(TokenCount::unavailable());
        }
        let RenderedPayload::Text(text) = &request.payload else {
            return Err(FitError::Accounting("text scope requires text".into()));
        };
        #[cfg(feature = "tiktoken")]
        {
            Ok(TokenCount::exact(
                self.estimator.count_bytes(text.as_bytes()) as u64,
            ))
        }
        #[cfg(not(feature = "tiktoken"))]
        {
            let _ = text;
            Err(FitError::Accounting("bundled encoding unavailable".into()))
        }
    }
}
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);
impl CancellationToken {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn check(&self) -> Result<(), FitError> {
        if self.0.load(Ordering::Acquire) {
            Err(FitError::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Debug, Clone)]
struct CachedCandidate {
    request: RenderedRequest,
    bytes: Vec<u8>,
    count: Option<TokenCount>,
}
#[derive(Debug, Clone, Copy)]
enum SessionState {
    New,
    Waiting {
        candidate_id: u64,
        index: usize,
        purpose: CountPurpose,
    },
    Done,
}
pub struct FitSession {
    input: FitInput,
    options: FitOptions,
    state: SessionState,
    cache: Vec<CachedCandidate>,
    next_id: u64,
    original: Option<usize>,
    cancellation: CancellationToken,
}
impl FitSession {
    pub fn new(input: FitInput, options: FitOptions) -> Result<Self, FitError> {
        if options.target_tokens > MAX_SAFE_INTEGER {
            return Err(FitError::InvalidInput("unsafe target".into()));
        }
        if options.counter_id.is_empty()
            || options.counter_id.len() > 256
            || options.counter_id.chars().any(char::is_control)
        {
            return Err(FitError::Config("invalid counter ID".into()));
        }
        validate_text(&options.counter_id)?;
        let groups = input.source_groups();
        if groups.len() > 512 {
            return Err(FitError::InvalidInput("too many groups".into()));
        }
        let mut ids = std::collections::HashSet::new();
        for group in groups {
            validate_id(&group.id, true)?;
            validate_text(&group.scoring_text)?;
            if !ids.insert(&group.id) {
                return Err(FitError::InvalidInput("duplicate group ID".into()));
            }
        }
        if let Some(query) = input.scoring_query() {
            validate_text(query)?;
            if query.is_empty() || query.len() > 4096 {
                return Err(FitError::InvalidInput("invalid scoring query".into()));
            }
        }
        // A missing bundled guide must never fall back to the byte heuristic.
        let _ = guide_estimator()?;
        Ok(Self {
            input,
            options,
            state: SessionState::New,
            cache: vec![],
            next_id: 1,
            original: None,
            cancellation: CancellationToken::default(),
        })
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn advance(&mut self, reply: Option<FitReply>) -> Result<FitAction, FitError> {
        self.cancellation.check()?;
        match (self.state, reply) {
            (SessionState::New, None) => {
                let index = self.prepare(RenderPass::Original)?;
                self.original = Some(index);
                self.count_action(index, CountPurpose::Original)
            }
            (
                SessionState::Waiting {
                    candidate_id,
                    index,
                    purpose,
                },
                Some(FitReply::Counted {
                    candidate_id: reply_id,
                    counter_id,
                    scope,
                    count,
                }),
            ) => {
                if candidate_id != reply_id {
                    return Err(FitError::Protocol);
                }
                if counter_id != self.options.counter_id || scope != self.cache[index].request.scope
                {
                    return Err(FitError::Accounting("counter ID or scope changed".into()));
                }
                count.validate()?;
                self.cache[index].count = Some(count);
                match purpose {
                    CountPurpose::Original => {
                        let proposal = self.prepare(RenderPass::CoreCompatible)?;
                        if self.cache[proposal].count.is_some() {
                            self.finish_core(proposal)
                        } else {
                            self.count_action(proposal, CountPurpose::Baseline)
                        }
                    }
                    CountPurpose::Baseline => self.finish_core(index),
                    _ => Err(FitError::Protocol),
                }
            }
            _ => Err(FitError::Protocol),
        }
    }
    /// Run local built-ins here; only custom host counters should expose Count to bindings.
    pub fn drive(
        &mut self,
        counter: &dyn TokenCounter,
        reply: Option<FitReply>,
    ) -> Result<FitAction, FitError> {
        if counter.id() != self.options.counter_id {
            return Err(FitError::Accounting("counter ID changed".into()));
        }
        let mut action = self.advance(reply)?;
        loop {
            self.cancellation.check()?;
            let FitAction::Count {
                candidate_id,
                request,
                ..
            } = action
            else {
                return Ok(action);
            };
            let count = counter.count(&request)?;
            self.cancellation.check()?;
            if counter.id() != self.options.counter_id {
                return Err(FitError::Accounting("counter ID changed".into()));
            }
            action = self.advance(Some(FitReply::Counted {
                candidate_id,
                counter_id: counter.id().into(),
                scope: request.scope,
                count,
            }))?;
        }
    }
    fn prepare(&mut self, pass: RenderPass) -> Result<usize, FitError> {
        self.cancellation.check()?;
        let ids = self
            .input
            .source_groups()
            .iter()
            .map(|g| g.id.clone())
            .collect::<Vec<_>>();
        let request = self.input.render(&ids, pass)?;
        self.cancellation.check()?;
        self.input.validate(&request, &ids)?;
        scan_rendered(&request)?;
        let bytes = request.payload.bytes()?;
        if let Some(original) = self.original {
            let original = &self.cache[original].request;
            if request.format != original.format || request.scope != original.scope {
                return Err(FitError::Accounting(
                    "renderer format or scope changed".into(),
                ));
            }
        }
        if let Some(index) = self
            .cache
            .iter()
            .position(|c| c.bytes == bytes && c.request == request)
        {
            return Ok(index);
        }
        if self.cache.len() == 4 {
            return Err(FitError::Internal);
        }
        self.cache.push(CachedCandidate {
            request,
            bytes,
            count: None,
        });
        Ok(self.cache.len() - 1)
    }
    fn count_action(&mut self, index: usize, purpose: CountPurpose) -> Result<FitAction, FitError> {
        let candidate_id = self.next_id;
        self.next_id += 1;
        self.state = SessionState::Waiting {
            candidate_id,
            index,
            purpose,
        };
        Ok(FitAction::Count {
            candidate_id,
            request: self.cache[index].request.clone(),
            purpose,
        })
    }
    fn finish_core(&mut self, proposed: usize) -> Result<FitAction, FitError> {
        self.cancellation.check()?;
        let original = self.original.ok_or(FitError::Internal)?;
        let raw = self.cache[original].count.ok_or(FitError::Internal)?;
        let proposal = self.cache[proposed].count.ok_or(FitError::Internal)?;
        let comparable = raw.basis != CountBasis::Unavailable && raw.basis == proposal.basis;
        let accepted = comparable && proposal.tokens <= raw.tokens;
        let baseline = if accepted { proposed } else { original };
        let current = &self.cache[baseline];
        let count = current.count.ok_or(FitError::Internal)?;
        let status = budget_status(count, self.options.target_tokens);
        let reason = if status == BudgetStatus::Met {
            None
        } else if self.options.budget_mode == BudgetMode::BestEffort
            && count.basis == CountBasis::Estimated
            && count
                .tokens
                .is_some_and(|n| n <= self.options.target_tokens)
        {
            Some(FitReason::EstimatedFit)
        } else if !comparable
            || count.basis == CountBasis::Unavailable
            || (count.basis == CountBasis::Estimated
                && self.options.budget_mode == BudgetMode::Strict)
        {
            Some(FitReason::UnknownAccounting)
        } else if !self.options.allow_selection {
            Some(FitReason::SelectionDisabled)
        } else if !self.input.source_groups().iter().any(|g| !g.required) {
            Some(FitReason::NoOptionalGroups)
        } else {
            Some(FitReason::RuntimeUnavailable)
        };
        let mut warnings = vec![];
        if count.basis == CountBasis::Estimated {
            warnings.push(FitWarning {
                code: "estimated_accounting".into(),
                message: "Local accounting is an estimate, not proof of provider acceptance."
                    .into(),
            });
        }
        let receipt = Receipt {
            kind: "tokenfold.fit".into(),
            schema_version: "1.0".into(),
            format: current.request.format,
            budget_mode: self.options.budget_mode,
            budget_status: status,
            reason,
            target_tokens: self.options.target_tokens,
            original_tokens: raw.tokens,
            baseline_tokens: count.tokens,
            final_tokens: count.tokens,
            accounting: Accounting {
                counter_id: self.options.counter_id.clone(),
                scope: current.request.scope,
                original_basis: raw.basis,
                baseline_basis: count.basis,
                final_basis: count.basis,
            },
            core_transforms: if accepted {
                current.request.core_transforms.clone()
            } else {
                vec![]
            },
            scorer_attempted: false,
            scorer_succeeded: false,
            selection_applied: false,
            scorer: None,
            groups_kept: self
                .input
                .source_groups()
                .iter()
                .map(|g| g.id.clone())
                .collect(),
            groups_dropped: vec![],
            warnings,
            timings_ms: TimingsMs::default(),
        };
        receipt.validate()?;
        let output = FitOutput {
            payload: current.request.payload.clone(),
            receipt,
        };
        self.state = SessionState::Done;
        Ok(FitAction::Done(output))
    }
}

pub fn budget_status(count: TokenCount, target: u64) -> BudgetStatus {
    match (count.basis, count.tokens) {
        (CountBasis::Exact, Some(n)) if n <= target => BudgetStatus::Met,
        (CountBasis::Exact, Some(_)) => BudgetStatus::Unmet,
        _ => BudgetStatus::Indeterminate,
    }
}
pub fn guide_estimator() -> Result<&'static dyn TokenEstimator, FitError> {
    #[cfg(feature = "tiktoken")]
    {
        static GUIDE: std::sync::OnceLock<Result<crate::token_estimator::TiktokenEstimator, ()>> =
            std::sync::OnceLock::new();
        GUIDE
            .get_or_init(|| crate::token_estimator::TiktokenEstimator::o200k_base().map_err(|_| ()))
            .as_ref()
            .map(|g| g as &dyn TokenEstimator)
            .map_err(|_| FitError::Accounting("bundled guide unavailable".into()))
    }
    #[cfg(not(feature = "tiktoken"))]
    {
        Err(FitError::Accounting("bundled guide unavailable".into()))
    }
}
