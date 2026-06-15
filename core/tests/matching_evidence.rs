//! PLAN-SPECTRAL-FINGERPRINT-MATCHING boundary 6 (§5) END-TO-END gates: the method-neutral
//! `Ctx::match_pair` / `Ctx::match_candidates` driven on REAL imported tracks (the full
//! decode → pyramid → fingerprint path), proving the §5.3 orchestration and the §9.3
//! classification on genuine same-master excerpts and unrelated recordings.
//!
//! These are the integration-level proofs (the pure-function mapping/one-index gates live in the
//! `matching` unit tests):
//!
//! - LegacyEnvelope `match_pair` agrees with the legacy `pair_correlate` lag (same matcher, method-
//!   neutral wrapper) — the default path is faithfully wrapped (§5.3);
//! - FingerprintShadow `match_pair` on a TRUE pair ⇒ `SpectralLandmarkV1` `Match` with the correct
//!   lag/drift sign; on an UNRELATED pair ⇒ `NoMatch`; sub-`min_overlap_s` ⇒ `InsufficientOverlap`;
//! - `match_candidates` returns a V×A matrix of evidence over a real batch;
//! - determinism: `match_pair` twice ⇒ byte-identical serialized `PairEvidence`.
//!
//! Run with `--release` (fixture generation is sinc-interpolation heavy, like `pair_gates`).
//! Requires ffmpeg on PATH (the `common` import harness decodes through it).

mod common;

use couple_core::fixture::{generate, FixtureSpec};
use couple_core::matching::{MatchClassification, MatchMethod};
use couple_core::pairing_params::{MatcherMode, PairingParams};
use couple_core::TrackRole;
use std::path::Path;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg on PATH")
}

/// Generate a genuine same-master true pair (camera.wav = A, external.wav = B) with the given
/// offset/ppm/durations, import both, and return (ctx, video_id, audio_id). Lag convention: content
/// at A-time t occurs at B-time u(t) = t·(1+ppm·1e-6) + offset_ms/1000, so a true match has
/// lag_ms ≈ +offset_ms and drift_ppm ≈ +ppm.
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
        // Clean-ish so the legacy matcher also locks (we compare against it); a little noise/AGC
        // keeps it realistic without defeating either matcher.
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

// ---- §5.3 LegacyEnvelope: match_pair wraps pair_correlate (same lag) ----
#[test]
fn legacy_match_pair_agrees_with_pair_correlate() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v, a) = true_pair(dir.path(), 2500.0, 0.0, 30.0, 40.0);
    // Boundary 13 flipped the DEFAULT to FingerprintPrimary; this test exercises the legacy wrapper,
    // so request the LegacyEnvelope developer override explicitly (§16.1).
    let params =
        PairingParams { matcher_mode: MatcherMode::LegacyEnvelope, ..PairingParams::default() };
    assert_eq!(params.matcher_mode, MatcherMode::LegacyEnvelope);

    let ev = ctx.match_pair(v, a, &params).unwrap();
    let corr = ctx.pair_correlate(v, a, &params).unwrap();

    assert_eq!(ev.method, MatchMethod::EnvelopeV1, "legacy mode ⇒ EnvelopeV1");
    // The method-neutral wrapper reports the legacy lag within tolerance of pair_correlate's lag.
    assert!(
        (ev.lag_ms - corr.lag_ms).abs() < 1.0,
        "wrapper lag {} must match pair_correlate lag {}",
        ev.lag_ms,
        corr.lag_ms
    );
    // A clean true pair is a legacy candidate ⇒ Match; the lag is near the truth.
    assert_eq!(ev.classification, MatchClassification::Match);
    assert!((ev.lag_ms - 2500.0).abs() < 20.0, "lag {} near +2500 ms truth", ev.lag_ms);
    // Legacy stage-1 carries no drift and no refined fields (boundary 7).
    assert_eq!(ev.drift_ppm, 0.0);
    assert_eq!(ev.refined_lag_ms, None);
    // No NaN/Infinity in the serialization.
    let json = serde_json::to_string(&ev).unwrap();
    for bad in ["NaN", "Infinity"] {
        assert!(!json.contains(bad), "no {bad} in evidence JSON");
    }
}

// ---- §5.3 FingerprintShadow: TRUE pair ⇒ SpectralLandmarkV1 Match, correct lag/drift sign ----
#[test]
fn fingerprint_shadow_true_pair_is_spectral_match() {
    let dir = tempfile::tempdir().unwrap();
    // A true pair with a positive offset and a small positive drift, so we can check BOTH signs.
    let (ctx, v, a) = true_pair(dir.path(), 1800.0, 60.0, 40.0, 55.0);
    let params =
        PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };

    let ev = ctx.match_pair(v, a, &params).unwrap();
    assert_eq!(ev.method, MatchMethod::SpectralLandmarkV1, "fingerprint mode ⇒ SpectralLandmarkV1");
    assert_eq!(
        ev.classification,
        MatchClassification::Match,
        "a genuine same-master pair must Match (lag {} ppm {} support {})",
        ev.lag_ms,
        ev.drift_ppm,
        ev.support
    );
    // Lag sign + magnitude: B is +1800 ms relative to A.
    assert!(ev.lag_ms > 0.0, "positive offset ⇒ positive lag, got {}", ev.lag_ms);
    assert!((ev.lag_ms - 1800.0).abs() < 30.0, "consensus lag {} near +1800 ms", ev.lag_ms);
    // Drift sign: +60 ppm. The coarse consensus drift is approximate at the 50 ms inlier tolerance
    // (fine drift is boundary 7), so assert the SIGN and a generous magnitude band.
    assert!(ev.drift_ppm > 0.0, "positive drift sign expected, got {}", ev.drift_ppm);
    assert!(ev.drift_ppm.abs() <= params.fingerprint_match.max_abs_drift_ppm, "drift within bound");
    // In shadow mode the legacy correlation is attached for comparison.
    assert!(ev.legacy.is_some(), "shadow mode attaches the legacy correlation");
    // BOUNDARY 7 (§11.2): refined fields ARE now populated for a refined Match (boundary 6 left them
    // None; the dedicated accuracy gates live in tests/match_refine.rs). They are finite (§20).
    assert!(ev.refined_lag_ms.is_some_and(|l| l.is_finite()), "refined lag populated by boundary 7");
    assert!(ev.refined_drift_ppm.is_some_and(|d| d.is_finite()));
    assert!(ev.refined_rms_ms.is_some_and(|r| r.is_finite()));
    // Residuals present for a Match (a fittable line); finite.
    assert!(ev.residual_rms_ms.is_some_and(|r| r.is_finite()));

    // Determinism: a second call serializes byte-identically.
    let ev2 = ctx.match_pair(v, a, &params).unwrap();
    assert_eq!(
        serde_json::to_string(&ev).unwrap(),
        serde_json::to_string(&ev2).unwrap(),
        "match_pair must be deterministic"
    );
}

// ---- §5.3 FingerprintShadow: UNRELATED pair ⇒ NoMatch ----
#[test]
fn fingerprint_shadow_unrelated_pair_is_no_match() {
    let dir = tempfile::tempdir().unwrap();
    // Generate a true pair (so two imports exist), but additionally author an INDEPENDENT external
    // recording with a different seed; the camera vs that independent recording is an unrelated
    // pair sharing no acoustic event.
    let spec = FixtureSpec {
        offset_ms: 1000.0,
        ppm: 0.0,
        dur_cam_s: 30.0,
        dur_ext_s: 40.0,
        snr_db: Some(25.0),
        agc: false,
        seed: 11,
        ..Default::default()
    };
    generate(dir.path(), &spec, &ffmpeg()).unwrap();
    // An independent master (different seed) of comparable length, written as its own WAV via the
    // fixture generator into a sibling dir, then imported into the SAME ctx.
    let dir2 = tempfile::tempdir().unwrap();
    let spec2 = FixtureSpec { seed: 99, dur_cam_s: 30.0, dur_ext_s: 40.0, snr_db: Some(25.0), agc: false, ..Default::default() };
    generate(dir2.path(), &spec2, &ffmpeg()).unwrap();

    let ctx = common::test_ctx(&dir.path().join("cache"));
    let v = common::import(&ctx, &dir.path().join("camera.wav"), TrackRole::Camera).unwrap();
    // The unrelated audio is the INDEPENDENT master's external file.
    let a_unrelated = common::import(&ctx, &dir2.path().join("external.wav"), TrackRole::External).unwrap();

    let params =
        PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
    let ev = ctx.match_pair(v, a_unrelated, &params).unwrap();
    assert_eq!(ev.method, MatchMethod::SpectralLandmarkV1);
    assert_ne!(
        ev.classification,
        MatchClassification::Match,
        "independent recordings must NOT Match (got lag {} support {})",
        ev.lag_ms,
        ev.support
    );
    // Concretely: no fittable affine line over enough support ⇒ NoMatch (the unrelated case in
    // §9.3). It must not be a confident Match; NoMatch or honest Ambiguity are both acceptable, but
    // for clearly independent content we expect NoMatch.
    assert!(
        matches!(ev.classification, MatchClassification::NoMatch | MatchClassification::Ambiguous),
        "unrelated ⇒ NoMatch/Ambiguous, got {:?}",
        ev.classification
    );
}

// ---- §5.3 / §9.3 InsufficientOverlap: sub-floor durations ⇒ InsufficientOverlap, no matcher ----
#[test]
fn sub_floor_overlap_is_insufficient_overlap() {
    let dir = tempfile::tempdir().unwrap();
    // Two short clips (4 s each) — below the 8 s default pairing floor.
    let (ctx, v, a) = true_pair(dir.path(), 500.0, 0.0, 4.0, 4.0);
    let params =
        PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };
    // The default floor is 8 s; 4 s overlap is below it.
    assert_eq!(params.min_overlap_s, 8.0);

    let ev = ctx.match_pair(v, a, &params).unwrap();
    assert_eq!(
        ev.classification,
        MatchClassification::InsufficientOverlap,
        "4 s clips are below the 8 s floor"
    );
    // The reported overlap is finite and below the floor.
    assert!(ev.overlap_s.is_finite() && ev.overlap_s < params.min_overlap_s);
}

// ---- §5.3 match_candidates: a V×A matrix of evidence over a real batch (one index) ----
#[test]
fn match_candidates_returns_matrix_over_batch() {
    let dir = tempfile::tempdir().unwrap();
    // A true pair (v0 ↔ a0). Add an independent external (a1) so the row has a true and a false
    // column. One video, two audios.
    let (ctx, v0, a0) = true_pair(dir.path(), 1200.0, 0.0, 30.0, 40.0);
    let dir2 = tempfile::tempdir().unwrap();
    let spec2 = FixtureSpec { seed: 55, dur_cam_s: 30.0, dur_ext_s: 40.0, snr_db: Some(25.0), agc: false, ..Default::default() };
    generate(dir2.path(), &spec2, &ffmpeg()).unwrap();
    let a1 = common::import(&ctx, &dir2.path().join("external.wav"), TrackRole::External).unwrap();

    let params =
        PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };

    let matrix = ctx.match_candidates(&[v0], &[a0, a1], &params).unwrap();
    assert_eq!(matrix.len(), 1, "one video row");
    assert_eq!(matrix[0].len(), 2, "two audio columns");
    // The true column Matches; the independent column does not.
    assert_eq!(matrix[0][0].classification, MatchClassification::Match, "v0↔a0 true pair");
    assert_eq!(matrix[0][0].method, MatchMethod::SpectralLandmarkV1);
    assert_ne!(matrix[0][1].classification, MatchClassification::Match, "v0↔a1 unrelated");

    // Cross-check: match_pair on the same true cell agrees with the matrix cell (byte-identical).
    let single = ctx.match_pair(v0, a0, &params).unwrap();
    assert_eq!(
        serde_json::to_string(&single).unwrap(),
        serde_json::to_string(&matrix[0][0]).unwrap(),
        "match_pair and the match_candidates cell must agree"
    );

    // The whole batch also works in LegacyEnvelope mode (the method-neutral legacy matrix). Boundary
    // 13 made FingerprintPrimary the default, so request the legacy override explicitly (§16.1).
    let legacy_params =
        PairingParams { matcher_mode: MatcherMode::LegacyEnvelope, ..PairingParams::default() };
    let legacy_matrix = ctx.match_candidates(&[v0], &[a0, a1], &legacy_params).unwrap();
    assert_eq!(legacy_matrix[0][0].method, MatchMethod::EnvelopeV1);
    assert_eq!(legacy_matrix[0][0].classification, MatchClassification::Match, "legacy true pair");
}

// ---- §13.4 match_candidates_graph: graph-adjusted evidence over a real batch (OBSERVATIONAL) ----
// The graph pass runs over the FULL track graph (every track vs every other). On a batch of one true
// pair + one unrelated recording there is no corroborating two-edge cycle, so graph consistency
// neither promotes nor demotes — the graph-adjusted matrix is BYTE-IDENTICAL to match_candidates,
// proving the §13.4 pass is safe and observational (the default resolver is untouched). It is also
// deterministic and carries no NaN/Infinity.
#[test]
fn match_candidates_graph_is_observational_on_real_batch() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, v0, a0) = true_pair(dir.path(), 1500.0, 0.0, 30.0, 40.0);
    let dir2 = tempfile::tempdir().unwrap();
    let spec2 = FixtureSpec {
        seed: 73,
        dur_cam_s: 30.0,
        dur_ext_s: 40.0,
        snr_db: Some(25.0),
        agc: false,
        ..Default::default()
    };
    generate(dir2.path(), &spec2, &ffmpeg()).unwrap();
    let a1 = common::import(&ctx, &dir2.path().join("external.wav"), TrackRole::External).unwrap();

    let params =
        PairingParams { matcher_mode: MatcherMode::FingerprintShadow, ..Default::default() };

    let plain = ctx.match_candidates(&[v0], &[a0, a1], &params).unwrap();
    let graph = ctx.match_candidates_graph(&[v0], &[a0, a1], &params).unwrap();

    // Same shape.
    assert_eq!(graph.len(), plain.len());
    assert!(graph.iter().zip(&plain).all(|(g, p)| g.len() == p.len()));

    // No corroborating triangle exists ⇒ graph adjustment changes NOTHING: byte-identical cells.
    for (grow, prow) in graph.iter().zip(&plain) {
        for (gcell, pcell) in grow.iter().zip(prow) {
            let gj = serde_json::to_string(gcell).unwrap();
            let pj = serde_json::to_string(pcell).unwrap();
            assert_eq!(gj, pj, "graph-adjusted cell must equal the plain cell (observational)");
            for bad in ["NaN", "Infinity"] {
                assert!(!gj.contains(bad), "no {bad} in graph-adjusted evidence");
            }
        }
    }
    // The true pair is still a Match through the graph surface; the unrelated column is not.
    assert_eq!(graph[0][0].classification, MatchClassification::Match, "v0↔a0 true pair");
    assert_ne!(graph[0][1].classification, MatchClassification::Match, "v0↔a1 unrelated");

    // Determinism: a second graph call is byte-identical.
    let graph2 = ctx.match_candidates_graph(&[v0], &[a0, a1], &params).unwrap();
    for (r1, r2) in graph.iter().zip(&graph2) {
        for (c1, c2) in r1.iter().zip(r2) {
            assert_eq!(
                serde_json::to_string(c1).unwrap(),
                serde_json::to_string(c2).unwrap(),
                "match_candidates_graph must be deterministic"
            );
        }
    }

    // LegacyEnvelope: the graph pass is a no-op ⇒ equals the plain legacy matrix. Boundary 13 made
    // FingerprintPrimary the default, so request the legacy override explicitly (§16.1).
    let legacy_params =
        PairingParams { matcher_mode: MatcherMode::LegacyEnvelope, ..PairingParams::default() };
    let legacy_plain = ctx.match_candidates(&[v0], &[a0, a1], &legacy_params).unwrap();
    let legacy_graph =
        ctx.match_candidates_graph(&[v0], &[a0, a1], &legacy_params).unwrap();
    for (gr, pr) in legacy_graph.iter().zip(&legacy_plain) {
        for (gc, pc) in gr.iter().zip(pr) {
            assert_eq!(
                serde_json::to_string(gc).unwrap(),
                serde_json::to_string(pc).unwrap(),
                "legacy graph pass is a no-op (equals plain legacy matrix)"
            );
        }
    }
}

// ---- §15 boundary 12: the FINGERPRINT corpus metrics are byte-reproducible (cold cache) ----
// The §15.2 "deterministic output across repeated runs" gate AND the boundary-12 byte-repro
// requirement, exercised on the actual fingerprint scoring path (`score_manifest_with` with the
// frozen profile_v1). Two runs over the SAME one-case synthetic manifest — each regenerating its
// media + rebuilding the fingerprint from scratch into a FRESH temp work dir / cache (cold) — must
// serialize the §10.4 `CorpusMetrics` byte-for-byte. This is the automated cold-cache analogue of
// the CLI two-run byte diff. Run with `--release`.
#[test]
fn fingerprint_corpus_metrics_are_byte_reproducible() {
    use couple_core::fingerprint::FingerprintMatchParams;
    use couple_core::matcher_corpus::{score_manifest_with, CorpusManifest, Matcher, CORPUS_SCHEMA};

    // A tiny one-case synthetic manifest: a clean 12 s true pair (reaches the matcher, a confident
    // Match under profile_v1). The synthetic `gen` block regenerates the media deterministically.
    let manifest: CorpusManifest = serde_json::from_value(serde_json::json!({
        "schema": CORPUS_SCHEMA,
        "split": "synthetic",
        "mediaRoot": "",
        "cases": [{
            "id": "det_clean_12s",
            "group": "synthetic",
            "a": "det_clean_12s/camera.wav",
            "b": "det_clean_12s/external.wav",
            "sameEvent": true,
            "expectedOffsetMs": 1500.0,
            "expectedDriftPpm": 0.0,
            "minimumOverlapS": 12.0,
            "tags": ["clean"],
            "allowedForTuning": true,
            "gen": {
                "offsetMs": 1500.0, "ppm": 0.0, "durCamS": 12.0, "durExtS": 18.0,
                "camSeedOffset": 0, "snrDb": null, "gainDb": 0.0, "agc": false, "seed": 4242
            }
        }]
    }))
    .unwrap();

    // The frozen calibrated profile under FingerprintPrimary (valid: profile version 1).
    let params = PairingParams {
        fingerprint_match: FingerprintMatchParams::profile_v1(),
        ..PairingParams::default()
    };
    assert_eq!(params.fingerprint_match.threshold_profile_version, 1);

    let run = || {
        let dir = tempfile::tempdir().unwrap();
        let ctx = common::test_ctx(&dir.path().join("cache"));
        let work = tempfile::tempdir().unwrap();
        let (m, _runtime) = score_manifest_with(
            &ctx,
            &manifest,
            dir.path(),
            work.path(),
            &params,
            Matcher::FingerprintPrimary,
            8.5,
        )
        .unwrap();
        serde_json::to_string(&m).unwrap()
    };

    let j1 = run();
    let j2 = run();
    assert_eq!(j1, j2, "fingerprint corpus metrics must be byte-identical across cold runs");
    // The clean 12 s pair Matches under profile_v1 (a sanity check that we measured a real verdict,
    // not an empty manifest), and no NaN/Infinity reaches the JSON (§20).
    assert!(j1.contains("\"truePositives\":1"), "the clean 12 s true pair Matches: {j1}");
    for bad in ["NaN", "Infinity"] {
        assert!(!j1.contains(bad), "no {bad} in metrics JSON");
    }
}
