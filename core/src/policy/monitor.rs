//! Monitor A/B decisions (UX-DECISIONS 2026-11-03) — the pure, cross-shell logic behind the
//! editor's 4-way monitor picker (Camera · Pan L/R · Overlay · External) and its two keys:
//! `a` flips between the camera-only and external-only solos, `b` returns to hearing both.
//! PLAN-CORE-SHELL-BOUNDARY §1: the shell keeps the two remembered modes it is handed back
//! here and presents them; it never derives the next mode itself.
//!
//! The product rules:
//!   - FLIP from a solo goes to the OTHER solo (camera ⇄ external).
//!   - FLIP from a both-mode (pan / overlay / null) goes to the solo heard LAST — camera the
//!     first time, so A is always the video's own audio.
//!   - BOTH from a solo restores the both-mode used LAST — overlay the first time (the app
//!     default). BOTH while already hearing both is a no-op.
//!   - SELECT (the picker) adopts the chosen mode as-is.
//!   - Memory follows what is heard: the mode being LEFT (`current`) is recorded first — a solo
//!     `current` is the side heard last, a both-mode `current` is the both-mode used last — then
//!     the outcome overrides its own slot. So the first flip away from Pan L/R, with no memory
//!     yet, still returns to Pan L/R on BOTH. Junk memory (a both-mode in `last_side`, a solo in
//!     `last_both`, or nothing) normalizes to the defaults above, so the plan is always
//!     well-formed.
//!
//! CONTRACT (the testable invariants):
//!   1. `next` is a solo after FLIP; after BOTH it is never a solo.
//!   2. FLIP twice from a solo returns to that solo; FLIP from both then BOTH returns to the
//!      both-mode you left.
//!   3. `last_side` is always a solo and `last_both` never is, whatever the request carried.
//!   4. SELECT never changes the mode to anything but `chosen` (falls back to `current` when
//!      the request names nothing).

use crate::audition::MonitorMode;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MonitorAction {
    Flip,
    Both,
    Select,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorRequest {
    pub action: MonitorAction,
    pub current: MonitorMode,
    /// SELECT only: the mode the user picked.
    #[serde(default)]
    pub chosen: Option<MonitorMode>,
    /// The solo heard last (shell-held memory from the previous plan); absent on first use.
    #[serde(default)]
    pub last_side: Option<MonitorMode>,
    /// The both-mode used last (shell-held memory from the previous plan); absent on first use.
    #[serde(default)]
    pub last_both: Option<MonitorMode>,
}

/// The outcome: the mode to set, plus the memory the shell stores for the next request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorPlan {
    pub next: MonitorMode,
    pub last_side: MonitorMode,
    pub last_both: MonitorMode,
}

fn other_side(m: MonitorMode) -> MonitorMode {
    match m {
        MonitorMode::CameraOnly => MonitorMode::ExternalOnly,
        _ => MonitorMode::CameraOnly,
    }
}

pub fn plan(req: &MonitorRequest) -> MonitorPlan {
    // What is heard NOW is authoritative for its own slot; the carried memory only fills the
    // other slot (and is normalized, so a junk or absent memory can't produce a junk plan).
    let last_side = match req.last_side {
        _ if req.current.is_solo() => req.current,
        Some(m) if m.is_solo() => m,
        _ => MonitorMode::CameraOnly,
    };
    let last_both = match req.last_both {
        _ if !req.current.is_solo() => req.current,
        Some(m) if !m.is_solo() => m,
        _ => MonitorMode::Overlay,
    };
    let next = match req.action {
        MonitorAction::Flip => {
            if req.current.is_solo() {
                other_side(req.current)
            } else {
                last_side
            }
        }
        MonitorAction::Both => {
            if req.current.is_solo() {
                last_both
            } else {
                req.current
            }
        }
        MonitorAction::Select => req.chosen.unwrap_or(req.current),
    };
    MonitorPlan {
        next,
        last_side: if next.is_solo() { next } else { last_side },
        last_both: if next.is_solo() { last_both } else { next },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use MonitorMode::*;

    fn req(action: MonitorAction, current: MonitorMode) -> MonitorRequest {
        MonitorRequest { action, current, chosen: None, last_side: None, last_both: None }
    }

    #[test]
    fn first_flip_from_both_goes_to_camera_and_remembers_overlay() {
        let p = plan(&req(MonitorAction::Flip, Overlay));
        assert_eq!(p, MonitorPlan { next: CameraOnly, last_side: CameraOnly, last_both: Overlay });
    }

    #[test]
    fn flip_alternates_sides_and_both_restores_the_mode_left() {
        let p1 = plan(&MonitorRequest { last_both: Some(PanLR), ..req(MonitorAction::Flip, PanLR) });
        assert_eq!(p1.next, CameraOnly);
        let p2 = plan(&MonitorRequest {
            last_side: Some(p1.last_side),
            last_both: Some(p1.last_both),
            ..req(MonitorAction::Flip, p1.next)
        });
        assert_eq!(p2.next, ExternalOnly);
        assert_eq!(p2.last_side, ExternalOnly);
        let p3 = plan(&MonitorRequest {
            last_side: Some(p2.last_side),
            last_both: Some(p2.last_both),
            ..req(MonitorAction::Both, p2.next)
        });
        assert_eq!(p3.next, PanLR, "both returns to the mode that was left");
        // The next flip resumes on the side heard last, not camera.
        let p4 = plan(&MonitorRequest {
            last_side: Some(p3.last_side),
            last_both: Some(p3.last_both),
            ..req(MonitorAction::Flip, p3.next)
        });
        assert_eq!(p4.next, ExternalOnly);
    }

    #[test]
    fn first_flip_away_from_pan_with_no_memory_returns_to_pan() {
        // The mirrors' first-use sequence: no memory carried at all.
        let p1 = plan(&req(MonitorAction::Flip, PanLR));
        assert_eq!(p1, MonitorPlan { next: CameraOnly, last_side: CameraOnly, last_both: PanLR });
        let p2 = plan(&MonitorRequest {
            last_side: Some(p1.last_side),
            last_both: Some(p1.last_both),
            ..req(MonitorAction::Both, p1.next)
        });
        assert_eq!(p2.next, PanLR);
    }

    #[test]
    fn both_while_hearing_both_is_a_noop() {
        let p = plan(&req(MonitorAction::Both, PanLR));
        assert_eq!(p.next, PanLR);
        assert_eq!(p.last_both, PanLR);
    }

    #[test]
    fn select_adopts_the_chosen_mode_and_updates_memory() {
        let p = plan(&MonitorRequest { chosen: Some(ExternalOnly), ..req(MonitorAction::Select, Overlay) });
        assert_eq!(p, MonitorPlan { next: ExternalOnly, last_side: ExternalOnly, last_both: Overlay });
        let p = plan(&MonitorRequest { chosen: None, ..req(MonitorAction::Select, Overlay) });
        assert_eq!(p.next, Overlay, "no chosen ⇒ keep current");
    }

    #[test]
    fn junk_memory_normalizes_to_defaults() {
        let p = plan(&MonitorRequest {
            last_side: Some(Overlay),
            last_both: Some(CameraOnly),
            ..req(MonitorAction::Flip, Null)
        });
        // The junk slots fall back to the defaults; `current` (Null, a both-mode) is
        // what is actually being left, so it — not the default — is remembered.
        assert_eq!(p, MonitorPlan { next: CameraOnly, last_side: CameraOnly, last_both: Null });
        let p = plan(&MonitorRequest { last_side: Some(Overlay), last_both: Some(CameraOnly), ..req(MonitorAction::Both, ExternalOnly) });
        assert_eq!(p, MonitorPlan { next: Overlay, last_side: ExternalOnly, last_both: Overlay });
    }

    #[test]
    fn wire_names_match_the_header_contract() {
        let j = serde_json::to_string(&MonitorPlan { next: CameraOnly, last_side: ExternalOnly, last_both: PanLR })
            .unwrap();
        assert_eq!(j, r#"{"next":"camera","lastSide":"external","lastBoth":"pan"}"#);
        let r: MonitorRequest = serde_json::from_str(r#"{"action":"flip","current":"overlay"}"#).unwrap();
        assert_eq!(r.action, MonitorAction::Flip);
        assert_eq!(r.current, Overlay);
    }

    fn any_mode() -> impl Strategy<Value = MonitorMode> {
        prop_oneof![
            Just(PanLR),
            Just(Overlay),
            Just(Null),
            Just(CameraOnly),
            Just(ExternalOnly)
        ]
    }

    fn any_action() -> impl Strategy<Value = MonitorAction> {
        prop_oneof![Just(MonitorAction::Flip), Just(MonitorAction::Both), Just(MonitorAction::Select)]
    }

    proptest! {
        #[test]
        fn contract_holds(action in any_action(), current in any_mode(),
                          chosen in proptest::option::of(any_mode()),
                          ls in proptest::option::of(any_mode()),
                          lb in proptest::option::of(any_mode())) {
            let r = MonitorRequest { action, current, chosen, last_side: ls, last_both: lb };
            let p = plan(&r);
            // 3. memory is always well-formed, and the mode being left is remembered
            prop_assert!(p.last_side.is_solo());
            prop_assert!(!p.last_both.is_solo());
            if current.is_solo() && !p.next.is_solo() { prop_assert_eq!(p.last_side, current); }
            if !current.is_solo() && p.next.is_solo() { prop_assert_eq!(p.last_both, current); }
            match action {
                // 1. flip lands on a solo; both never does
                MonitorAction::Flip => {
                    prop_assert!(p.next.is_solo());
                    prop_assert_eq!(p.last_side, p.next);
                    // 2. flipping again returns to where we started when we were on a solo
                    if current.is_solo() {
                        let back = plan(&MonitorRequest {
                            current: p.next, last_side: Some(p.last_side), last_both: Some(p.last_both), ..r
                        });
                        prop_assert_eq!(back.next, current);
                    }
                }
                MonitorAction::Both => {
                    prop_assert!(!p.next.is_solo());
                    prop_assert_eq!(p.last_both, p.next);
                    if !current.is_solo() { prop_assert_eq!(p.next, current); }
                }
                // 4. select is exactly the choice
                MonitorAction::Select => prop_assert_eq!(p.next, chosen.unwrap_or(current)),
            }
        }
    }
}
