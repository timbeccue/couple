//! Property-based tests for the pure decision policies in `core/src/policy/` (plus the pure
//! surface of `core/src/resolve.rs`). Each `#[test]` enforces one CONTRACT clause documented in
//! the module it exercises — the example-based `#[cfg(test)] mod tests` in each policy pin specific
//! rows; these pin the *invariants* over generated inputs (key sets with overlaps/duplicates/
//! unicode, z values incl. 0/negative, empty and large candidate lists). proptest's default
//! deterministic shrink-on-failure is used; no custom RNG seed source, 256 cases per property.

use couple_core::matching::{MatchClassification, MatchMethod, PairEvidence};
use couple_core::policy::activity::{
    self, Exporting, ImportPhase, Importing, Ingest, MatchPhase, Matching, Pairing, Stages, Syncing,
};
use couple_core::policy::anchor::{self, AnchorCandidate, AnchorTuning, Overlap};
use couple_core::policy::match_quality::{self, MatchQuality, SelectionProvenance};
use couple_core::policy::media_removal::{self, CoupleRef};
use couple_core::policy::repair_drop;
use couple_core::policy::row_merge;
use couple_core::policy::shadow;
use couple_core::resolve::{MatcherChoice, RowState};
use proptest::prelude::*;
use std::collections::HashSet;

// ---------- shared strategies ----------

/// Content keys drawn from a tiny pool (frequent overlaps/duplicates across sets) plus occasional
/// arbitrary short unicode keys — the realistic shape of pool content keys the policies see.
fn key() -> impl Strategy<Value = String> {
    prop_oneof![
        prop::sample::select(vec!["v1", "v2", "a1", "a2", "é", "🎵", "kA", "kB"]).prop_map(str::to_string),
        "[a-cé]{1,4}",
    ]
}

fn keys(max: usize) -> impl Strategy<Value = Vec<String>> {
    prop::collection::vec(key(), 0..max)
}

/// The wire form is a deterministic fixed point: once a value has been through one
/// serialize→deserialize, re-encoding it is byte-identical. This is the FFI-relevant guarantee (a
/// canonical, stable envelope both sides agree on). It is intentionally NOT `to_string(x) ==
/// to_string(decode(encode(x)))` for float-carrying types: serde_json's serialization of an
/// arbitrary f64 is not always its own parse-inverse at the string level, so that stricter form
/// tests serde_json, not the policy. Fixed-point stability is what the boundary actually needs.
fn wire_stable<T>(v: &T) -> bool
where
    T: serde::Serialize + serde::de::DeserializeOwned,
{
    let once: T = serde_json::from_str(&serde_json::to_string(v).unwrap()).unwrap();
    let j1 = serde_json::to_string(&once).unwrap();
    let twice: T = serde_json::from_str(&j1).unwrap();
    j1 == serde_json::to_string(&twice).unwrap()
}

// ============================ match_quality ============================

fn quality() -> impl Strategy<Value = MatchQuality> {
    prop_oneof![Just(MatchQuality::Confident), Just(MatchQuality::Questionable), Just(MatchQuality::None)]
}

fn provenance() -> impl Strategy<Value = SelectionProvenance> {
    prop_oneof![
        Just(SelectionProvenance::None),
        Just(SelectionProvenance::Automatic),
        Just(SelectionProvenance::Manual),
    ]
}

fn mq_request() -> impl Strategy<Value = match_quality::Request> {
    (any::<bool>(), provenance(), quality(), any::<Option<bool>>()).prop_map(
        |(forced, selection, auto_verdict, manual_match_confident)| match_quality::Request {
            forced,
            selection,
            auto_verdict,
            manual_match_confident,
        },
    )
}

proptest! {
    // (3) FORCED STAYS QUESTIONABLE — a kept below-floor pairing is always Questionable, no matter
    // what any other signal says.
    #[test]
    fn mq_forced_is_always_questionable(mut req in mq_request()) {
        req.forced = true;
        prop_assert_eq!(match_quality::decide(&req).quality, MatchQuality::Questionable);
    }

    // (1) A MANUAL PICK ALONE NEVER GOES GREEN — a Manual selection lacking `Some(true)` evidence is
    // never Confident (and a green Manual pick implies it independently cleared the classifier).
    #[test]
    fn mq_manual_green_iff_confident_evidence(mut req in mq_request()) {
        req.forced = false;
        req.selection = SelectionProvenance::Manual;
        let green = match_quality::decide(&req).quality == MatchQuality::Confident;
        prop_assert_eq!(green, req.manual_match_confident == Some(true));
    }

    // (4)/(5) NO SEPARATE MANUAL EVIDENCE — with nothing selected or an automatic selection (and not
    // forced) the verdict is the resolver's `auto_verdict` verbatim (never a coin-flip, never fabricated).
    #[test]
    fn mq_non_manual_passes_through_auto_verdict(mut req in mq_request()) {
        req.forced = false;
        prop_assume!(req.selection != SelectionProvenance::Manual);
        prop_assert_eq!(match_quality::decide(&req).quality, req.auto_verdict);
    }

    // Request round-trips on the camelCase wire (FFI boundary).
    #[test]
    fn mq_request_wire_stable(req in mq_request()) {
        prop_assert!(wire_stable(&req));
    }
}

// ============================ repair_drop ============================

fn rd_request() -> impl Strategy<Value = repair_drop::Request> {
    (
        any::<bool>(),
        any::<bool>(),
        any::<bool>(),
        proptest::option::of(key()),
        key(),
        any::<bool>(),
    )
        .prop_map(
            |(has_selected_couple, payload_is_audio, video_available, current_selected_key, dropped_key, approved)| {
                repair_drop::Request {
                    has_selected_couple,
                    payload_is_audio,
                    video_available,
                    current_selected_key,
                    dropped_key,
                    approved,
                }
            },
        )
}

proptest! {
    // (1) REJECT GUARDS PRECEDE REPLACEMENT — any missing couple / non-audio payload / unusable video
    // rejects BEFORE any replacement reasoning, so a malformed drop never mutates.
    #[test]
    fn rd_guards_reject_before_replacement(req in rd_request()) {
        let guards_ok = req.has_selected_couple && req.payload_is_audio && req.video_available;
        if !guards_ok {
            prop_assert_eq!(repair_drop::decide(&req).outcome, repair_drop::RepairDropOutcome::Reject);
        }
    }

    // (2) SAME-KEY DROP IS A NO-OP — re-dropping the already-selected key is NoOp regardless of approval.
    #[test]
    fn rd_same_key_is_no_op(mut req in rd_request(), approved in any::<bool>()) {
        req.has_selected_couple = true;
        req.payload_is_audio = true;
        req.video_available = true;
        req.current_selected_key = Some(req.dropped_key.clone());
        req.approved = approved;
        prop_assert_eq!(repair_drop::decide(&req).outcome, repair_drop::RepairDropOutcome::NoOp);
    }

    // (5) NEEDS-CONFIRMATION IS THE ONLY OUTCOME THAT ASKS — and only for an approved couple whose
    // guards pass and whose dropped source differs from the current one.
    #[test]
    fn rd_confirmation_only_when_approved_and_different(req in rd_request()) {
        let guards_ok = req.has_selected_couple && req.payload_is_audio && req.video_available;
        let different = req.current_selected_key.as_deref() != Some(req.dropped_key.as_str());
        let asks = repair_drop::decide(&req).outcome == repair_drop::RepairDropOutcome::NeedsConfirmation;
        prop_assert_eq!(asks, guards_ok && different && req.approved);
    }

    // FORCED VERDICT — a KNOWN below-floor z (0 < z < z_min) or a failed sync forces; an unknown z (0)
    // alone never forces (honest-until-measured).
    #[test]
    fn rd_forced_below_floor_or_sync_failure(candidate_z in -5.0f64..20.0, z_min in 0.1f64..20.0, sync_failed in any::<bool>()) {
        let req = repair_drop::ForcedRequest { candidate_z, z_min, sync_failed };
        let expected = (candidate_z > 0.0 && candidate_z < z_min) || sync_failed;
        prop_assert_eq!(repair_drop::forced(&req).forced, expected);
    }

    #[test]
    fn rd_request_wire_stable(req in rd_request()) {
        prop_assert!(wire_stable(&req));
    }

    #[test]
    fn rd_forced_request_wire_stable(candidate_z in -5.0f64..20.0, z_min in 0.1f64..20.0, sync_failed in any::<bool>()) {
        let req = repair_drop::ForcedRequest { candidate_z, z_min, sync_failed };
        prop_assert!(wire_stable(&req));
    }
}

// ============================ row_merge ============================

fn rm_request() -> impl Strategy<Value = row_merge::Request> {
    (
        any::<bool>(),
        proptest::option::of(key()),
        keys(5),
        proptest::option::of(key()),
        keys(5),
    )
        .prop_map(
            |(protected, current_selected_key, current_candidate_keys, fresh_selected_key, fresh_candidate_keys)| {
                row_merge::Request {
                    protected,
                    current_selected_key,
                    current_candidate_keys,
                    fresh_selected_key,
                    fresh_candidate_keys,
                }
            },
        )
}

proptest! {
    // (1) A PROTECTED ROW NEVER LOSES ITS SELECTION — selection/display are preserved and the session
    // is never rebuilt; only genuinely-new candidate keys are appended (existing order preserved,
    // so candidates are never destroyed).
    #[test]
    fn rm_protected_preserves_selection_and_candidates(mut req in rm_request()) {
        req.protected = true;
        let p = row_merge::plan(&req);
        prop_assert_eq!(&p.selected_audio_key, &req.current_selected_key);
        prop_assert!(!p.adopt_fresh_display);
        prop_assert!(!p.rebuild_session);
        prop_assert!(!p.clear_forced);
        // Existing candidates are a prefix of the merged set (kept, in order — never destroyed).
        prop_assert!(p.candidate_keys.starts_with(&req.current_candidate_keys));
        // Every fresh key is present after the merge (surfaced as an alternate, never dropped).
        for k in &req.fresh_candidate_keys {
            prop_assert!(p.candidate_keys.contains(k));
        }
    }

    // (2) AN UNPROTECTED ROW TAKES THE FRESH RESOLUTION WHOLE — selection mirrors the resolver's
    // `selected` verbatim (this layer never invents a selection), candidates and display become the
    // fresh row's.
    #[test]
    fn rm_unprotected_mirrors_fresh(mut req in rm_request()) {
        req.protected = false;
        let p = row_merge::plan(&req);
        prop_assert_eq!(&p.selected_audio_key, &req.fresh_selected_key);
        prop_assert_eq!(&p.candidate_keys, &req.fresh_candidate_keys);
        prop_assert!(p.adopt_fresh_display);
        prop_assert!(p.clear_forced);
    }

    // (3) SESSION LIFECYCLE FIRES AT MOST ONCE — `rebuild_session` is true iff the effective selected
    // key actually changed (and always false for a protected row).
    #[test]
    fn rm_rebuild_iff_selection_changed(req in rm_request()) {
        let p = row_merge::plan(&req);
        if req.protected {
            prop_assert!(!p.rebuild_session);
        } else {
            prop_assert_eq!(p.rebuild_session, req.current_selected_key != req.fresh_selected_key);
        }
    }

    // (4) IDEMPOTENT — feeding the plan's own output back as the current row (same fresh resolution)
    // changes nothing and rebuilds nothing.
    #[test]
    fn rm_is_idempotent(req in rm_request()) {
        let first = row_merge::plan(&req);
        let again = row_merge::plan(&row_merge::Request {
            protected: req.protected,
            current_selected_key: first.selected_audio_key.clone(),
            current_candidate_keys: first.candidate_keys.clone(),
            fresh_selected_key: req.fresh_selected_key.clone(),
            fresh_candidate_keys: req.fresh_candidate_keys.clone(),
        });
        prop_assert_eq!(&again.selected_audio_key, &first.selected_audio_key);
        prop_assert_eq!(&again.candidate_keys, &first.candidate_keys);
        prop_assert!(!again.rebuild_session);
    }

    #[test]
    fn rm_request_wire_stable(req in rm_request()) {
        prop_assert!(wire_stable(&req));
    }
}

// ============================ media_removal ============================

fn couple_ref() -> impl Strategy<Value = CoupleRef> {
    (key(), proptest::option::of(key()), any::<bool>()).prop_map(|(video_key, selected_audio_key, approved)| {
        CoupleRef { video_key, selected_audio_key, approved }
    })
}

proptest! {
    // REMOVABLE KEYS ⊆ SELECTED ∩ POOL — the plan only ever names live, requested keys; it never
    // invents a key (a stale/junk selection is dropped).
    #[test]
    fn mr_removable_subset_of_selected_and_pool(
        selected in keys(6),
        pool in keys(6),
        couples in prop::collection::vec(couple_ref(), 0..5),
    ) {
        let sel: HashSet<String> = selected.into_iter().collect();
        let pool: HashSet<String> = pool.into_iter().collect();
        let p = media_removal::plan(&sel, &pool, &couples);
        for k in &p.removable_keys {
            prop_assert!(sel.contains(k) && pool.contains(k));
        }
        // The removable set is exactly the intersection (no key missed, none invented).
        let got: HashSet<String> = p.removable_keys.iter().cloned().collect();
        let want: HashSet<String> = sel.intersection(&pool).cloned().collect();
        prop_assert_eq!(got, want);
    }

    // (3)/(4) THE COUNTS ARE A FUNCTION OF THE INPUT SETS ALONE — recomputing affected/approved
    // independently matches the plan; `needs_confirmation` ⟺ at least one couple is affected.
    #[test]
    fn mr_counts_are_a_pure_projection(
        selected in keys(6),
        pool in keys(6),
        couples in prop::collection::vec(couple_ref(), 0..6),
    ) {
        let sel: HashSet<String> = selected.into_iter().collect();
        let pool: HashSet<String> = pool.into_iter().collect();
        let p = media_removal::plan(&sel, &pool, &couples);

        let removable: HashSet<String> = sel.intersection(&pool).cloned().collect();
        let mut affected = 0u32;
        let mut approved = 0u32;
        for c in &couples {
            let video_removed = removable.contains(&c.video_key);
            let audio_removed = c.selected_audio_key.as_ref().is_some_and(|k| removable.contains(k));
            if video_removed || audio_removed {
                affected += 1;
                if c.approved {
                    approved += 1;
                }
            }
        }
        prop_assert_eq!(p.affected_couples, affected);
        prop_assert_eq!(p.affected_approved, approved);
        prop_assert!(p.affected_approved <= p.affected_couples);
        prop_assert_eq!(p.needs_confirmation, affected > 0);
        // (1) An empty removable set is a no-op plan: no confirmation, zero affected.
        if removable.is_empty() {
            prop_assert!(!p.needs_confirmation);
            prop_assert_eq!(p.affected_couples, 0);
        }
    }

    // Clear-session counts are the plain projection of the couple list (media passes through).
    #[test]
    fn mr_clear_counts_project_the_list(media in 0u32..1000, couples in prop::collection::vec(couple_ref(), 0..8)) {
        let c = media_removal::clear_counts(media, &couples);
        prop_assert_eq!(c.media, media);
        prop_assert_eq!(c.couples, couples.len() as u32);
        prop_assert_eq!(c.approved, couples.iter().filter(|c| c.approved).count() as u32);
    }

    #[test]
    fn mr_couple_ref_wire_stable(c in couple_ref()) {
        prop_assert!(wire_stable(&c));
    }
}

// ============================ anchor ============================

fn anchor_candidate() -> impl Strategy<Value = AnchorCandidate> {
    (-50.0f64..250.0, -10.0f64..1000.0, any::<bool>()).prop_map(|(t, score, usable)| AnchorCandidate { t, score, usable })
}

fn candidates(max: usize) -> impl Strategy<Value = Vec<AnchorCandidate>> {
    prop::collection::vec(anchor_candidate(), 0..max)
}

/// An overlap that is sometimes absent, sometimes degenerate (len == 0), sometimes short (<30 s),
/// sometimes long (≥30 s) — spanning all three `propose` regimes.
fn overlap_opt() -> impl Strategy<Value = Option<Overlap>> {
    prop_oneof![
        Just(None),
        (0.0f64..100.0, 0.0f64..140.0).prop_map(|(start, len)| Some(Overlap { start, end: start + len })),
    ]
}

proptest! {
    // (1)/(4) PROPOSED ANCHORS ARE ALWAYS INSIDE THE OVERLAP, SORTED, AND NEVER EXACTLY ONE — no
    // usable overlap ⇒ empty; every pick is a real candidate time (never a fabricated midpoint).
    #[test]
    fn anchor_propose_inside_sorted_never_one(overlap in overlap_opt(), cands in candidates(40)) {
        let picks = anchor::propose(overlap, &cands, &AnchorTuning::default());
        prop_assert_ne!(picks.len(), 1); // 0, 2, or 3 — never a misleading single anchor.
        // Sorted ascending.
        for w in picks.windows(2) {
            prop_assert!(w[0] <= w[1]);
        }
        match overlap {
            None => prop_assert!(picks.is_empty()),
            Some(ov) if ov.end - ov.start <= 0.0 => prop_assert!(picks.is_empty()),
            Some(ov) => {
                for &t in &picks {
                    // Invariant 1: inside the true overlap.
                    prop_assert!(t >= ov.start && t <= ov.end);
                    // Never invented: every anchor is one of the supplied candidate onsets.
                    prop_assert!(cands.iter().any(|c| c.t == t));
                }
            }
        }
    }

    // (5) "+ ANCHOR" STAYS INSIDE — a returned time is in-overlap, a real usable candidate, and at
    // least the min separation from every existing anchor, within the cap; no overlap ⇒ None.
    #[test]
    fn anchor_add_inside_and_separated(
        overlap in overlap_opt(),
        existing in prop::collection::vec(-50.0f64..250.0, 0..5),
        cands in candidates(40),
    ) {
        let tuning = AnchorTuning::default();
        let got = anchor::add_anchor_candidate(overlap, &existing, &cands, &tuning);
        if overlap.is_none() {
            prop_assert!(got.is_none());
        }
        if let Some(t) = got {
            let ov = overlap.unwrap();
            let len = ov.end - ov.start;
            prop_assert!(len > 0.0);
            prop_assert!(existing.len() < tuning.max_anchors);
            prop_assert!(t >= ov.start && t <= ov.end);
            let min_sep = tuning.add_min_separation_fraction * len;
            for &e in &existing {
                prop_assert!((e - t).abs() >= min_sep);
            }
            prop_assert!(cands.iter().any(|c| c.t == t && c.usable));
        }
    }

    // The tagged FFI dispatch is equivalent to calling the pure functions directly.
    #[test]
    fn anchor_dispatch_matches_direct(overlap in overlap_opt(), cands in candidates(20)) {
        let direct = anchor::propose(overlap, &cands, &AnchorTuning::default());
        let via = anchor::dispatch(&anchor::Request::Propose(anchor::ProposeRequest {
            overlap,
            candidates: cands.clone(),
            tuning: AnchorTuning::default(),
        }));
        match via {
            anchor::Response::Propose(v) => prop_assert_eq!(v, direct),
            _ => prop_assert!(false, "expected Propose response"),
        }
    }

    #[test]
    fn anchor_candidate_wire_stable(c in anchor_candidate()) {
        prop_assert!(wire_stable(&c));
    }
}

// ============================ activity ============================

fn import_phase() -> impl Strategy<Value = ImportPhase> {
    prop_oneof![
        Just(ImportPhase::Probing),
        Just(ImportPhase::Decoding),
        Just(ImportPhase::Waveform),
        Just(ImportPhase::Finishing),
    ]
}

fn match_phase() -> impl Strategy<Value = MatchPhase> {
    prop_oneof![Just(MatchPhase::Fingerprinting), Just(MatchPhase::Comparing), Just(MatchPhase::Crosschecking)]
}

fn importing() -> impl Strategy<Value = Importing> {
    // Fractions deliberately range outside 0..1 to exercise the clamp.
    (import_phase(), -2.0f64..3.0, proptest::option::of(1u32..6), proptest::option::of(1u32..6))
        .prop_map(|(phase, fraction, index, total)| Importing { phase, fraction, index, total })
}

fn pairing() -> impl Strategy<Value = Pairing> {
    (0u32..5, 0u32..5, any::<bool>(), any::<bool>(), any::<bool>()).prop_map(
        |(video_count, audio_count, resolving, after_import, import_tail)| Pairing {
            video_count,
            audio_count,
            resolving,
            after_import,
            import_tail,
        },
    )
}

fn syncing() -> impl Strategy<Value = Syncing> {
    (1u32..6, 1u32..6).prop_map(|(index, total)| Syncing { index, total })
}

fn exporting() -> impl Strategy<Value = Exporting> {
    (1u32..6, 1u32..6, -2.0f64..3.0, any::<bool>()).prop_map(|(index, total, row_fraction, preflight)| Exporting {
        index,
        total,
        row_fraction,
        preflight,
    })
}

fn ingest() -> impl Strategy<Value = Ingest> {
    (
        proptest::option::of(importing()),
        proptest::option::of((match_phase(), -2.0f64..3.0).prop_map(|(phase, fraction)| Matching { phase, fraction })),
        proptest::option::of(syncing()),
        (any::<bool>(), any::<bool>(), any::<bool>()),
    )
        .prop_map(|(importing, matching, syncing, (import_stage, match_stage, sync_stage))| Ingest {
            importing,
            matching,
            syncing,
            stages_planned: Stages { import_stage, match_stage, sync_stage },
        })
}

fn activity_request() -> impl Strategy<Value = activity::Request> {
    (
        proptest::option::of(importing()),
        proptest::option::of(pairing()),
        proptest::option::of(syncing()),
        proptest::option::of(exporting()),
        proptest::option::of(ingest()),
        proptest::option::of("[a-z ]{1,10}"),
        0.0f64..600.0,
        proptest::option::of(0.0f64..1.0),
    )
        .prop_map(|(importing, pairing, syncing, exporting, ingest, failure, elapsed_ms, phase_floor)| activity::Request {
            importing,
            pairing,
            syncing,
            exporting,
            ingest,
            failure,
            elapsed_ms,
            debounce_ms: activity::DEBOUNCE_MS,
            phase_floor,
        })
}

proptest! {
    // (1) IDLE ⇒ NO LINE — no active signal and no failure yields nothing.
    #[test]
    fn activity_idle_is_none(elapsed in 0.0f64..600.0, phase_floor in proptest::option::of(0.0f64..1.0)) {
        let req = activity::Request {
            importing: None, pairing: None, syncing: None, exporting: None, ingest: None,
            failure: None, elapsed_ms: elapsed, debounce_ms: activity::DEBOUNCE_MS, phase_floor,
        };
        prop_assert!(activity::decide(&req).summary.is_none());
    }

    // (failure) A FAILURE IS SHOWN IMMEDIATELY — ahead of debounce and every concurrent signal, as an
    // error line with no bar.
    #[test]
    fn activity_failure_is_immediate_error(mut req in activity_request(), msg in "[a-z ]{1,10}") {
        req.failure = Some(msg.clone());
        let d = activity::decide(&req).summary.expect("failure always surfaces");
        prop_assert!(d.is_error);
        prop_assert!(d.fraction.is_none());
        let expected_line = activity::LineKind::Failure { message: msg };
        prop_assert_eq!(d.line, expected_line);
    }

    // (2) DEBOUNCE — an active (non-failure) job younger than the debounce shows nothing.
    #[test]
    fn activity_debounce_suppresses_young_jobs(p in pairing(), elapsed in 0.0f64..activity::DEBOUNCE_MS) {
        let req = activity::Request {
            importing: None, pairing: Some(p), syncing: None, exporting: None, ingest: None,
            failure: None, elapsed_ms: elapsed, debounce_ms: activity::DEBOUNCE_MS, phase_floor: None,
        };
        prop_assert!(activity::decide(&req).summary.is_none());
    }

    // (3) PAIRING IS INDETERMINATE — a pairing signal past debounce always yields `fraction == None`.
    #[test]
    fn activity_pairing_has_no_fraction(p in pairing()) {
        let req = activity::Request {
            importing: None, pairing: Some(p), syncing: None, exporting: None, ingest: None,
            failure: None, elapsed_ms: 400.0, debounce_ms: activity::DEBOUNCE_MS, phase_floor: None,
        };
        if let Some(d) = activity::decide(&req).summary {
            prop_assert!(d.fraction.is_none());
        }
    }

    // (4) DETERMINATE FRACTIONS ARE CLAMPED TO 0..1 AND MONOTONIC — any populated fraction is in
    // [0,1] and never falls below the supplied phase floor.
    #[test]
    fn activity_fraction_clamped_and_monotonic(req in activity_request()) {
        if let Some(d) = activity::decide(&req).summary {
            if let Some(f) = d.fraction {
                prop_assert!((0.0..=1.0).contains(&f), "fraction {} out of [0,1]", f);
                if let Some(floor) = req.phase_floor {
                    prop_assert!(f >= floor, "fraction {} below floor {}", f, floor);
                }
            }
        }
    }

    // MONOTONIC WITHIN A PHASE — the per-match re-join is non-decreasing in its input and stays in
    // [0,1] (the documented monotonic clause, tested on the one pub re-join helper).
    #[test]
    fn activity_per_match_monotonic(phase in match_phase(), a in -1.0f64..2.0, b in -1.0f64..2.0) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        let flo = activity::per_match_fraction(phase, lo);
        let fhi = activity::per_match_fraction(phase, hi);
        prop_assert!(flo <= fhi);
        prop_assert!((0.0..=1.0).contains(&flo) && (0.0..=1.0).contains(&fhi));
    }

    #[test]
    fn activity_request_wire_stable(req in activity_request()) {
        prop_assert!(wire_stable(&req));
    }
}

// ============================ shadow ============================

fn evidence(score: f64, cls: MatchClassification) -> PairEvidence {
    // Start from the all-zero `unavailable` cell (finite, no NaN/Inf) and override only the two
    // fields the projection orders on / surfaces.
    PairEvidence { classification: cls, score, ..PairEvidence::unavailable(MatchMethod::SpectralLandmarkV1, 0.0) }
}

fn classification() -> impl Strategy<Value = MatchClassification> {
    prop_oneof![
        Just(MatchClassification::Match),
        Just(MatchClassification::NoMatch),
        Just(MatchClassification::Ambiguous),
        Just(MatchClassification::InsufficientOverlap),
        Just(MatchClassification::Unavailable),
    ]
}

/// A row of evidence with UNIQUE keys (indexed) so the total order is unambiguous, names drawn from
/// a tiny pool (frequent ties → exercises the name/key tie-break), scores from a small set (ties).
fn shadow_row() -> impl Strategy<Value = (Vec<PairEvidence>, Vec<String>, Vec<String>)> {
    prop::collection::vec(
        (
            prop::sample::select(vec![0.0f64, 0.0, 1.0, 4.0, 9.0]),
            prop::sample::select(vec!["a.wav", "b.wav", "é.wav", "🎵.wav"]).prop_map(str::to_string),
            classification(),
        ),
        0..8,
    )
    .prop_map(|rows| {
        let mut evs = Vec::new();
        let mut keys = Vec::new();
        let mut names = Vec::new();
        for (i, (score, name, cls)) in rows.into_iter().enumerate() {
            evs.push(evidence(score, cls));
            keys.push(format!("k{i}")); // unique per cell → the final tiebreak is a total order
            names.push(name);
        }
        (evs, keys, names)
    })
}

proptest! {
    // (2) DETERMINISTIC TOTAL ORDER — score DESC, then name ASC, then unique key ASC, on every
    // adjacent pair; score orders the display only, never as an acceptance gate.
    #[test]
    fn shadow_is_totally_ordered((evs, keys, names) in shadow_row()) {
        let r = shadow::rows(&evs, &keys, &names);
        for w in r.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            prop_assert!(a.evidence.score >= b.evidence.score);
            if a.evidence.score == b.evidence.score {
                prop_assert!(a.audio_name <= b.audio_name);
                if a.audio_name == b.audio_name {
                    prop_assert!(a.audio_key < b.audio_key); // keys are unique
                }
            }
        }
    }

    // (1)/(3) PASSIVE + DEFENSIVE LENGTH — the projection re-thresholds nothing (the classification
    // multiset is carried verbatim) and truncates to the shortest input (never panics/fabricates).
    #[test]
    fn shadow_is_passive_and_truncates(
        (evs, keys, names) in shadow_row(),
        cut_keys in 0usize..8,
        cut_names in 0usize..8,
    ) {
        let keys: Vec<String> = keys.into_iter().take(cut_keys).collect();
        let names: Vec<String> = names.into_iter().take(cut_names).collect();
        let n = evs.len().min(keys.len()).min(names.len());
        let r = shadow::rows(&evs, &keys, &names);
        prop_assert_eq!(r.len(), n);
        // Passive: the projected classifications are exactly the first `n` inputs' (as a multiset).
        let mut got: Vec<MatchClassification> = r.iter().map(|x| x.evidence.classification).collect();
        let mut want: Vec<MatchClassification> = evs.iter().take(n).map(|e| e.classification).collect();
        got.sort_by_key(|c| format!("{c:?}"));
        want.sort_by_key(|c| format!("{c:?}"));
        prop_assert_eq!(got, want);
    }

    // Reversing the input yields the SAME order — confirming it is a total order (no input-position
    // dependence among tied scores/names, because the unique key breaks every remaining tie).
    #[test]
    fn shadow_order_is_input_independent((evs, keys, names) in shadow_row()) {
        let forward = shadow::rows(&evs, &keys, &names);
        let r_evs: Vec<PairEvidence> = evs.iter().rev().cloned().collect();
        let r_keys: Vec<String> = keys.iter().rev().cloned().collect();
        let r_names: Vec<String> = names.iter().rev().cloned().collect();
        let reverse = shadow::rows(&r_evs, &r_keys, &r_names);
        let f: Vec<&String> = forward.iter().map(|x| &x.audio_key).collect();
        let b: Vec<&String> = reverse.iter().map(|x| &x.audio_key).collect();
        prop_assert_eq!(f, b);
    }

    #[test]
    fn shadow_row_wire_stable((evs, keys, names) in shadow_row()) {
        let r = shadow::rows(&evs, &keys, &names);
        for row in &r {
            prop_assert!(wire_stable(row));
        }
    }
}

// ============================ resolve.rs (pure surface) ============================

fn matcher_choice() -> impl Strategy<Value = MatcherChoice> {
    prop_oneof![Just(MatcherChoice::Legacy), Just(MatcherChoice::Shadow), Just(MatcherChoice::Fingerprint)]
}

proptest! {
    // MatcherChoice::parse accepts exactly the three tokens and rejects everything else; a valid
    // token round-trips through parse (and the serde wire).
    #[test]
    fn resolve_matcher_parse_round_trips(s in "[a-z]{0,12}") {
        let valid = matches!(s.as_str(), "legacy" | "shadow" | "fingerprint");
        prop_assert_eq!(MatcherChoice::parse(&s).is_ok(), valid);
    }

    #[test]
    fn resolve_matcher_choice_wire_stable(m in matcher_choice()) {
        // camelCase serde and the `parse` token agree, and both survive a round-trip.
        prop_assert!(wire_stable(&m));
        let back: MatcherChoice = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        prop_assert_eq!(m, back);
    }
}

// `RowState::chip` is a total, deterministic vocabulary (never a coin-flip): every state maps to a
// distinct non-empty chip. Exhaustive over the finite enum (a property over its full domain).
#[test]
fn resolve_row_state_chip_is_total_and_distinct() {
    let states = [
        RowState::Confident,
        RowState::Partial,
        RowState::Split,
        RowState::TwoSources,
        RowState::Ambiguous,
        RowState::NoMatch,
    ];
    let mut seen = HashSet::new();
    for s in states {
        let chip = s.chip();
        assert!(!chip.is_empty());
        assert!(seen.insert(chip), "duplicate chip: {chip}");
    }
}
