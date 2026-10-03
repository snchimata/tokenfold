//! EP-13 / NF-22: an isolated, optional semantic strategy with a guaranteed fallback.
//!
//! # The rule this module exists to enforce
//!
//! A semantic strategy is allowed to be *wrong*. What it is never allowed to do is take the
//! deterministic path down with it. So the strategy runs in isolation behind a trait, and **every**
//! failure mode -- inventing a fact, timing out, corrupting a citation, returning nonsense, or
//! simply not existing -- resolves to the same declared fallback: the lossless baseline, untouched.
//!
//! The failure is always *loud* too. [`SemanticOutcome::reason`] carries the cause, so a silent
//! degradation is visible in a receipt rather than being mistaken for "the strategy just ran and
//! saved nothing".
//!
//! # What this deliberately does not do
//!
//! It loads no model and downloads nothing. The strategy is a trait, so the deterministic fake used
//! by the tests and any future real runtime are interchangeable. Model loading belongs in the
//! runtime, never in Core, and an unauthorized download is not something this module can perform.

use std::time::Duration;

/// The input handed to a strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryRequest {
    /// The text to summarize, already redacted and already framed by the lossless path.
    pub content: String,
    /// Hard cap the strategy must respect, in the strategy's own unit.
    pub max_output_bytes: usize,
}

/// One assertion the summary makes, paired with where it says that came from.
///
/// Claims are what make "did this strategy invent something?" answerable at all. A free-form
/// summary cannot be checked; a list of attributed claims can: each one's text must appear in the
/// input, and each one's source must resolve. Anything else is an invention or a bad citation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    /// The asserted content. Must occur verbatim in the request content.
    pub text: String,
    /// Where the claim came from. Must also occur in the request content.
    pub source: String,
}

/// What a strategy produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    /// The summarized text.
    pub text: String,
    /// Every factual assertion, each attributed. An unverified claim is an invention.
    pub claims: Vec<Claim>,
}

/// Why a strategy's answer was not usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StrategyFailure {
    /// The strategy asserted a fact that does not appear in the input. This is the failure mode
    /// that makes summarization dangerous, so it is checked, not assumed away.
    InventedFact { claimed: String },
    /// The strategy took longer than its allowance.
    TimedOut { elapsed_ms: u64, budget_ms: u64 },
    /// The summary cites a source that is not in the request.
    CorruptCitation { cited: String },
    /// The strategy produced something unusable for an ordinary reason.
    Unusable { detail: String },
    /// No strategy is configured at all.
    NotConfigured,
}

/// Why the final answer is what it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticDisposition {
    /// The strategy ran and its answer passed every check.
    Summarized,
    /// The strategy failed and the lossless baseline was returned instead.
    FellBack,
    /// The feature is off.
    Disabled,
}

/// A semantic strategy. Implemented by the deterministic fake in tests and by any future runtime.
pub trait SemanticStrategy {
    /// A stable name for reporting. Not required to be unique across processes.
    fn name(&self) -> &str;

    /// Summarize. Implementations report their own failures through the error type; this module
    /// never assumes a returned value is trustworthy.
    fn summarize(&self, request: &SummaryRequest) -> Result<Summary, StrategyFailure>;
}

/// The result of attempting a semantic strategy, always usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SemanticOutcome {
    /// The bytes to actually send. Either the verified summary or the untouched baseline.
    pub bytes: Vec<u8>,
    pub disposition: SemanticDisposition,
    /// `None` on success; otherwise why the fallback was used.
    pub reason: Option<StrategyFailure>,
    /// Which strategy produced the answer, or attempted to.
    pub strategy: String,
}

/// Verifies a strategy's answer against the request it was given.
///
/// This is the security-relevant step. A strategy is an untrusted component: it can be wrong, and
/// the failure mode that matters is a *confident* wrong answer, because a downstream reader cannot
/// tell it from a true one.
fn verify(summary: &Summary, request: &SummaryRequest) -> Result<(), StrategyFailure> {
    if summary.text.len() > request.max_output_bytes {
        return Err(StrategyFailure::Unusable {
            detail: format!(
                "summary is {} bytes, over the {} byte cap",
                summary.text.len(),
                request.max_output_bytes
            ),
        });
    }
    if summary.text.trim().is_empty() {
        return Err(StrategyFailure::Unusable {
            detail: "summary is empty".into(),
        });
    }
    for claim in &summary.claims {
        // An invented fact: an assertion whose text is not in the input at all. This is the
        // failure that makes summarization dangerous, because the output reads as confident.
        if !request.content.contains(claim.text.as_str()) {
            return Err(StrategyFailure::InventedFact {
                claimed: claim.text.clone(),
            });
        }
        // A corrupt citation: a real fact attributed to a source that is not there.
        if !request.content.contains(claim.source.as_str()) {
            return Err(StrategyFailure::CorruptCitation {
                cited: claim.source.clone(),
            });
        }
    }
    Ok(())
}

/// Runs `strategy` over `baseline`, falling back to `baseline` on any problem.
///
/// `baseline` must already be lossless and redacted: it is the safe answer, and this function's
/// whole job is to make sure it is what ships when anything goes wrong.
pub fn summarize(
    baseline: &[u8],
    request: &SummaryRequest,
    strategy: Option<&dyn SemanticStrategy>,
    budget: Duration,
) -> SemanticOutcome {
    let untouched = || SemanticOutcome {
        bytes: baseline.to_vec(),
        disposition: SemanticDisposition::FellBack,
        strategy: strategy
            .map(|s| s.name().to_string())
            .unwrap_or_else(|| "none".to_string()),
        reason: None,
    };

    let Some(strategy) = strategy else {
        return SemanticOutcome {
            reason: Some(StrategyFailure::NotConfigured),
            ..untouched()
        };
    };

    // The strategy runs first and is treated as untrusted output from this point on.
    let started = std::time::Instant::now();
    let summary = match strategy.summarize(request) {
        Ok(summary) => summary,
        Err(failure) => {
            return SemanticOutcome {
                reason: Some(failure),
                ..untouched()
            };
        }
    };
    let elapsed = started.elapsed();
    // Enforced after the call because the strategy is opaque: we cannot interrupt it, so the only
    // honest options are to notice and fall back, or to pretend it was fast enough. This does the
    // former, which means a slow strategy costs latency but never correctness.
    if elapsed > budget {
        return SemanticOutcome {
            reason: Some(StrategyFailure::TimedOut {
                elapsed_ms: elapsed.as_millis() as u64,
                budget_ms: budget.as_millis() as u64,
            }),
            ..untouched()
        };
    }

    if let Err(failure) = verify(&summary, request) {
        return SemanticOutcome {
            reason: Some(failure),
            ..untouched()
        };
    }

    SemanticOutcome {
        bytes: summary.text.into_bytes(),
        disposition: SemanticDisposition::Summarized,
        reason: None,
        strategy: strategy.name().to_string(),
    }
}

/// A strategy that fails in a specific, named way. Each variant is one hostile behavior the
/// fallback must survive.
struct HostileStrategy {
    behavior: Behavior,
    delay: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Behavior {
    /// Returns a summary asserting something absent from the input.
    Invent,
    /// Sleeps past its allowance.
    Slow,
    /// Cites a source the request never contained.
    BadCitation,
    /// Returns an oversized summary.
    Oversized,
    /// Returns empty output.
    Empty,
    /// Reports an ordinary failure.
    Error,
    /// Behaves correctly.
    Good,
}

impl SemanticStrategy for HostileStrategy {
    fn name(&self) -> &str {
        "hostile-test-strategy"
    }

    fn summarize(&self, request: &SummaryRequest) -> Result<Summary, StrategyFailure> {
        if self.delay > Duration::ZERO {
            std::thread::sleep(self.delay);
        }
        match self.behavior {
            Behavior::Invent => Ok(Summary {
                text: "the build failed because of a disk quota".into(),
                // Asserted, but nowhere in the request.
                claims: vec![Claim {
                    text: "disk quota".into(),
                    source: "build failed".into(),
                }],
            }),
            Behavior::Slow => Ok(Summary {
                text: "a late but plausible summary".into(),
                claims: vec![Claim {
                    text: "build failed".into(),
                    source: "build failed".into(),
                }],
            }),
            Behavior::BadCitation => Ok(Summary {
                text: "a plausible summary of the request".into(),
                // The fact is real; the attribution is not.
                claims: vec![Claim {
                    text: "build failed".into(),
                    source: "a source that was never in the request".into(),
                }],
            }),
            Behavior::Oversized => Ok(Summary {
                text: "x".repeat(request.max_output_bytes + 1),
                claims: vec![],
            }),
            Behavior::Empty => Ok(Summary {
                text: "   ".into(),
                claims: vec![],
            }),
            Behavior::Error => Err(StrategyFailure::Unusable {
                detail: "the strategy gave up".into(),
            }),
            Behavior::Good => Ok(Summary {
                text: "a verified summary of the build failure".into(),
                // Every claim is real and correctly attributed.
                claims: vec![Claim {
                    text: "build failed".into(),
                    source: "parser.rs".into(),
                }],
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASELINE: &[u8] = b"the build failed with a parse error in parser.rs";
    const CONTENT: &str = "the build failed with a parse error in parser.rs";

    fn request() -> SummaryRequest {
        SummaryRequest {
            content: CONTENT.to_string(),
            max_output_bytes: 200,
        }
    }

    fn strategy(behavior: Behavior) -> HostileStrategy {
        HostileStrategy {
            behavior,
            delay: Duration::ZERO,
        }
    }

    fn budget() -> Duration {
        Duration::from_millis(500)
    }

    // --- every failure falls back ----------------------------------------------

    #[test]
    fn an_inventing_strategy_falls_back_to_the_untouched_baseline() {
        // The dangerous failure: a confident claim the input never supported.
        let out = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::Invent)),
            budget(),
        );
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE, "an invented fact reached the output");
        assert!(matches!(
            out.reason,
            Some(StrategyFailure::InventedFact { .. })
        ));
    }

    #[test]
    fn a_slow_strategy_falls_back_rather_than_shipping_a_late_answer() {
        let slow = HostileStrategy {
            behavior: Behavior::Slow,
            delay: Duration::from_millis(120),
        };
        let out = summarize(BASELINE, &request(), Some(&slow), Duration::from_millis(10));
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE);
        assert!(matches!(out.reason, Some(StrategyFailure::TimedOut { .. })));
    }

    #[test]
    fn a_strategy_citing_something_it_never_received_falls_back() {
        let out = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::BadCitation)),
            budget(),
        );
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE);
        assert!(matches!(
            out.reason,
            Some(StrategyFailure::CorruptCitation { .. })
        ));
    }

    #[test]
    fn an_oversized_summary_falls_back_rather_than_being_truncated() {
        // Truncating a semantic answer mid-sentence would fabricate meaning.
        let out = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::Oversized)),
            budget(),
        );
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE);
    }

    #[test]
    fn an_empty_summary_falls_back() {
        let out = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::Empty)),
            budget(),
        );
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE);
    }

    #[test]
    fn a_strategy_error_falls_back_and_keeps_the_reason() {
        let out = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::Error)),
            budget(),
        );
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE);
        assert_eq!(
            out.reason,
            Some(StrategyFailure::Unusable {
                detail: "the strategy gave up".into()
            })
        );
    }

    #[test]
    fn no_strategy_configured_falls_back_rather_than_failing() {
        let out = summarize(BASELINE, &request(), None, budget());
        assert_eq!(out.disposition, SemanticDisposition::FellBack);
        assert_eq!(out.bytes, BASELINE);
        assert_eq!(out.reason, Some(StrategyFailure::NotConfigured));
    }

    #[test]
    fn the_fallback_is_byte_identical_to_the_baseline_in_every_failure_case() {
        // The strongest form of the guarantee: whatever goes wrong, the safe bytes are unchanged.
        for behavior in [
            Behavior::Invent,
            Behavior::BadCitation,
            Behavior::Oversized,
            Behavior::Empty,
            Behavior::Error,
        ] {
            let out = summarize(BASELINE, &request(), Some(&strategy(behavior)), budget());
            assert_eq!(
                out.bytes, BASELINE,
                "{behavior:?} did not fall back exactly"
            );
            assert!(out.reason.is_some(), "{behavior:?} fell back silently");
        }
    }

    // --- the success path ------------------------------------------------------

    #[test]
    fn a_verified_summary_is_used() {
        let out = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::Good)),
            budget(),
        );
        assert_eq!(out.disposition, SemanticDisposition::Summarized);
        assert_eq!(out.reason, None);
        assert_eq!(out.strategy, "hostile-test-strategy");
        assert!(out.bytes.starts_with(b"a verified summary"));
    }

    #[test]
    fn a_citation_that_resolves_is_accepted() {
        // Citations are checked for resolvability, not banned outright.
        let citing = HostileStrategy {
            behavior: Behavior::Good,
            delay: Duration::ZERO,
        };
        let out = summarize(BASELINE, &request(), Some(&citing), budget());
        assert_eq!(out.disposition, SemanticDisposition::Summarized);
    }

    // --- determinism -----------------------------------------------------------

    #[test]
    fn repeated_attempts_with_the_same_strategy_agree() {
        // A strategy that drifts between turns would make a transcript non-reproducible.
        let first = summarize(
            BASELINE,
            &request(),
            Some(&strategy(Behavior::Invent)),
            budget(),
        );
        for _ in 0..4 {
            let again = summarize(
                BASELINE,
                &request(),
                Some(&strategy(Behavior::Invent)),
                budget(),
            );
            assert_eq!(again, first);
        }
    }

    #[test]
    fn repeated_summaries_do_not_drift_when_the_strategy_is_stable() {
        // Guard against a strategy being applied cumulatively to its own previous output.
        let s = strategy(Behavior::Good);
        let request = request();
        let mut seen = Vec::new();
        for _ in 0..4 {
            let out = summarize(BASELINE, &request, Some(&s), budget());
            assert_eq!(out.disposition, SemanticDisposition::Summarized);
            seen.push(out.bytes);
        }
        assert!(
            seen.windows(2).all(|w| w[0] == w[1]),
            "repeated summarization drifted"
        );
    }
}
