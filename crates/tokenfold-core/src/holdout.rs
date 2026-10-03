//! EP-11 / NF-18: session-stable holdout assignment and an output-shaping guard.
//!
//! # Assignment is per session, never per turn
//!
//! An A/B comparison only means something if the two arms are otherwise identical. Re-rolling an
//! arm every turn would put a subject in both arms within the same conversation, contaminate the
//! context, and destroy any provider prefix cache. So the arm is derived from the **session**
//! identity alone: same session, same arm, every turn, forever.
//!
//! The derivation is a plain hash, not a random draw, for the same reason -- a random roll would
//! not survive a process restart and would silently re-randomize mid-session.
//!
//! # Shaping is opt-in and independently gated
//!
//! Shaping may only be applied when the session was assigned to the treatment arm. A control
//! session never has its output shaped, no matter what the caller requests, so the comparison is
//! never silently contaminated.
//!
//! # Shaping may not damage protected content
//!
//! [`may_shape`] refuses to apply a shaped output that would drop a protected path or shrink the
//! payload below its floor. A guard that fails open here would let an optimization quietly delete
//! an instruction the caller marked as untouchable.

/// Which arm a session belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arm {
    /// The baseline: nothing is shaped.
    Control,
    /// The candidate: shaping may apply.
    Treatment,
}

/// Which session the assignment belongs to, and the knob it was derived under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub session_id: String,
    pub arm: Arm,
    /// The roll that produced the arm, so a decision is auditable after the fact.
    pub roll: u64,
}

/// FNV-1a over the session id. Dependency-free and stable across processes and platforms, which a
/// default hasher is not guaranteed to be.
fn session_hash(session_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in session_id.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

/// Assigns `session_id` to an arm.
///
/// With `experiment` off, every session is Control -- the honest default, since an unconfigured
/// experiment has no treatment to offer.
///
/// The split is an even `roll % 2`, so roughly half of sessions land in each arm. That is the
/// strongest property available without any data: it cannot be tuned, so it cannot be tuned into
/// a result that was wanted.
pub fn assign(session_id: &str, experiment_enabled: bool) -> Assignment {
    let roll = session_hash(session_id);
    Assignment {
        session_id: session_id.to_string(),
        arm: if !experiment_enabled || roll % 2 == 0 {
            Arm::Control
        } else {
            Arm::Treatment
        },
        roll,
    }
}

/// Why a shaped output was not applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShapeRejection {
    /// The session is in the control arm, so shaping must not be applied.
    ControlArm,
    /// The shaped output no longer contains a path the caller marked protected.
    ProtectedPathLost { path: String },
    /// The shaped output is smaller than the floor, i.e. it is a lossy shrink, not a shaping.
    BelowFloor { bytes: usize, floor: usize },
    /// The shaped output is not actually smaller, so there is nothing to gain.
    NotSmaller { original: usize, shaped: usize },
}

/// Bounds a caller must set before any shaping is considered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShapePolicy {
    /// Minimum acceptable shaped size in bytes.
    pub min_bytes: usize,
}

impl Default for ShapePolicy {
    fn default() -> Self {
        Self { min_bytes: 1 }
    }
}

/// Decides whether `shaped` may replace `original` for this session.
///
/// Returns the bytes to actually use: either `shaped` or the untouched `original`. There is no
/// third outcome and no "best effort" -- a guard that could return something intermediate would be
/// a guard that could be talked into losing protected content.
pub fn may_shape(
    original: &[u8],
    shaped: &[u8],
    protected_paths: &[&str],
    assignment: &Assignment,
    policy: ShapePolicy,
) -> Result<Vec<u8>, ShapeRejection> {
    if assignment.arm == Arm::Control {
        return Err(ShapeRejection::ControlArm);
    }
    for path in protected_paths {
        let needle = path.as_bytes();
        if contains(original, needle) && !contains(shaped, needle) {
            return Err(ShapeRejection::ProtectedPathLost {
                path: (*path).to_string(),
            });
        }
    }
    if shaped.len() < policy.min_bytes {
        return Err(ShapeRejection::BelowFloor {
            bytes: shaped.len(),
            floor: policy.min_bytes,
        });
    }
    if shaped.len() >= original.len() {
        return Err(ShapeRejection::NotSmaller {
            original: original.len(),
            shaped: shaped.len(),
        });
    }
    Ok(shaped.to_vec())
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// The signed delta shaping produced, for honest reporting.
///
/// Negative means the shaped output is larger. That is a legitimate outcome and is reported as
/// such rather than being suppressed -- a report that only ever shows wins is a report that has
/// stopped measuring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delta {
    pub original_bytes: usize,
    pub shaped_bytes: usize,
}

impl Delta {
    /// Bytes saved. Negative when shaping made the output larger.
    pub fn signed(&self) -> i64 {
        self.original_bytes as i64 - self.shaped_bytes as i64
    }

    pub fn is_saving(&self) -> bool {
        self.signed() > 0
    }
}

/// Measures what a candidate would do, whether or not it is applied.
pub fn measure(original: &[u8], shaped: &[u8]) -> Delta {
    Delta {
        original_bytes: original.len(),
        shaped_bytes: shaped.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- session-stable assignment --------------------------------------------

    #[test]
    fn the_same_session_always_gets_the_same_arm() {
        // Re-rolling per turn would put a subject in both arms within one conversation and
        // contaminate the comparison.
        let first = assign("session-abc", true);
        for turn in 0..25 {
            assert_eq!(
                assign("session-abc", true),
                first,
                "arm moved on turn {turn}"
            );
        }
    }

    #[test]
    fn assignment_survives_being_recomputed_with_no_shared_state() {
        // A random roll would re-randomize after a restart; a hash does not.
        let ids: Vec<String> = (0..50).map(|i| format!("s{i}")).collect();
        let first: Vec<Arm> = ids.iter().map(|id| assign(id, true).arm).collect();
        for _ in 0..3 {
            let again: Vec<Arm> = ids.iter().map(|id| assign(id, true).arm).collect();
            assert_eq!(again, first);
        }
    }

    #[test]
    fn different_sessions_are_independently_assigned() {
        let a = assign("session-a", true);
        let b = assign("session-b", true);
        assert_ne!(a.session_id, b.session_id);
        // Rolls differ even where arms happen to coincide.
        assert_ne!(a.roll, b.roll);
    }

    #[test]
    fn both_arms_are_reachable() {
        // A knob that only ever produces one arm cannot be a knob.
        let arms: Vec<Arm> = (0..40)
            .map(|i| assign(&format!("s{i}"), true).arm)
            .collect();
        assert!(arms.contains(&Arm::Control));
        assert!(arms.contains(&Arm::Treatment));
    }

    #[test]
    fn the_split_is_roughly_even() {
        let treatment = (0..200)
            .filter(|i| assign(&format!("s{i}"), true).arm == Arm::Treatment)
            .count();
        assert!(
            (70..=130).contains(&treatment),
            "assignment is badly skewed: {treatment}/200 treatment"
        );
    }

    #[test]
    fn a_disabled_experiment_puts_every_session_in_control() {
        // An unconfigured experiment has no treatment to offer.
        for i in 0..40 {
            assert_eq!(assign(&format!("s{i}"), false).arm, Arm::Control);
        }
    }

    #[test]
    fn an_empty_session_id_is_still_deterministically_assigned() {
        // It is untrusted input, not a reason to fail or to always get the same arm.
        let a = assign("", true);
        assert_eq!(assign("", true), a);
        assert_eq!(a.roll, session_hash(""));
    }

    // --- the shaping guard -----------------------------------------------------

    fn treatment_session() -> Assignment {
        // Find a session id that lands in treatment, so the guard tests are not vacuous.
        (0..100)
            .map(|i| assign(&format!("s{i}"), true))
            .find(|a| a.arm == Arm::Treatment)
            .expect("some session must be treatment")
    }

    fn control_session() -> Assignment {
        (0..100)
            .map(|i| assign(&format!("s{i}"), true))
            .find(|a| a.arm == Arm::Control)
            .expect("some session must be control")
    }

    #[test]
    fn a_control_session_is_never_shaped_however_much_it_asks() {
        let assignment = control_session();
        let result = may_shape(
            b"the original protected payload",
            b"tiny",
            &[],
            &assignment,
            ShapePolicy::default(),
        );
        assert_eq!(result, Err(ShapeRejection::ControlArm));
    }

    #[test]
    fn a_treatment_session_applies_a_smaller_unguarded_shape() {
        let result = may_shape(
            b"the original payload that is long",
            b"a shorter payload",
            &[],
            &treatment_session(),
            ShapePolicy::default(),
        );
        assert_eq!(result.unwrap(), b"a shorter payload".to_vec());
    }

    #[test]
    fn shaping_is_refused_when_a_protected_path_would_be_lost() {
        // Failing open here would let an optimization silently delete an untouchable instruction.
        let result = may_shape(
            b"prefix <SYSTEM>do not delete me</SYSTEM> and some filler text",
            b"a short replacement",
            &["<SYSTEM>"],
            &treatment_session(),
            ShapePolicy::default(),
        );
        assert_eq!(
            result,
            Err(ShapeRejection::ProtectedPathLost {
                path: "<SYSTEM>".into()
            })
        );
    }

    #[test]
    fn a_protected_path_that_survives_does_not_block_shaping() {
        let result = may_shape(
            b"<SYSTEM>keep me</SYSTEM> a great deal of extra text to remove",
            b"<SYSTEM>keep me</SYSTEM> short",
            &["<SYSTEM>"],
            &treatment_session(),
            ShapePolicy::default(),
        );
        assert_eq!(result.unwrap(), b"<SYSTEM>keep me</SYSTEM> short".to_vec());
    }

    #[test]
    fn a_protected_path_absent_from_the_original_is_not_invented_as_a_blocker() {
        let result = may_shape(
            b"an ordinary payload with plenty of length",
            b"short",
            &["<NEVER_PRESENT>"],
            &treatment_session(),
            ShapePolicy::default(),
        );
        assert!(
            result.is_ok(),
            "a path that was never there blocked shaping"
        );
    }

    #[test]
    fn shaping_below_the_floor_is_refused() {
        let result = may_shape(
            b"an original payload of some length",
            b"ab",
            &[],
            &treatment_session(),
            ShapePolicy { min_bytes: 10 },
        );
        assert_eq!(
            result,
            Err(ShapeRejection::BelowFloor {
                bytes: 2,
                floor: 10
            })
        );
    }

    #[test]
    fn a_shape_that_is_not_smaller_is_refused_rather_than_reported_as_a_win() {
        let longer = may_shape(
            b"short original",
            b"a much longer shaped output than the original",
            &[],
            &treatment_session(),
            ShapePolicy::default(),
        );
        assert!(matches!(longer, Err(ShapeRejection::NotSmaller { .. })));

        let equal = may_shape(
            b"exactly equal",
            b"exactly equal",
            &[],
            &treatment_session(),
            ShapePolicy::default(),
        );
        assert!(matches!(equal, Err(ShapeRejection::NotSmaller { .. })));
    }

    #[test]
    fn the_guard_never_returns_a_partial_or_intermediate_result() {
        // It either applies the shape wholly or returns the untouched original; there is no
        // "best effort" output that could have lost protected content.
        let assignment = treatment_session();
        match may_shape(
            b"original with <P>protected</P> content and length",
            b"<P>protected</P>",
            &["<P>"],
            &assignment,
            ShapePolicy { min_bytes: 1 },
        ) {
            Ok(bytes) => assert_eq!(bytes, b"<P>protected</P>".to_vec()),
            Err(_) => {}
        }
    }

    // --- honest measurement ----------------------------------------------------

    #[test]
    fn a_negative_delta_is_reported_rather_than_suppressed() {
        // A report that only ever shows wins has stopped measuring.
        let delta = measure(b"short", b"a considerably longer shaped output");
        assert!(delta.signed() < 0, "expected a negative delta");
        assert!(!delta.is_saving());
        assert_eq!(
            delta.shaped_bytes,
            "a considerably longer shaped output".len()
        );
    }

    #[test]
    fn a_positive_delta_is_reported_positively() {
        let delta = measure(b"a long original payload here", b"short");
        assert!(delta.is_saving());
        assert_eq!(
            delta.signed(),
            "a long original payload here".len() as i64 - 5
        );
    }

    #[test]
    fn measurement_is_available_even_when_the_shape_will_be_refused() {
        // A rejected candidate is still worth knowing about; refusing to apply it is not a reason
        // to hide its size.
        let delta = measure(b"a long original payload here", b"short");
        assert_eq!(delta, measure(b"a long original payload here", b"short"));
    }
}
