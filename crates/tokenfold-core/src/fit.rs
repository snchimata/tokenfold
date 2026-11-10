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
    fn selection_supported(&self) -> bool {
        true
    }
    fn render(&self, kept_ids: &[GroupId], pass: RenderPass) -> Result<RenderedRequest, FitError>;
    fn validate(&self, candidate: &RenderedRequest, kept_ids: &[GroupId]) -> Result<(), FitError>;
}
#[derive(Debug, Clone)]
pub struct FitOptions {
    pub target_tokens: u64,
    pub budget_mode: BudgetMode,
    pub allow_selection: bool,
    pub counter_id: String,
    /// Explicit, approved runtime pin. None never authorizes inference.
    pub select: Option<ScorerInfo>,
}
impl Default for FitOptions {
    fn default() -> Self {
        Self {
            target_tokens: 0,
            budget_mode: BudgetMode::Strict,
            allow_selection: false,
            counter_id: "encoding:o200k_base:v1".into(),
            select: None,
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
        scorer: ScorerInfo,
    },
    Done(FitOutput),
}
#[derive(Debug, Clone, PartialEq)]
pub struct GroupScore {
    pub id: GroupId,
    pub score: f64,
}
/// Sanitized outcomes only; arbitrary runtime errors never enter receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreFailure {
    Failure,
    Timeout,
    ResponseInvalid,
    QueueTimeout,
    SelectionLimit,
    SelectionLimitAttempted,
}
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScoreMetadata {
    pub outside_training_group_counts: bool,
    pub cross_window_context_unqualified: bool,
    pub score_queue_ms: Option<f64>,
    pub startup_ms: Option<f64>,
    pub scoring_ms: Option<f64>,
}
#[derive(Debug, Clone, PartialEq)]
pub enum ScoreOutcome {
    Scored {
        scores: Vec<GroupScore>,
        scorer: ScorerInfo,
    },
    Failed(ScoreFailure),
}
#[derive(Debug, Clone, PartialEq)]
pub enum FitReply {
    ScoreCompleted {
        action_id: u64,
        outcome: ScoreOutcome,
        metadata: ScoreMetadata,
    },
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
    ScoreFailed {
        action_id: u64,
        failure: ScoreFailure,
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
    Scoring {
        action_id: u64,
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
    baseline: Option<usize>,
    required: Option<usize>,
    pass: RenderPass,
    kept: Vec<GroupId>,
    scores: Vec<GroupScore>,
    scorer_attempted: bool,
    scorer_succeeded: bool,
    score_metadata: ScoreMetadata,
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
        if let Some(pin) = &options.select {
            pin.validate()
                .map_err(|_| FitError::Config("invalid scorer pin".into()))?;
        }
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
            baseline: None,
            required: None,
            pass: RenderPass::Original,
            kept: vec![],
            scores: vec![],
            scorer_attempted: false,
            scorer_succeeded: false,
            score_metadata: ScoreMetadata::default(),
            cancellation: CancellationToken::default(),
        })
    }
    pub fn cancellation_token(&self) -> CancellationToken {
        self.cancellation.clone()
    }
    pub fn advance(&mut self, reply: Option<FitReply>) -> Result<FitAction, FitError> {
        self.cancellation.check()?;
        let reply = if let Some(FitReply::ScoreCompleted {
            action_id,
            outcome,
            metadata,
        }) = reply
        {
            if !matches!(self.state, SessionState::Scoring {action_id: expected} if expected == action_id)
                || [
                    metadata.score_queue_ms,
                    metadata.startup_ms,
                    metadata.scoring_ms,
                ]
                .into_iter()
                .flatten()
                .any(|n| !n.is_finite() || n < 0.0)
            {
                return Err(FitError::Protocol);
            }
            self.score_metadata = metadata;
            Some(match outcome {
                ScoreOutcome::Scored { scores, scorer } => FitReply::Scored {
                    action_id,
                    scores,
                    scorer,
                },
                ScoreOutcome::Failed(failure) => FitReply::ScoreFailed { action_id, failure },
            })
        } else {
            reply
        };
        match (self.state, reply) {
            (SessionState::New, None) => {
                let ids = self.all_ids();
                let index = self.prepare(&ids, RenderPass::Original)?;
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
                        let ids = self.all_ids();
                        let proposal = self.prepare(&ids, RenderPass::CoreCompatible)?;
                        if self.cache[proposal].count.is_some() {
                            self.finish_core(proposal)
                        } else {
                            self.count_action(proposal, CountPurpose::Baseline)
                        }
                    }
                    CountPurpose::Baseline => self.finish_core(index),
                    CountPurpose::Required => self.finish_required(index),
                    CountPurpose::Selected => self.finish_selected(index),
                }
            }
            (
                SessionState::Scoring { action_id },
                Some(FitReply::Scored {
                    action_id: reply_id,
                    scores,
                    scorer,
                }),
            ) => {
                if action_id != reply_id {
                    return Err(FitError::Protocol);
                }
                self.scorer_attempted = true;
                let valid = self.options.select.as_ref() == Some(&scorer)
                    && scores.len() == self.input.source_groups().len()
                    && scores
                        .iter()
                        .zip(self.input.source_groups())
                        .all(|(s, g)| s.id == g.id && s.score.is_finite());
                if !valid {
                    return self.finish_baseline(FitReason::ScorerResponseInvalid);
                }
                self.scorer_succeeded = true;
                self.scores = scores;
                let guide = guide_estimator()?;
                let groups = self
                    .input
                    .source_groups()
                    .iter()
                    .zip(&self.scores)
                    .map(|(g, s)| crate::allocation::Group {
                        id: g.id.clone(),
                        required: g.required,
                        members: vec![crate::allocation::Candidate {
                            id: g.id.clone(),
                            cost: guide.count_bytes(g.scoring_text.as_bytes()),
                            score: s.score,
                        }],
                    })
                    .collect::<Vec<_>>();
                let fixed = guide.count_bytes(&self.input.fixed_guide_bytes()) as u64;
                let budget = self
                    .options
                    .target_tokens
                    .saturating_sub(fixed)
                    .min(usize::MAX as u64) as usize;
                self.kept = crate::allocation::allocate(&groups, budget).0;
                self.selected_action()
            }
            (
                SessionState::Scoring { action_id },
                Some(FitReply::ScoreFailed {
                    action_id: reply_id,
                    failure,
                }),
            ) => {
                if action_id != reply_id {
                    return Err(FitError::Protocol);
                }
                self.scorer_attempted = !matches!(
                    failure,
                    ScoreFailure::QueueTimeout | ScoreFailure::SelectionLimit
                );
                let reason = match failure {
                    ScoreFailure::Failure => FitReason::ScorerFailure,
                    ScoreFailure::Timeout => FitReason::ScorerTimeout,
                    ScoreFailure::ResponseInvalid => FitReason::ScorerResponseInvalid,
                    ScoreFailure::QueueTimeout => FitReason::ScorerQueueTimeout,
                    ScoreFailure::SelectionLimit | ScoreFailure::SelectionLimitAttempted => {
                        FitReason::SelectionLimit
                    }
                };
                self.finish_baseline(reason)
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
    fn all_ids(&self) -> Vec<GroupId> {
        self.input
            .source_groups()
            .iter()
            .map(|g| g.id.clone())
            .collect()
    }
    fn prepare(&mut self, ids: &[GroupId], pass: RenderPass) -> Result<usize, FitError> {
        self.cancellation.check()?;
        let request = self.input.render(ids, pass)?;
        self.cancellation.check()?;
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
            // Structure/scan/count are reusable only for the same complete immutable candidate.
            validate_survivors(self.input.source_groups(), ids)?;
            return Ok(index);
        }
        self.input.validate(&request, ids)?;
        scan_rendered(&request)?;
        let candidate = CachedCandidate {
            request,
            bytes,
            count: None,
        };
        if let Some(index) = (0..self.cache.len()).find(|i| {
            Some(*i) != self.original && Some(*i) != self.baseline && Some(*i) != self.required
        }) {
            self.cache[index] = candidate;
            return Ok(index);
        }
        if self.cache.len() == 4 {
            return Err(FitError::Internal);
        }
        self.cache.push(candidate);
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
        self.baseline = Some(baseline);
        self.pass = if accepted {
            RenderPass::CoreCompatible
        } else {
            RenderPass::Original
        };
        self.kept = self.all_ids();
        let count = self.cache[baseline].count.ok_or(FitError::Internal)?;
        if self.fits(count) {
            return self.finish(baseline, None, false);
        }
        let reason = if !comparable
            || count.basis == CountBasis::Unavailable
            || (count.basis == CountBasis::Estimated
                && self.options.budget_mode == BudgetMode::Strict)
        {
            Some(FitReason::UnknownAccounting)
        } else if !self.options.allow_selection {
            Some(FitReason::SelectionDisabled)
        } else if !self.input.source_groups().iter().any(|g| !g.required) {
            Some(FitReason::NoOptionalGroups)
        } else if !self.input.selection_supported() {
            Some(FitReason::UnsupportedSelection)
        } else {
            None
        };
        if let Some(reason) = reason {
            return self.finish_baseline(reason);
        }
        if self.input.scoring_query().is_none() {
            return Err(FitError::InvalidInput(
                "selection requires an explicit query".into(),
            ));
        }
        let ids = self
            .input
            .source_groups()
            .iter()
            .filter(|g| g.required)
            .map(|g| g.id.clone())
            .collect::<Vec<_>>();
        let required = self.prepare(&ids, self.pass)?;
        self.required = Some(required);
        if self.cache[required].count.is_some() {
            self.finish_required(required)
        } else {
            self.count_action(required, CountPurpose::Required)
        }
    }
    fn fits(&self, count: TokenCount) -> bool {
        (count.basis == CountBasis::Exact
            || (count.basis == CountBasis::Estimated
                && self.options.budget_mode == BudgetMode::BestEffort))
            && count
                .tokens
                .is_some_and(|n| n <= self.options.target_tokens)
    }
    fn comparable_to_baseline(&self, count: TokenCount) -> Result<bool, FitError> {
        let baseline = self.cache[self.baseline.ok_or(FitError::Internal)?]
            .count
            .ok_or(FitError::Internal)?;
        Ok(count.basis != CountBasis::Unavailable && count.basis == baseline.basis)
    }
    fn finish_required(&mut self, index: usize) -> Result<FitAction, FitError> {
        let count = self.cache[index].count.ok_or(FitError::Internal)?;
        if !self.comparable_to_baseline(count)? {
            return self.finish_baseline(FitReason::UnknownAccounting);
        }
        // Conservative preflight only: a non-monotone counter may have a fitting superset.
        if !self.fits(count) {
            return self.finish_baseline(FitReason::RequiredOverBudget);
        }
        let Some(scorer) = self.options.select.clone() else {
            return self.finish_baseline(FitReason::RuntimeUnavailable);
        };
        let action_id = self.next_id;
        self.next_id += 1;
        self.state = SessionState::Scoring { action_id };
        Ok(FitAction::Score {
            action_id,
            query: self.input.scoring_query().ok_or(FitError::Internal)?.into(),
            groups: self.input.source_groups().to_vec(),
            scorer,
        })
    }
    fn selected_action(&mut self) -> Result<FitAction, FitError> {
        let kept = self.kept.clone();
        let index = self.prepare(&kept, self.pass)?;
        if self.cache[index].count.is_some() {
            self.finish_selected(index)
        } else {
            self.count_action(index, CountPurpose::Selected)
        }
    }
    fn finish_selected(&mut self, index: usize) -> Result<FitAction, FitError> {
        self.cancellation.check()?;
        let count = self.cache[index].count.ok_or(FitError::Internal)?;
        if !self.comparable_to_baseline(count)? {
            return self.finish_baseline(FitReason::UnknownAccounting);
        }
        let baseline = self.baseline.ok_or(FitError::Internal)?;
        let before = self.cache[baseline].count.ok_or(FitError::Internal)?;
        let omitted = self.kept.len() < self.input.source_groups().len();
        let nonempty = match &self.cache[index].request.payload {
            RenderedPayload::Text(s) => !s.is_empty(),
            RenderedPayload::Json(v) => !v.as_object().is_none_or(|o| o.is_empty()),
        };
        if self.fits(count) && omitted && nonempty && count.tokens < before.tokens {
            return self.finish(index, None, true);
        }
        let lowest = self
            .input
            .source_groups()
            .iter()
            .zip(&self.scores)
            .enumerate()
            .filter(|(_, (g, _))| !g.required && self.kept.contains(&g.id))
            .min_by(|(ia, (_, a)), (ib, (_, b))| {
                a.score
                    .partial_cmp(&b.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(ib.cmp(ia))
            })
            .map(|(_, (g, _))| g.id.clone());
        if let Some(id) = lowest {
            self.kept.retain(|g| g != &id);
            self.selected_action()
        } else {
            self.finish_baseline(if count.tokens >= before.tokens {
                FitReason::NoGain
            } else {
                FitReason::NoValidFit
            })
        }
    }
    fn finish_baseline(&mut self, reason: FitReason) -> Result<FitAction, FitError> {
        self.kept = self.all_ids();
        self.finish(
            self.baseline.ok_or(FitError::Internal)?,
            Some(reason),
            false,
        )
    }
    fn finish(
        &mut self,
        index: usize,
        reason: Option<FitReason>,
        selection_applied: bool,
    ) -> Result<FitAction, FitError> {
        self.cancellation.check()?;
        let raw = self.cache[self.original.ok_or(FitError::Internal)?]
            .count
            .ok_or(FitError::Internal)?;
        let baseline = self.cache[self.baseline.ok_or(FitError::Internal)?]
            .count
            .ok_or(FitError::Internal)?;
        let current = &self.cache[index];
        let count = current.count.ok_or(FitError::Internal)?;
        let mut warnings = vec![];
        if count.basis == CountBasis::Estimated {
            warnings.push(FitWarning {
                code: "estimated_accounting".into(),
                message: "Local accounting is an estimate, not proof of provider acceptance."
                    .into(),
            });
        }
        for (enabled, code, message) in [
            (
                self.score_metadata.outside_training_group_counts,
                "outside_training_group_counts",
                "Scorer group counts are outside the training profile; quality is unqualified.",
            ),
            (
                self.score_metadata.cross_window_context_unqualified,
                "cross_window_context_unqualified",
                "Multiple scoring windows lack qualified cross-window context.",
            ),
        ] {
            if enabled {
                warnings.push(FitWarning {
                    code: code.into(),
                    message: message.into(),
                });
            }
        }
        let receipt = Receipt {
            kind: "tokenfold.fit".into(),
            schema_version: "1.0".into(),
            format: current.request.format,
            budget_mode: self.options.budget_mode,
            budget_status: budget_status(count, self.options.target_tokens),
            reason: if self.fits(count) && count.basis == CountBasis::Estimated {
                Some(FitReason::EstimatedFit)
            } else {
                reason
            },
            target_tokens: self.options.target_tokens,
            original_tokens: raw.tokens,
            baseline_tokens: baseline.tokens,
            final_tokens: count.tokens,
            accounting: Accounting {
                counter_id: self.options.counter_id.clone(),
                scope: current.request.scope,
                original_basis: raw.basis,
                baseline_basis: baseline.basis,
                final_basis: count.basis,
            },
            core_transforms: if index == self.original.unwrap() && self.pass == RenderPass::Original
            {
                vec![]
            } else {
                current.request.core_transforms.clone()
            },
            scorer_attempted: self.scorer_attempted,
            scorer_succeeded: self.scorer_succeeded,
            selection_applied,
            scorer: if self.scorer_attempted {
                self.options.select.clone()
            } else {
                None
            },
            groups_kept: self.kept.clone(),
            groups_dropped: self
                .input
                .source_groups()
                .iter()
                .filter(|g| !self.kept.contains(&g.id))
                .map(|g| g.id.clone())
                .collect(),
            warnings,
            timings_ms: TimingsMs {
                score_queue: self.score_metadata.score_queue_ms,
                startup: self.score_metadata.startup_ms,
                scoring: self.score_metadata.scoring_ms,
                ..TimingsMs::default()
            },
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

#[cfg(all(test, feature = "tiktoken"))]
mod selection_cache_tests {
    use super::*;
    #[test]
    fn trimming_keeps_only_one_current_candidate_slot() {
        let input = ContextRequest::new(
            "q".into(),
            (0..12)
                .map(|i| ContextGroup {
                    id: format!("g{i}"),
                    content: GroupContent::Text("x".into()),
                    required: false,
                })
                .collect(),
        )
        .unwrap();
        let pin = ScorerInfo {
            revision: "rev".into(),
            manifest_hash: "manifest".into(),
            runtime_id: "runtime".into(),
            device: "cpu".into(),
            precision: "fp32".into(),
            qualification: "unqualified".into(),
        };
        let mut session = FitSession::new(
            Box::new(input),
            FitOptions {
                target_tokens: 100,
                counter_id: "test".into(),
                allow_selection: true,
                select: Some(pin),
                ..FitOptions::default()
            },
        )
        .unwrap();
        let mut action = session.advance(None).unwrap();
        let mut counts = 0;
        loop {
            assert!(
                session.cache.len() <= 3,
                "original/baseline alias plus required plus one current"
            );
            action = match action {
                FitAction::Count {
                    candidate_id,
                    request,
                    ..
                } => {
                    counts += 1;
                    let count = if request.payload == RenderedPayload::Text("q".into()) {
                        1
                    } else {
                        200
                    };
                    session
                        .advance(Some(FitReply::Counted {
                            candidate_id,
                            counter_id: "test".into(),
                            scope: request.scope,
                            count: TokenCount::exact(count),
                        }))
                        .unwrap()
                }
                FitAction::Score {
                    action_id,
                    groups,
                    scorer,
                    ..
                } => {
                    let scores = groups
                        .into_iter()
                        .map(|g| GroupScore {
                            id: g.id,
                            score: 1.,
                        })
                        .collect();
                    session
                        .advance(Some(FitReply::Scored {
                            action_id,
                            scores,
                            scorer,
                        }))
                        .unwrap()
                }
                FitAction::Done(output) => {
                    assert!(output.receipt.selection_applied);
                    assert_eq!(output.receipt.groups_kept.len(), 0);
                    assert_eq!(counts, 13);
                    break;
                }
            };
        }
    }
}
