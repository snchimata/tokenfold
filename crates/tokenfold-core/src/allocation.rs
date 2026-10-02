//! EP-06 structured allocation: atomic groups, caller-declared requirements, and an optional
//! versioned scorer contract.
//!
//! # Why this is separate from `json_prune`
//!
//! `transforms::json_prune` decides *which array items* in a document are droppable. This module
//! decides *which caller-declared groups of items survive a budget*. It holds no JSON traversal
//! and no document rewriting: the caller supplies already-scored candidates, this module spends a
//! budget over them, and the caller applies the result. That keeps the allocation rule reusable
//! by any caller (pruning, observation, a future Select runtime) instead of entangling a budget
//! walk with one transform's internals.
//!
//! # The three properties this module exists to guarantee
//!
//! 1. **Groups are atomic.** A group is kept whole or dropped whole. Splitting one would emit a
//!    record whose meaning depends on a record that is no longer there -- the classic failure
//!    where a tool's error row survives but the request it answers does not.
//! 2. **Requirements are declared, never inferred.** A caller marks a group `required` and it is
//!    kept. Nothing here infers that a high-ranking row makes another row unnecessary, because
//!    that inference is exactly the silent data loss this package exists to prevent.
//! 3. **Selection is never empty.** A failing or absent scorer falls back to a declared strategy.
//!    There is no path through this module that returns zero candidates because a scorer
//!    misbehaved; the worst case is "weaker ranking", never "no context at all".
//!
//! # Trust boundary
//!
//! A scorer is untrusted input. It can only *rank* candidates: the response type carries scores and
//! nothing else, so a scorer structurally cannot rewrite candidate text or demand that a candidate
//! be retained. Every response is validated (schema version, ID alignment, finite scores, batch
//! size) before it is allowed to influence selection, and any failure discards it in favor of the
//! declared fallback.

use std::collections::BTreeMap;

/// The version of the scorer request/response contract this build speaks.
///
/// Bumped whenever the wire shape changes incompatibly. A response carrying a different version is
/// rejected rather than best-effort parsed, so a stale scorer fails into the fallback instead of
/// silently scoring against a shape it does not understand.
pub const SELECT_SCHEMA_VERSION: u32 = 1;

/// The largest candidate batch one scorer request may carry.
///
/// A bound, not a tuning knob: an unbounded batch is a memory-growth lever on a path that runs
/// inside a compression pipeline, and a request too large to answer in time is better refused up
/// front than timed out halfway.
pub const MAX_SELECT_BATCH: usize = 512;

/// One indivisible allocation unit within a group.
///
/// `id` is opaque and caller-assigned; this module never parses it. It is echoed back and forth
/// solely so a scorer's answers can be matched to their candidates.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: String,
    /// Cost of retaining this candidate, in the caller's own unit (tokens in every real use).
    pub cost: usize,
    /// Caller-supplied relevance score. Higher is more relevant. A caller with no scorer can leave
    /// every score at `0.0` and rely on source order instead.
    pub score: f64,
}

/// A set of candidates that must be kept or dropped together.
///
/// `required` is the caller's explicit declaration, not an inference. A required group is always
/// retained even when it does not fit the budget; the overflow is reported rather than hidden, so
/// the caller can see that the budget was not met and decide what to do about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub id: String,
    pub members: Vec<Candidate>,
    pub required: bool,
}

impl Group {
    /// Total cost of retaining every member.
    pub fn total_cost(&self) -> usize {
        self.members.iter().map(|m| m.cost).sum()
    }

    /// Highest member score, or `0.0` for an empty group so an empty required group sorts
    /// predictably instead of participating in NaN comparisons.
    pub fn best_score(&self) -> f64 {
        self.members
            .iter()
            .map(|m| m.score)
            .fold(f64::NEG_INFINITY, f64::max)
            .max(0.0)
    }
}

/// What the allocator actually did.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AllocationReport {
    /// Groups retained, counted in source order.
    pub kept_groups: usize,
    /// Groups dropped entirely.
    pub dropped_groups: usize,
    /// Required groups that were retained.
    pub kept_required: usize,
    /// Cost of everything retained.
    pub retained_cost: usize,
    /// How far required content exceeded `budget_tokens`. Zero when requirements fit. Never
    /// negative.
    pub required_overflow: usize,
    /// True when the budget could not be honored because required content exceeded it. The
    /// requirements were kept anyway: dropping a declared requirement to meet a budget would make
    /// the budget the more important contract, which it is not.
    pub over_budget: bool,
    /// True when no scorer was consulted and the declared fallback ranking was used.
    pub used_fallback_ranking: bool,
}

/// Spends `budget_tokens` over `groups`, keeping whole groups only.
///
/// Deterministic by construction: required groups are retained unconditionally, then the rest are
/// taken by descending score with ties broken by position in the input, so the same input always
/// produces the same output. The returned ids are in **source order**, not selection order, so
/// applying them to a source document cannot reorder it.
pub fn allocate(groups: &[Group], budget_tokens: usize) -> (Vec<String>, AllocationReport) {
    let mut report = AllocationReport::default();
    let mut kept_flags = vec![false; groups.len()];

    let mut retained_cost = 0usize;
    let mut required_cost = 0usize;
    for (index, group) in groups.iter().enumerate() {
        if !group.required {
            continue;
        }
        // Retained even when it does not fit. The budget is the softer contract; a caller that
        // declared a requirement explicitly would rather exceed its budget than lose the content.
        kept_flags[index] = true;
        retained_cost += group.total_cost();
        required_cost += group.total_cost();
        report.kept_required += 1;
    }

    let mut optional: Vec<(usize, &Group)> = groups
        .iter()
        .enumerate()
        .filter(|(_, g)| !g.required)
        .collect();
    optional.sort_by(|(ia, a), (ib, b)| {
        b.best_score()
            .partial_cmp(&a.best_score())
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(ia.cmp(ib))
    });

    for (index, group) in optional {
        let cost = group.total_cost();
        // A zero-cost group is always worth keeping: there is no reason to drop evidence for free.
        if retained_cost + cost <= budget_tokens {
            retained_cost += cost;
            kept_flags[index] = true;
        }
    }

    let mut kept_ids = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        if kept_flags[index] {
            kept_ids.push(group.id.clone());
            report.kept_groups += 1;
        } else {
            report.dropped_groups += 1;
        }
    }
    report.retained_cost = retained_cost;
    if required_cost > budget_tokens {
        report.over_budget = true;
        report.required_overflow = required_cost - budget_tokens;
    }
    (kept_ids, report)
}

/// Why a scorer's answer could not be used.
///
/// Every variant is a *reason to use the declared fallback*, never a reason to return nothing.
/// There is deliberately no variant meaning "the candidates were all bad" -- a scorer cannot
/// empty the selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectRejection {
    /// The response declared a schema version this build does not speak.
    SchemaVersion { got: u32 },
    /// The response's ids are not exactly the request's ids -- missing, extra, or duplicated.
    IdMismatch,
    /// A score was NaN or infinite, which would make every ranking comparison meaningless.
    NonFiniteScore { id: String },
    /// The request or response exceeded [`MAX_SELECT_BATCH`].
    BatchTooLarge { got: usize },
    /// The scorer did not answer, or answered so slowly it was abandoned.
    Unavailable,
}

/// A versioned request for relevance scores over a caller's candidates.
///
/// Carries no text: only opaque ids. The scorer that receives it already has whatever content it
/// is allowed to see, and this type cannot be used to smuggle a request for the document itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectRequest {
    pub schema_version: u32,
    /// The caller's retrieval question, if it has one. Opaque to this module.
    pub query: String,
    /// Pins the scorer's model revision so a result can be attributed to an exact version.
    pub model_revision: String,
    /// Candidate ids to score, in source order.
    pub candidate_ids: Vec<String>,
}

impl SelectRequest {
    /// A well-formed request for `candidate_ids`.
    ///
    /// Returns `Err(BatchTooLarge)` rather than truncating: silently scoring a subset would let a
    /// caller believe every candidate had been considered.
    pub fn new(
        query: impl Into<String>,
        model_revision: impl Into<String>,
        candidate_ids: Vec<String>,
    ) -> Result<Self, SelectRejection> {
        if candidate_ids.len() > MAX_SELECT_BATCH {
            return Err(SelectRejection::BatchTooLarge {
                got: candidate_ids.len(),
            });
        }
        Ok(Self {
            schema_version: SELECT_SCHEMA_VERSION,
            query: query.into(),
            model_revision: model_revision.into(),
            candidate_ids,
        })
    }
}

/// A scorer's answer: one finite score per requested candidate, and nothing else.
///
/// The shape is the trust boundary. There is no field for text, no field for "retain this", and no
/// free-form payload, so a scorer cannot do anything except rank -- it physically cannot alter
/// what is kept beyond influencing order, and it cannot force retention of anything.
#[derive(Debug, Clone, PartialEq)]
pub struct SelectResponse {
    pub schema_version: u32,
    pub model_revision: String,
    /// `(candidate_id, score)`, one per requested candidate.
    pub scores: Vec<(String, f64)>,
}

/// Checks a response against the request that produced it.
///
/// Rejects, in order: a wrong schema version, a batch over [`MAX_SELECT_BATCH`], a non-finite
/// score, and finally any id-set mismatch. Ids are compared as a set through a `BTreeMap`, so
/// duplicates and omissions are both caught rather than one shadowing the other.
pub fn validate_response(
    request: &SelectRequest,
    response: &SelectResponse,
) -> Result<BTreeMap<String, f64>, SelectRejection> {
    if response.schema_version != SELECT_SCHEMA_VERSION {
        return Err(SelectRejection::SchemaVersion {
            got: response.schema_version,
        });
    }
    if response.scores.len() > MAX_SELECT_BATCH {
        return Err(SelectRejection::BatchTooLarge {
            got: response.scores.len(),
        });
    }

    let mut scored = BTreeMap::new();
    for (id, score) in &response.scores {
        if !score.is_finite() {
            return Err(SelectRejection::NonFiniteScore { id: id.clone() });
        }
        // A duplicate id means the scorer answered a question it was not asked; `insert`
        // returning a value proves the collision rather than letting the last write win.
        if scored.insert(id.clone(), *score).is_some() {
            return Err(SelectRejection::IdMismatch);
        }
    }

    if scored.len() != request.candidate_ids.len() {
        return Err(SelectRejection::IdMismatch);
    }
    if !request
        .candidate_ids
        .iter()
        .all(|id| scored.contains_key(id))
    {
        return Err(SelectRejection::IdMismatch);
    }
    Ok(scored)
}

/// The result of asking a scorer for help: which groups survived, and what actually happened.
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredAllocation {
    pub kept_group_ids: Vec<String>,
    pub report: AllocationReport,
    /// `None` when the scorer's ranking was used, or the reason it was discarded.
    pub rejection: Option<SelectRejection>,
}

/// Allocates with an optional scorer's ranking, falling back safely on any scorer problem.
///
/// This is the entry point a caller should use whenever a scorer is configured. The guarantee it
/// exists to provide: **the output is never empty and requirements are never dropped because a
/// scorer misbehaved.** A scorer that is absent, timed out, or answers wrongly contributes nothing
/// beyond ordering; the declared fallback ranking (the caller's own scores, i.e. source order for a
/// caller with none) is used instead, and the reason is reported rather than hidden.
pub fn allocate_with_scorer(
    groups: &[Group],
    budget_tokens: usize,
    scored: Option<BTreeMap<String, f64>>,
) -> ScoredAllocation {
    // An empty group list legitimately yields an empty selection -- there is nothing to keep, and
    // inventing a placeholder would be worse than saying so.
    if groups.is_empty() {
        return ScoredAllocation {
            kept_group_ids: Vec::new(),
            report: AllocationReport::default(),
            rejection: None,
        };
    }

    let rejection = match &scored {
        Some(map) if map.len() != groups.len() => Some(SelectRejection::IdMismatch),
        Some(map) if !groups.iter().all(|g| map.contains_key(&g.id)) => {
            Some(SelectRejection::IdMismatch)
        }
        Some(map) if map.values().any(|s| !s.is_finite()) => Some(
            map.iter()
                .find(|(_, s)| !s.is_finite())
                .map(|(id, _)| SelectRejection::NonFiniteScore { id: id.clone() })
                .unwrap_or(SelectRejection::IdMismatch),
        ),
        Some(_) => None,
        None => Some(SelectRejection::Unavailable),
    };

    let used_scores = rejection.is_none();
    let rescored: Vec<Group> = groups
        .iter()
        .map(|group| {
            let mut group = group.clone();
            // The scored map is keyed by GROUP id -- that is what `allocate_with_scorer`
            // validates against above, so it is the only key it may look up here. Applying the
            // group's score to every member keeps `best_score` reflecting the scorer rather than
            // a stale per-member score the caller set before scoring happened.
            if let Some(score) = scored
                .as_ref()
                .filter(|_| used_scores)
                .and_then(|map| map.get(&group.id))
            {
                for member in &mut group.members {
                    member.score = *score;
                }
            }
            group
        })
        .collect();

    let (kept_group_ids, mut report) = allocate(&rescored, budget_tokens);
    report.used_fallback_ranking = !used_scores;
    ScoredAllocation {
        kept_group_ids,
        report,
        rejection,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(id: &str, cost: usize, score: f64) -> Candidate {
        Candidate {
            id: id.to_string(),
            cost,
            score,
        }
    }

    fn group(id: &str, members: Vec<Candidate>, required: bool) -> Group {
        Group {
            id: id.to_string(),
            members,
            required,
        }
    }

    fn request(ids: &[&str]) -> SelectRequest {
        SelectRequest::new(
            "why did the build fail",
            "fake-scorer@1",
            ids.iter().map(|s| s.to_string()).collect(),
        )
        .unwrap()
    }

    fn response(ids: &[&str], scores: &[f64]) -> SelectResponse {
        SelectResponse {
            schema_version: SELECT_SCHEMA_VERSION,
            model_revision: "fake-scorer@1".to_string(),
            scores: ids
                .iter()
                .zip(scores)
                .map(|(id, s)| (id.to_string(), *s))
                .collect(),
        }
    }

    // --- atomic groups ---------------------------------------------------------

    #[test]
    fn a_group_is_kept_whole_or_dropped_whole_never_split() {
        // A paired tool record whose answer is dropped alongside its request would leave an
        // answer that refers to nothing.
        let groups = vec![
            group(
                "paired-a",
                vec![candidate("req", 30, 0.9), candidate("res", 30, 0.9)],
                false,
            ),
            group("solo", vec![candidate("solo", 5, 0.5)], false),
        ];
        // A budget that fits the solo item and only half the pair.
        let (kept, report) = allocate(&groups, 40);
        assert_eq!(kept, vec!["solo".to_string()]);
        assert_eq!(report.dropped_groups, 1);

        // A budget that fits the whole pair must not drop half of it.
        let (kept, report) = allocate(&groups, 60);
        assert_eq!(kept, vec!["paired-a".to_string()]);
        assert_eq!(report.dropped_groups, 1);
    }

    #[test]
    fn a_required_group_is_kept_even_when_it_cannot_fit() {
        // Required content is a declared requirement, not a suggestion. Exceeding the budget is
        // reported; silently dropping the requirement is not an option.
        let groups = vec![group(
            "required-units",
            vec![candidate("a", 100, 0.1), candidate("b", 100, 0.1)],
            true,
        )];
        let (kept, report) = allocate(&groups, 10);
        assert_eq!(kept, vec!["required-units".to_string()]);
        assert_eq!(report.kept_required, 1);
        assert!(report.over_budget);
        assert_eq!(report.required_overflow, 190);
        // The overflow is measured against the budget, not against zero.
        assert_eq!(report.retained_cost, 200);
    }

    #[test]
    fn required_content_still_yields_the_remaining_budget_to_optional_groups() {
        let groups = vec![
            group("req", vec![candidate("r", 40, 0.0)], true),
            group("opt-high", vec![candidate("h", 30, 0.9)], false),
            group("opt-low", vec![candidate("l", 30, 0.1)], false),
        ];
        let (kept, report) = allocate(&groups, 80);
        assert_eq!(kept, vec!["req".to_string(), "opt-high".to_string()]);
        assert!(!report.over_budget);
        assert_eq!(report.retained_cost, 70);
    }

    #[test]
    fn a_high_ranking_row_never_makes_another_row_unnecessary() {
        // Explicitly the inference EP-06 forbids: relevance is not a substitute for
        // requirement. A high score never suppresses a declared requirement.
        let groups = vec![
            group("top-ranked", vec![candidate("t", 1, 100.0)], false),
            group("required-low-rank", vec![candidate("r", 100, 0.001)], true),
        ];
        let (kept, report) = allocate(&groups, 1);
        assert!(kept.contains(&"required-low-rank".to_string()));
        assert!(report.over_budget);
    }

    #[test]
    fn source_order_is_preserved_after_selection() {
        // Selection happens by score, but the result must be applied in source order so a
        // document is never reordered by the allocator.
        let groups = vec![
            group("first", vec![candidate("f", 10, 0.1)], false),
            group("second", vec![candidate("s", 10, 0.9)], false),
            group("third", vec![candidate("t", 10, 0.5)], false),
        ];
        assert_eq!(allocate(&groups, 100).0, vec!["first", "second", "third"]);
        assert_eq!(allocate(&groups, 20).0, vec!["second", "third"]);
    }

    #[test]
    fn allocation_is_deterministic_across_repeated_runs() {
        let groups: Vec<Group> = (0..12)
            .map(|i| {
                group(
                    &format!("g{i}"),
                    vec![candidate(&format!("c{i}"), 10 + i, (i % 4) as f64)],
                    false,
                )
            })
            .collect();
        let first = allocate(&groups, 60).0;
        for _ in 0..8 {
            assert_eq!(allocate(&groups, 60).0, first);
        }
    }

    #[test]
    fn a_zero_cost_group_is_never_dropped() {
        // There is no budget argument for discarding evidence that costs nothing.
        let groups = vec![group("free", vec![candidate("f", 0, 0.0)], false)];
        assert_eq!(allocate(&groups, 0).0, vec!["free".to_string()]);
    }

    #[test]
    fn an_empty_group_is_handled_without_producing_nan_rankings() {
        // An empty required group must not poison the sort with a NaN score.
        let groups = vec![
            group("empty-required", vec![], true),
            group("normal", vec![candidate("n", 10, 0.5)], false),
        ];
        let (kept, report) = allocate(&groups, 10);
        assert_eq!(kept, vec!["empty-required", "normal"]);
        assert_eq!(report.kept_required, 1);
        assert!(!report.over_budget);
    }

    #[test]
    fn conflicting_evidence_in_one_group_keeps_the_group_intact() {
        // Keeping only the "winning" row would hide the conflict the caller must resolve. A
        // budget that fits one row but not both must keep BOTH or neither.
        let groups = vec![
            group(
                "conflict",
                vec![candidate("claim-a", 40, 0.9), candidate("claim-b", 40, 0.9)],
                false,
            ),
            group("other", vec![candidate("o", 5, 0.1)], false),
        ];
        assert_eq!(allocate(&groups, 80).0, vec!["conflict".to_string()]);
        assert_eq!(allocate(&groups, 45).0, vec!["other".to_string()]);
    }

    // --- scorer contract -------------------------------------------------------

    #[test]
    fn a_well_formed_response_validates_to_the_scores_it_carries() {
        let scored =
            validate_response(&request(&["a", "b"]), &response(&["a", "b"], &[0.25, 0.75]))
                .unwrap();
        assert_eq!(scored.get("a"), Some(&0.25));
        assert_eq!(scored.get("b"), Some(&0.75));
    }

    #[test]
    fn the_request_pins_a_schema_version_and_a_model_revision() {
        // Attribution requires an exact version, not a floating "latest".
        let req = request(&["a"]);
        assert_eq!(req.schema_version, SELECT_SCHEMA_VERSION);
        assert_eq!(req.model_revision, "fake-scorer@1");
    }

    #[test]
    fn a_response_from_a_different_schema_version_is_rejected() {
        let mut resp = response(&["a"], &[1.0]);
        resp.schema_version = SELECT_SCHEMA_VERSION + 1;
        assert_eq!(
            validate_response(&request(&["a"]), &resp),
            Err(SelectRejection::SchemaVersion {
                got: SELECT_SCHEMA_VERSION + 1
            })
        );
    }

    #[test]
    fn a_response_naming_a_different_candidate_set_is_rejected() {
        let req = request(&["a", "b"]);
        assert_eq!(
            validate_response(&req, &response(&["a"], &[1.0])),
            Err(SelectRejection::IdMismatch)
        );
        assert_eq!(
            validate_response(&req, &response(&["a", "c"], &[1.0, 1.0])),
            Err(SelectRejection::IdMismatch)
        );
    }

    #[test]
    fn a_duplicate_candidate_id_is_rejected_rather_than_last_write_wins() {
        let req = request(&["a"]);
        assert_eq!(
            validate_response(&req, &response(&["a", "a"], &[0.1, 0.9])),
            Err(SelectRejection::IdMismatch)
        );
    }

    #[test]
    fn a_non_finite_score_is_rejected() {
        // NaN makes every ranking comparison meaningless; infinity lets one candidate dominate
        // the budget unconditionally.
        let req = request(&["a", "b"]);
        for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                validate_response(&req, &response(&["a", "b"], &[1.0, bad])),
                Err(SelectRejection::NonFiniteScore {
                    id: "b".to_string()
                }),
                "a non-finite score was accepted: {bad}"
            );
        }
    }

    #[test]
    fn an_oversized_batch_is_refused_up_front_rather_than_truncated() {
        let ids: Vec<String> = (0..=MAX_SELECT_BATCH).map(|i| format!("c{i}")).collect();
        assert_eq!(
            SelectRequest::new("q", "rev", ids),
            Err(SelectRejection::BatchTooLarge {
                got: MAX_SELECT_BATCH + 1
            })
        );
    }

    #[test]
    fn a_batch_at_the_limit_is_accepted() {
        let ids: Vec<String> = (0..MAX_SELECT_BATCH).map(|i| format!("c{i}")).collect();
        assert!(SelectRequest::new("q", "rev", ids).is_ok());
    }

    #[test]
    fn a_scorer_cannot_force_retention_of_anything() {
        // The trust boundary as a test: even an enormously-scored optional group loses to a
        // budget, and a low scorer score does not demote a declared requirement.
        let groups = vec![
            group("huge", vec![candidate("h", 100, 0.0)], false),
            group("required", vec![candidate("r", 10, 0.0)], true),
        ];
        let out = allocate_with_scorer(
            &groups,
            20,
            Some(BTreeMap::from([
                ("huge".to_string(), 1e9),
                ("required".to_string(), -1e9),
            ])),
        );
        assert!(out.kept_group_ids.contains(&"required".to_string()));
        assert_eq!(out.rejection, None);
    }

    #[test]
    fn a_missing_scorer_falls_back_instead_of_returning_nothing() {
        // "Every scorer failure selects a declared fallback, never empty context."
        let groups = vec![
            group("a", vec![candidate("ca", 10, 0.9)], false),
            group("b", vec![candidate("cb", 10, 0.1)], false),
        ];
        let out = allocate_with_scorer(&groups, 20, None);
        assert_eq!(out.rejection, Some(SelectRejection::Unavailable));
        assert!(out.report.used_fallback_ranking);
        assert_eq!(out.kept_group_ids.len(), 2);
    }

    #[test]
    fn a_misaligned_scorer_response_falls_back_without_changing_requirements() {
        let groups = vec![
            group("required", vec![candidate("r", 10, 0.0)], true),
            group("optional", vec![candidate("o", 10, 0.0)], false),
        ];
        let out = allocate_with_scorer(
            &groups,
            10,
            Some(BTreeMap::from([("ghost".to_string(), 1.0)])),
        );
        assert_eq!(out.rejection, Some(SelectRejection::IdMismatch));
        assert!(out.kept_group_ids.contains(&"required".to_string()));
        assert!(!out.kept_group_ids.is_empty());
    }

    #[test]
    fn a_non_finite_scorer_score_falls_back_rather_than_poisoning_the_sort() {
        let groups = vec![
            group("required", vec![candidate("r", 10, 0.0)], true),
            group("a", vec![candidate("ca", 10, 0.0)], false),
            group("b", vec![candidate("cb", 10, 0.0)], false),
        ];
        let out = allocate_with_scorer(
            &groups,
            20,
            Some(BTreeMap::from([
                ("required".to_string(), f64::NAN),
                ("a".to_string(), 1.0),
                ("b".to_string(), 2.0),
            ])),
        );
        assert!(matches!(
            out.rejection,
            Some(SelectRejection::NonFiniteScore { .. })
        ));
        assert!(out.kept_group_ids.contains(&"required".to_string()));
        assert_eq!(out.kept_group_ids.len(), 2);
    }

    #[test]
    fn a_valid_scorer_changes_the_ranking_and_is_reported_as_used() {
        let groups = vec![
            group("low", vec![candidate("l", 10, 0.1)], false),
            group("high", vec![candidate("h", 10, 0.9)], false),
        ];
        let out = allocate_with_scorer(
            &groups,
            10,
            Some(BTreeMap::from([
                ("low".to_string(), 0.99),
                ("high".to_string(), 0.01),
            ])),
        );
        assert_eq!(out.rejection, None);
        assert!(!out.report.used_fallback_ranking);
        assert_eq!(out.kept_group_ids, vec!["low".to_string()]);
    }

    #[test]
    fn an_empty_group_list_returns_empty_rather_than_a_placeholder() {
        // There is genuinely nothing to keep; a placeholder would be worse than saying so.
        let out = allocate_with_scorer(&[], 100, None);
        assert!(out.kept_group_ids.is_empty());
    }

    /// The deterministic fake scorer EP-06 asks to start with: in-process, no model loading, no
    /// download, and stable for a given input.
    fn fake_scorer(req: &SelectRequest) -> SelectResponse {
        let ids: Vec<&str> = req.candidate_ids.iter().map(String::as_str).collect();
        response(&ids, &vec![1.0; ids.len()])
    }

    #[test]
    fn the_deterministic_fake_scorer_drives_a_real_allocation_end_to_end() {
        let req = request(&["lo", "hi"]);
        let scored = validate_response(&req, &fake_scorer(&req))
            .expect("the fake scorer must be well-formed");
        let groups = vec![
            group("lo", vec![candidate("lo", 10, 0.0)], false),
            group("hi", vec![candidate("hi", 10, 0.0)], false),
        ];
        let out = allocate_with_scorer(&groups, 10, Some(scored));
        assert_eq!(out.rejection, None);
        assert_eq!(out.kept_group_ids.len(), 1);
    }

    #[test]
    fn the_fake_scorer_is_stable_for_the_same_request() {
        let req = request(&["a", "b", "c"]);
        assert_eq!(fake_scorer(&req), fake_scorer(&req));
    }
}
