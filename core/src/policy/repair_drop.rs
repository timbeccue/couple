//! Manual drag-to-pair decision (TODO-013) — the PURE, testable core behind both drop targets
//! that re-pair a couple with an audio file. PLAN-CORE-SHELL-BOUNDARY Phase E policy 2; relocated
//! verbatim from `mac/Sources/CoupleKit/RepairDropPolicy.swift` (a RELOCATION, not a behavior
//! change). The two gestures (queue-row drop and center-workspace drop) MUST produce identical
//! pairing results, so this single decision is the cross-platform authority; the shell owns the
//! side effects (presenting the confirmation, then `repair`).
//!
//! The replacement rules (TODO-013 / SPEC-V2 §V2.6) are deliberately simple:
//!   - Dropping the ALREADY-selected audio is a no-op.
//!   - A couple with NO current audio (unresolved) accepts the drop immediately.
//!   - An UNAPPROVED couple with a DIFFERENT source replaces immediately, no dialog.
//!   - An APPROVED couple needs confirmation (replacing it removes it from the export queue).
//!   - No couple / non-audio payload / no available video rejects the drop with no state change.
//!
//! CONTRACT (the testable invariants):
//!   1. REJECT needs no couple, no audio, or no video — checked BEFORE any replacement reasoning
//!      (a malformed/non-audio drop never mutates).
//!   2. SAME AUDIO IS A NO-OP — dropping the already-selected key ⇒ `NoOp` regardless of approval.
//!   3. UNRESOLVED ACCEPTS IMMEDIATELY — no current selection (different, non-null dropped key) ⇒
//!      `Accept`.
//!   4. UNAPPROVED + DIFFERENT ACCEPTS IMMEDIATELY — `Accept`, no confirmation.
//!   5. APPROVED + DIFFERENT NEEDS CONFIRMATION — `NeedsConfirmation`; the ONLY outcome that asks.

use serde::{Deserialize, Serialize};

/// What a manual audio drop onto a couple should do (TODO-013). The shell maps each case to a
/// side effect; both drop targets consume this so they cannot diverge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RepairDropOutcome {
    /// Drop the already-selected audio — nothing changes.
    NoOp,
    /// Replace immediately (unresolved, or an unapproved couple with a new source).
    Accept,
    /// Replacing an APPROVED couple — confirm first, then replace on confirm.
    NeedsConfirmation,
    /// No selected couple, a non-audio payload, or an unavailable video — ignore the drop.
    Reject,
}

/// The decision inputs (mirrors `RepairDropPolicy.decide` verbatim).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// There is a couple under/selected for this drop target.
    pub has_selected_couple: bool,
    /// The dragged content key names an AUDIO pool entry.
    pub payload_is_audio: bool,
    /// The couple's video can actually be paired (its track handle exists).
    pub video_available: bool,
    /// The couple's currently selected audio key, or `null` when unresolved.
    #[serde(default)]
    pub current_selected_key: Option<String>,
    /// The dragged audio's content key.
    pub dropped_key: String,
    /// The couple is approved (≡ queued for export).
    pub approved: bool,
}

/// The decision output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub outcome: RepairDropOutcome,
}

/// Decide the outcome of dropping an audio file onto the selected couple's drop target (TODO-013).
/// Used by BOTH the queue-row and center-workspace gestures so they produce identical results.
pub fn decide(req: &Request) -> Response {
    let outcome = decide_outcome(req);
    Response { outcome }
}

fn decide_outcome(req: &Request) -> RepairDropOutcome {
    // (1) Nothing to pair: no couple, a non-audio payload, or no usable video. Checked first so a
    //     malformed/non-audio drop can never reach the replacement reasoning and never mutates.
    if !(req.has_selected_couple && req.payload_is_audio && req.video_available) {
        return RepairDropOutcome::Reject;
    }
    // (2) Re-dropping the already-selected source changes nothing (independent of approval).
    if req.current_selected_key.as_deref() == Some(req.dropped_key.as_str()) {
        return RepairDropOutcome::NoOp;
    }
    // (3)/(4) A new source on an unresolved or unapproved couple replaces immediately.
    if !req.approved {
        return RepairDropOutcome::Accept;
    }
    // (5) A new source on an APPROVED couple removes it from the export queue, so confirm first.
    RepairDropOutcome::NeedsConfirmation
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(
        has_selected_couple: bool,
        payload_is_audio: bool,
        video_available: bool,
        current_selected_key: Option<&str>,
        dropped_key: &str,
        approved: bool,
    ) -> RepairDropOutcome {
        decide(&Request {
            has_selected_couple,
            payload_is_audio,
            video_available,
            current_selected_key: current_selected_key.map(str::to_string),
            dropped_key: dropped_key.to_string(),
            approved,
        })
        .outcome
    }

    // (1)/(2) Dropping the already-selected source ⇒ NoOp, regardless of approval.
    #[test]
    fn dropping_current_source_is_no_op() {
        assert_eq!(d(true, true, true, Some("x"), "x", false), RepairDropOutcome::NoOp);
        assert_eq!(d(true, true, true, Some("x"), "x", true), RepairDropOutcome::NoOp);
    }

    // (3) An unresolved couple accepts immediately.
    #[test]
    fn unresolved_couple_accepts_immediately() {
        assert_eq!(d(true, true, true, None, "new", false), RepairDropOutcome::Accept);
    }

    // (4) An unapproved couple with a different source replaces immediately.
    #[test]
    fn unapproved_different_source_accepts_immediately() {
        assert_eq!(d(true, true, true, Some("old"), "new", false), RepairDropOutcome::Accept);
    }

    // (5) An approved couple with a different source needs confirmation.
    #[test]
    fn approved_different_source_needs_confirmation() {
        assert_eq!(
            d(true, true, true, Some("old"), "new", true),
            RepairDropOutcome::NeedsConfirmation
        );
        assert_eq!(d(true, true, true, None, "new", true), RepairDropOutcome::NeedsConfirmation);
    }

    // (5)/(6)/(7) The reject guards take precedence over replacement reasoning.
    #[test]
    fn reject_guards_take_precedence() {
        assert_eq!(d(false, true, true, Some("x"), "y", false), RepairDropOutcome::Reject);
        assert_eq!(d(false, true, true, None, "new", false), RepairDropOutcome::Reject);
        assert_eq!(d(true, false, true, Some("x"), "x", false), RepairDropOutcome::Reject);
        assert_eq!(d(true, true, false, Some("old"), "new", true), RepairDropOutcome::Reject);
        // All guards failing ⇒ a single clean reject.
        assert_eq!(d(false, false, false, None, "x", false), RepairDropOutcome::Reject);
    }

    // (9) Pure: identical inputs always give the same outcome (the shared-path guarantee).
    #[test]
    fn same_inputs_yield_same_outcome() {
        let cases: [(Option<&str>, &str, bool); 4] =
            [(None, "a", false), (Some("a"), "a", true), (Some("a"), "b", false), (Some("a"), "b", true)];
        for (cur, dropped, approved) in cases {
            assert_eq!(
                d(true, true, true, cur, dropped, approved),
                d(true, true, true, cur, dropped, approved)
            );
        }
    }

    #[test]
    fn request_deserializes_camel_case() {
        let req: Request = serde_json::from_str(
            r#"{"hasSelectedCouple":true,"payloadIsAudio":true,"videoAvailable":true,
                "currentSelectedKey":"old","droppedKey":"new","approved":true}"#,
        )
        .unwrap();
        assert_eq!(decide(&req).outcome, RepairDropOutcome::NeedsConfirmation);
    }
}
