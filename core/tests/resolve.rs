//! `couple_core::resolve` parity gates (PLAN-CORE-SHELL-BOUNDARY Phase A).
//!
//! These reproduce the SPEC-V2 §7 gate scenarios from `cli/tests/pair_gates.rs` at the
//! `Ctx::resolve` level — the SAME fixed-seed sessions, imported into a `Ctx`, resolved via
//! `ctx.resolve(...)`, asserting the SAME row states / selected audio / lags / clusters /
//! overflow the CLI gates assert. The CLI is a thin shell over this resolver (Phase B), so
//! the row contract proven here is the cross-language contract.
//!
//! The fixture corpus is byte-identical to the gates (`generate_session` with the same seeds
//! and layouts), so a divergence here would be a relocation defect, not a fixture drift.
//!
//! Run with `--release` (fixture generation is sinc-interpolation heavy). Requires ffmpeg on
//! PATH (the import path decodes through it).

mod common;

use couple_core::fixture::{generate_session, SessionCam, SessionExt, SessionSpec};
use couple_core::resolve::{MatcherChoice, ResolveParams, ResolvedRow, RowState};
use couple_core::{Ctx, TrackRole};
use std::path::Path;
use std::sync::Arc;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg on PATH")
}

fn cam(name: &str, t0: f64, dur: f64) -> SessionCam {
    SessionCam { name: name.into(), master_t0: t0, dur_s: dur, ..Default::default() }
}

fn ext(name: &str, t0: f64, dur: f64) -> SessionExt {
    SessionExt { name: name.into(), master_t0: t0, dur_s: dur, ..Default::default() }
}

/// Import a generated session's videos (cameras) and audios (externals) into a fresh Ctx, in
/// NAME-SORTED order (the deterministic order the CLI's directory scan produces). Audio-less
/// (stripped) cameras are skipped — the IMPORTED-TRACK-ONLY `resolve` contract excludes them
/// (a shell wraps those as inert no-match rows), exactly like `run_pair`'s `importable` set.
/// Returns (ctx, video_ids, audio_ids) where ids align to the name-sorted video/audio names.
fn import_session(dir: &Path) -> (Arc<Ctx>, Vec<u64>, Vec<u64>) {
    let cache = dir.join("cache");
    let ctx = common::test_ctx(&cache);

    // Deterministic listing: plain files, sorted by name, dotfiles ignored (mirrors run_pair).
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .filter(|e| e.path().is_file())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| !n.starts_with('.'))
        .collect();
    names.sort();

    let mut video_ids = Vec::new();
    let mut audio_ids = Vec::new();
    for n in &names {
        let path = dir.join(n);
        // Classify by probe like run_pair: a video stream ⇒ camera; audio-only ⇒ external.
        match ctx.probe_lenient(&path) {
            Ok(info) if info.video.is_some() => {
                // Audio-less videos are excluded from the imported-track resolve set.
                if info.audio.is_empty() {
                    continue;
                }
                let id = common::import(&ctx, &path, TrackRole::Camera).unwrap();
                video_ids.push(id);
            }
            Ok(info) if !info.audio.is_empty() => {
                let id = common::import(&ctx, &path, TrackRole::External).unwrap();
                audio_ids.push(id);
            }
            _ => {} // skipped litter
        }
    }
    (ctx, video_ids, audio_ids)
}

/// The fingerprint default ResolveParams.
fn fp_params() -> ResolveParams {
    ResolveParams::default()
}

/// The `--matcher legacy` developer-override params (the CLI legacy resolver never escalates
/// RMS, so `rms_escalation` stays false here).
fn legacy_params() -> ResolveParams {
    ResolveParams { matcher: MatcherChoice::Legacy, ..ResolveParams::default() }
}

fn row<'a>(rows: &'a [ResolvedRow], name: &str) -> &'a ResolvedRow {
    rows.iter()
        .find(|r| r.video_name == name)
        .unwrap_or_else(|| panic!("no row for {name}"))
}

fn selected_audio(r: &ResolvedRow) -> Option<&str> {
    r.selected.as_ref().map(|s| s.audio_name.as_str())
}

fn alternates(r: &ResolvedRow) -> Vec<&str> {
    r.alternates.iter().map(|a| a.audio_name.as_str()).collect()
}

const LAG_TOL_MS: f64 = 2.0;

fn assert_lag(r: &ResolvedRow, expect_ms: f64) {
    let lag = r.selected.as_ref().expect("selected").lag_ms;
    assert!(
        (lag - expect_ms).abs() <= LAG_TOL_MS,
        "{}: lag {lag:.3} ms, expected {expect_ms:.3} ± {LAG_TOL_MS}",
        r.video_name
    );
}

/// Audio names no resolved row selects — the §3.6 unused tray.
fn unused_audio<'a>(rows: &[ResolvedRow], audio_names: &'a [&str]) -> Vec<&'a str> {
    let used: std::collections::HashSet<&str> =
        rows.iter().filter_map(selected_audio).collect();
    audio_names.iter().copied().filter(|n| !used.contains(n)).collect()
}

// ============================ gate 1 — continuous recorder ============================

#[test]
fn gate1_continuous_recorder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 11,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 150.0, 40.0), cam("video3", 270.0, 40.0)],
        externals: vec![ext("external", 0.0, 330.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    let rows = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    assert_eq!(rows.len(), 3);
    for (v, t0) in [("video1.mp4", 30.0), ("video2.mp4", 150.0), ("video3.mp4", 270.0)] {
        let r = row(&rows, v);
        assert_eq!(r.state, RowState::Confident, "{v}: {r:?}");
        assert_eq!(selected_audio(r), Some("external.wav"));
        assert!(r.selected.as_ref().unwrap().coverage.fraction >= 0.98);
        assert_lag(r, t0 * 1000.0);
    }
    assert!(unused_audio(&rows, &["external.wav"]).is_empty());
}

// ============================ gate 2 — recorder restart ============================

#[test]
fn gate2_recorder_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 22,
        cameras: vec![cam("video1", 20.0, 80.0), cam("video2", 120.0, 90.0), cam("video3", 240.0, 80.0)],
        externals: vec![ext("ext1", 0.0, 150.0), ext("ext2", 150.0, 330.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);

    // Fingerprint DEFAULT: full-coverage true pairs Match; video2 selects its dominant true
    // source ext2 (the ⅓-coverage ext1↔video2 partial is honestly suppressed).
    let r = ctx.resolve(&vids, &auds, &fp_params()).unwrap();
    let r1 = row(&r, "video1.mp4");
    assert_eq!(r1.state, RowState::Confident, "{r1:?}");
    assert_eq!(selected_audio(r1), Some("ext1.wav"));
    assert_lag(r1, 20.0 * 1000.0);
    let r3 = row(&r, "video3.mp4");
    assert_eq!(r3.state, RowState::Confident, "{r3:?}");
    assert_eq!(selected_audio(r3), Some("ext2.wav"));
    assert_lag(r3, (240.0 - 150.0) * 1000.0);
    let r2 = row(&r, "video2.mp4");
    assert!(
        r2.state == RowState::Confident || r2.state == RowState::Partial,
        "video2 fingerprint state: {r2:?}"
    );
    assert_eq!(selected_audio(r2), Some("ext2.wav"), "video2 selects ext2: {r2:?}");
    assert_lag(r2, (120.0 - 150.0) * 1000.0);

    // `--matcher legacy`: video2 is `split` with disjoint clusters, ext2 (max coverage)
    // selected, ext1 the alternate — the recorder-restart CLUSTERING policy.
    let rl = ctx.resolve(&vids, &auds, &legacy_params()).unwrap();
    let rl1 = row(&rl, "video1.mp4");
    assert_eq!(rl1.state, RowState::Confident, "{rl1:?}");
    assert_eq!(selected_audio(rl1), Some("ext1.wav"));
    let rl3 = row(&rl, "video3.mp4");
    assert_eq!(rl3.state, RowState::Confident, "{rl3:?}");
    assert_eq!(selected_audio(rl3), Some("ext2.wav"));
    let rl2 = row(&rl, "video2.mp4");
    assert_eq!(rl2.state, RowState::Split, "{rl2:?}");
    assert_eq!(selected_audio(rl2), Some("ext2.wav"));
    assert_eq!(alternates(rl2), vec!["ext1.wav"]);
    assert_lag(rl2, (120.0 - 150.0) * 1000.0);
    let cov = &rl2.selected.as_ref().unwrap().coverage;
    assert!((cov.t0 - 30.0).abs() < 0.5, "{cov:?}");
    assert!((cov.t1 - 90.0).abs() < 0.5, "{cov:?}");
    assert_eq!(rl2.resolution.clusters, 2);
}

// ============================ gate 3 — simultaneous recorders ============================

#[test]
fn gate3_simultaneous_recorders() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 33,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![
            ext("extA", 0.0, 120.0),
            SessionExt {
                name: "extB".into(),
                master_t0: 0.0,
                dur_s: 120.0,
                gain_db: -6.0,
                snr_db: Some(12.0),
                agc: true,
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    let rows = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    let r = row(&rows, "video.mp4");
    assert_eq!(r.state, RowState::TwoSources, "{r:?}");
    let sel = selected_audio(r).unwrap();
    let alts = alternates(r);
    assert_eq!(alts.len(), 1);
    assert_ne!(sel, alts[0]);
    // Best z (spectral score) selected.
    let sel_z = r.selected.as_ref().unwrap().z;
    let alt_z = r.alternates[0].z;
    assert!(sel_z >= alt_z, "selected z {sel_z} < alternate z {alt_z}");
    // Triangle closure (§3.4): the affine composed-map check closed.
    let tri = &r.resolution.triangles[0];
    assert!(tri.simultaneous, "{tri:?}");
    assert_lag(r, 30.0 * 1000.0);
}

// ============================ gate 4 — unrelated audio ============================

#[test]
fn gate4_unrelated_audio() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 44,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 120.0, 40.0)],
        externals: vec![
            ext("ext_true", 0.0, 80.0),
            SessionExt {
                name: "unrelated".into(),
                master_t0: 0.0,
                dur_s: 160.0,
                master_seed: Some(999),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    let rows = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    let r1 = row(&rows, "video1.mp4");
    assert_eq!(r1.state, RowState::Confident, "{r1:?}");
    assert_eq!(selected_audio(r1), Some("ext_true.wav"));

    let r2 = row(&rows, "video2.mp4");
    assert_eq!(r2.state, RowState::NoMatch, "{r2:?}");
    assert!(r2.selected.is_none());

    assert_eq!(unused_audio(&rows, &["ext_true.wav", "unrelated.wav"]), vec!["unrelated.wav"]);
}

// ============================ gate 5 — mixed bag ============================

#[test]
fn gate5_mixed_bag() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 55,
        cameras: vec![
            cam("video", 10.0, 40.0),
            SessionCam {
                name: "muted".into(),
                master_t0: 0.0,
                dur_s: 30.0,
                strip_audio: true,
                ..Default::default()
            },
        ],
        externals: vec![ext("external", 0.0, 60.0)],
        junk_files: vec!["notes.txt".into()],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    // The stripped `muted.mp4` is excluded by import_session (imported-track-only contract):
    // a shell wraps it as an inert no-match row, so core resolves only the analyzable `video`.
    let rows = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    assert_eq!(rows.len(), 1, "only the analyzable video is an imported-track row");
    let ok = row(&rows, "video.mp4");
    assert_eq!(ok.state, RowState::Confident, "{ok:?}");
    assert_eq!(selected_audio(ok), Some("external.wav"));
}

// ============================ gate 6 — stability ============================

#[test]
fn gate6_stability() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 66,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 100.0, 40.0)],
        externals: vec![ext("external", 0.0, 150.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    let r1 = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    // Drop in one new (unrelated) audio file.
    let extra_dir = tmp.path().join("extra");
    generate_session(
        &extra_dir,
        &SessionSpec {
            seed: 66,
            externals: vec![SessionExt {
                name: "newfile".into(),
                master_t0: 0.0,
                dur_s: 60.0,
                master_seed: Some(777),
                ..Default::default()
            }],
            ..Default::default()
        },
        &ffmpeg(),
    )
    .unwrap();
    std::fs::copy(extra_dir.join("newfile.wav"), dir.join("newfile.wav")).unwrap();
    let (ctx2, vids2, auds2) = import_session(&dir);
    let r2 = ctx2.resolve(&vids2, &auds2, &fp_params()).unwrap();

    for v in ["video1.mp4", "video2.mp4"] {
        let a = serde_json::to_string(row(&r1, v)).unwrap();
        let b = serde_json::to_string(row(&r2, v)).unwrap();
        assert_eq!(a, b, "row {v} changed after an unrelated file was added");
    }
    assert_eq!(unused_audio(&r2, &["external.wav", "newfile.wav"]), vec!["newfile.wav"]);
}

// ============================ gate 7 — budget / all-simultaneous ============================

#[test]
fn gate7_matrix_all_simultaneous() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cameras = (0..6).map(|k| cam(&format!("video{k}"), k as f64 * 30.0, 30.0)).collect();
    let externals = (0..6)
        .map(|k| SessionExt {
            name: format!("ext{k}"),
            master_t0: 0.0,
            dur_s: 180.0,
            snr_db: Some(20.0 + k as f64),
            ..Default::default()
        })
        .collect();
    let spec = SessionSpec { seed: 77, cameras, externals, ..Default::default() };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    let rows = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    assert_eq!(rows.len(), 6);
    for k in 0..6 {
        let r = row(&rows, &format!("video{k}.mp4"));
        assert_eq!(r.state, RowState::TwoSources, "{r:?}");
        assert!(r.selected.is_some());
    }
}

// ============================ gate 8 — honesty floor ============================

#[test]
fn gate8_honesty_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 88,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![
            ext("ext_true", 0.0, 120.0),
            SessionExt {
                name: "ext_decoy".into(),
                master_t0: 0.0,
                dur_s: 120.0,
                plan_jitter_ms: 400.0,
                jitter_seed: 4242,
                snr_db: Some(20.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);
    let rows = ctx.resolve(&vids, &auds, &fp_params()).unwrap();

    let r = row(&rows, "video.mp4");
    // Never confident-and-wrong: any selection must be the true take; ambiguous is acceptable.
    match selected_audio(r) {
        Some(sel) => assert_eq!(sel, "ext_true.wav", "decoy selected: {r:?}"),
        None => assert_eq!(r.state, RowState::Ambiguous, "{r:?}"),
    }
}

// ============================ §3.5 margin rule (legacy) ============================

#[test]
fn margin_rule_prefers_ambiguous() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 99,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![
            ext("extA", 0.0, 120.0),
            SessionExt {
                name: "extB".into(),
                master_t0: 0.0,
                dur_s: 120.0,
                snr_db: Some(15.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);

    // LEGACY z-margin conflict path via legacy-only constants (triangle-tol-ms, z-margin) —
    // the CLI `--matcher legacy --params triangle-tol-ms=0.000001 --params z-margin=1000`
    // equivalent. The CLI legacy resolver never escalates RMS (rms_escalation stays false), so
    // the undecided cluster lands ambiguous.
    let mut p = legacy_params();
    p.triangle_tol_ms = 0.000001;
    p.z_margin = 1000.0;
    let rows = ctx.resolve(&vids, &auds, &p).unwrap();

    let r = row(&rows, "video.mp4");
    assert_eq!(r.state, RowState::Ambiguous, "{r:?}");
    assert!(r.selected.is_none(), "ambiguous row must not select (V2-F2)");
    assert_eq!(alternates(r).len(), 2, "all candidates listed for the picker");
    assert!(r.resolution.margin.is_some());
}

// ============================ RMS escalation (legacy, Swift parity) ============================

/// The Swift legacy resolver's RMS-tiebreaker ESCALATION (which the CLI legacy resolver does
/// NOT run): the `pair_fit_batch` → `escalate` → `rms_escalate_conservative` two-pass path the
/// `rms_escalation` flag gates. This drives the conflict path with legacy-only constants
/// (`triangle_tol_ms`, `z_margin`) exactly like `margin_rule_prefers_ambiguous`, then checks
/// the flag's effect on the SAME fixture.
///
/// extA/extB are genuine same-master duplicates that BOTH drift-fit cleanly (low RMS), so the
/// CONSERVATIVE form correctly DECLINES to flip — a comparably-clean rival means the z-leader
/// is not decisively better, so the row stays honestly `ambiguous` (V2-F2: never a coin-flip).
/// This is the documented Swift behavior (a successful clean-vs-dirty rescue needs real-corpus
/// material — synthetic same-plan takes both lock, the gate8 limitation). The VALUE here:
/// (1) the escalation path runs without panic and is deterministic; (2) it produces a row that
/// is byte-identical to the CLI legacy resolver on this fixture — escalation must NOT
/// manufacture a selection out of two equally-clean duplicates.
#[test]
fn legacy_rms_escalation_path_runs_and_stays_honest() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 99,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![
            ext("extA", 0.0, 120.0),
            SessionExt {
                name: "extB".into(),
                master_t0: 0.0,
                dur_s: 120.0,
                snr_db: Some(15.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);

    // OFF (CLI legacy default): the undecided contested cluster ⇒ ambiguous, no selection.
    let mut off = legacy_params();
    off.triangle_tol_ms = 0.000001;
    off.z_margin = 1000.0;
    off.rms_escalation = false;
    let r_off = ctx.resolve(&vids, &auds, &off).unwrap();
    let row_off = row(&r_off, "video.mp4");
    assert_eq!(row_off.state, RowState::Ambiguous, "escalation OFF stays ambiguous: {row_off:?}");
    assert!(row_off.selected.is_none());

    // ON (Swift legacy default), conservative form: the path runs `pair_fit_batch` over the
    // contested pair and `escalate`/`rms_escalate_conservative` correctly DECLINES (both
    // duplicates lock cleanly ⇒ the rival is comparably clean ⇒ no decisive leader). The row
    // stays honestly ambiguous with no selection — never a coin-flip.
    let mut on = off.clone();
    on.rms_escalation = true;
    let r_on = ctx.resolve(&vids, &auds, &on).unwrap();
    let row_on = row(&r_on, "video.mp4");
    eprintln!("rms escalation ON (conservative) row: {row_on:?}");
    assert_eq!(
        row_on.state,
        RowState::Ambiguous,
        "two equally-clean duplicates must NOT be rescued by the conservative form: {row_on:?}"
    );
    assert!(row_on.selected.is_none(), "no coin-flip selection on a clean-duplicate tie: {row_on:?}");

    // STRONG form: with TWO clean locks the strong form also declines (`cleans.count == 2 != 1`),
    // so it likewise stays ambiguous — the escalation never invents a winner.
    let mut on_strong = on.clone();
    on_strong.rms_strong_form = true;
    let r_strong = ctx.resolve(&vids, &auds, &on_strong).unwrap();
    let row_strong = row(&r_strong, "video.mp4");
    eprintln!("rms escalation ON (strong) row: {row_strong:?}");
    assert_eq!(
        row_strong.state,
        RowState::Ambiguous,
        "strong form with two clean locks also declines: {row_strong:?}"
    );

    // Honesty parity: the escalation-ON conservative row is byte-identical to the CLI legacy
    // (escalation-OFF) row on this fixture — escalation that declines changes NOTHING.
    assert_eq!(
        serde_json::to_string(row_on).unwrap(),
        serde_json::to_string(row_off).unwrap(),
        "a declined escalation must leave the row byte-identical to the CLI legacy resolver"
    );
}

// ============================ classifier: short genuine clip (legacy) ============================

#[test]
fn gate_short_genuine_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 1212,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![ext("clip", 50.0, 20.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);

    // Baseline (flag OFF, z-min raised to 11): the z-gate rejects the clip's z≈9.86 ⇒ no match.
    let mut off = legacy_params();
    off.verify_active = false;
    off.z_min = 11.0;
    let r_off = ctx.resolve(&vids, &auds, &off).unwrap();
    let row_off = row(&r_off, "video.mp4");
    assert_eq!(row_off.state, RowState::NoMatch, "flag-OFF z-gate (z-min=11) rejects: {row_off:?}");

    // Flag ON (same z-min=11): the classifier bypasses z; the fine waveform-lock admits it.
    let mut on = legacy_params();
    on.verify_active = true;
    on.z_min = 11.0;
    let r_on = ctx.resolve(&vids, &auds, &on).unwrap();
    let row_on = row(&r_on, "video.mp4");
    assert!(
        row_on.state == RowState::Confident || row_on.state == RowState::Partial,
        "classifier should select the genuine 20 s clip: {row_on:?}"
    );
    assert_eq!(selected_audio(row_on), Some("clip.wav"), "{row_on:?}");
}

// ============================ classifier: short spurious clip ============================

#[test]
fn gate_short_spurious_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 1313,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![SessionExt {
            name: "spurious".into(),
            master_t0: 50.0,
            dur_s: 20.0,
            master_seed: Some(9001),
            ..Default::default()
        }],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);

    // Default (fingerprint) + verify_active mirrors the CLI `--params verify-active=1` (no
    // --matcher, so fingerprint default). The different-master clip must not match.
    let mut p = fp_params();
    p.verify_active = true;
    let rows = ctx.resolve(&vids, &auds, &p).unwrap();
    let r = row(&rows, "video.mp4");
    assert_eq!(r.state, RowState::NoMatch, "different-master clip must not match: {r:?}");
    assert!(r.selected.is_none(), "{r:?}");
    assert_eq!(unused_audio(&rows, &["spurious.wav"]), vec!["spurious.wav"]);
}

// ============================ classifier: takes (legacy) ============================

#[test]
fn verify_classifies_takes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let spec = SessionSpec {
        seed: 1414,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![
            ext("ext_true", 0.0, 120.0),
            SessionExt {
                name: "ext_decoy".into(),
                master_t0: 0.0,
                dur_s: 120.0,
                plan_jitter_ms: 400.0,
                jitter_seed: 4242,
                snr_db: Some(20.0),
                ..Default::default()
            },
        ],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let (ctx, vids, auds) = import_session(&dir);

    let mut p = legacy_params();
    p.verify_active = true;
    let rows = ctx.resolve(&vids, &auds, &p).unwrap();
    let r = row(&rows, "video.mp4");
    eprintln!("verify_classifies_takes row: {r:?}");
    // Any selection must be the true take — a confident decoy is the failure gate8 forbids.
    if let Some(sel) = selected_audio(r) {
        assert_eq!(sel, "ext_true.wav", "decoy selected by the classifier: {r:?}");
    }
}

// ============================ candidate overflow (legacy) ============================

#[test]
fn candidate_overflow_no_silent_eviction() {
    let names: [&str; 8] =
        ["aaa_rec", "bbb_rec", "ccc_rec", "ddd_rec", "eee_rec", "fff_rec", "ggg_rec", "zzz_true"];
    let make = |dir: &Path, names: &[&str; 8]| {
        let externals = names
            .iter()
            .enumerate()
            .map(|(k, n)| SessionExt {
                name: (*n).into(),
                master_t0: 0.0,
                dur_s: 120.0,
                snr_db: Some(20.0 + k as f64),
                ..Default::default()
            })
            .collect();
        let spec = SessionSpec {
            seed: 4040,
            cameras: vec![cam("video", 30.0, 60.0)],
            externals,
            ..Default::default()
        };
        generate_session(dir, &spec, &ffmpeg()).unwrap();
    };

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    make(&dir, &names);
    let (ctx, vids, auds) = import_session(&dir);

    // (a) Normal limits (default max-verify-pairs-per-video = 32 ≫ 8): every candidate verified;
    //     the true pair (past position six) is present in the resolved row.
    let mut on = legacy_params();
    on.verify_active = true;
    let r = ctx.resolve(&vids, &auds, &on).unwrap();
    let row_a = row(&r, "video.mp4");
    eprintln!("overflow (a) normal row: {row_a:?}");
    assert_ne!(row_a.state, RowState::NoMatch, "8 genuine candidates must not resolve noMatch: {row_a:?}");
    assert!(row_a.resolution.candidate_overflow.is_none(), "no overflow under normal limits: {row_a:?}");
    let mut present: Vec<&str> = alternates(row_a);
    if let Some(s) = selected_audio(row_a) {
        present.push(s);
    }
    assert!(present.contains(&"zzz_true.wav"), "the true pair must be verified, not evicted: {row_a:?}");
    assert_eq!(present.len(), 8, "all eight candidates visible: {row_a:?}");

    // (b) Low per-video limit (4 < 8 eligible) ⇒ explicit ambiguity with the overflow detail.
    let mut low = legacy_params();
    low.verify_active = true;
    low.pairing.max_verify_pairs_per_video = 4;
    let rl = ctx.resolve(&vids, &auds, &low).unwrap();
    let row_b = row(&rl, "video.mp4");
    eprintln!("overflow (b) low-limit row: {row_b:?}");
    assert_eq!(row_b.state, RowState::Ambiguous, "overflow must be ambiguous: {row_b:?}");
    assert!(row_b.selected.is_none(), "overflow row must not select (V2-F2): {row_b:?}");
    let ov = row_b.resolution.candidate_overflow.as_ref().expect("overflow detail");
    assert_eq!(ov.eligible, 8, "eligible count surfaced: {row_b:?}");
    assert_eq!(ov.limit, 4, "configured limit surfaced: {row_b:?}");
    assert_eq!(ov.scope, "perVideo", "ceiling identified: {row_b:?}");
    let mut alts: Vec<&str> = alternates(row_b);
    alts.sort();
    let mut expect: Vec<String> = names.iter().map(|n| format!("{n}.wav")).collect();
    expect.sort();
    assert_eq!(alts, expect, "ALL eight candidate names stay visible under overflow: {row_b:?}");

    // (c) Filenames do NOT decide retention: reverse the names, re-run the low limit — the
    //     overflow set is the SAME full eight (none silently retained by name).
    let dir2 = tmp.path().join("session2");
    let mut rev = names;
    rev.reverse();
    make(&dir2, &rev);
    let (ctx2, vids2, auds2) = import_session(&dir2);
    let rl2 = ctx2.resolve(&vids2, &auds2, &low).unwrap();
    let row_c = row(&rl2, "video.mp4");
    eprintln!("overflow (c) renamed low-limit row: {row_c:?}");
    assert_eq!(row_c.state, RowState::Ambiguous, "{row_c:?}");
    assert_eq!(row_c.resolution.candidate_overflow.as_ref().unwrap().eligible, 8, "{row_c:?}");
    let mut alts2: Vec<&str> = alternates(row_c);
    alts2.sort();
    assert_eq!(alts2, expect, "overflow set is name-order-independent: {row_c:?}");
}

// ============================ realdata finish line ============================

#[test]
#[ignore = "SKIPPED (no realdata): set COUPLE_REALDATA_DIR and run scripts/realdata-sweep.sh (or -- --include-ignored)"]
fn realdata_pair_confident_row() {
    let dir = common::realdata_dir();
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");
    let ctx = common::test_ctx(&cache);
    let video = dir.join("schumann-video.mp4");
    let audio = dir.join("schumann-audio.wav");
    let v = common::import(&ctx, &video, TrackRole::Camera).unwrap();
    let a = common::import(&ctx, &audio, TrackRole::External).unwrap();

    let rows = ctx.resolve(&[v], &[a], &fp_params()).unwrap();
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.state, RowState::Confident, "{r:?}");
    assert_eq!(selected_audio(r), Some("schumann-audio.wav"));

    // Frozen container-time offset −5536.639 ms; the §5.3 stage ladder lands within ±25 ms.
    let lag = r.selected.as_ref().unwrap().lag_ms;
    assert!((lag - -5536.639).abs() <= 25.0, "lag {lag} vs frozen −5536.639 ± 25");
    eprintln!("realdata row: {r:?}");
}

// 2026-07-11 RE-FREEZE — resolver-constant VALUE LOCK (PLAN-REALDATA-CALIBRATION §4.4,
// V2-F2 closure). A change is a re-calibration decision, not an edit.
#[test]
fn resolve_params_value_lock() {
    let p = ResolveParams::default();
    assert_eq!(p.coverage_full, 0.95, "re-frozen 2026-07-11: real full-true min 0.9628 vs genuine partial 0.082");
    // Held (zero true different-take conflicts observed; gap RECORDED).
    assert_eq!(p.z_min, 8.5);
    assert_eq!(p.z_margin, 6.0);
    assert_eq!(p.triangle_tol_ms, 250.0);
    assert_eq!(p.rms_trust_floor_ms, 9.0);
    assert_eq!(p.rms_ratio, 3.0);
}
