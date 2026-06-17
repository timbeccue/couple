//! Shadow-telemetry projection (PLAN-SPECTRAL-FINGERPRINT-MATCHING §14.2/§14.4) — the PURE
//! projection behind Swift `ShadowTelemetry.rows`. PLAN-CORE-SHELL-BOUNDARY Phase E policy 6;
//! relocated verbatim from `mac/Sources/CoupleKit/Wire.swift` (a RELOCATION, not a behavior
//! change). It is a PASSIVE projection of a row of core [`PairEvidence`] (the matrix row for ONE
//! video, one cell per audio) into ordered, displayable rows — NO acceptance thresholding. The
//! shell's `ShadowEvidenceRow` (display fields/strings) is rebuilt from each projected row's
//! verbatim evidence; core decides only the ORDER and the index-alignment/truncation.
//!
//! CONTRACT (the testable invariants):
//!   1. PASSIVE — it copies core fields and does NO acceptance thresholding / no `matched`
//!      recomputation. `classification` is whatever core decided (§14.2): a `match` and a `noMatch`
//!      both appear, verbatim.
//!   2. DETERMINISTIC TOTAL ORDER — rows sort by `score` DESCENDING, ties broken by `audioName`
//!      ascending then `audioKey` ascending. Because the final tiebreak (`audioKey`, the unique
//!      pool content key) is unique per cell, the order is a TOTAL order: equal/zero scores never
//!      leave the result ambiguous (`score` orders the display ONLY, never as an acceptance gate).
//!   3. DEFENSIVE LENGTH HANDLING — mismatched `evidence`/`audio_keys`/`audio_names` truncate to
//!      the shortest (no crash, no fabrication).

use crate::matching::PairEvidence;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;

/// One projected shadow-telemetry row: the audio identity (key + name) plus the core
/// [`PairEvidence`] for that (video, audio) cell, carried VERBATIM. The shell rebuilds its display
/// `ShadowEvidenceRow` (method/classification/lag/drift/score/quality fields + any strings) from
/// `evidence`; nothing is re-decided here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShadowEvidenceRow {
    /// Pool content key of the audio this evidence scores (stable identity; the unique final
    /// ordering tiebreak).
    pub audio_key: String,
    /// Audio file name (the displayed identity; also the first ordering tiebreak).
    pub audio_name: String,
    /// The core evidence cell, copied verbatim (the projector never re-thresholds it).
    pub evidence: PairEvidence,
}

/// The projection request: a video's evidence row plus the index-aligned audio keys/names (cell `i`
/// scores audio `i`, exactly the column order `match_candidates` returns).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub evidence: Vec<PairEvidence>,
    pub audio_keys: Vec<String>,
    pub audio_names: Vec<String>,
}

/// Project + order one video's evidence row. Mismatched lengths defensively truncate to the
/// shortest. Pure; no ctx, no IO.
pub fn rows(evidence: &[PairEvidence], audio_keys: &[String], audio_names: &[String]) -> Vec<ShadowEvidenceRow> {
    let n = evidence.len().min(audio_keys.len()).min(audio_names.len());
    let mut out: Vec<ShadowEvidenceRow> = (0..n)
        .map(|i| ShadowEvidenceRow {
            audio_key: audio_keys[i].clone(),
            audio_name: audio_names[i].clone(),
            evidence: evidence[i].clone(),
        })
        .collect();
    // Deterministic TOTAL order: score desc, then name asc, then key asc (key is unique). `score`
    // orders the display; it is NEVER an acceptance gate (that is core's classification).
    out.sort_by(|a, b| {
        match b
            .evidence
            .score
            .partial_cmp(&a.evidence.score)
            .unwrap_or(Ordering::Equal)
        {
            Ordering::Equal => {}
            ord => return ord,
        }
        match a.audio_name.cmp(&b.audio_name) {
            Ordering::Equal => {}
            ord => return ord,
        }
        a.audio_key.cmp(&b.audio_key)
    });
    out
}

/// Project from a decoded `Request`.
pub fn project(req: &Request) -> Vec<ShadowEvidenceRow> {
    rows(&req.evidence, &req.audio_keys, &req.audio_names)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::{MatchClassification, MatchMethod};

    /// A minimal evidence builder (mirrors the Swift test `ev` helper): only the fields the
    /// projection's order + the passthrough assertions touch.
    fn ev(cls: MatchClassification, score: f64, lag: f64, refined_lag: Option<f64>, refined_drift: Option<f64>, support: u32, p95: Option<f64>) -> PairEvidence {
        PairEvidence {
            method: MatchMethod::SpectralLandmarkV1,
            classification: cls,
            lag_ms: lag,
            drift_ppm: 0.0,
            score,
            runner_up_score: 0.0,
            score_margin: 0.0,
            support,
            candidate_correspondences: 0,
            inlier_fraction: 0.0,
            temporal_coverage: 0.0,
            residual_rms_ms: None,
            residual_p95_ms: p95,
            overlap_s: 0.0,
            refined_lag_ms: refined_lag,
            refined_drift_ppm: refined_drift,
            refined_rms_ms: None,
            legacy: None,
            refine_reason: Default::default(),
        }
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    // (1) Orders by score DESCENDING and surfaces classification/score/refined mapping verbatim.
    #[test]
    fn orders_by_score_and_projects_fields() {
        let evidence = vec![
            ev(MatchClassification::NoMatch, 1.0, 100.0, None, None, 5, None), // b.wav
            ev(MatchClassification::Match, 9.0, 1800.0, Some(1801.0), Some(60.0), 50, Some(0.9)), // a.wav
            ev(MatchClassification::Ambiguous, 4.0, 500.0, None, None, 20, None), // c.wav
        ];
        let r = rows(&evidence, &s(&["kb", "ka", "kc"]), &s(&["b.wav", "a.wav", "c.wav"]));
        assert_eq!(r.len(), 3);
        assert_eq!(r.iter().map(|x| x.audio_name.as_str()).collect::<Vec<_>>(), vec!["a.wav", "c.wav", "b.wav"]);
        assert_eq!(r.iter().map(|x| x.evidence.score).collect::<Vec<_>>(), vec![9.0, 4.0, 1.0]);
        assert_eq!(
            r.iter().map(|x| x.evidence.classification).collect::<Vec<_>>(),
            vec![MatchClassification::Match, MatchClassification::Ambiguous, MatchClassification::NoMatch]
        );
        assert_eq!(r[0].evidence.refined_lag_ms, Some(1801.0));
        assert_eq!(r[0].evidence.refined_drift_ppm, Some(60.0));
        assert_eq!(r[0].evidence.residual_p95_ms, Some(0.9));
        assert_eq!(r[0].evidence.support, 50);
        assert_eq!(r[2].evidence.lag_ms, 100.0); // b.wav
        assert_eq!(r[1].evidence.lag_ms, 500.0); // c.wav
        assert_eq!(r[0].audio_key, "ka");
    }

    // (2) NO acceptance thresholding — a match and a noMatch both surface; a high-score noMatch
    //     sorts ahead of a low-score match (score is display-only, not a gate).
    #[test]
    fn does_no_acceptance_thresholding() {
        let evidence = vec![
            ev(MatchClassification::Match, 0.5, 0.0, None, None, 3, None),
            ev(MatchClassification::NoMatch, 8.0, 0.0, None, None, 99, None),
        ];
        let r = rows(&evidence, &s(&["k0", "k1"]), &s(&["m.wav", "n.wav"]));
        assert_eq!(r[0].evidence.classification, MatchClassification::NoMatch);
        assert_eq!(r[0].evidence.score, 8.0);
        assert_eq!(r[1].evidence.classification, MatchClassification::Match);

        // Every variant passes through unchanged (score desc preserves input order here).
        let all = [
            MatchClassification::Match,
            MatchClassification::NoMatch,
            MatchClassification::Ambiguous,
            MatchClassification::InsufficientOverlap,
            MatchClassification::Unavailable,
        ];
        let evidence: Vec<PairEvidence> = all
            .iter()
            .enumerate()
            .map(|(i, &c)| ev(c, (all.len() - i) as f64, 0.0, None, None, 0, None))
            .collect();
        let keys: Vec<String> = (0..all.len()).map(|i| format!("k{i}")).collect();
        let names: Vec<String> = (0..all.len()).map(|i| format!("f{i}.wav")).collect();
        let r = rows(&evidence, &keys, &names);
        assert_eq!(r.iter().map(|x| x.evidence.classification).collect::<Vec<_>>(), all.to_vec());
    }

    // Determinism — equal/zero scores fall back to name asc then unique key asc; reversing the
    // input yields the SAME order (a total order).
    #[test]
    fn deterministic_total_order_on_tied_scores() {
        let keys = s(&["k3", "k1", "k2"]);
        let names = s(&["c.wav", "a.wav", "a.wav"]); // two share a name ⇒ key breaks the tie
        let evidence: Vec<PairEvidence> =
            (0..3).map(|_| ev(MatchClassification::Ambiguous, 0.0, 0.0, None, None, 0, None)).collect();
        let forward = rows(&evidence, &keys, &names);
        assert_eq!(
            forward.iter().map(|x| format!("{}/{}", x.audio_name, x.audio_key)).collect::<Vec<_>>(),
            vec!["a.wav/k1", "a.wav/k2", "c.wav/k3"]
        );
        let r_keys: Vec<String> = keys.iter().rev().cloned().collect();
        let r_names: Vec<String> = names.iter().rev().cloned().collect();
        let reverse = rows(&evidence, &r_keys, &r_names);
        assert_eq!(
            reverse.iter().map(|x| x.audio_key.clone()).collect::<Vec<_>>(),
            forward.iter().map(|x| x.audio_key.clone()).collect::<Vec<_>>()
        );
    }

    // Defensive length handling — mismatched lengths truncate to the shortest, never panic.
    #[test]
    fn truncates_mismatched_lengths() {
        let evidence = vec![
            ev(MatchClassification::Match, 1.0, 0.0, None, None, 0, None),
            ev(MatchClassification::NoMatch, 2.0, 0.0, None, None, 0, None),
        ];
        let r = rows(&evidence, &s(&["k0"]), &s(&["only.wav"]));
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].audio_key, "k0");
    }

    #[test]
    fn request_deserializes_and_projects() {
        let req: Request = serde_json::from_str(
            r#"{"evidence":[
                  {"method":"spectralLandmarkV1","classification":"noMatch","lagMs":0,"driftPpm":0,
                   "score":1,"runnerUpScore":0,"scoreMargin":0,"support":0,"candidateCorrespondences":0,
                   "inlierFraction":0,"temporalCoverage":0,"residualRmsMs":null,"residualP95Ms":null,
                   "overlapS":0,"refinedLagMs":null,"refinedDriftPpm":null,"refinedRmsMs":null},
                  {"method":"spectralLandmarkV1","classification":"match","lagMs":0,"driftPpm":0,
                   "score":9,"runnerUpScore":0,"scoreMargin":0,"support":0,"candidateCorrespondences":0,
                   "inlierFraction":0,"temporalCoverage":0,"residualRmsMs":null,"residualP95Ms":null,
                   "overlapS":0,"refinedLagMs":null,"refinedDriftPpm":null,"refinedRmsMs":null}],
                "audioKeys":["kb","ka"],"audioNames":["b.wav","a.wav"]}"#,
        )
        .unwrap();
        let r = project(&req);
        assert_eq!(r[0].audio_name, "a.wav"); // score 9 sorts first
        assert_eq!(r[0].evidence.classification, MatchClassification::Match);
    }
}
