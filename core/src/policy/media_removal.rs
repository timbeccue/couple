//! Media removal / clear-session decisions (TODO-001) — the PURE, testable core behind
//! `AppModel.removeSelectedMedia` and `AppModel.clearSession`. PLAN-CORE-SHELL-BOUNDARY Phase E
//! policy 4; relocated verbatim from `mac/Sources/CoupleKit/MediaRemovalPolicy.swift` (a
//! RELOCATION, not a behavior change). The shell owns the side effects (stopping
//! preview/audition/sync, freeing handles, re-running the resolver, wiping the persisted restore
//! record); this decides only WHETHER a confirmation is owed and WITH WHAT COUNTS, so the two
//! removal entry points cannot diverge.
//!
//! The product rules (TODO-001 / SPEC-V2 §V2.7):
//!   - UNUSED media (referenced by no couple) is removed IMMEDIATELY, no dialog.
//!   - If ANY selected key participates in ≥1 couple, ONE consequence-focused confirmation is shown
//!     for the WHOLE batch (never one dialog per file).
//!   - A couple is AFFECTED when its video is removed (the row goes away) OR its currently selected
//!     audio is removed (the row re-resolves). A removed key that is only a non-selected ALTERNATE
//!     is NOT counted as affected.
//!   - Clearing the session always confirms, with the media/couple/approved counts.
//!
//! CONTRACT (the testable invariants):
//!   1. EMPTY removal (no keys, or only keys absent from the pool) needs no confirmation and
//!      reports zero affected/approved.
//!   2. UNUSED-ONLY removal needs no confirmation and reports zero affected couples.
//!   3. A removal touching ≥1 referenced key needs confirmation; `affected_couples` counts the
//!      DISTINCT couples whose video or SELECTED audio is removed; `affected_approved` counts how
//!      many of those were approved.
//!   4. The counts are a function of the input sets ALONE (no call-site flavor).

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// The identity of one couple as removal reasons about it: its video key, the audio key it
/// currently uses (`null` when unresolved), and whether it is approved (≡ queued for export).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoupleRef {
    pub video_key: String,
    #[serde(default)]
    pub selected_audio_key: Option<String>,
    pub approved: bool,
}

/// The outcome of a per-file removal request (TODO-001). The shell reads this and applies it: when
/// `removable_keys` is empty it does nothing; when `needs_confirmation` is false it removes
/// immediately; otherwise it shows ONE dialog using the counts and removes only on confirm.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRemovalPlan {
    /// The selected keys that actually name a live pool entry (junk/stale keys are dropped). Empty
    /// ⇒ the caller no-ops. Serialized as a JSON array (a set; order is not significant).
    pub removable_keys: Vec<String>,
    /// True iff ≥1 removable key participates in a couple (as a video or a SELECTED audio).
    pub needs_confirmation: bool,
    /// Distinct couples whose video is removed OR whose selected audio is removed.
    pub affected_couples: u32,
    /// How many of the affected couples were approved (queued for export).
    pub affected_approved: u32,
}

/// Counts for the Clear Session confirmation (TODO-001): a pure projection so the dialog copy is
/// testable without the shell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClearSessionCounts {
    pub media: u32,
    pub couples: u32,
    pub approved: u32,
}

/// Decide the per-file removal plan (TODO-001) for a selection over the live pool. Pure; no IO.
///
/// - `selected_keys`: the content keys the user asked to remove. Keys not in `pool_keys` are
///   dropped (a stale row mid-removal can never poison the plan).
/// - `pool_keys`: every live pool content key.
/// - `couples`: a couple is affected when its `video_key` is removed OR its `selected_audio_key`
///   is removed; a removed non-selected alternate does NOT count.
pub fn plan(selected_keys: &HashSet<String>, pool_keys: &HashSet<String>, couples: &[CoupleRef]) -> MediaRemovalPlan {
    // Only ever remove live entries; an unknown key is dropped.
    let removable: HashSet<String> = selected_keys.intersection(pool_keys).cloned().collect();
    if removable.is_empty() {
        return MediaRemovalPlan {
            removable_keys: Vec::new(),
            needs_confirmation: false,
            affected_couples: 0,
            affected_approved: 0,
        };
    }

    // A couple is AFFECTED iff its video is going away (row removed) or the audio it currently
    // uses is going away (row re-resolves). A removed alternate is not a consequence.
    let mut affected = 0u32;
    let mut affected_approved = 0u32;
    for c in couples {
        let video_removed = removable.contains(&c.video_key);
        let audio_removed = c.selected_audio_key.as_ref().is_some_and(|k| removable.contains(k));
        if video_removed || audio_removed {
            affected += 1;
            if c.approved {
                affected_approved += 1;
            }
        }
    }

    MediaRemovalPlan {
        removable_keys: removable.into_iter().collect(),
        needs_confirmation: affected > 0,
        affected_couples: affected,
        affected_approved,
    }
}

/// The Clear Session confirmation counts (TODO-001) — media files, couples, and approved couples.
pub fn clear_counts(media_count: u32, couples: &[CoupleRef]) -> ClearSessionCounts {
    ClearSessionCounts {
        media: media_count,
        couples: couples.len() as u32,
        approved: couples.iter().filter(|c| c.approved).count() as u32,
    }
}

// ============================ FFI request dispatch ============================

/// The tagged request the single `cpl_policy_media_removal` symbol dispatches on. `op` selects the
/// pure function; the remaining fields are that function's inputs. Kept here (not in the FFI crate)
/// so the wire shape lives beside the decisions it drives.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Request {
    /// Per-file removal plan over a selection.
    #[serde(rename_all = "camelCase")]
    Plan {
        #[serde(default)]
        selected_keys: HashSet<String>,
        #[serde(default)]
        pool_keys: HashSet<String>,
        #[serde(default)]
        couples: Vec<CoupleRef>,
    },
    /// Clear-session confirmation counts.
    #[serde(rename_all = "camelCase")]
    ClearCounts {
        media_count: u32,
        #[serde(default)]
        couples: Vec<CoupleRef>,
    },
}

/// The tagged response the envelope's `value` carries — the matching output for the dispatched
/// `op`, so the wrapper decodes exactly one shape.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum Response {
    Plan(MediaRemovalPlan),
    ClearCounts(ClearSessionCounts),
}

/// Dispatch a decoded `Request` to its pure function.
pub fn dispatch(req: &Request) -> Response {
    match req {
        Request::Plan { selected_keys, pool_keys, couples } => {
            Response::Plan(plan(selected_keys, pool_keys, couples))
        }
        Request::ClearCounts { media_count, couples } => {
            Response::ClearCounts(clear_counts(*media_count, couples))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(keys: &[&str]) -> HashSet<String> {
        keys.iter().map(|s| s.to_string()).collect()
    }
    fn cref(v: &str, a: Option<&str>, approved: bool) -> CoupleRef {
        CoupleRef { video_key: v.to_string(), selected_audio_key: a.map(str::to_string), approved }
    }

    // (1) Unused removal is immediate (no confirmation, zero affected).
    #[test]
    fn unused_removal_is_immediate() {
        let couples = [cref("v1", Some("a1"), false), cref("v2", Some("a2"), false)];
        let p = plan(&set(&["unused"]), &set(&["v1", "v2", "a1", "a2", "unused"]), &couples);
        assert_eq!(p.removable_keys, vec!["unused".to_string()]);
        assert!(!p.needs_confirmation);
        assert_eq!(p.affected_couples, 0);
        assert_eq!(p.affected_approved, 0);
    }

    // (1b) A removed key that is only a non-selected alternate is not an affected couple.
    #[test]
    fn removed_alternate_is_not_an_affected_couple() {
        let couples = [cref("v1", Some("a1"), false)];
        let p = plan(&set(&["a2"]), &set(&["v1", "a1", "a2"]), &couples);
        assert!(!p.needs_confirmation);
        assert_eq!(p.affected_couples, 0);
    }

    // (2) Removing a selected audio needs confirmation.
    #[test]
    fn removing_selected_audio_needs_confirmation() {
        let couples = [cref("v1", Some("a1"), false), cref("v2", Some("a2"), false)];
        let p = plan(&set(&["a1"]), &set(&["v1", "v2", "a1", "a2"]), &couples);
        assert!(p.needs_confirmation);
        assert_eq!(p.affected_couples, 1);
        assert_eq!(p.affected_approved, 0);
    }

    // (3) Removing a video needs confirmation.
    #[test]
    fn removing_video_needs_confirmation() {
        let couples = [cref("v1", Some("a1"), false)];
        let p = plan(&set(&["v1"]), &set(&["v1", "a1"]), &couples);
        assert!(p.needs_confirmation);
        assert_eq!(p.affected_couples, 1);
    }

    // (4) Approved rows are counted.
    #[test]
    fn affected_approved_count() {
        let couples =
            [cref("v1", Some("a1"), true), cref("v2", Some("a1"), false), cref("v3", Some("a3"), true)];
        let p = plan(&set(&["a1"]), &set(&["v1", "v2", "v3", "a1", "a3"]), &couples);
        assert_eq!(p.affected_couples, 2);
        assert_eq!(p.affected_approved, 1);
    }

    // (5) One plan for the whole batch (mixed selection, combined counts).
    #[test]
    fn batch_mixes_unused_and_referenced_into_one_plan() {
        let couples = [cref("v1", Some("a1"), true), cref("v2", Some("a2"), false)];
        let p = plan(&set(&["v2", "a1", "unused"]), &set(&["v1", "v2", "a1", "a2", "unused"]), &couples);
        assert_eq!(p.removable_keys.iter().cloned().collect::<HashSet<_>>(), set(&["v2", "a1", "unused"]));
        assert!(p.needs_confirmation);
        assert_eq!(p.affected_couples, 2);
        assert_eq!(p.affected_approved, 1);
    }

    // (6) Stale keys are dropped; an all-stale selection is an empty no-op plan.
    #[test]
    fn stale_keys_are_dropped_and_empty_plan_no_ops() {
        let couples = [cref("v1", Some("a1"), false)];
        let mixed = plan(&set(&["a1", "ghost"]), &set(&["v1", "a1"]), &couples);
        assert_eq!(mixed.removable_keys, vec!["a1".to_string()]);
        assert!(mixed.needs_confirmation);

        let all_stale = plan(&set(&["ghost", "phantom"]), &set(&["v1", "a1"]), &couples);
        assert!(all_stale.removable_keys.is_empty());
        assert!(!all_stale.needs_confirmation);
        assert_eq!(all_stale.affected_couples, 0);
    }

    // (8) Clear-session counts.
    #[test]
    fn clear_session_counts() {
        let couples =
            [cref("v1", Some("a1"), true), cref("v2", Some("a2"), false), cref("v3", None, false)];
        assert_eq!(clear_counts(5, &couples), ClearSessionCounts { media: 5, couples: 3, approved: 1 });
        assert_eq!(clear_counts(0, &[]), ClearSessionCounts { media: 0, couples: 0, approved: 0 });
    }

    // The tagged FFI request dispatches to the right function.
    #[test]
    fn request_dispatch_plan_and_clear_counts() {
        let plan_req: Request = serde_json::from_str(
            r#"{"op":"plan","selectedKeys":["a1"],"poolKeys":["v1","a1"],
                "couples":[{"videoKey":"v1","selectedAudioKey":"a1","approved":true}]}"#,
        )
        .unwrap();
        match dispatch(&plan_req) {
            Response::Plan(p) => {
                assert!(p.needs_confirmation);
                assert_eq!(p.affected_approved, 1);
            }
            _ => panic!("expected Plan response"),
        }

        let cc_req: Request = serde_json::from_str(
            r#"{"op":"clearCounts","mediaCount":3,
                "couples":[{"videoKey":"v1","selectedAudioKey":null,"approved":true}]}"#,
        )
        .unwrap();
        match dispatch(&cc_req) {
            Response::ClearCounts(c) => assert_eq!(c, ClearSessionCounts { media: 3, couples: 1, approved: 1 }),
            _ => panic!("expected ClearCounts response"),
        }
    }
}
