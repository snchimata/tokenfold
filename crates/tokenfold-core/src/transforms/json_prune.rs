//! `json_prune` — Phase 1 opt-in **lossy** JSON array-item selection (canonical id
//! `"json_prune"`). Unlike every other transform in this module, this one is NOT lossless: it
//! drops array items to hit a token budget, replacing each with a recoverable `{"$tf_ref":...}`
//! marker. It is never part of the default lossless pipeline (`modes.rs`/`apply_transforms`) —
//! see `pipeline::apply_lossy_reduction`, which only runs it when `policy.lossy.is_some()`, as a
//! terminal stage strictly after the normal lossless transform loop.
//!
//! Full design/rationale: `docs/solution-design/lossy-json-compression.md` (gitignored,
//! local-only). Summary of the algorithm implemented here:
//!
//! - **Explicit preserve, not inferred force-keep**: an array survives untouched iff its path is
//!   listed in `LossyOptions.preserve_paths`. There is no generic "this row is important"
//!   inference — that was verified to not exist anywhere in this codebase and to not be
//!   invent-able generically.
//! - **Static per-item rank**: value-aware structural failure signal (typed field checks, not
//!   substring matching) > per-array-field median-absolute-deviation numeric outlier > edge
//!   position > original index. Compared lexicographically, no weighted sum, so no cross-scale
//!   calibration problem.
//! - **Diversity as a separate allocator step**: candidates are grouped by a coarse structural
//!   fingerprint and walked round-robin across groups (in each group's own static-rank order),
//!   not folded into the static rank tuple — diversity is set-dependent, the rank tuple isn't.
//! - **One global, shared budget** over every eligible array's items combined (not an
//!   independent ratio per array), spent via a deterministic greedy walk mirroring
//!   `eval/run_baselines.py::allocate()`'s three-tier fit check (proven-safe byte bound →
//!   tested-safe heuristic-plus-margin bound → exact re-tokenize fallback) — never summed
//!   independent per-item token estimates, which are not additive across tokenizer boundaries.
//! - **Deterministic**: no randomness anywhere; the same input always produces the same output.
//!
//! This module never touches `RetrievalStore` — per `transforms/mod.rs`'s convention ("the
//! pipeline owns all bookkeeping"), [`prune`] only decides *which* items to drop and returns
//! their original bytes; `pipeline::apply_lossy_reduction` does the actual (fail-closed) storing
//! and marker substitution.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::retrieval_store::hex_sha256;
use crate::token_estimator::TokenEstimator;

pub const TRANSFORM_ID: &str = "json_prune";
pub const TRANSFORM_VERSION: &str = "1.0.0";

/// Arrays with fewer items than this are never worth pruning — same threshold `json_field_fold`
/// uses for its own "worth folding" early-out (`json_fold::MIN_ROWS`).
const MIN_ARRAY_LEN: usize = 2;

/// Sentinel field key representing "the array item's own scalar value" (used for arrays of bare
/// numbers, as opposed to arrays of objects, in the per-field numeric-outlier stats map).
const SELF_FIELD: &str = "$self";

/// A discrete-outlier score assigned when `MAD == 0` and a value differs from the shared modal
/// value anyway (design doc §4B item 2: deviating from a value the majority of a field's
/// observations share is itself rare, and must not be scored as "no signal"). Any realistic
/// modified z-score stays well under this, so it reliably outranks continuous outliers too.
const DISCRETE_OUTLIER_SCORE: f64 = 1_000.0;

/// Mirrors `eval/run_baselines.py::allocate()`'s empirically-calibrated tested-safe margin
/// constants (design doc §4, "Selection mechanism").
const MARGIN_FLOOR: i64 = 32;
const MARGIN_PER_ITEM: f64 = 0.5;

#[derive(Debug, thiserror::Error)]
pub enum JsonPruneError {
    #[error("invalid json: {0}")]
    Invalid(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct LossyOptions {
    /// Dot-separated object-key paths (e.g. `"items"`, `"data.results"`) whose arrays must never
    /// be pruned. Deliberately a minimal path syntax, not full JSONPath — see `is_preserved`.
    pub preserve_paths: Vec<String>,
    /// BEST-EFFORT selection hint, not an enforced budget (see `CompressionPolicy::lossy_ratio`):
    /// the fraction (0.0..=1.0, clamped) of the prunable pool's own estimated token cost to keep.
    /// It sets `budget_tokens` for the walk below and is never re-checked against the final
    /// document — the pool is only the droppable candidates, so the achieved whole-document ratio
    /// differs by design. `>= 1.0` is treated as "nothing to prune" and short-circuits to
    /// `Ok(None)`.
    pub ratio: f64,
    /// Retrieval namespace `pipeline::apply_lossy_reduction` will store dropped items under —
    /// only used here to size the marker template for cost accounting, never for storage itself.
    pub namespace: String,
}

/// One dropped item's original bytes, keyed by its content hash (== the hash the marker
/// substituted in its place points at). `pipeline::apply_lossy_reduction` persists these
/// fail-closed: an item is only actually removed from the output if `RetrievalStore::store`
/// returns `Ok` for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DroppedItem {
    pub hash: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PruneReport {
    pub eligible_arrays: usize,
    pub total_candidates: usize,
    pub preserved_candidates: usize,
    pub kept_candidates: usize,
    pub dropped_candidates: usize,
}

#[derive(Debug, Clone)]
pub struct PruneOutcome {
    /// The document with every proposed-drop item replaced by a `{"$tf_ref":...}` marker.
    /// `pipeline::apply_lossy_reduction` may still put some items back (fail-closed) if storing
    /// them fails — this is the *proposal*, not the final output.
    pub json: Value,
    pub dropped: Vec<DroppedItem>,
    pub report: PruneReport,
}

/// Runs tiered lossy selection over `input`. Returns `Ok(None)` when there's nothing eligible to
/// prune (no arrays of length >= [`MIN_ARRAY_LEN`] outside preserved paths, or `ratio >= 1.0`) —
/// callers should treat that as a clean no-op, not force an empty transform report.
pub fn prune(
    input: &[u8],
    options: &LossyOptions,
    estimator: &dyn TokenEstimator,
) -> Result<Option<PruneOutcome>, JsonPruneError> {
    if input.is_empty() || options.ratio >= 1.0 {
        return Ok(None);
    }
    let value: Value = serde_json::from_slice(input)?;

    let mut arrays = Vec::new();
    collect_eligible_arrays(&value, String::new(), &mut arrays);
    if arrays.is_empty() {
        return Ok(None);
    }

    let preserved_array: Vec<bool> = arrays
        .iter()
        .map(|a| is_preserved(&a.path, &options.preserve_paths))
        .collect();
    let field_stats: Vec<BTreeMap<String, FieldStats>> = arrays
        .iter()
        .map(|a| numeric_field_stats(&a.items))
        .collect();
    let markers: Vec<Vec<Value>> = arrays
        .iter()
        .map(|a| {
            a.items
                .iter()
                .map(|item| {
                    let bytes = serde_json::to_vec(item).unwrap_or_default();
                    marker_json(&hex_sha256(&bytes), &options.namespace)
                })
                .collect()
        })
        .collect();
    let marker_bytes_cache: Vec<Vec<Vec<u8>>> = markers
        .iter()
        .map(|per_array| {
            per_array
                .iter()
                .map(|m| serde_json::to_vec(m).unwrap_or_default())
                .collect()
        })
        .collect();

    let total_candidates: usize = arrays.iter().map(|a| a.items.len()).sum();
    let preserved_candidates: usize = arrays
        .iter()
        .zip(&preserved_array)
        .filter(|&(_, &p)| p)
        .map(|(a, _)| a.items.len())
        .sum();

    let mut candidates = Vec::new();
    for (array_idx, array) in arrays.iter().enumerate() {
        if preserved_array[array_idx] {
            continue;
        }
        let stats = &field_stats[array_idx];
        let n = array.items.len();
        for (item_idx, item) in array.items.iter().enumerate() {
            candidates.push(Candidate {
                array_idx,
                item_idx,
                bytes: serde_json::to_vec(item).unwrap_or_default(),
                rank: RankKey {
                    failure_signal: has_structural_failure_signal(item),
                    outlier_score: numeric_outlier_score(item, stats),
                    edge_bonus: item_idx == 0 || item_idx + 1 == n,
                },
                fingerprint: structural_fingerprint(item),
            });
        }
    }

    if candidates.is_empty() {
        return Ok(None);
    }

    // Tier 2's running totals must use the SAME estimator as the exact tier 3 and the ceiling
    // below — mixing a cheap heuristic estimator into one side of the interpolation and the real
    // (e.g. tiktoken) estimator into the other would compare two different scales. Per
    // `eval/run_baselines.py::allocate()`'s own design, the "cheap" half of tier 2 comes from
    // tokenizing each unit INDEPENDENTLY once (fast: no growing joint string), not from a
    // different, less accurate estimator — the real estimator is used throughout, just applied
    // per-item instead of to a joint re-tokenize until tier 3.
    let marker_tokens_cache: Vec<Vec<i64>> = marker_bytes_cache
        .iter()
        .map(|per_array| {
            per_array
                .iter()
                .map(|m| estimator.count_bytes(m) as i64)
                .collect()
        })
        .collect();
    let real_tokens_cache: Vec<i64> = candidates
        .iter()
        .map(|c| estimator.count_bytes(&c.bytes) as i64)
        .collect();

    let mut kept_mask: Vec<Vec<bool>> = arrays.iter().map(|a| vec![false; a.items.len()]).collect();

    // A `$tf_ref` marker has a real, fixed cost dominated by its 64-hex-char content hash, which
    // a real tokenizer (BPE has no repeated-pattern structure to exploit in near-random hex)
    // often encodes far LESS efficiently than ordinary text. So "start every candidate as a
    // marker and upgrade toward budget" — the framing everything below assumes — is only sound
    // for candidates where the marker is actually cheaper than the real item. For any candidate
    // where it ISN'T (real_tok <= marker_tok: dropping it can only make things worse or is a
    // wash), auto-keep it unconditionally, for free, with no budget spent — this is not an
    // optimization, it's a correctness requirement: without it, a document made mostly of
    // token-cheap real content and token-expensive markers gets a budget that undershoots even
    // the "drop everything" baseline, and the walk below "correctly" spends nothing while still
    // leaving every item dropped, which is a real regression the pipeline's own final safety
    // gate has to roll back wholesale.
    let mut droppable: Vec<usize> = Vec::new();
    for (idx, c) in candidates.iter().enumerate() {
        let marker_tok = marker_tokens_cache[c.array_idx][c.item_idx];
        if real_tokens_cache[idx] > marker_tok {
            droppable.push(idx);
        } else {
            kept_mask[c.array_idx][c.item_idx] = true;
        }
    }

    let walk_order = diversity_walk_order(&candidates, &droppable);

    let mut pool_tokens: i64 = droppable
        .iter()
        .map(|&idx| marker_tokens_cache[candidates[idx].array_idx][candidates[idx].item_idx])
        .sum();
    // `pool_bytes`/`pool_tokens` start at the marker baseline for the droppable subset (auto-kept
    // candidates above already contribute their real cost, not a marker cost, since they're
    // never markers) and walk upward toward the droppable subset's all-real ceiling as items are
    // upgraded. The budget must live on that SAME scale — a naive `ratio * sum(real bytes)`
    // target would be dwarfed by the marker baseline whenever markers cost more than the items
    // they'd replace, making every candidate look unaffordable regardless of ratio. Interpolating
    // between the two baselines keeps "ratio" meaning what it says: 0.0 stays at all-markers
    // (for the droppable subset only), 1.0 would be all-real (short-circuited above for the
    // whole document, but the droppable subset itself can still be ratio-limited at any value
    // below that), values between move proportionally.
    let mut pool_bytes: i64 = droppable
        .iter()
        .map(|&idx| {
            marker_bytes_cache[candidates[idx].array_idx][candidates[idx].item_idx].len() as i64
        })
        .sum();
    let real_total_tokens: i64 = droppable.iter().map(|&idx| real_tokens_cache[idx]).sum();
    let budget_tokens = pool_tokens
        + ((real_total_tokens - pool_tokens) as f64 * options.ratio.clamp(0.0, 1.0)).round() as i64;
    let mut accepted = 0usize;
    // Tier 3 re-serializes and re-tokenizes the WHOLE candidate's array on every call — O(array
    // length), not O(1). Left uncapped, an array where many candidates land right at the budget
    // boundary (a realistic shape: uniform-size items) falls through to tier 3 for most of them,
    // turning the walk into O(n^2) for that array. Capping how many exact calls any one array
    // may spend bounds tier 3's total cost to O(array length) regardless of how many candidates
    // are borderline — candidates beyond the cap fall back to tiers 1/2 only (a more conservative
    // reject, never a wrong accept: the outer pipeline's own final exact-recount regression check
    // still gates the whole transform, so a slightly-too-conservative per-candidate decision here
    // can only under-prune, never produce a silent regression).
    const MAX_EXACT_TIER_CALLS_PER_ARRAY: usize = 16;
    let mut exact_calls_used = vec![0usize; arrays.len()];

    for &idx in &walk_order {
        let c = &candidates[idx];
        let marker_len = marker_bytes_cache[c.array_idx][c.item_idx].len() as i64;
        let marker_tok = marker_tokens_cache[c.array_idx][c.item_idx];
        let real_len = c.bytes.len() as i64;
        let real_tok = real_tokens_cache[idx];

        let trial_bytes = pool_bytes - marker_len + real_len;
        let trial_tokens = pool_tokens - marker_tok + real_tok;
        let margin = MARGIN_FLOOR + (MARGIN_PER_ITEM * accepted as f64) as i64;
        // Set only when tier 3 actually ran: its joint re-tokenize is the exact cost of this
        // upgrade, so once it has been paid for it must also be what gets CHARGED to the running
        // pool. Charging the cheaper independent tier-2 estimate instead (the old behavior) threw
        // the exact number away and let per-item BPE-boundary error accumulate across every
        // tier-3 acceptance, so the running total drifted from what the same document really
        // costs -- exactly the drift tier 3 exists to eliminate.
        let mut exact_delta: Option<i64> = None;

        // Tier 1 (proven-safe): byte length is always >= token count, so if the byte-bound
        // trial already fits, the real token count fits too — free accept, no estimator call.
        let fits = trial_bytes <= budget_tokens
            // Tier 2 (tested-safe): each unit's own real token count (independently computed,
            // no joint re-tokenize) plus an empirically-calibrated margin — accept without the
            // expensive joint-assembly exact call.
            || trial_tokens + margin <= budget_tokens
            // Tier 3 (exact, the only real decider when 1/2 are inconclusive, capped above): re-
            // tokenize this candidate's OWN array as currently assembled, with and without it
            // upgraded — a real joint re-tokenize, never a summed independent estimate, of the
            // one place non-additive BPE boundary effects actually occur (adjacent items in the
            // same array). The resulting delta is charged against the shared global budget.
            || (exact_calls_used[c.array_idx] < MAX_EXACT_TIER_CALLS_PER_ARRAY && {
                exact_calls_used[c.array_idx] += 1;
                kept_mask[c.array_idx][c.item_idx] = true;
                let with = estimator.count_bytes(&assemble(&arrays[c.array_idx], &kept_mask[c.array_idx], &markers[c.array_idx]));
                kept_mask[c.array_idx][c.item_idx] = false;
                let without = estimator.count_bytes(&assemble(&arrays[c.array_idx], &kept_mask[c.array_idx], &markers[c.array_idx]));
                let delta = with as i64 - without as i64;
                exact_delta = Some(delta);
                pool_tokens + delta <= budget_tokens
            });

        if fits {
            kept_mask[c.array_idx][c.item_idx] = true;
            pool_bytes = trial_bytes;
            pool_tokens = match exact_delta {
                Some(delta) => pool_tokens + delta,
                None => trial_tokens,
            };
            accepted += 1;
        }
    }

    let mut cursor = 0usize;
    let pruned = rewrite_tree(&value, &mut cursor, &preserved_array, &kept_mask, &markers);

    let mut dropped = Vec::new();
    for (array_idx, mask) in kept_mask.iter().enumerate() {
        // Preserved arrays are never candidates in the first place (see the loop that builds
        // `candidates` above), so their `kept_mask` row is left all-`false` by construction —
        // NOT because every item was dropped. `rewrite_tree` already knows this and leaves them
        // untouched; this loop must agree, or every preserved item gets wrongly reported (and
        // persisted to the retrieval store by `pipeline::apply_lossy_reduction`) as dropped.
        if preserved_array[array_idx] {
            continue;
        }
        for (item_idx, &kept) in mask.iter().enumerate() {
            if !kept {
                dropped.push(DroppedItem {
                    hash: hex_sha256(
                        &serde_json::to_vec(&arrays[array_idx].items[item_idx]).unwrap_or_default(),
                    ),
                    bytes: serde_json::to_vec(&arrays[array_idx].items[item_idx])
                        .unwrap_or_default(),
                });
            }
        }
    }

    if dropped.is_empty() {
        return Ok(None);
    }

    let dropped_candidates = dropped.len();
    Ok(Some(PruneOutcome {
        json: pruned,
        dropped,
        report: PruneReport {
            eligible_arrays: arrays.len(),
            total_candidates,
            preserved_candidates,
            kept_candidates: total_candidates - preserved_candidates - dropped_candidates,
            dropped_candidates,
        },
    }))
}

struct RankKey {
    failure_signal: bool,
    outlier_score: f64,
    edge_bonus: bool,
}

impl RankKey {
    /// Higher is more important to keep. `Ordering::Greater` from this means `self` outranks
    /// `other`. Ties are NOT broken here (original index is the final tiebreak, applied by the
    /// stable sort that calls this, per candidate insertion order).
    fn cmp(&self, other: &RankKey) -> Ordering {
        self.failure_signal
            .cmp(&other.failure_signal)
            .then_with(|| {
                self.outlier_score
                    .partial_cmp(&other.outlier_score)
                    .unwrap_or(Ordering::Equal)
            })
            .then_with(|| self.edge_bonus.cmp(&other.edge_bonus))
    }
}

struct Candidate {
    array_idx: usize,
    item_idx: usize,
    bytes: Vec<u8>,
    rank: RankKey,
    fingerprint: String,
}

/// Design §4C: groups candidates by structural fingerprint, ranks within each group by the
/// static [`RankKey`] (highest first — a stable sort, so original insertion order = original
/// array-then-item order is the final tiebreak), then interleaves groups round-robin (groups
/// themselves ordered by their own best member's rank) so an early budget cutoff doesn't drain
/// one group before ever touching another. `BTreeMap` keeps fingerprint iteration
/// lexicographically deterministic — a `HashMap` here would make the walk order (and therefore
/// the output) nondeterministic between runs, since Rust's default hasher is randomized.
/// `eligible` restricts the walk to a subset of `candidates` (the ones actually worth
/// considering for dropping — see the auto-keep partition in `prune`); indices not in `eligible`
/// never appear in the returned order.
fn diversity_walk_order(candidates: &[Candidate], eligible: &[usize]) -> Vec<usize> {
    let mut groups: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for &idx in eligible {
        let c = &candidates[idx];
        groups.entry(c.fingerprint.clone()).or_default().push(idx);
    }
    for members in groups.values_mut() {
        members.sort_by(|&a, &b| candidates[b].rank.cmp(&candidates[a].rank));
    }
    let mut group_order: Vec<String> = groups.keys().cloned().collect();
    group_order.sort_by(|a, b| {
        let ra = &candidates[groups[a][0]].rank;
        let rb = &candidates[groups[b][0]].rank;
        rb.cmp(ra)
    });

    let mut cursors: BTreeMap<String, usize> =
        group_order.iter().map(|k| (k.clone(), 0usize)).collect();
    let mut order = Vec::with_capacity(eligible.len());
    // Round-robin merge across groups (each already non-empty and internally rank-sorted),
    // ordered by group priority. `active` is pruned of drained groups after every round instead
    // of being rescanned in full each time — without this, a document with many small/singleton
    // fingerprint groups plus one large one costs O(rounds * groups) to walk (every drained
    // singleton group gets needlessly re-visited on every remaining round), which is ~O(n^2) for
    // that shape.
    let mut active: Vec<String> = group_order;
    while !active.is_empty() {
        for key in &active {
            let members = &groups[key];
            let cursor = cursors.get_mut(key).expect("seeded above");
            order.push(members[*cursor]);
            *cursor += 1;
        }
        active.retain(|key| cursors[key] < groups[key].len());
    }
    order
}

fn assemble(array: &EligibleArray, kept_mask: &[bool], markers: &[Value]) -> Vec<u8> {
    let items: Vec<Value> = array
        .items
        .iter()
        .zip(kept_mask)
        .zip(markers)
        .map(|((item, &kept), marker)| if kept { item.clone() } else { marker.clone() })
        .collect();
    serde_json::to_vec(&Value::Array(items)).unwrap_or_default()
}

fn marker_json(hash: &str, namespace: &str) -> Value {
    let mut inner = Map::new();
    inner.insert("hash".to_string(), Value::String(hash.to_string()));
    inner.insert("alg".to_string(), Value::String("sha256".to_string()));
    inner.insert(
        "namespace".to_string(),
        Value::String(namespace.to_string()),
    );
    let mut outer = Map::new();
    outer.insert("$tf_ref".to_string(), Value::Object(inner));
    Value::Object(outer)
}

struct EligibleArray {
    path: String,
    items: Vec<Value>,
}

/// Recursively finds every array with `len() >= MIN_ARRAY_LEN` in document order (objects
/// visited key-by-key, arrays visited index-by-index). Deliberately does NOT recurse into an
/// eligible array's own items to look for further nested eligible arrays inside them — doing so
/// would let a dropped parent item "orphan" child candidates that were independently scored,
/// which has no coherent selection semantics. Revisit only if real payloads need multi-level
/// nested pruning; sibling/independent eligible arrays elsewhere in the tree are unaffected by
/// this cut. Must stay traversal-identical to [`rewrite_tree`] — both increment their cursor in
/// lockstep over the same `Value`, which is how the two passes stay correlated without needing a
/// separate stable identity per array instance.
fn collect_eligible_arrays(value: &Value, path: String, out: &mut Vec<EligibleArray>) {
    match value {
        Value::Array(items) if items.len() >= MIN_ARRAY_LEN => {
            out.push(EligibleArray {
                path,
                items: items.clone(),
            });
        }
        Value::Array(items) => {
            for item in items {
                collect_eligible_arrays(item, path.clone(), out);
            }
        }
        Value::Object(map) => {
            for (k, v) in map {
                let child_path = if path.is_empty() {
                    k.clone()
                } else {
                    format!("{path}.{k}")
                };
                collect_eligible_arrays(v, child_path, out);
            }
        }
        _ => {}
    }
}

/// Mirror of [`collect_eligible_arrays`]'s traversal, consuming `preserved_array`/`kept_mask`/
/// `markers` (indexed by the same cursor order) to rebuild the document with dropped items
/// replaced by their markers.
fn rewrite_tree(
    value: &Value,
    cursor: &mut usize,
    preserved_array: &[bool],
    kept_mask: &[Vec<bool>],
    markers: &[Vec<Value>],
) -> Value {
    match value {
        Value::Array(items) if items.len() >= MIN_ARRAY_LEN => {
            let idx = *cursor;
            *cursor += 1;
            if preserved_array[idx] {
                return value.clone();
            }
            let rebuilt: Vec<Value> = items
                .iter()
                .enumerate()
                .map(|(i, item)| {
                    if kept_mask[idx][i] {
                        item.clone()
                    } else {
                        markers[idx][i].clone()
                    }
                })
                .collect();
            Value::Array(rebuilt)
        }
        Value::Array(items) => Value::Array(
            items
                .iter()
                .map(|item| rewrite_tree(item, cursor, preserved_array, kept_mask, markers))
                .collect(),
        ),
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(
                    k.clone(),
                    rewrite_tree(v, cursor, preserved_array, kept_mask, markers),
                );
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

/// Puts specific dropped items back, keyed by content hash — the fail-closed half of the
/// contract (`pipeline::apply_lossy_reduction`): an item whose `RetrievalStore::store` call
/// failed must not be silently lost, so the caller restores it here rather than leaving its
/// `$tf_ref` marker in place. Recognizes exactly the marker shape [`marker_json`] builds; any
/// other node (including one that merely looks similar) is left untouched.
pub fn revert_markers(json: &Value, restore: &std::collections::HashMap<String, Value>) -> Value {
    if let Value::Object(map) = json
        && map.len() == 1
        && let Some(Value::Object(inner)) = map.get("$tf_ref")
        && let Some(Value::String(hash)) = inner.get("hash")
        && let Some(original) = restore.get(hash)
    {
        return original.clone();
    }
    match json {
        Value::Array(items) => {
            Value::Array(items.iter().map(|v| revert_markers(v, restore)).collect())
        }
        Value::Object(map) => {
            let mut out = Map::new();
            for (k, v) in map {
                out.insert(k.clone(), revert_markers(v, restore));
            }
            Value::Object(out)
        }
        _ => json.clone(),
    }
}

/// Minimal dot-separated path match (exact equality, plus one fail-safe fallback) —
/// deliberately not full JSONPath (no `$`, `[*]`, filters, or recursive descent). A whole array
/// is preserved or it isn't; there's no field-level partial preservation in Phase 1.
///
/// `collect_eligible_arrays` never recurses into an array once it's deemed eligible, so a
/// preserve path naming something INSIDE an eligible array (e.g. `"groups.users"` when
/// `"groups"` itself is eligible) can never match its own array — nothing named `"groups.users"`
/// is ever registered as an `EligibleArray`. Silently matching nothing (the old behavior) means
/// `--lossy-preserve` can silently protect NOTHING, which is a safety promise a caller relied on
/// going unmet without any signal. Instead, treat any preserve path that is a strict
/// dot-separated prefix-extension of `array_path` as protecting the nearest enclosing eligible
/// array — the only array that path could possibly have meant, given Phase 1's no-recursion
/// scope cut.
///
/// The ROOT array is the degenerate case of that rule and must not be special-cased away: an
/// eligible root array has `array_path == ""`, and when the root itself is eligible it is the
/// ONLY eligible array in the document (`collect_eligible_arrays` stops there), so *every*
/// non-empty preserve path can only have meant something inside it. Building the prefix
/// conditionally — `""` at the root, `"{path}."` elsewhere — makes both cases the same
/// "strictly longer than the prefix, and starts with it" test; the earlier
/// `starts_with("{array_path}.")` form silently protected nothing at the root, since no path
/// starts with a bare `"."`.
fn is_preserved(array_path: &str, preserve_paths: &[String]) -> bool {
    let prefix = if array_path.is_empty() {
        String::new()
    } else {
        format!("{array_path}.")
    };
    preserve_paths
        .iter()
        .any(|p| p == array_path || (p.len() > prefix.len() && p.starts_with(&prefix)))
}

#[derive(Debug, Clone, Copy)]
struct FieldStats {
    median: f64,
    mad: f64,
}

/// Per-array, per-field median and MAD, computed once over every item in the array (outlier-ness
/// is a property of the whole distribution, independent of which items are later chosen).
/// Object items contribute their numeric fields by key; bare-number items contribute under
/// [`SELF_FIELD`]. Uses a full sort for the exact median — `slice::select_nth_unstable` would be
/// the O(n) alternative; sort is simpler and plenty fast at realistic array sizes.
fn numeric_field_stats(items: &[Value]) -> BTreeMap<String, FieldStats> {
    let mut values: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for item in items {
        match item {
            Value::Number(n) => {
                if let Some(f) = n.as_f64() {
                    values.entry(SELF_FIELD.to_string()).or_default().push(f);
                }
            }
            Value::Object(map) => {
                for (k, v) in map {
                    if let Value::Number(n) = v
                        && let Some(f) = n.as_f64()
                    {
                        values.entry(k.clone()).or_default().push(f);
                    }
                }
            }
            _ => {}
        }
    }
    values
        .into_iter()
        .filter(|(_, v)| v.len() >= MIN_ARRAY_LEN)
        .map(|(k, mut v)| {
            let median = exact_median(&mut v);
            let mut deviations: Vec<f64> = v.iter().map(|x| (x - median).abs()).collect();
            let mad = exact_median(&mut deviations);
            (k, FieldStats { median, mad })
        })
        .collect()
}

fn exact_median(values: &mut [f64]) -> f64 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let n = values.len();
    if n == 0 {
        return 0.0;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

/// Design §4B item 2's corrected `MAD == 0` handling: `MAD > 0` uses the standard modified
/// z-score; `MAD == 0` (a value shared by the majority) means any *different* value is itself a
/// strong discrete-outlier signal, not "no signal" — the earlier draft had this backwards.
fn modified_z_score(value: f64, stats: &FieldStats) -> f64 {
    if stats.mad > 0.0 {
        (value - stats.median).abs() / (1.4826 * stats.mad)
    } else if value == stats.median {
        0.0
    } else {
        DISCRETE_OUTLIER_SCORE
    }
}

fn numeric_outlier_score(item: &Value, stats: &BTreeMap<String, FieldStats>) -> f64 {
    match item {
        Value::Number(n) => n
            .as_f64()
            .and_then(|f| stats.get(SELF_FIELD).map(|s| modified_z_score(f, s)))
            .unwrap_or(0.0),
        Value::Object(map) => map
            .iter()
            .filter_map(|(k, v)| {
                let Value::Number(n) = v else { return None };
                let f = n.as_f64()?;
                let s = stats.get(k)?;
                Some(modified_z_score(f, s))
            })
            .fold(0.0, f64::max),
        _ => 0.0,
    }
}

const STATUS_FALSE_KEYS: &[&str] = &["success", "ok", "healthy", "passed", "valid"];
const ERROR_COUNT_KEYS: &[&str] = &[
    "error_count",
    "errors",
    "failures",
    "failure_count",
    "retries",
    "retry_count",
];
const STATUS_CODE_KEYS: &[&str] = &["status", "status_code", "http_status", "code"];

/// Value-aware structural failure-signal detection — typed field checks, never substring
/// matching on serialized text (which would false-positive on e.g. `"error_count": 0` or
/// `"failed": false`, since those strings literally contain the flagged word).
fn has_structural_failure_signal(item: &Value) -> bool {
    let Value::Object(map) = item else {
        return false;
    };
    for (k, v) in map {
        let lower = k.to_ascii_lowercase();
        if STATUS_FALSE_KEYS.contains(&lower.as_str()) && v == &Value::Bool(false) {
            return true;
        }
        if ERROR_COUNT_KEYS.contains(&lower.as_str()) && v.as_f64().is_some_and(|f| f > 0.0) {
            return true;
        }
        if STATUS_CODE_KEYS.contains(&lower.as_str())
            && v.as_i64().is_some_and(|n| (400..=599).contains(&n))
        {
            return true;
        }
    }
    false
}

/// Coarse structural shape signature for the diversity allocator (design §4C): object items
/// group by their sorted key set, everything else groups by its JSON type name.
fn structural_fingerprint(item: &Value) -> String {
    match item {
        Value::Object(map) => {
            let mut keys: Vec<&str> = map.keys().map(|k| k.as_str()).collect();
            keys.sort_unstable();
            keys.join(",")
        }
        Value::Array(_) => "array".to_string(),
        Value::Number(_) => "number".to_string(),
        Value::String(_) => "string".to_string(),
        Value::Bool(_) => "bool".to_string(),
        Value::Null => "null".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::token_estimator::ByteHeuristicEstimator;

    fn opts(ratio: f64) -> LossyOptions {
        LossyOptions {
            preserve_paths: Vec::new(),
            ratio,
            namespace: "default".to_string(),
        }
    }

    /// A `$tf_ref` marker has real, fixed overhead (hash/alg/namespace scaffolding, ~120+
    /// bytes) — an item genuinely smaller than that can never be worth dropping (see the
    /// budget-scale comment in `prune`), so most tests need items padded comfortably past it to
    /// exercise actual dropping, not just the "too small to bother" free-keep path.
    fn padding() -> String {
        "x".repeat(150)
    }

    #[test]
    fn ratio_at_or_above_one_is_a_clean_noop() {
        let input = br#"{"items":[{"a":1},{"a":2},{"a":3}]}"#;
        assert!(
            prune(input, &opts(1.0), &ByteHeuristicEstimator)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn no_eligible_arrays_is_a_clean_noop() {
        let input = br#"{"a":1,"items":[1]}"#; // single-item array is below MIN_ARRAY_LEN
        assert!(
            prune(input, &opts(0.1), &ByteHeuristicEstimator)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn preserved_array_is_never_touched_even_at_zero_ratio() {
        let input = serde_json::json!({"items": [{"a":1},{"a":2},{"a":3},{"a":4}]});
        let mut o = opts(0.0);
        o.preserve_paths = vec!["items".to_string()];
        let bytes = serde_json::to_vec(&input).unwrap();
        assert!(
            prune(&bytes, &o, &ByteHeuristicEstimator)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn preserved_array_alongside_a_prunable_one_never_leaks_into_dropped_or_the_report() {
        // Regression test: an adversarial review caught that the dropped-item collection loop
        // didn't skip preserved arrays, so every item of `keep_me` was wrongly pushed into
        // `outcome.dropped` (even though the output JSON correctly left it untouched) -- which
        // both persisted "preserved" content to the retrieval store one layer up, and caused
        // `total_candidates - preserved_candidates - dropped_candidates` to underflow whenever
        // preserved items outnumbered the real kept count among prunable arrays, exactly the
        // shape here (3 preserved vs. only a few of the 4 prune_me items surviving at ratio 0.1).
        let p = padding();
        // `keep_me` items carry a distinct marker field so byte-comparison below can never
        // coincidentally match a genuinely-dropped `prune_me` item -- a real leak vs. a
        // fixture content collision must not be ambiguous.
        let keep_items: Vec<Value> = (0..3)
            .map(|i| serde_json::json!({"a": i, "guard": "KEEP_ME", "pad": p}))
            .collect();
        let prune_items: Vec<Value> = (0..4)
            .map(|i| serde_json::json!({"a": i, "pad": p}))
            .collect();
        let input = serde_json::json!({"keep_me": keep_items, "prune_me": prune_items});
        let bytes = serde_json::to_vec(&input).unwrap();
        let mut o = opts(0.1);
        o.preserve_paths = vec!["keep_me".to_string()];

        let outcome = prune(&bytes, &o, &ByteHeuristicEstimator).unwrap().unwrap();

        // No dropped item's bytes may parse back as one of the preserved keep_me items.
        let keep_me_bytes: Vec<Vec<u8>> = keep_items
            .iter()
            .map(|v| serde_json::to_vec(v).unwrap())
            .collect();
        for d in &outcome.dropped {
            assert!(
                !keep_me_bytes.contains(&d.bytes),
                "a preserved item leaked into outcome.dropped: {:?}",
                String::from_utf8_lossy(&d.bytes)
            );
        }
        // The report's arithmetic must be internally consistent (this would have panicked with
        // an underflow before the fix).
        assert_eq!(
            outcome.report.total_candidates,
            outcome.report.preserved_candidates
                + outcome.report.kept_candidates
                + outcome.report.dropped_candidates
        );
        assert_eq!(outcome.report.preserved_candidates, 3);
        // keep_me's 3 items must all still be present, untouched, in the output.
        let out_keep_me = outcome.json["keep_me"].as_array().unwrap();
        assert_eq!(out_keep_me, &keep_items);
    }

    #[test]
    fn large_uniform_array_prunes_without_quadratic_blowup() {
        // Regression test for an adversarial review's O(n^2) finding: many candidates landing
        // right at the budget boundary (uniform item size) used to fall through to the
        // expensive exact-recount tier for nearly every item once the budget saturated, with no
        // cap -- turning the walk quadratic. This asserts it stays fast at a size where the old
        // behavior was measured taking multiple seconds (debug build, cheap heuristic
        // estimator): a real regression would make this test time out or take far longer than
        // this generous bound.
        let p = "x".repeat(150);
        let items: Vec<Value> = (0..3000)
            .map(|i| serde_json::json!({"n": i, "pad": p}))
            .collect();
        let input = serde_json::json!({"items": items});
        let bytes = serde_json::to_vec(&input).unwrap();

        let start = std::time::Instant::now();
        let outcome = prune(&bytes, &opts(0.3), &ByteHeuristicEstimator).unwrap();
        let elapsed = start.elapsed();

        assert!(
            elapsed < std::time::Duration::from_secs(5),
            "prune() on 3000 uniform items took {elapsed:?} -- likely a quadratic regression"
        );
        // Still does real work, not a no-op that trivially "passes" by doing nothing.
        assert!(outcome.is_some());
    }

    #[test]
    fn zero_ratio_drops_low_priority_items_from_an_unpreserved_array() {
        let p = padding();
        let input = serde_json::json!({"items": (0..6).map(|i| serde_json::json!({"a": i, "pad": p})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&input).unwrap();
        let outcome = prune(&bytes, &opts(0.0), &ByteHeuristicEstimator)
            .unwrap()
            .expect("some items should drop at ratio 0.0");
        assert!(outcome.report.dropped_candidates > 0);
        assert_eq!(
            outcome.report.dropped_candidates + outcome.report.kept_candidates,
            outcome.report.total_candidates
        );
        // Every dropped item's marker must actually be present in the rewritten document.
        let s = serde_json::to_string(&outcome.json).unwrap();
        assert_eq!(s.matches("$tf_ref").count(), outcome.dropped.len());
    }

    #[test]
    fn mad_zero_and_equal_to_median_has_no_outlier_signal() {
        let stats = FieldStats {
            median: 0.0,
            mad: 0.0,
        };
        assert_eq!(modified_z_score(0.0, &stats), 0.0);
    }

    #[test]
    fn mad_zero_and_different_from_median_is_a_strong_discrete_outlier() {
        // The [0,0,0,0,1]-shaped case: MAD collapses to 0, but the lone `1` must NOT be
        // suppressed as "no signal" -- it's exactly what MAD exists to catch.
        let stats = FieldStats {
            median: 0.0,
            mad: 0.0,
        };
        assert_eq!(modified_z_score(1.0, &stats), DISCRETE_OUTLIER_SCORE);
    }

    #[test]
    fn mad_positive_uses_the_standard_modified_z_score_formula() {
        let stats = FieldStats {
            median: 10.0,
            mad: 2.0,
        };
        let expected = (15.0_f64 - 10.0).abs() / (1.4826 * 2.0);
        assert!((modified_z_score(15.0, &stats) - expected).abs() < 1e-9);
    }

    #[test]
    fn structural_failure_signal_is_value_aware_not_substring_matched() {
        // These must NOT be flagged: the substring "error" appears, but the value is falsy/zero.
        assert!(!has_structural_failure_signal(
            &serde_json::json!({"error_count": 0})
        ));
        assert!(!has_structural_failure_signal(
            &serde_json::json!({"failed": false})
        ));
        // These MUST be flagged: real signal.
        assert!(has_structural_failure_signal(
            &serde_json::json!({"success": false})
        ));
        assert!(has_structural_failure_signal(
            &serde_json::json!({"error_count": 3})
        ));
        assert!(has_structural_failure_signal(
            &serde_json::json!({"status_code": 503})
        ));
        assert!(!has_structural_failure_signal(
            &serde_json::json!({"status_code": 200})
        ));
    }

    #[test]
    fn structural_failure_signal_survives_the_full_pipeline_at_low_ratio() {
        // A planted mid-array anomaly with a value-aware failure signal, surrounded by bland
        // filler -- the exact "plant the answer mid-array" shape the design doc's eval plan
        // requires. Must survive even at an aggressive (low) ratio.
        let p = padding();
        let mut items: Vec<Value> = (0..20)
            .map(|i| serde_json::json!({"id": i, "success": true, "pad": p}))
            .collect();
        items[10] = serde_json::json!({"id": 10, "success": false, "pad": p});
        let input = serde_json::json!({"items": items});
        let bytes = serde_json::to_vec(&input).unwrap();
        let outcome = prune(&bytes, &opts(0.1), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        let arr = outcome.json["items"].as_array().unwrap();
        assert_eq!(arr[10]["success"], serde_json::json!(false));
    }

    #[test]
    fn is_deterministic_across_repeated_runs() {
        let p = padding();
        let input = serde_json::json!({"items": (0..15).map(|i| serde_json::json!({"n": i, "pad": p})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&input).unwrap();
        let a = prune(&bytes, &opts(0.3), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        let b = prune(&bytes, &opts(0.3), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        assert_eq!(a.json, b.json);
        assert_eq!(
            a.dropped.iter().map(|d| &d.hash).collect::<Vec<_>>(),
            b.dropped.iter().map(|d| &d.hash).collect::<Vec<_>>()
        );
    }

    #[test]
    fn diversity_walk_spreads_across_fingerprint_groups_before_draining_one() {
        // Two structurally distinct groups (different key sets => different fingerprints), all
        // otherwise tied on rank. A pure "sort by rank, take top N" would happily drain one
        // group entirely before touching the other; round-robin must not.
        let p = padding();
        let mut items = Vec::new();
        for i in 0..6 {
            items.push(serde_json::json!({"kind_a": i, "pad": p}));
        }
        for i in 0..6 {
            items.push(serde_json::json!({"kind_b": i, "pad": p}));
        }
        let input = serde_json::json!({"items": items});
        let bytes = serde_json::to_vec(&input).unwrap();
        let outcome = prune(&bytes, &opts(0.4), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        let arr = outcome.json["items"].as_array().unwrap();
        let kind_a_kept = arr[0..6]
            .iter()
            .filter(|v| v.get("kind_a").is_some())
            .count();
        let kind_b_kept = arr[6..12]
            .iter()
            .filter(|v| v.get("kind_b").is_some())
            .count();
        assert!(
            kind_a_kept > 0,
            "round-robin should keep at least one kind_a item"
        );
        assert!(
            kind_b_kept > 0,
            "round-robin should keep at least one kind_b item"
        );
    }

    #[test]
    fn pruned_output_is_always_valid_json() {
        let p = padding();
        let input = serde_json::json!({"items": (0..10).map(|i| serde_json::json!({"n": i, "pad": p})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&input).unwrap();
        let outcome = prune(&bytes, &opts(0.5), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        let round_trip = serde_json::to_vec(&outcome.json).unwrap();
        assert!(serde_json::from_slice::<Value>(&round_trip).is_ok());
    }

    #[test]
    fn dropped_item_hashes_match_their_own_bytes() {
        let p = padding();
        let input = serde_json::json!({"items": (0..8).map(|i| serde_json::json!({"n": i, "pad": p})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&input).unwrap();
        let outcome = prune(&bytes, &opts(0.1), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        for d in &outcome.dropped {
            assert_eq!(hex_sha256(&d.bytes), d.hash);
        }
    }

    #[test]
    fn nested_arrays_are_found_but_not_recursed_into_when_the_parent_is_eligible() {
        // The outer "groups" array is eligible; its items' inner "users" arrays are deliberately
        // NOT separately recursed into (design's documented Phase 1 scope cut).
        let input = serde_json::json!({
            "groups": [
                {"users": [1,2,3]},
                {"users": [4,5,6]},
            ]
        });
        let bytes = serde_json::to_vec(&input).unwrap();
        let mut arrays = Vec::new();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        collect_eligible_arrays(&value, String::new(), &mut arrays);
        assert_eq!(
            arrays.len(),
            1,
            "only the outer array is eligible, not the nested ones"
        );
        assert_eq!(arrays[0].path, "groups");
    }

    #[test]
    fn revert_markers_restores_only_the_named_hash_and_leaves_everything_else_alone() {
        let p = padding();
        let input = serde_json::json!({"items": (0..10).map(|i| serde_json::json!({"n": i, "pad": p})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&input).unwrap();
        let outcome = prune(&bytes, &opts(0.1), &ByteHeuristicEstimator)
            .unwrap()
            .unwrap();
        assert!(!outcome.dropped.is_empty());

        let mut restore = std::collections::HashMap::new();
        let first = &outcome.dropped[0];
        let original: Value = serde_json::from_slice(&first.bytes).unwrap();
        restore.insert(first.hash.clone(), original.clone());

        let reverted = revert_markers(&outcome.json, &restore);
        let s = serde_json::to_string(&reverted).unwrap();
        // The reverted item's marker is gone, but any remaining dropped items' markers survive.
        let remaining_markers = outcome.dropped.len() - 1;
        assert_eq!(s.matches("$tf_ref").count(), remaining_markers);
        let arr = reverted["items"].as_array().unwrap();
        assert!(arr.contains(&original));
    }

    #[test]
    fn preserve_path_naming_something_inside_an_eligible_array_protects_that_array() {
        // Round-4 external review: `collect_eligible_arrays` never recurses into an array once
        // it's deemed eligible, so a preserve path naming something INSIDE that array (e.g.
        // "groups.users" when "groups" itself is eligible) could never match anything -- the
        // flag silently protected nothing, which is unacceptable for a caller relying on it as
        // a safety guarantee. It must now protect the nearest enclosing eligible array instead.
        let p = padding();
        let input = serde_json::json!({
            "groups": (0..6).map(|i| serde_json::json!({"users": [1,2,3], "a": i, "pad": p})).collect::<Vec<_>>()
        });
        let bytes = serde_json::to_vec(&input).unwrap();
        let mut o = opts(0.0); // maximum drop pressure
        o.preserve_paths = vec!["groups.users".to_string()];
        let outcome = prune(&bytes, &o, &ByteHeuristicEstimator).unwrap();
        assert!(
            outcome.is_none(),
            "\"groups.users\" must protect the whole \"groups\" array, leaving nothing to prune"
        );
    }

    #[test]
    fn preserve_path_protects_an_eligible_root_array_too() {
        // Round-5 external review: the nearest-eligible-ancestor rule above was implemented as
        // `p.starts_with("{array_path}.")`, which is unsatisfiable at the root (`array_path` is
        // "", and no path starts with a bare "."). A live repro confirmed a top-level array of
        // objects carrying a `users` field was pruned to 20 markers despite
        // `--lossy-preserve users`. An eligible root array is the ONLY eligible array in its
        // document, so any preserve path at all must protect it.
        let p = padding();
        let input = serde_json::json!(
            (0..6)
                .map(|i| serde_json::json!({"users": [1,2,3], "a": i, "pad": p}))
                .collect::<Vec<_>>()
        );
        let bytes = serde_json::to_vec(&input).unwrap();
        let mut o = opts(0.0); // maximum drop pressure
        o.preserve_paths = vec!["users".to_string()];
        assert!(
            prune(&bytes, &o, &ByteHeuristicEstimator)
                .unwrap()
                .is_none(),
            "a preserve path must protect the eligible ROOT array, leaving nothing to prune"
        );
        // Control: without the preserve path, the same document DOES prune -- otherwise the
        // assertion above would pass for the wrong reason (nothing prunable in the first place).
        assert!(
            prune(&bytes, &opts(0.0), &ByteHeuristicEstimator)
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn an_unrelated_preserve_path_does_not_protect_a_named_sibling_array() {
        // The prefix rule must stay strict for non-root arrays: "other.thing" names nothing
        // inside "items", so "items" is still prunable. (At the root the rule is deliberately
        // total -- see the test above -- but that must not leak into named paths.)
        let p = padding();
        let input = serde_json::json!({"items": (0..6).map(|i| serde_json::json!({"a": i, "pad": p})).collect::<Vec<_>>()});
        let bytes = serde_json::to_vec(&input).unwrap();
        let mut o = opts(0.0);
        o.preserve_paths = vec!["other.thing".to_string(), "items_extra".to_string()];
        assert!(
            prune(&bytes, &o, &ByteHeuristicEstimator)
                .unwrap()
                .is_some(),
            "neither an unrelated path nor a non-dot prefix extension may protect \"items\""
        );
    }

    #[test]
    fn a_tier_three_acceptance_charges_the_exact_delta_not_the_independent_estimate() {
        // Round-5 external review: tier 3 paid for an exact joint re-tokenize and then threw the
        // result away, charging the cheaper independent tier-2 estimate to the running pool. This
        // asserts the invariant that made the exact call worth making: whatever the walk accepts,
        // the FINAL exact token count of the pruned document must not exceed the input's -- i.e.
        // the accounting the walk ran on has to track reality, not drift above it.
        let p = padding();
        let items: Vec<Value> = (0..40)
            .map(|i| serde_json::json!({"n": i, "pad": p, "note": format!("row {i}")}))
            .collect();
        let input = serde_json::json!({"items": items});
        let bytes = serde_json::to_vec(&input).unwrap();
        let est = ByteHeuristicEstimator;
        for ratio in [0.0, 0.2, 0.5, 0.9] {
            let Some(outcome) = prune(&bytes, &opts(ratio), &est).unwrap() else {
                continue;
            };
            let out_bytes = serde_json::to_vec(&outcome.json).unwrap();
            assert!(
                est.count_bytes(&out_bytes) <= est.count_bytes(&bytes),
                "ratio {ratio}: pruned output ({}) costs more than the input ({})",
                est.count_bytes(&out_bytes),
                est.count_bytes(&bytes)
            );
        }
    }

    #[test]
    fn sibling_eligible_arrays_at_different_paths_are_both_found() {
        let input = serde_json::json!({
            "a": [1,2,3],
            "b": {"c": [4,5,6]},
        });
        let bytes = serde_json::to_vec(&input).unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let mut arrays = Vec::new();
        collect_eligible_arrays(&value, String::new(), &mut arrays);
        let mut paths: Vec<&str> = arrays.iter().map(|a| a.path.as_str()).collect();
        paths.sort_unstable();
        assert_eq!(paths, vec!["a", "b.c"]);
    }
}
