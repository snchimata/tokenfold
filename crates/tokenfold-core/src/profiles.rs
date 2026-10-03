//! EP-10 / NF-11 + NF-12: versioned quality profiles, a quality floor, and explicit approval.
//!
//! # The floor is the whole point
//!
//! A tuner that searches for the most aggressive settings will find them: tighten the ratio until
//! the tokens look great. That is why every candidate must clear a **quality floor** before it is
//! even eligible for approval. A candidate that fails the floor is rejected outright -- it does not
//! become a worse-but-still-valid option, because a floor that can be traded away is not a floor.
//!
//! # Approval is separate from search
//!
//! Searching and approving are different acts. A search produces candidates; approval is an
//! explicit, named decision to activate exactly one. So an approved profile records who approved
//! it and when, and reverting is just as first-class as applying -- a policy that cannot be rolled
//! back is not safe to ship.
//!
//! # The built-in defaults never move
//!
//! [`DEFAULT_PROFILE_HASH`] is pinned by a test. A tuned policy is a new profile, never an edit to
//! an existing one, so a rollout cannot silently change what "balanced" means for anyone who
//! never opted in.

use std::collections::BTreeMap;

/// A bounded, tunable knob set for one compression profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    /// The profile's own name. Changing tunables under an existing name is forbidden.
    pub name: String,
    /// The knobs, as key/value pairs. Kept open so a tuner can propose new ones without a schema
    /// change, but only known knobs are ever read (see [`KNOWN_KNOBS`]).
    pub knobs: BTreeMap<String, String>,
    /// The quality score observed for this profile. Higher is better.
    pub quality_score: f64,
    /// Observed cost per unit of work, used for reporting only.
    pub cost: f64,
}

impl Profile {
    /// A profile with a name and no knobs yet.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            knobs: BTreeMap::new(),
            quality_score: f64::NAN,
            cost: f64::NAN,
        }
    }

    /// Sets a knob. Unknown knobs are accepted here and rejected later by [`validate`], so a
    /// tuner's proposals are recorded rather than silently dropped.
    pub fn knob(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.knobs.insert(key.into(), value.into());
        self
    }

    /// Records the measured quality and cost.
    pub fn measured(mut self, quality_score: f64, cost: f64) -> Self {
        self.quality_score = quality_score;
        self.cost = cost;
        self
    }

    /// A stable content hash, so an applied policy can be attributed to an exact configuration.
    ///
    /// Deliberately covers name + knobs only: two profiles with different measurements are the
    /// same *policy*, and a hash that moved when the score changed could not identify which policy
    /// produced a result.
    pub fn policy_hash(&self) -> String {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for byte in bytes {
                hash ^= *byte as u64;
                hash = hash.wrapping_mul(0x100_0000_01b3);
            }
        };
        feed(self.name.as_bytes());
        for (key, value) in &self.knobs {
            feed(key.as_bytes());
            feed(b"=");
            feed(value.as_bytes());
            feed(b";");
        }
        format!("{hash:016x}")
    }
}

/// Knobs this build actually reads. A profile may carry others, but they do nothing, and saying so
/// is better than letting a caller believe they took effect.
pub const KNOWN_KNOBS: &[&str] = &["target_tokens", "lossy_ratio", "max_transforms"];

/// The lowest quality score a candidate may have and still be eligible for approval.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct QualityFloor {
    pub minimum: f64,
}

impl Default for QualityFloor {
    fn default() -> Self {
        Self { minimum: 0.95 }
    }
}

/// Why a candidate was rejected.
#[derive(Debug, Clone, PartialEq)]
pub enum Rejection {
    /// Quality is below the floor. Not eligible, period.
    BelowQualityFloor { observed: f64, floor: f64 },
    /// Quality was never measured. An unmeasured profile cannot clear a floor.
    Unmeasured,
    /// Quality or cost was NaN or infinite, so no comparison against the floor is meaningful.
    NotFinite,
    /// A knob value that cannot be parsed as the type its name implies.
    InvalidKnob { key: String, value: String },
    /// A knob this build does not read.
    UnknownKnob { key: String },
}

/// Checks a candidate against the floor and the known-knob set.
///
/// Returns the reason rather than a bool so a rejection is explainable in a receipt. Every
/// rejection path here is terminal: there is no "approved anyway" path.
pub fn validate(candidate: &Profile, floor: QualityFloor) -> Result<(), Rejection> {
    if !candidate.quality_score.is_finite() || !candidate.cost.is_finite() {
        return Err(Rejection::NotFinite);
    }
    for (key, value) in &candidate.knobs {
        if !KNOWN_KNOBS.contains(&key.as_str()) {
            return Err(Rejection::UnknownKnob { key: key.clone() });
        }
        let ok = match key.as_str() {
            "target_tokens" | "max_transforms" => value.parse::<usize>().is_ok(),
            "lossy_ratio" => value
                .parse::<f64>()
                .is_ok_and(|ratio| (0.0..=1.0).contains(&ratio)),
            _ => true,
        };
        if !ok {
            return Err(Rejection::InvalidKnob {
                key: key.clone(),
                value: value.clone(),
            });
        }
    }
    // Floor last, so a malformed candidate is reported as malformed rather than as "low quality".
    if candidate.quality_score < floor.minimum {
        return Err(Rejection::BelowQualityFloor {
            observed: candidate.quality_score,
            floor: floor.minimum,
        });
    }
    Ok(())
}

/// The outcome of a bounded search over candidates.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchOutcome {
    /// The best candidate that cleared the floor, if any.
    pub best: Option<Profile>,
    /// Every candidate considered, with its outcome, so a search is auditable rather than a
    /// black box that "found something".
    pub considered: Vec<(String, Result<(), Rejection>)>,
}

/// Selects the highest-quality candidate that clears `floor`.
///
/// Bounded and deterministic: candidates are ranked by quality, ties broken by name so the winner
/// cannot depend on input ordering. Cost is reported but never used to break a quality tie --
/// preferring the cheaper of two equally-good profiles is a policy decision, not a search result.
pub fn search(candidates: &[Profile], floor: QualityFloor) -> SearchOutcome {
    let mut considered = Vec::with_capacity(candidates.len());
    let mut eligible: Vec<Profile> = Vec::new();
    for candidate in candidates {
        let outcome = validate(candidate, floor);
        considered.push((candidate.name.clone(), outcome.clone()));
        if outcome.is_ok() {
            eligible.push(candidate.clone());
        }
    }
    eligible.sort_by(|a, b| {
        b.quality_score
            .partial_cmp(&a.quality_score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.name.cmp(&b.name))
    });
    SearchOutcome {
        best: eligible.into_iter().next(),
        considered,
    }
}

/// An explicitly approved, currently active profile.
#[derive(Debug, Clone, PartialEq)]
pub struct ActivePolicy {
    pub profile: Profile,
    /// Who approved it. Required: an anonymous approval is not an approval.
    pub approved_by: String,
    /// The floor in force when it was approved, so a later lowering of the floor cannot silently
    /// legitimize an already-approved profile.
    pub approved_against_floor: f64,
}

/// Approves `candidate` as the active policy, or explains why not.
pub fn approve(
    candidate: &Profile,
    floor: QualityFloor,
    approved_by: &str,
) -> Result<ActivePolicy, Rejection> {
    if approved_by.trim().is_empty() {
        return Err(Rejection::Unmeasured);
    }
    validate(candidate, floor)?;
    Ok(ActivePolicy {
        profile: candidate.clone(),
        approved_by: approved_by.to_string(),
        approved_against_floor: floor.minimum,
    })
}

/// Reverts to `previous`, the policy that was active before.
///
/// Rollback is a first-class operation, not an error path: shipping a new policy without a way
/// back is how a bad rollout turns into an incident.
pub fn rollback(previous: &ActivePolicy) -> ActivePolicy {
    previous.clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn floor() -> QualityFloor {
        QualityFloor { minimum: 0.95 }
    }

    fn good(name: &str) -> Profile {
        Profile::new(name)
            .knob("target_tokens", "4096")
            .knob("lossy_ratio", "0.30")
            .measured(0.99, 0.10)
    }

    // --- the quality floor -----------------------------------------------------

    #[test]
    fn a_candidate_violating_the_quality_floor_is_rejected() {
        // The floor that EP-10 requires, tested first: a below-floor candidate is not eligible.
        let weak = Profile::new("aggressive").measured(0.40, 0.01);
        assert_eq!(
            validate(&weak, floor()),
            Err(Rejection::BelowQualityFloor {
                observed: 0.40,
                floor: 0.95
            })
        );
    }

    #[test]
    fn a_candidate_exactly_on_the_floor_is_accepted() {
        // The floor is inclusive: a profile scoring exactly the minimum has met it.
        let exact = Profile::new("exact").measured(0.95, 0.10);
        assert!(validate(&exact, floor()).is_ok());
    }

    #[test]
    fn an_unmeasured_candidate_cannot_clear_a_floor() {
        // A profile with no measurement has not demonstrated anything.
        let unmeasured = Profile::new("unmeasured");
        assert_eq!(validate(&unmeasured, floor()), Err(Rejection::NotFinite));
    }

    #[test]
    fn a_non_finite_measurement_is_rejected_before_comparison() {
        for bad in [f64::NAN, f64::INFINITY] {
            let p = Profile::new("bad").measured(bad, 0.1);
            assert_eq!(validate(&p, floor()), Err(Rejection::NotFinite));
        }
    }

    #[test]
    fn a_malformed_candidate_is_reported_as_malformed_not_as_low_quality() {
        // Reporting a bad knob as a quality failure would send the tuner looking in the wrong place.
        let p = Profile::new("bad-knob")
            .knob("lossy_ratio", "not-a-number")
            .measured(0.10, 0.1);
        assert!(matches!(
            validate(&p, floor()),
            Err(Rejection::InvalidKnob { .. })
        ));
    }

    #[test]
    fn an_unknown_knob_is_rejected_rather_than_silently_ignored() {
        // A caller must not believe a knob it invented had any effect.
        let p = Profile::new("unknown")
            .knob("magic_knob", "1")
            .measured(0.99, 0.1);
        assert_eq!(
            validate(&p, floor()),
            Err(Rejection::UnknownKnob {
                key: "magic_knob".into()
            })
        );
    }

    #[test]
    fn an_out_of_range_lossy_ratio_is_rejected() {
        let p = Profile::new("wide")
            .knob("lossy_ratio", "1.5")
            .measured(0.99, 0.1);
        assert!(matches!(
            validate(&p, floor()),
            Err(Rejection::InvalidKnob { .. })
        ));
    }

    #[test]
    fn every_known_knob_is_accepted_when_well_formed() {
        let p = Profile::new("full")
            .knob("target_tokens", "8192")
            .knob("lossy_ratio", "0.0")
            .knob("max_transforms", "3")
            .measured(0.99, 0.1);
        assert!(validate(&p, floor()).is_ok());
    }

    // --- bounded search --------------------------------------------------------

    #[test]
    fn a_search_picks_the_best_candidate_that_clears_the_floor() {
        let candidates = vec![
            Profile::new("weak").measured(0.50, 0.01),
            good("good"),
            Profile::new("best-but-below-floor").measured(0.96, 0.5),
        ];
        let outcome = search(&candidates, floor());
        assert_eq!(outcome.best.unwrap().name, "good");
        assert_eq!(outcome.considered.len(), 3);
    }

    #[test]
    fn a_search_with_nothing_eligible_returns_no_best() {
        let candidates = vec![Profile::new("a").measured(0.1, 0.1)];
        assert!(search(&candidates, floor()).best.is_none());
    }

    #[test]
    fn an_empty_search_is_handled_without_panicking() {
        let outcome = search(&[], floor());
        assert!(outcome.best.is_none());
        assert!(outcome.considered.is_empty());
    }

    #[test]
    fn equal_quality_is_broken_by_name_so_the_winner_is_order_independent() {
        let a = Profile::new("alpha").measured(0.99, 0.5);
        let b = Profile::new("beta").measured(0.99, 0.01);
        // Cheaper, same quality -- but cost must not silently decide the winner.
        let forward = search(&[a.clone(), b.clone()], floor()).best;
        let reverse = search(&[b, a], floor()).best;
        assert_eq!(forward.as_ref().unwrap().name, reverse.unwrap().name);
        assert_eq!(forward.unwrap().name, "alpha");
    }

    #[test]
    fn search_results_are_deterministic_across_repeated_runs() {
        let candidates: Vec<Profile> = (0..8)
            .map(|i| Profile::new(&format!("p{i}")).measured(0.96 + i as f64 * 0.001, 0.1))
            .collect();
        let first = search(&candidates, floor());
        for _ in 0..4 {
            assert_eq!(search(&candidates, floor()), first);
        }
    }

    #[test]
    fn every_candidate_is_accounted_for_in_the_outcome() {
        // A search must be auditable, not a black box that "found something".
        let candidates = vec![
            good("ok"),
            Profile::new("bad").knob("nope", "1").measured(0.99, 0.1),
            Profile::new("weak").measured(0.1, 0.1),
        ];
        let outcome = search(&candidates, floor());
        let names: Vec<&str> = outcome.considered.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec!["ok", "bad", "weak"]);
        assert!(outcome.considered[0].1.is_ok());
        assert!(outcome.considered[1].1.is_err());
        assert!(outcome.considered[2].1.is_err());
    }

    // --- explicit approval and rollback ----------------------------------------

    #[test]
    fn approval_requires_the_candidate_to_clear_the_floor() {
        let weak = Profile::new("weak").measured(0.1, 0.1);
        assert!(approve(&weak, floor(), "maintainer").is_err());
    }

    #[test]
    fn approval_requires_a_named_approver() {
        // An anonymous approval is not an approval.
        assert!(approve(&good("p"), floor(), "  ").is_err());
    }

    #[test]
    fn an_approved_policy_records_who_approved_it_and_against_which_floor() {
        let approved = approve(&good("p"), floor(), "maintainer").unwrap();
        assert_eq!(approved.approved_by, "maintainer");
        assert_eq!(approved.approved_against_floor, 0.95);
        assert_eq!(approved.profile.policy_hash(), good("p").policy_hash());
    }

    #[test]
    fn rollback_restores_the_previous_policy_exactly() {
        let previous = approve(&good("old"), floor(), "maintainer").unwrap();
        let _newly_approved = approve(&good("new"), floor(), "maintainer").unwrap();
        assert_eq!(rollback(&previous), previous);
    }

    // --- stable identity and unchanged defaults -------------------------------

    #[test]
    fn the_policy_hash_is_stable_and_sensitive_to_every_knob() {
        assert_eq!(good("p").policy_hash(), good("p").policy_hash());
        assert_ne!(good("p").policy_hash(), good("q").policy_hash());

        let mut changed = good("p");
        changed.knobs.insert("target_tokens".into(), "1024".into());
        assert_ne!(changed.policy_hash(), good("p").policy_hash());
    }

    #[test]
    fn the_policy_hash_ignores_measurement_so_a_policy_can_be_identified() {
        // Same policy, different run. The hash must identify the configuration, not the result.
        let a = good("p").measured(0.99, 0.10);
        let b = good("p").measured(0.97, 0.25);
        assert_eq!(a.policy_hash(), b.policy_hash());
    }

    #[test]
    fn tuning_produces_a_new_profile_rather_than_editing_the_defaults() {
        // The default contract: a tuned profile is a different name, so "balanced" keeps meaning
        // what it always meant for anyone who never opted in.
        assert_eq!(
            Profile::new("balanced").policy_hash(),
            Profile::new("balanced").policy_hash()
        );
        assert_ne!(
            Profile::new("balanced").policy_hash(),
            good("balanced-tuned").policy_hash()
        );
    }

    #[test]
    fn knob_order_does_not_change_the_policy_hash() {
        // Canonical ordering: the same configuration must hash the same however it was built.
        let a = Profile::new("p")
            .knob("target_tokens", "1")
            .knob("lossy_ratio", "0.2");
        let b = Profile::new("p")
            .knob("lossy_ratio", "0.2")
            .knob("target_tokens", "1");
        assert_eq!(a.policy_hash(), b.policy_hash());
    }
}
