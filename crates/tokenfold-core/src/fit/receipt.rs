use super::*;
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounting {
    pub counter_id: String,
    pub scope: AccountingScope,
    pub original_basis: CountBasis,
    pub baseline_basis: CountBasis,
    pub final_basis: CountBasis,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScorerInfo {
    pub revision: String,
    pub manifest_hash: String,
    pub runtime_id: String,
    pub device: String,
    pub precision: String,
    pub qualification: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FitWarning {
    pub code: String,
    pub message: String,
}
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimingsMs {
    pub total: Option<f64>,
    pub core: Option<f64>,
    pub accounting: Option<f64>,
    pub score_queue: Option<f64>,
    pub startup: Option<f64>,
    pub scoring: Option<f64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Receipt {
    pub kind: String,
    pub schema_version: String,
    pub format: FitFormat,
    pub budget_mode: BudgetMode,
    pub budget_status: BudgetStatus,
    pub reason: Option<FitReason>,
    pub target_tokens: u64,
    pub original_tokens: Option<u64>,
    pub baseline_tokens: Option<u64>,
    pub final_tokens: Option<u64>,
    pub accounting: Accounting,
    pub core_transforms: Vec<String>,
    pub scorer_attempted: bool,
    pub scorer_succeeded: bool,
    pub selection_applied: bool,
    pub scorer: Option<ScorerInfo>,
    pub groups_kept: Vec<GroupId>,
    pub groups_dropped: Vec<GroupId>,
    pub warnings: Vec<FitWarning>,
    pub timings_ms: TimingsMs,
}
impl Receipt {
    pub fn to_json(&self) -> Result<String, FitError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| FitError::InvalidInput("invalid receipt".into()))
    }
    pub fn from_json(source: &str) -> Result<Self, FitError> {
        let value = parse_json_unique(source)?;
        fn required(value: &serde_json::Value, keys: &[&str]) -> Result<(), FitError> {
            if !value.is_object() || keys.iter().any(|k| value.get(k).is_none()) {
                return Err(FitError::InvalidInput("missing receipt field".into()));
            }
            Ok(())
        }
        required(
            &value,
            &[
                "kind",
                "schema_version",
                "format",
                "budget_mode",
                "budget_status",
                "reason",
                "target_tokens",
                "original_tokens",
                "baseline_tokens",
                "final_tokens",
                "accounting",
                "core_transforms",
                "scorer_attempted",
                "scorer_succeeded",
                "selection_applied",
                "scorer",
                "groups_kept",
                "groups_dropped",
                "warnings",
                "timings_ms",
            ],
        )?;
        required(
            &value["accounting"],
            &[
                "counter_id",
                "scope",
                "original_basis",
                "baseline_basis",
                "final_basis",
            ],
        )?;
        required(
            &value["timings_ms"],
            &[
                "total",
                "core",
                "accounting",
                "score_queue",
                "startup",
                "scoring",
            ],
        )?;
        let receipt: Self = serde_json::from_value(value)
            .map_err(|_| FitError::InvalidInput("invalid receipt schema".into()))?;
        receipt.validate()?;
        Ok(receipt)
    }
    pub fn tokens_saved(&self) -> Option<i64> {
        delta(
            self.original_tokens,
            self.accounting.original_basis,
            self.final_tokens,
            self.accounting.final_basis,
        )
    }
    pub fn selection_tokens_saved(&self) -> Option<i64> {
        delta(
            self.baseline_tokens,
            self.accounting.baseline_basis,
            self.final_tokens,
            self.accounting.final_basis,
        )
    }
    pub fn estimated_fit(&self) -> Option<bool> {
        (self.accounting.final_basis == CountBasis::Estimated)
            .then_some(self.final_tokens? <= self.target_tokens)
    }
    pub fn validate(&self) -> Result<(), FitError> {
        let invalid = || FitError::InvalidInput("inconsistent fit receipt".into());
        if self.kind != "tokenfold.fit"
            || self.schema_version != "1.0"
            || self.target_tokens > MAX_SAFE_INTEGER
        {
            return Err(invalid());
        }
        for count in [
            TokenCount {
                tokens: self.original_tokens,
                basis: self.accounting.original_basis,
            },
            TokenCount {
                tokens: self.baseline_tokens,
                basis: self.accounting.baseline_basis,
            },
            TokenCount {
                tokens: self.final_tokens,
                basis: self.accounting.final_basis,
            },
        ] {
            count.validate().map_err(|_| invalid())?;
        }
        let final_count = TokenCount {
            tokens: self.final_tokens,
            basis: self.accounting.final_basis,
        };
        if budget_status(final_count, self.target_tokens) != self.budget_status {
            return Err(invalid());
        }
        if (self.budget_status == BudgetStatus::Met) != self.reason.is_none() {
            return Err(invalid());
        }
        let accepted_estimate =
            self.budget_mode == BudgetMode::BestEffort && self.estimated_fit() == Some(true);
        if (self.reason == Some(FitReason::EstimatedFit)) != accepted_estimate {
            return Err(invalid());
        }
        if (self.accounting.final_basis == CountBasis::Unavailable
            || (self.budget_mode == BudgetMode::Strict
                && self.budget_status == BudgetStatus::Indeterminate))
            && (self.reason != Some(FitReason::UnknownAccounting) || self.scorer_attempted)
        {
            return Err(invalid());
        }
        if (matches!(
            self.format,
            FitFormat::OpenaiChat | FitFormat::AnthropicMessages
        )) != (self.accounting.scope == AccountingScope::ProviderRequest)
        {
            return Err(invalid());
        }
        bounded_metadata(&self.accounting.counter_id, 256)?;
        if self.scorer_attempted != self.scorer.is_some()
            || (self.scorer_succeeded && !self.scorer_attempted)
        {
            return Err(invalid());
        }
        let scorer_failed = matches!(
            self.reason,
            Some(
                FitReason::ScorerFailure
                    | FitReason::ScorerTimeout
                    | FitReason::ScorerResponseInvalid
            )
        );
        if scorer_failed != (self.scorer_attempted && !self.scorer_succeeded) {
            return Err(invalid());
        }
        match self.reason {
            Some(
                FitReason::SelectionDisabled
                | FitReason::RuntimeUnavailable
                | FitReason::NoOptionalGroups
                | FitReason::RequiredOverBudget
                | FitReason::UnsupportedSelection
                | FitReason::SelectionLimit
                | FitReason::ScorerQueueTimeout,
            ) if self.scorer_attempted => return Err(invalid()),
            Some(FitReason::NoGain | FitReason::NoValidFit) if !self.scorer_succeeded => {
                return Err(invalid());
            }
            _ => {}
        }
        if let Some(scorer) = &self.scorer {
            for s in [
                &scorer.revision,
                &scorer.manifest_hash,
                &scorer.runtime_id,
                &scorer.device,
                &scorer.precision,
            ] {
                bounded_metadata(s, 256)?;
            }
            if scorer.qualification != "unqualified" {
                return Err(invalid());
            }
        }
        if self.groups_kept.len() + self.groups_dropped.len() > 512 {
            return Err(invalid());
        }
        let mut ids = std::collections::HashSet::new();
        for id in self.groups_kept.iter().chain(&self.groups_dropped) {
            validate_id(id, true)?;
            if !ids.insert(id) {
                return Err(invalid());
            }
        }
        if !self.selection_applied && !self.groups_dropped.is_empty() {
            return Err(invalid());
        }
        if self.selection_applied
            && (!self.scorer_succeeded
                || self.groups_dropped.is_empty()
                || self.accounting.final_basis != self.accounting.baseline_basis
                || self.final_tokens.is_none()
                || self.final_tokens >= self.baseline_tokens
                || self.final_tokens.is_none_or(|n| n > self.target_tokens)
                || (self.budget_mode == BudgetMode::Strict
                    && self.accounting.final_basis != CountBasis::Exact))
        {
            return Err(invalid());
        }
        for t in &self.core_transforms {
            bounded_metadata(t, 64)?;
        }
        if self.core_transforms.len() > 32 || self.warnings.len() > 32 {
            return Err(invalid());
        }
        for w in &self.warnings {
            bounded_metadata(&w.code, 64)?;
            bounded_metadata(&w.message, 256)?;
        }
        if self.accounting.final_basis == CountBasis::Estimated
            && !self
                .warnings
                .iter()
                .any(|w| w.code == "estimated_accounting")
        {
            return Err(invalid());
        }
        for timing in [
            self.timings_ms.total,
            self.timings_ms.core,
            self.timings_ms.accounting,
            self.timings_ms.score_queue,
            self.timings_ms.startup,
            self.timings_ms.scoring,
        ]
        .into_iter()
        .flatten()
        {
            if !timing.is_finite() || timing < 0.0 {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
fn delta(
    before: Option<u64>,
    before_basis: CountBasis,
    after: Option<u64>,
    after_basis: CountBasis,
) -> Option<i64> {
    if before_basis == CountBasis::Unavailable || before_basis != after_basis {
        return None;
    }
    Some(before? as i64 - after? as i64)
}
fn bounded_metadata(text: &str, limit: usize) -> Result<(), FitError> {
    if text.is_empty() || text.len() > limit || text.chars().any(char::is_control) {
        return Err(FitError::InvalidInput("invalid receipt metadata".into()));
    }
    validate_text(text)
}
