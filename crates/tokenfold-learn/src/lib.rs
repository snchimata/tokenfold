//! Phase 6 "discover"/"learn" policy mining (`roadmap.md`: "`discover`/`learn` proposes policy
//! changes without silently changing defaults").
//!
//! This crate is pure data-in/data-out: [`propose_policy_changes`] takes a slice of
//! [`tokenfold_core::stats::LedgerRecord`] (the only durable history tokenfold keeps — see
//! `tokenfold-core::stats`) and returns proposed policy changes as plain data. It never performs
//! file I/O and never reads or writes `tokenfold.toml` itself; a `tokenfold-cli` `learn`/
//! `discover` subcommand (wired up separately) is responsible for reading the ledger, printing
//! these proposals by default, and only writing config changes when the user passes `--apply`.
//! Keeping this crate free of I/O keeps that "propose, never silently apply" boundary structural
//! rather than a convention someone can forget.

use tokenfold_core::stats::LedgerRecord;

/// Minimum number of records a group must have before it can produce a [`PolicyProposal`].
/// Below this, a group's mean `savings_pct` is dominated by a handful of sessions and would
/// produce a noisy, low-confidence recommendation — better to say nothing than to suggest a
/// policy change off a tiny, unrepresentative sample.
pub const MIN_SAMPLE_SIZE: usize = 5;

/// One proposed policy change, derived purely from observed [`LedgerRecord`] history.
///
/// `scope` names the group of records the observation was drawn from (currently a bare `mode`
/// string, e.g. `"conservative"`, since `LedgerRecord` has no per-transform-id breakdown to
/// group by anything finer). `observation` is a human-readable summary of what was measured;
/// `suggestion` is the proposed change; `confidence` is a `0.0..=1.0` score derived from sample
/// size; `sample_size` is the exact number of records the observation was computed from.
#[derive(Debug, Clone, PartialEq)]
pub struct PolicyProposal {
    pub scope: String,
    pub observation: String,
    pub suggestion: String,
    pub confidence: f64,
    pub sample_size: usize,
}

/// Mines `records` for policy-change proposals. Pure and deterministic: the same input slice
/// always produces the same output `Vec`, sorted by `scope`.
///
/// Only `status == "compressed"` records are considered — that is the only status
/// (`crate::status::Status` in `tokenfold-core`, see its `passthrough`/`best_effort`/
/// `unreachable_target` siblings) where `savings_pct` reflects compression that actually applied,
/// rather than a passthrough or best-effort fallback whose `savings_pct` doesn't mean the same
/// thing. Records are grouped by `mode`.
///
/// Exactly one heuristic is implemented today: if the `"conservative"` mode group has at least
/// [`MIN_SAMPLE_SIZE`] records and its mean `savings_pct` is below `15.0`, propose trying
/// `balanced` mode instead. Every other group — or a `"conservative"` group that doesn't meet
/// the sample-size floor or is already saving enough — produces no proposal. This is
/// deliberately a single concrete, testable rule rather than a speculative rule engine; new
/// heuristics should be added the same way, one at a time, once there's a real signal for them.
pub fn propose_policy_changes(records: &[LedgerRecord]) -> Vec<PolicyProposal> {
    let mut groups: std::collections::BTreeMap<&str, Vec<&LedgerRecord>> =
        std::collections::BTreeMap::new();
    for record in records {
        if record.status == "compressed" {
            groups.entry(record.mode.as_str()).or_default().push(record);
        }
    }

    let mut proposals = Vec::new();

    if let Some(conservative) = groups.get("conservative") {
        let sample_size = conservative.len();
        if sample_size >= MIN_SAMPLE_SIZE {
            let mean_savings_pct =
                conservative.iter().map(|r| r.savings_pct).sum::<f64>() / sample_size as f64;
            if mean_savings_pct < 15.0 {
                proposals.push(PolicyProposal {
                    scope: "conservative".to_string(),
                    observation: format!(
                        "{sample_size} conservative-mode sessions averaged {mean_savings_pct:.1}% savings"
                    ),
                    suggestion: "consider trying balanced mode as your default for higher realized savings"
                        .to_string(),
                    confidence: (sample_size as f64 / 50.0).min(1.0),
                    sample_size,
                });
            }
        }
    }

    proposals.sort_by(|a, b| a.scope.cmp(&b.scope));
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(mode: &str, status: &str, savings_pct: f64, request_id: &str) -> LedgerRecord {
        LedgerRecord {
            request_id: request_id.to_string(),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            surface: "cli".to_string(),
            format: "plain_text".to_string(),
            mode: mode.to_string(),
            status: status.to_string(),
            original_tokens: 100,
            compressed_tokens: 90,
            saved_tokens: 10,
            savings_pct,
            bypass_reason: None,
            project_hash: None,
        }
    }

    #[test]
    fn proposes_when_conservative_savings_are_low_and_sample_is_large_enough() {
        let records: Vec<LedgerRecord> = (0..MIN_SAMPLE_SIZE + 1)
            .map(|i| record("conservative", "compressed", 8.0, &format!("tc-{i}")))
            .collect();

        let proposals = propose_policy_changes(&records);

        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].scope, "conservative");
        assert!(!proposals[0].suggestion.is_empty());
    }

    #[test]
    fn no_proposal_below_minimum_sample_size() {
        let records: Vec<LedgerRecord> = (0..MIN_SAMPLE_SIZE - 1)
            .map(|i| record("conservative", "compressed", 5.0, &format!("tc-{i}")))
            .collect();

        assert!(propose_policy_changes(&records).is_empty());
    }

    #[test]
    fn no_proposal_when_savings_are_already_good() {
        let records: Vec<LedgerRecord> = (0..MIN_SAMPLE_SIZE + 1)
            .map(|i| record("conservative", "compressed", 40.0, &format!("tc-{i}")))
            .collect();

        assert!(propose_policy_changes(&records).is_empty());
    }

    #[test]
    fn deterministic_output() {
        let records: Vec<LedgerRecord> = (0..MIN_SAMPLE_SIZE + 1)
            .map(|i| record("conservative", "compressed", 8.0, &format!("tc-{i}")))
            .collect();

        let first = propose_policy_changes(&records);
        let second = propose_policy_changes(&records);

        assert_eq!(first, second);
    }
}
