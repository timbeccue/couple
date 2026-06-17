//! PLAN-SPECTRAL-FINGERPRINT-MATCHING boundary 10 (§12) END-TO-END gates: metadata time hints are
//! ADVISORY and NON-VETOING. Two halves:
//!
//! 1. The §12.2 reliability classification TABLE — parse [`TimeHint`] from constructed [`MediaInfo`]
//!    and assert the source/reliability for each rule (BWF+dates ⇒ medium; creation-only ⇒ low;
//!    implausible disagreement ⇒ debug-only/not-ranked; same-device ⇒ order-only). No files needed —
//!    pure metadata parsing.
//!
//! 2. The §12.3 REQUIRED proof on a REAL imported true pair: the known camera/Zoom ~11.6-YEAR
//!    metadata disagreement CANNOT harm the correct audio match. We import a genuine same-master pair
//!    (full decode → pyramid → fingerprint), then drive the PRODUCTION orchestrator
//!    `match_candidates_fingerprint` (the exact function `Ctx::match_pair` delegates to) over the two
//!    REAL fingerprints with an injected 11.6-year [`TimeHint`], and compare against the SAME call
//!    with NO hint. The 11.6-year hint produces a hypothesis with ~zero audio inliers and loses; the
//!    classification, accepted lag, and refined mapping are UNCHANGED; the bad hint never turns the
//!    result into a wrong Match or suppresses the right one.
//!
//! The orchestrator injects the hint as a closure exactly so this can be tested without authoring a
//! `bext`-chunk WAV (the parse itself is covered by half 1 + the unit tests). It also lets us assert
//! the §12.3 structural rule — metadata never restricts the landmark lookup — by SUBSTITUTION:
//! identical correspondences/index, only the hint differs.
//!
//! Run with `--release` (fixture generation is sinc-interpolation heavy). Requires ffmpeg on PATH.

mod common;

use couple_core::fingerprint::{
    FingerprintFormatV1, FingerprintReader, HintReliability, TimeHint, TimeHintSource,
};
use couple_core::fixture::{generate, FixtureSpec};
use couple_core::matching::{match_candidates_fingerprint, MatchClassification, MatchMethod};
use couple_core::media::{AudioStreamInfo, BwfInfo, MediaInfo};
use couple_core::pairing_params::{MatcherMode, PairingParams};
use couple_core::TrackRole;
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg on PATH")
}

// ===================================================================================================
// Half 1 — §12.2 reliability classification table (pure MediaInfo → TimeHint, no files)
// ===================================================================================================

/// A minimal WAV-ish [`MediaInfo`] carrying the given BWF fields + container creation time + a
/// device tag. Absent pieces are `None`/empty. Only the fields the hint parser reads are populated.
fn media(
    bwf_time_ref: Option<u64>,
    sample_rate: u32,
    bwf_date: Option<&str>,
    bwf_time: Option<&str>,
    creation_time: Option<&str>,
    device: Option<&str>,
) -> MediaInfo {
    let bwf = bwf_time_ref
        .map(|tr| BwfInfo {
            date: bwf_date.map(String::from),
            time: bwf_time.map(String::from),
            time_reference_samples: Some(tr),
            encoded_by: device.map(String::from),
            coding_history: None,
        })
        .or_else(|| {
            // A device-only / date-only BWF without a sample reference is still useful for ordering.
            (bwf_date.is_some() || device.is_some()).then_some(BwfInfo {
                date: bwf_date.map(String::from),
                time: bwf_time.map(String::from),
                time_reference_samples: None,
                encoded_by: device.map(String::from),
                coding_history: None,
            })
        });
    let mut tags = BTreeMap::new();
    if let Some(d) = device {
        tags.insert("encoder".to_string(), d.to_string());
    }
    MediaInfo {
        path: "synthetic.wav".to_string(),
        container: "wav".to_string(),
        duration_s: 60.0,
        video: None,
        audio: vec![AudioStreamInfo {
            codec: "pcm_s16le".to_string(),
            sample_rate,
            channels: 1,
            bits_per_sample: Some(16),
            start_time_s: 0.0,
            duration_s: Some(60.0),
        }],
        creation_time_utc: creation_time.map(String::from),
        bwf,
        tags,
    }
}

#[test]
fn reliability_table_bwf_with_dates_is_medium() {
    // BWF sample reference, matching dates ⇒ Medium (§12.2). A 2.0 s sample-ref difference.
    let a = media(
        Some(96_000),
        48_000,
        Some("2026-06-14"),
        Some("10:00:02"),
        None,
        None,
    );
    let b = media(
        Some(0),
        48_000,
        Some("2026-06-14"),
        Some("10:00:00"),
        None,
        None,
    );
    let h = TimeHint::from_media(&a, &b).expect("BWF sample-ref hint");
    assert_eq!(h.source, TimeHintSource::BwfSampleReference);
    assert_eq!(h.reliability, HintReliability::Medium);
    assert!(h.reliability.can_rank(), "medium ⇒ may seed a hypothesis");
    assert!(
        (h.lag_ms - 2000.0).abs() < 1e-6,
        "A − B = +2.0 s, got {} ms",
        h.lag_ms
    );
}

#[test]
fn reliability_table_creation_only_is_low() {
    // Container creation time only (no BWF) ⇒ Low cross-device (§12.2).
    let a = media(None, 48_000, None, None, Some("2026-06-14T10:00:03Z"), None);
    let b = media(None, 48_000, None, None, Some("2026-06-14T10:00:00Z"), None);
    let h = TimeHint::from_media(&a, &b).expect("creation-time hint");
    assert_eq!(h.source, TimeHintSource::ContainerCreationTime);
    assert_eq!(h.reliability, HintReliability::Low);
    assert!(
        h.reliability.can_rank(),
        "low ⇒ still may seed (it loses if wrong)"
    );
    assert!(
        (h.lag_ms - 3000.0).abs() < 1e-6,
        "A − B = +3.0 s, got {} ms",
        h.lag_ms
    );
}

#[test]
fn reliability_table_implausible_disagreement_is_debug_only() {
    // The camera/Zoom case: dates ~11.6 years apart ⇒ DebugOnly, NOT ranked (§12.2).
    let a = media(
        Some(0),
        48_000,
        Some("2004-01-01"),
        Some("00:00:00"),
        None,
        None,
    );
    let b = media(
        Some(0),
        48_000,
        Some("2015-08-01"),
        Some("12:00:00"),
        None,
        None,
    );
    let h = TimeHint::from_media(&a, &b).expect("a (debug-only) hint is still produced");
    assert_eq!(
        h.reliability,
        HintReliability::DebugOnly,
        "11.6-year gap ⇒ debug-only"
    );
    assert!(!h.reliability.can_rank(), "DebugOnly must NOT be ranked");
    assert!(!h.reliability.can_order(), "DebugOnly must NOT order a tie");
}

#[test]
fn reliability_table_same_device_is_order_only() {
    // Same device tag, BWF sample reference present but dates DIFFER (so the sample-ref ranking path
    // declines), and NO BWF time / creation time (so neither wall-clock ranking path applies) ⇒ the
    // only signal left is the §12.1 #4 same-device ORDERING (the sign of the sample reference).
    let a = media(
        Some(480_000),
        48_000,
        Some("2026-06-14"),
        None,
        None,
        Some("ZOOM H4n"),
    );
    let b = media(
        Some(240_000),
        48_000,
        Some("2026-06-13"),
        None,
        None,
        Some("ZOOM H4n"),
    );
    let h = TimeHint::from_media(&a, &b).expect("same-device order hint");
    assert_eq!(h.source, TimeHintSource::SameDeviceOrdering);
    assert_eq!(h.reliability, HintReliability::OrderOnly);
    assert!(
        !h.reliability.can_rank(),
        "OrderOnly must NOT seed a hypothesis"
    );
    assert!(
        h.reliability.can_order(),
        "OrderOnly MAY order an exact tie"
    );
    // A's sample reference (480k) is later than B's (240k) ⇒ positive order sign.
    assert!(
        h.lag_ms > 0.0,
        "A later ⇒ positive order key, got {}",
        h.lag_ms
    );
}

#[test]
fn reliability_table_no_metadata_no_hint() {
    // The synthetic-fixture reality: WAVs with no BWF / creation time / device ⇒ no hint at all.
    let a = media(None, 48_000, None, None, None, None);
    let b = media(None, 48_000, None, None, None, None);
    assert!(
        TimeHint::from_media(&a, &b).is_none(),
        "no metadata ⇒ no hint (graceful absence)"
    );
}

// ===================================================================================================
// Half 2 — §12.3 REQUIRED: the 11.6-year disagreement cannot harm a REAL true match
// ===================================================================================================

/// Import a genuine same-master true pair and return (ctx, video_id, audio_id). Mirrors the
/// `matching_evidence` helper: clean-ish so both matchers lock, deterministic seed.
fn true_pair(dir: &Path, offset_ms: f64, ppm: f64) -> (Arc<couple_core::Ctx>, u64, u64) {
    let spec = FixtureSpec {
        offset_ms,
        ppm,
        dur_cam_s: 40.0,
        dur_ext_s: 55.0,
        snr_db: Some(25.0),
        agc: false,
        seed: 7,
        ..Default::default()
    };
    generate(dir, &spec, &ffmpeg()).unwrap();
    let ctx = common::test_ctx(&dir.join("cache"));
    let v = common::import(&ctx, &dir.join("camera.wav"), TrackRole::Camera).unwrap();
    let a = common::import(&ctx, &dir.join("external.wav"), TrackRole::External).unwrap();
    (ctx, v, a)
}

/// Run the PRODUCTION orchestrator on the two real fingerprints with an OPTIONAL injected hint, and
/// return the single (1×1) cell's evidence. This is exactly the path `Ctx::match_pair` drives (real
/// correspondences → `consensus_with_hint` → §9.3 mapping → §11 refine), with ONLY the metadata hint
/// substituted — so a difference between two runs is attributable solely to the hint.
fn evidence_with_hint(
    ctx: &Arc<couple_core::Ctx>,
    v: u64,
    a: u64,
    params: &PairingParams,
    hint: Option<TimeHint>,
) -> couple_core::matching::PairEvidence {
    let tv = ctx.track(v).unwrap();
    let ta = ctx.track(a).unwrap();
    let fp_v: Arc<FingerprintReader> = tv.fingerprint_v1().unwrap();
    let fp_a: Arc<FingerprintReader> = ta.fingerprint_v1().unwrap();
    let fmt = FingerprintFormatV1::fixed();
    let dur_v = tv.pyramid.duration_s();
    let dur_a = ta.pyramid.duration_s();
    let videos = [(Some(fp_v.as_ref()), dur_v)];
    let audios = [(Some(fp_a.as_ref()), dur_a)];
    let (mut matrix, _stats) = match_candidates_fingerprint(
        &videos,
        &audios,
        &fmt,
        params,
        true, // shadow (attach legacy for parity, like Ctx::match_pair in shadow)
        |_, _| {
            Some(couple_core::pair::pair_correlate(
                &tv.pyramid,
                &ta.pyramid,
                params,
            ))
        },
        |_, _, lag_ms, drift_ppm, t_min, t_max| {
            couple_core::match_refine::refine_affine(
                &tv.pyramid,
                &ta.pyramid,
                lag_ms,
                drift_ppm,
                t_min,
                t_max,
            )
        },
        |_, _| hint,
        |_| {},
    );
    matrix.remove(0).remove(0)
}

#[test]
fn eleven_point_six_year_disagreement_cannot_harm_true_match() {
    let dir = tempfile::tempdir().unwrap();
    // A genuine same-master pair: B is +1800 ms, +60 ppm relative to A. The CORRECT audio match.
    let (ctx, v, a) = true_pair(dir.path(), 1800.0, 60.0);
    let params = PairingParams {
        matcher_mode: MatcherMode::FingerprintShadow,
        ..Default::default()
    };

    // Baseline: NO metadata hint (the synthetic fixtures have none anyway). The correct audio Match.
    let base = evidence_with_hint(&ctx, v, a, &params, None);
    assert_eq!(base.method, MatchMethod::SpectralLandmarkV1);
    assert_eq!(
        base.classification,
        MatchClassification::Match,
        "the true pair Matches without metadata"
    );
    assert!(
        (base.lag_ms - 1800.0).abs() < 30.0,
        "baseline lag ~+1800 ms, got {}",
        base.lag_ms
    );

    // The 11.6-year clock-misconfig hint: a real camera (1904 QuickTime epoch) vs a 2016 Zoom. Build
    // the EXACT TimeHint our parser yields for that disagreement and FORCE it to the worst case —
    // even pretend it is rankable (Medium) — to prove the STRUCTURE, not just the DebugOnly filter,
    // protects the match. (The parser would actually mark it DebugOnly; that is the belt, this is the
    // suspenders.)
    let eleven_six_years_ms = 11.6 * 365.25 * 24.0 * 3600.0 * 1000.0;
    let bad_hint = TimeHint {
        lag_ms: eleven_six_years_ms,
        tolerance_ms: 1000.0,
        source: TimeHintSource::ContainerCreationTime,
        reliability: HintReliability::Medium, // forced worst case (rankable)
    };
    let with_bad = evidence_with_hint(&ctx, v, a, &params, Some(bad_hint));

    // 1) The classification is UNCHANGED — still the correct Match (not turned wrong, not suppressed).
    assert_eq!(
        with_bad.classification, base.classification,
        "the 11.6-year hint must not change the classification (still {:?})",
        base.classification
    );
    assert_eq!(
        with_bad.classification,
        MatchClassification::Match,
        "still the correct Match"
    );
    // 2) The accepted/refined mapping is UNCHANGED — the bad hint's hypothesis earned no inliers and
    //    lost; the winning lag is still the AUDIO lag, nowhere near 11.6 years.
    assert!(
        (with_bad.lag_ms - base.lag_ms).abs() < 1e-6,
        "accepted lag unchanged by the bad hint"
    );
    assert_eq!(
        with_bad.refined_lag_ms, base.refined_lag_ms,
        "refined lag unchanged"
    );
    assert_eq!(
        with_bad.refined_drift_ppm, base.refined_drift_ppm,
        "refined drift unchanged"
    );
    assert!(
        with_bad.lag_ms.abs() < 60_000.0,
        "winner is the audio lag, not 11.6 years ({} ms)",
        with_bad.lag_ms
    );
    // 3) Byte-identical evidence: the wrong hint perturbs nothing in the serialized result (§12.3 +
    //    §20 no NaN/Inf). The strongest non-veto statement: with vs without are the same bytes.
    let j_base = serde_json::to_string(&base).unwrap();
    let j_bad = serde_json::to_string(&with_bad).unwrap();
    assert_eq!(
        j_base, j_bad,
        "the 11.6-year hint must produce a byte-identical PairEvidence"
    );
    for bad in ["NaN", "Infinity"] {
        assert!(!j_bad.contains(bad), "no {bad} in evidence JSON");
    }
}

#[test]
fn metadata_does_not_restrict_landmark_lookup() {
    // §12.3 / §20: a metadata hint must NEVER restrict the landmark lookup to a time window. We prove
    // this STRUCTURALLY by substitution: the SAME two fingerprints (hence the SAME index + the SAME
    // correspondences — the lookup is identical) produce evidence whose CANDIDATE-CORRESPONDENCE COUNT
    // is unchanged whether a (wrong, far-away) hint is present or not. If the hint gated the lookup,
    // the correspondence count would shrink toward the hint's window; it does not.
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v, a) = true_pair(dir.path(), 1800.0, 60.0);
    let params = PairingParams {
        matcher_mode: MatcherMode::FingerprintShadow,
        ..Default::default()
    };

    let base = evidence_with_hint(&ctx, v, a, &params, None);
    // A wrong, far-away hint that — IF it gated the lookup — would exclude the true correspondences.
    let far_hint = TimeHint {
        lag_ms: 7_200_000.0, // +2 hours away from the true +1.8 s line
        tolerance_ms: 1000.0,
        source: TimeHintSource::BwfSampleReference,
        reliability: HintReliability::Medium,
    };
    let with_far = evidence_with_hint(&ctx, v, a, &params, Some(far_hint));

    // The correspondence set the consensus saw is IDENTICAL (the hint enters only at hypothesis
    // generation, never at index/correspondence construction).
    assert_eq!(
        with_far.candidate_correspondences, base.candidate_correspondences,
        "the landmark lookup/correspondences must be unchanged by a metadata hint (got {} vs {})",
        with_far.candidate_correspondences, base.candidate_correspondences
    );
    // And the match is still correct (the far hint lost), confirming the lookup was not narrowed.
    assert_eq!(with_far.classification, MatchClassification::Match);
    assert!(
        (with_far.lag_ms - base.lag_ms).abs() < 1e-6,
        "accepted lag unchanged"
    );
}
