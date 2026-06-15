//! PLAN-SPECTRAL-FINGERPRINT-MATCHING boundary 7 (§11) END-TO-END gates on REAL imported tracks
//! (the full decode → pyramid → fingerprint path):
//!
//! - §11.1/§11.2: a `SpectralLandmarkV1` `Match` on a true synthetic pair gets `refined_*`
//!   populated; the refined lag/drift SIGN is correct; the refinement reuses the EXISTING local
//!   aligner (`measure_local`) and does NOT rerun the global envelope scan (proven by the §11.3
//!   stage-1 scan counter NOT advancing across the refinement);
//! - §11.4 accuracy: refined offset error ≤ 1 ms for overlap ≥ 20 s, ≤ 3 ms for an 8–20 s clip;
//! - determinism: the refined `PairEvidence` is byte-identical across two runs; no NaN/Infinity;
//! - §11.3 full-sync seed: a seeded full sync produces the same final offset/drift as the unseeded
//!   run, WITHOUT rerunning stage 1 (the stage-1 counter stays 0 when seeded); and — crucially —
//!   with NO seed the `auto_sync_tracks` result is byte-identical to `auto_sync_tracks_seeded(None)`.
//!
//! Run with `--release` (fixture generation is sinc-interpolation heavy). Requires ffmpeg on PATH.

mod common;

use couple_core::fixture::{generate, FixtureSpec};
use couple_core::job::{noop_progress, CancelToken, ProgressEmitter};
use couple_core::matching::{MatchClassification, MatchMethod};
use couple_core::pairing_params::{MatcherMode, PairingParams};
use couple_core::sync::{
    auto_sync_tracks, auto_sync_tracks_seeded, reset_stage1_scan_count, stage1_scan_count,
    SyncParams, SyncSeed, SyncStatus,
};
use couple_core::TrackRole;
use std::path::Path;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg on PATH")
}

/// Generate a genuine same-master true pair (camera.wav = A, external.wav = B), import both, and
/// return (ctx, video_id, audio_id). Lag convention: A-time t occurs at B-time
/// u(t) = t·(1+ppm·1e-6) + offset_ms/1000, so a true match has lag_ms ≈ +offset_ms and drift ≈ +ppm.
fn true_pair(
    dir: &Path,
    offset_ms: f64,
    ppm: f64,
    dur_cam_s: f64,
    dur_ext_s: f64,
) -> (std::sync::Arc<couple_core::Ctx>, u64, u64) {
    let spec = FixtureSpec {
        offset_ms,
        ppm,
        dur_cam_s,
        dur_ext_s,
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

fn no_nan(ev: &couple_core::PairEvidence) {
    let json = serde_json::to_string(ev).unwrap();
    for bad in ["NaN", "Infinity", "inf", "nan"] {
        assert!(!json.contains(bad), "evidence JSON must not contain {bad:?}: {json}");
    }
}

// ---- §11.1 / §11.2 / §11.4: a long (≥20 s overlap) true pair refines to ≤ 1 ms ----
#[test]
fn refined_long_pair_within_1ms_and_signs_correct() {
    let dir = tempfile::tempdir().unwrap();
    // +1800 ms offset, +60 ppm drift, 40 s / 55 s ⇒ overlap ≈ 40 s ≥ 20 s.
    let (ctx, v, a) = true_pair(dir.path(), 1800.0, 60.0, 40.0, 55.0);
    let params = PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };

    let ev = ctx.match_pair(v, a, &params).unwrap();
    assert_eq!(ev.method, MatchMethod::SpectralLandmarkV1);
    assert_eq!(
        ev.classification,
        MatchClassification::Match,
        "a genuine long pair must Match (lag {} support {})",
        ev.lag_ms,
        ev.support
    );

    // §11.2: refined_* are now POPULATED (boundary 6 left them None).
    let refined_lag = ev.refined_lag_ms.expect("refined_lag_ms populated for a refined Match");
    let refined_drift = ev.refined_drift_ppm.expect("refined_drift_ppm populated");
    let refined_rms = ev.refined_rms_ms.expect("refined_rms_ms populated");
    assert!(refined_lag.is_finite() && refined_drift.is_finite() && refined_rms.is_finite());
    no_nan(&ev);

    // Signs: positive offset ⇒ positive refined lag; positive drift ⇒ positive refined drift.
    assert!(refined_lag > 0.0, "positive offset ⇒ positive refined lag, got {refined_lag}");
    assert!(refined_drift > 0.0, "positive drift sign expected, got {refined_drift}");

    // §11.4 accuracy: refined offset error ≤ 1 ms at this offset's anchor (the intercept is near
    // t=0, and the refined line is the fit; compare the refined lag at the overlap MIDPOINT against
    // the ground-truth mapping there to be drift-fair).
    // Ground-truth lag at A-time t is u(t) − t = offset_ms/1000·1000 + (ppm·1e-6)·t·1000 ms.
    // At t = 0 the truth is exactly +1800 ms; the refined intercept should match within 1 ms.
    assert!(
        (refined_lag - 1800.0).abs() <= 1.0,
        "refined lag {refined_lag} must be within 1 ms of +1800 ms truth (≥20 s overlap §11.4)"
    );
    // Refined drift is far more accurate than the coarse consensus drift; require the sign and a
    // tight-ish band (the local-lock fit resolves drift to a few ppm on a clean 40 s pair).
    assert!((refined_drift - 60.0).abs() <= 40.0, "refined drift {refined_drift} near +60 ppm");

    // The refined mapping is at least as accurate as the coarse consensus lag it replaces.
    assert!(
        (refined_lag - 1800.0).abs() <= (ev.lag_ms - 1800.0).abs() + 1e-9,
        "refined lag {refined_lag} no worse than coarse consensus lag {}",
        ev.lag_ms
    );
}

// ---- §11.4: an 8–20 s clip refines to ≤ 3 ms ----
#[test]
fn refined_short_clip_within_3ms() {
    let dir = tempfile::tempdir().unwrap();
    // A ~12 s camera clip against a longer external ⇒ overlap ≈ 12 s (in the 8–20 s band).
    let (ctx, v, a) = true_pair(dir.path(), 900.0, 0.0, 12.0, 30.0);
    let params = PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };

    let ev = ctx.match_pair(v, a, &params).unwrap();
    assert_eq!(ev.method, MatchMethod::SpectralLandmarkV1);
    // The clip must reach the matcher and be a Match (or, if landmark support is thin on a short
    // clip, at least carry refined evidence) — for this clean pair we expect a confident Match.
    assert_eq!(
        ev.classification,
        MatchClassification::Match,
        "a clean 12 s clip must Match (lag {} support {})",
        ev.lag_ms,
        ev.support
    );
    let refined_lag = ev.refined_lag_ms.expect("short-clip Match is refined");
    no_nan(&ev);
    assert!(
        (refined_lag - 900.0).abs() <= 3.0,
        "refined lag {refined_lag} must be within 3 ms of +900 ms truth (8–20 s clip §11.4)"
    );
}

// ---- structural: the refinement reuses measure_local and does NOT rerun the global envelope scan
// ---- (proven by the stage-1 GLOBAL-SCAN counter NOT advancing across the fingerprint match_pair) ----
#[test]
fn refinement_does_not_run_global_scan() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v, a) = true_pair(dir.path(), 1500.0, 0.0, 30.0, 40.0);
    let params = PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };

    // A fingerprint match_pair that REFINES a Match must use the LOCAL aligner (measure_local), not
    // the global envelope scan (offset_search → stage1_scan). The stage-1 counter is the structural
    // proof: it stays 0 across the whole fingerprint match_pair + refinement.
    reset_stage1_scan_count();
    let ev = ctx.match_pair(v, a, &params).unwrap();
    assert_eq!(ev.classification, MatchClassification::Match);
    assert!(ev.refined_lag_ms.is_some(), "the Match was refined");
    assert_eq!(
        stage1_scan_count(),
        0,
        "§11.1/§20: refinement must reuse measure_local, NOT rerun the global stage-1 envelope scan"
    );

    // Contrast: a FULL sync (auto_sync_tracks) DOES run the global scan — so the counter is the real
    // discriminator, not always-zero. (Proves the assertion above is meaningful.)
    let cam = ctx.track(v).unwrap();
    let ext = ctx.track(a).unwrap();
    reset_stage1_scan_count();
    let mut prog = ProgressEmitter::new(0, noop_progress());
    let _ = auto_sync_tracks(&cam.pyramid, &ext.pyramid, &SyncParams::default(), &mut prog, &CancelToken::new()).unwrap();
    assert!(stage1_scan_count() >= 1, "an unseeded full sync DOES run the global stage-1 scan");
}

// ---- determinism: a refined PairEvidence is byte-identical across two runs ----
#[test]
fn refined_evidence_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v, a) = true_pair(dir.path(), 1200.0, 30.0, 35.0, 45.0);
    let params = PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
    let ev1 = ctx.match_pair(v, a, &params).unwrap();
    let ev2 = ctx.match_pair(v, a, &params).unwrap();
    assert!(ev1.refined_lag_ms.is_some(), "the pair refined");
    assert_eq!(
        serde_json::to_string(&ev1).unwrap(),
        serde_json::to_string(&ev2).unwrap(),
        "refined PairEvidence must be byte-identical across runs"
    );
    // And the in-memory refined fields (which serde(skip) would hide) match bit-for-bit too.
    assert_eq!(ev1.refined_lag_ms.unwrap().to_bits(), ev2.refined_lag_ms.unwrap().to_bits());
    assert_eq!(ev1.refined_drift_ppm.unwrap().to_bits(), ev2.refined_drift_ppm.unwrap().to_bits());
    assert_eq!(ev1.refine_reason, ev2.refine_reason);
}

// ===================================================================================================
// §11.3 — full-sync seed
// ===================================================================================================

// With NO seed, auto_sync_tracks_seeded delegates to auto_sync_tracks ⇒ the result is BYTE-IDENTICAL
// (every field except the wall-clock compute_ms, which is non-deterministic by nature and not part
// of any byte gate). This is the §11.3 "absent ⇒ existing behavior byte-identical" contract.
#[test]
fn no_seed_is_byte_identical_to_auto_sync() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v, a) = true_pair(dir.path(), 1500.0, 40.0, 40.0, 55.0);
    let cam = ctx.track(v).unwrap();
    let ext = ctx.track(a).unwrap();
    let params = SyncParams::default();

    let mut p1 = ProgressEmitter::new(0, noop_progress());
    let unseeded = auto_sync_tracks(&cam.pyramid, &ext.pyramid, &params, &mut p1, &CancelToken::new()).unwrap();
    let mut p2 = ProgressEmitter::new(0, noop_progress());
    let none_seed =
        auto_sync_tracks_seeded(&cam.pyramid, &ext.pyramid, &params, None, &mut p2, &CancelToken::new()).unwrap();

    // Compare every field except compute_ms by serializing with compute_ms zeroed.
    let strip = |mut r: couple_core::sync::SyncResult| {
        r.compute_ms = 0.0;
        serde_json::to_string(&r).unwrap()
    };
    assert_eq!(
        strip(unseeded),
        strip(none_seed),
        "auto_sync_tracks_seeded(None) must be byte-identical to auto_sync_tracks (§11.3)"
    );
}

// A SEEDED full sync produces the same final offset/drift/status as the unseeded run (within
// tolerance) but WITHOUT rerunning stage 1 — proven by the stage-1 GLOBAL-SCAN counter: 0 increments
// when seeded, ≥ 1 when not.
#[test]
fn seeded_sync_matches_unseeded_without_stage1() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v, a) = true_pair(dir.path(), 2000.0, 80.0, 45.0, 60.0);
    let cam = ctx.track(v).unwrap();
    let ext = ctx.track(a).unwrap();
    let params = SyncParams::default();

    // Unseeded reference: runs the global scan (counter advances).
    reset_stage1_scan_count();
    let mut p1 = ProgressEmitter::new(0, noop_progress());
    let unseeded = auto_sync_tracks(&cam.pyramid, &ext.pyramid, &params, &mut p1, &CancelToken::new()).unwrap();
    let unseeded_scans = stage1_scan_count();
    assert!(unseeded_scans >= 1, "the unseeded path runs the stage-1 global scan");

    // Seed from the (accurate) ground-truth-ish offset/drift the unseeded run found — exactly what an
    // accepted fingerprint PairEvidence would carry. The seed is intentionally a few ms off the
    // unseeded offset to prove the refinement ladder still converges from the seed.
    let seed = SyncSeed { offset_ms: unseeded.offset_ms + 5.0, drift_ppm: unseeded.drift_ppm };
    reset_stage1_scan_count();
    let mut p2 = ProgressEmitter::new(0, noop_progress());
    let seeded =
        auto_sync_tracks_seeded(&cam.pyramid, &ext.pyramid, &params, Some(seed), &mut p2, &CancelToken::new())
            .unwrap();
    let seeded_scans = stage1_scan_count();

    // §11.3 PROOF: the seeded path did NOT run the global stage-1 scan.
    assert_eq!(seeded_scans, 0, "a seeded full sync must NOT rerun stage 1 (§11.3)");

    // The seeded result lands on the same final mapping (the §5.5 drift pass + §5.3 ladder converge
    // from the seed to the same offset/drift within sub-ms / few-ppm tolerance).
    assert!(
        (seeded.offset_ms - unseeded.offset_ms).abs() <= 1.0,
        "seeded offset {} vs unseeded {} within 1 ms",
        seeded.offset_ms,
        unseeded.offset_ms
    );
    assert!(
        (seeded.drift_ppm - unseeded.drift_ppm).abs() <= 5.0,
        "seeded drift {} vs unseeded {} within 5 ppm",
        seeded.drift_ppm,
        unseeded.drift_ppm
    );
    // The status verdict matches (both Synced for this clean pair).
    assert!(
        matches!(seeded.status, SyncStatus::Synced) == matches!(unseeded.status, SyncStatus::Synced),
        "seeded and unseeded reach the same Synced verdict"
    );
}
