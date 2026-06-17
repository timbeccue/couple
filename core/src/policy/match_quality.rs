//! Match-quality decision (TODO-009) — the PURE, testable core behind `CoupleRow.verdict`.
//! PLAN-CORE-SHELL-BOUNDARY Phase E policy 1; relocated verbatim from
//! `mac/Sources/CoupleKit/MatchQuality.swift` (a RELOCATION, not a behavior change).
//!
//! The verdict dot's color is the answer to "does the CURRENTLY SELECTED video/audio pair have
//! confident match evidence?" — a fact about the recording, NOT a fact about the user's actions.
//! Selecting a source is a decision; it is not evidence that the source matches. So a manual pick
//! must stay questionable UNLESS that specific pair independently clears the SAME confident-match
//! classifier an automatic confident pairing uses.
//!
//! The three signals TODO-009 separates are kept strictly distinct:
//!   - SELECTION (which audio, and whether it was auto-resolved or manually chosen) is an INPUT,
//!     never a promoter.
//!   - MATCH QUALITY (this function's output) derives ONLY from classifier/resolver evidence for
//!     the selected pair.
//!   - APPROVAL and ALIGNMENT FIT (RMS) are deliberately NOT inputs: approving a couple, a
//!     successful auto-sync, or a low alignment RMS must never turn the dot green.
//!
//! CONTRACT (the testable invariants):
//!   1. A MANUAL PICK ALONE NEVER GOES GREEN — `Manual` selection is `Questionable` unless
//!      `manual_match_confident == Some(true)`.
//!   2. SAME STANDARD FOR MANUAL AND AUTOMATIC — `manual_match_confident` is computed from the
//!      resolver's classifier; the only way to green a manual pick is to clear the automatic bar.
//!   3. FORCED STAYS QUESTIONABLE — a kept below-floor manual pairing is always `Questionable`,
//!      regardless of any other signal (honest, SPEC-V2 §3.6).
//!   4. NO SELECTION FALLS BACK TO THE RESOLVER — with no selected pair the verdict is the
//!      resolver's own `auto_verdict` (never a coin-flip).
//!   5. AN AUTOMATIC SELECTION TRUSTS THE RESOLVER — an auto-resolved selection takes
//!      `auto_verdict` directly; no separate manual evidence is needed or consulted.

use serde::{Deserialize, Serialize};

/// Three-category pairing quality (UX-DECISIONS-V2 §V2.3) — the evidence-based dot color, not a
/// continuous score. The shell's UI `Verdict` maps 1:1 from this; the shell owns the localized
/// strings/colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchQuality {
    /// The selected pair independently meets the confident-match criteria.
    Confident,
    /// A human should check this: an ambiguous resolution, a manual pick whose pair did not clear
    /// the classifier, or a kept below-floor (`forced`) pairing.
    Questionable,
    /// No supported match for this video (and nothing selected to vouch for).
    None,
}

/// How the row's currently selected audio came to be selected (TODO-009). Persisted shell-side so
/// a restored manual pick is not silently re-classified as an automatic one. `pinned` manual picks
/// are `Manual`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SelectionProvenance {
    /// No audio is selected (ambiguous / no-match carries no selection).
    None,
    /// The resolver auto-selected this source for a confident/partial/etc. row.
    Automatic,
    /// The user chose this source (candidate pick or pool drag); the row is pinned.
    Manual,
}

/// The decision inputs (mirrors the Swift `MatchQualityPolicy.decide` parameters verbatim).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// The pairing is a kept below-floor manual choice (always questionable, honest — overrides
    /// every other signal).
    pub forced: bool,
    /// How the current audio was selected.
    pub selection: SelectionProvenance,
    /// The resolver's own quality for this row's resolution (`RowState.verdict`) — the evidence for
    /// an automatic selection or the fallback when nothing is selected.
    pub auto_verdict: MatchQuality,
    /// For a `Manual` selection ONLY, whether that specific pair independently cleared the confident
    /// classifier. `None` ("not yet evaluated") is treated as not-confident. Ignored for non-manual
    /// selections.
    #[serde(default)]
    pub manual_match_confident: Option<bool>,
}

/// The decision output: the evidence-based match quality for the row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub quality: MatchQuality,
}

/// Decide the evidence-based match quality for a row (TODO-009). Pure; no ctx, no IO.
pub fn decide(req: &Request) -> Response {
    // (3) A below-floor forced pairing is always questionable — it is kept on purpose but the
    //     evidence does not support it.
    if req.forced {
        return Response { quality: MatchQuality::Questionable };
    }
    let quality = match req.selection {
        // (4) Nothing selected ⇒ the resolver's verdict (never a coin-flip).
        SelectionProvenance::None => req.auto_verdict,
        // (5) The resolver picked and classified this pair ⇒ trust its verdict.
        SelectionProvenance::Automatic => req.auto_verdict,
        // (1)/(2) A manual pick is green ONLY when that exact pair independently cleared the same
        //         confident classifier; otherwise it stays questionable no matter how it was
        //         selected.
        SelectionProvenance::Manual => {
            if req.manual_match_confident == Some(true) {
                MatchQuality::Confident
            } else {
                MatchQuality::Questionable
            }
        }
    };
    Response { quality }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decide_q(
        forced: bool,
        selection: SelectionProvenance,
        auto_verdict: MatchQuality,
        manual_match_confident: Option<bool>,
    ) -> MatchQuality {
        decide(&Request { forced, selection, auto_verdict, manual_match_confident }).quality
    }

    // (A) AMBIGUOUS MANUAL SELECTION STAYS QUESTIONABLE — clicking a candidate on an ambiguous row
    //     must NOT flip the dot green; absent/false evidence is questionable.
    #[test]
    fn ambiguous_manual_selection_stays_questionable() {
        assert_eq!(
            decide_q(false, SelectionProvenance::Manual, MatchQuality::Questionable, Some(false)),
            MatchQuality::Questionable
        );
        // Not-yet-evaluated (the instant after the click): also questionable.
        assert_eq!(
            decide_q(false, SelectionProvenance::Manual, MatchQuality::Questionable, None),
            MatchQuality::Questionable
        );
        // Even a "good" auto row does not promote a manual pick whose own pair failed.
        assert_eq!(
            decide_q(false, SelectionProvenance::Manual, MatchQuality::Confident, Some(false)),
            MatchQuality::Questionable
        );
    }

    // (B) CONFIDENT AUTOMATIC SELECTION IS GREEN — takes the resolver verdict directly; a stale
    //     manual flag is ignored.
    #[test]
    fn confident_automatic_selection_is_green() {
        assert_eq!(
            decide_q(false, SelectionProvenance::Automatic, MatchQuality::Confident, None),
            MatchQuality::Confident
        );
        assert_eq!(
            decide_q(false, SelectionProvenance::Automatic, MatchQuality::Confident, Some(false)),
            MatchQuality::Confident
        );
    }

    // (C) BELOW-FLOOR FORCING IS QUESTIONABLE — always, overriding every other signal.
    #[test]
    fn below_floor_forced_is_questionable() {
        assert_eq!(
            decide_q(true, SelectionProvenance::Manual, MatchQuality::Confident, Some(true)),
            MatchQuality::Questionable
        );
        assert_eq!(
            decide_q(true, SelectionProvenance::Automatic, MatchQuality::Confident, None),
            MatchQuality::Questionable
        );
        assert_eq!(
            decide_q(true, SelectionProvenance::None, MatchQuality::None, None),
            MatchQuality::Questionable
        );
    }

    // (D) EVIDENCE-BASED PROMOTION AFTER RE-ANALYSIS — the only path that greens a manual pick.
    #[test]
    fn manual_selection_promoted_when_pair_passes() {
        assert_eq!(
            decide_q(false, SelectionProvenance::Manual, MatchQuality::Questionable, None),
            MatchQuality::Questionable
        );
        assert_eq!(
            decide_q(false, SelectionProvenance::Manual, MatchQuality::Questionable, Some(true)),
            MatchQuality::Confident
        );
    }

    // (E) NO SELECTION FALLS BACK TO THE RESOLVER VERDICT (never a coin-flip).
    #[test]
    fn no_selection_uses_resolver_verdict() {
        assert_eq!(
            decide_q(false, SelectionProvenance::None, MatchQuality::Questionable, None),
            MatchQuality::Questionable
        );
        assert_eq!(
            decide_q(false, SelectionProvenance::None, MatchQuality::None, None),
            MatchQuality::None
        );
        // A leftover manual flag is ignored once nothing is selected.
        assert_eq!(
            decide_q(false, SelectionProvenance::None, MatchQuality::None, Some(true)),
            MatchQuality::None
        );
    }

    // (F) APPROVAL AND ALIGNMENT FIT ARE NOT INPUTS — they cannot appear in the signature, so the
    //     same inputs always yield the same verdict (documented structurally).
    #[test]
    fn quality_is_independent_of_approval_and_fit() {
        let a = decide_q(false, SelectionProvenance::Manual, MatchQuality::Confident, Some(false));
        let b = decide_q(false, SelectionProvenance::Manual, MatchQuality::Confident, Some(false));
        assert_eq!(a, b);
        assert_eq!(a, MatchQuality::Questionable);
    }

    // Request JSON round-trips (the camelCase wire the FFI envelope carries).
    #[test]
    fn request_deserializes_camel_case() {
        let req: Request = serde_json::from_str(
            r#"{"forced":false,"selection":"manual","autoVerdict":"questionable","manualMatchConfident":true}"#,
        )
        .unwrap();
        assert_eq!(decide(&req).quality, MatchQuality::Confident);
    }
}
