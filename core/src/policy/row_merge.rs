//! Re-resolution merge policy (TODO-007) — the PURE, testable core behind `AppModel.mergeResolution`.
//! PLAN-CORE-SHELL-BOUNDARY Phase E policy 3; relocated verbatim from
//! `mac/Sources/CoupleKit/RowMergePolicy.swift` (a RELOCATION, not a behavior change).
//!
//! When new media is imported, the whole pool is re-resolved and each fresh `ResolvedRow` must be
//! folded into the EXISTING queue row. An unapproved, unpinned unresolved row must adopt the newly
//! confident result WHOLESALE; an approved or manually pinned row must NEVER have its chosen source
//! changed silently (new evidence may only surface as extra alternates). The shell owns the side
//! effects (rebuild the PairSession, enqueue auto-sync, reset the `forced` chip); this decides only
//! WHAT the merged row should be.
//!
//! CONTRACT (the testable invariants):
//!   1. PROTECTED ROWS ARE STABLE — when `protected` (approved || pinned), the selected source and
//!      display are preserved; only genuinely-new candidate keys are appended (order preserved); the
//!      session is never rebuilt (SPEC-V2 §3.6 stability).
//!   2. UNPROTECTED ROWS TAKE THE FRESH RESOLUTION WHOLE — selection, candidates, and display all
//!      become the fresh row's; selection mirrors the resolver's `selected` verbatim (this layer
//!      never invents a selection).
//!   3. SESSION LIFECYCLE FIRES AT MOST ONCE — `rebuild_session` is true only when the effective
//!      selected key actually changed.
//!   4. IDEMPOTENT — applying the same fresh resolution twice yields the same row and (after the
//!      first) `rebuild_session == false`.

use serde::{Deserialize, Serialize};

/// The complete, atomic outcome of folding one fresh `ResolvedRow` into an existing queue row. The
/// shell reads this and performs the mutations together so the row is never left partially-updated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RowMergePlan {
    /// The selected audio content key after the merge (`null` ⇒ ambiguous/no-match — never a
    /// coin-flip).
    pub selected_audio_key: Option<String>,
    /// The candidate audio keys after the merge, in final order. Unprotected: the fresh order;
    /// protected: the existing order followed by any genuinely-new fresh keys.
    pub candidate_keys: Vec<String>,
    /// Whether the caller must take the fresh row's `state`/`chip`/`detail` (and the candidate
    /// VALUES). False for a protected row.
    pub adopt_fresh_display: bool,
    /// Whether the selected source changed and the caller must rebuild the `PairSession` and
    /// (re)queue auto-sync exactly once. Always false for a protected row.
    pub rebuild_session: bool,
    /// Whether the caller must clear a stale `forced` flag. Only an unprotected row that adopts a
    /// fresh resolution can clear it.
    pub clear_forced: bool,
}

/// The decision inputs (mirrors `RowMergePolicy.plan` verbatim).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    /// `approved || pinned` — the SPEC-V2 §3.6 protection boundary.
    pub protected: bool,
    /// The row's selected audio key before the merge.
    #[serde(default)]
    pub current_selected_key: Option<String>,
    /// The row's candidate keys before the merge, in order.
    pub current_candidate_keys: Vec<String>,
    /// The resolver's selected key for the re-resolved row (`null` for ambiguous/no-match).
    #[serde(default)]
    pub fresh_selected_key: Option<String>,
    /// The resolver's full candidate keys for the re-resolved row, in resolver order.
    pub fresh_candidate_keys: Vec<String>,
}

/// Decide how to fold a fresh resolution into an existing row (TODO-007). Pure; no ctx, no IO.
pub fn plan(req: &Request) -> RowMergePlan {
    if req.protected {
        // §3.6 stability: keep the user's selection and display untouched; append only candidate
        // keys not already present, preserving order. Idempotent: a fully-present fresh set
        // appends nothing.
        let mut merged = req.current_candidate_keys.clone();
        let known: std::collections::HashSet<&str> =
            req.current_candidate_keys.iter().map(String::as_str).collect();
        for k in &req.fresh_candidate_keys {
            if !known.contains(k.as_str()) {
                merged.push(k.clone());
            }
        }
        return RowMergePlan {
            selected_audio_key: req.current_selected_key.clone(),
            candidate_keys: merged,
            adopt_fresh_display: false,
            rebuild_session: false,
            clear_forced: false,
        };
    }
    // Unprotected: adopt the fresh resolution wholesale. The session is rebuilt (and sync
    // re-queued) only when the effective selection actually changes.
    let rebuild_session = req.current_selected_key != req.fresh_selected_key;
    RowMergePlan {
        selected_audio_key: req.fresh_selected_key.clone(),
        candidate_keys: req.fresh_candidate_keys.clone(),
        adopt_fresh_display: true,
        rebuild_session,
        clear_forced: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(
        protected: bool,
        cur_sel: Option<&str>,
        cur_cand: &[&str],
        fresh_sel: Option<&str>,
        fresh_cand: &[&str],
    ) -> Request {
        Request {
            protected,
            current_selected_key: cur_sel.map(str::to_string),
            current_candidate_keys: cur_cand.iter().map(|s| s.to_string()).collect(),
            fresh_selected_key: fresh_sel.map(str::to_string),
            fresh_candidate_keys: fresh_cand.iter().map(|s| s.to_string()).collect(),
        }
    }

    // (1) Unprotected unresolved row adopts a decisive confident result whole, rebuilds once.
    #[test]
    fn video_first_then_decisive_audio_adopts_confident() {
        let p = plan(&req(false, None, &[], Some("A"), &["A"]));
        assert_eq!(p.selected_audio_key.as_deref(), Some("A"));
        assert_eq!(p.candidate_keys, vec!["A"]);
        assert!(p.adopt_fresh_display);
        assert!(p.rebuild_session); // nil → A
        assert!(p.clear_forced);
    }

    // (2) Unprotected + weak fresh (no selection): the candidate is surfaced without promotion.
    #[test]
    fn video_first_then_weak_audio_stays_unresolved() {
        let p = plan(&req(false, None, &[], None, &["W"]));
        assert_eq!(p.selected_audio_key, None);
        assert_eq!(p.candidate_keys, vec!["W"]);
        assert!(p.adopt_fresh_display);
        assert!(!p.rebuild_session); // nil → nil
    }

    // (3) A pinned row keeps its selection; a stronger fresh source is only appended.
    #[test]
    fn pinned_row_keeps_selection_when_stronger_audio_arrives() {
        let p = plan(&req(true, Some("chosen"), &["chosen"], Some("stronger"), &["stronger", "chosen"]));
        assert_eq!(p.selected_audio_key.as_deref(), Some("chosen"));
        assert_eq!(p.candidate_keys, vec!["chosen", "stronger"]); // existing first, new appended
        assert!(!p.adopt_fresh_display);
        assert!(!p.rebuild_session);
        assert!(!p.clear_forced);
    }

    // (4) An approved row protects identically.
    #[test]
    fn approved_row_keeps_selection_when_stronger_audio_arrives() {
        let p = plan(&req(true, Some("queued"), &["queued"], Some("stronger"), &["stronger", "queued"]));
        assert_eq!(p.selected_audio_key.as_deref(), Some("queued"));
        assert_eq!(p.candidate_keys, vec!["queued", "stronger"]);
        assert!(!p.rebuild_session);
    }

    // (5) Idempotent — re-applying the same resolution duplicates nothing and re-builds nothing.
    #[test]
    fn repeated_resolve_is_idempotent_no_duplicate_work() {
        let first = plan(&req(false, None, &[], Some("A"), &["A"]));
        assert!(first.rebuild_session);
        let cur_cand: Vec<&str> = first.candidate_keys.iter().map(String::as_str).collect();
        let second = plan(&req(false, first.selected_audio_key.as_deref(), &cur_cand, Some("A"), &["A"]));
        assert_eq!(second.selected_audio_key.as_deref(), Some("A"));
        assert_eq!(second.candidate_keys, vec!["A"]);
        assert!(!second.rebuild_session);

        let pinned_again =
            plan(&req(true, Some("chosen"), &["chosen", "stronger"], Some("stronger"), &["stronger", "chosen"]));
        assert_eq!(pinned_again.candidate_keys, vec!["chosen", "stronger"]);
        assert!(!pinned_again.rebuild_session);
    }

    // Unprotected re-pick to a different source rebuilds once and swaps selection.
    #[test]
    fn unprotected_repick_swaps_selection_once() {
        let p = plan(&req(false, Some("old"), &["old", "new"], Some("new"), &["new", "old"]));
        assert_eq!(p.selected_audio_key.as_deref(), Some("new"));
        assert_eq!(p.candidate_keys, vec!["new", "old"]);
        assert!(p.rebuild_session);
    }

    // Unprotected confident → ambiguous tears down once.
    #[test]
    fn unprotected_confident_to_ambiguous_tears_down_once() {
        let p = plan(&req(false, Some("A"), &["A"], None, &["A", "B"]));
        assert_eq!(p.selected_audio_key, None);
        assert_eq!(p.candidate_keys, vec!["A", "B"]);
        assert!(p.rebuild_session); // A → nil
    }

    // Protected no-op when nothing new.
    #[test]
    fn protected_no_op_when_nothing_new() {
        let p = plan(&req(true, Some("chosen"), &["chosen", "alt"], Some("chosen"), &["chosen", "alt"]));
        assert_eq!(p.candidate_keys, vec!["chosen", "alt"]);
        assert!(!p.adopt_fresh_display);
        assert!(!p.rebuild_session);
    }

    #[test]
    fn request_deserializes_camel_case() {
        let r: Request = serde_json::from_str(
            r#"{"protected":false,"currentSelectedKey":null,"currentCandidateKeys":[],
                "freshSelectedKey":"A","freshCandidateKeys":["A"]}"#,
        )
        .unwrap();
        let p = plan(&r);
        assert_eq!(p.selected_audio_key.as_deref(), Some("A"));
        assert!(p.rebuild_session);
    }
}
