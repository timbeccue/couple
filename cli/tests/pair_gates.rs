//! SPEC-V2 §7 gates 1–8 — headless, via the real `couple pair` binary over
//! generated session folders. Run with `--release` (fixture generation is
//! sinc-interpolation heavy).

use couple_core::fixture::{generate_session, SessionCam, SessionExt, SessionSpec};
use serde_json::Value;
use std::path::Path;
use std::process::Command;

fn ffmpeg() -> std::path::PathBuf {
    couple_core::ffmpeg::resolve_tool("ffmpeg", None).expect("ffmpeg on PATH")
}

fn cam(name: &str, t0: f64, dur: f64) -> SessionCam {
    SessionCam {
        name: name.into(),
        master_t0: t0,
        dur_s: dur,
        ..Default::default()
    }
}

fn ext(name: &str, t0: f64, dur: f64) -> SessionExt {
    SessionExt {
        name: name.into(),
        master_t0: t0,
        dur_s: dur,
        ..Default::default()
    }
}

fn pair_json(dir: &Path, cache: &Path, extra: &[&str]) -> Value {
    let out = Command::new(env!("CARGO_BIN_EXE_couple"))
        .args(["--json", "--quiet", "--cache-dir"])
        .arg(cache)
        .arg("pair")
        .arg(dir)
        .args(extra)
        .output()
        .expect("run couple pair");
    assert!(
        out.status.success(),
        "couple pair failed: {}\n{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).expect("pair JSON")
}

fn row<'a>(report: &'a Value, video: &str) -> &'a Value {
    report["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["video"] == video)
        .unwrap_or_else(|| panic!("no row for {video}"))
}

fn selected_audio(row: &Value) -> Option<&str> {
    row["selected"]["audio"].as_str()
}

fn alternates(row: &Value) -> Vec<&str> {
    row["alternates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["audio"].as_str().unwrap())
        .collect()
}

/// Stage-lag tolerance for the truth-table gates, ms. The refined §5.3 ladder
/// lands sub-ms on clean fixtures (§9.3); AAC round-trip on the camera files
/// stays well inside this.
const LAG_TOL_MS: f64 = 2.0;

fn assert_lag(report: &Value, video: &str, expect_ms: f64) {
    let lag = row(report, video)["selected"]["lagMs"].as_f64().unwrap();
    assert!(
        (lag - expect_ms).abs() <= LAG_TOL_MS,
        "{video}: lag {lag:.3} ms, expected {expect_ms:.3} ± {LAG_TOL_MS}"
    );
}

/// Gate 1 — continuous recorder: 3 videos cut from one master + 1 external covering
/// all ⇒ 3 rows, same audio, full coverage, correct per-row lags.
#[test]
fn gate1_continuous_recorder() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 11,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 150.0, 40.0), cam("video3", 270.0, 40.0)],
        externals: vec![ext("external", 0.0, 330.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let r = pair_json(&dir, &cache, &[]);

    assert_eq!(r["rows"].as_array().unwrap().len(), 3);
    for (v, t0) in [("video1.mp4", 30.0), ("video2.mp4", 150.0), ("video3.mp4", 270.0)] {
        let row = row(&r, v);
        assert_eq!(row["state"], "confident", "{v}: {row}");
        assert_eq!(selected_audio(row), Some("external.wav"));
        assert!(row["selected"]["coverage"]["fraction"].as_f64().unwrap() >= 0.98);
        assert_lag(&r, v, t0 * 1000.0);
    }
    assert!(r["unusedAudio"].as_array().unwrap().is_empty());
}

/// Gate 2 — recorder restart: 2 externals splitting the master mid-video-2 ⇒
/// videos 1/3 full coverage; video 2 `split` with max coverage selected.
/// Take lengths are realistic (≥ 80 s): at 40 s takes a same-master/disjoint-span
/// cell measured z 8.9 (above the 8.5 floor; Session-C unrelated ceiling was 7.5),
/// and with true z only ~14 the margin rule honestly said `ambiguous` — recorded
/// as V2-F2 calibration data in PROGRESS.md.
#[test]
fn gate2_recorder_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 22,
        cameras: vec![cam("video1", 20.0, 80.0), cam("video2", 120.0, 90.0), cam("video3", 240.0, 80.0)],
        externals: vec![ext("ext1", 0.0, 150.0), ext("ext2", 150.0, 330.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let r = pair_json(&dir, &cache, &[]);

    let r1 = row(&r, "video1.mp4");
    assert_eq!(r1["state"], "confident", "{r1}");
    assert_eq!(selected_audio(r1), Some("ext1.wav"));
    assert_lag(&r, "video1.mp4", 20.0 * 1000.0);

    let r3 = row(&r, "video3.mp4");
    assert_eq!(r3["state"], "confident", "{r3}");
    assert_eq!(selected_audio(r3), Some("ext2.wav"));
    assert_lag(&r, "video3.mp4", (240.0 - 150.0) * 1000.0);

    // Video 2 straddles the restart: ext1 covers [0,30), ext2 covers [30,90) of its
    // 90 s — disjoint coverage, max coverage (ext2) selected, ext1 alternate.
    let r2 = row(&r, "video2.mp4");
    assert_eq!(r2["state"], "split", "{r2}");
    assert_eq!(selected_audio(r2), Some("ext2.wav"));
    assert_eq!(alternates(r2), vec!["ext1.wav"]);
    assert_lag(&r, "video2.mp4", (120.0 - 150.0) * 1000.0);
    let cov = &r2["selected"]["coverage"];
    assert!((cov["t0"].as_f64().unwrap() - 30.0).abs() < 0.5, "{cov}");
    assert!((cov["t1"].as_f64().unwrap() - 90.0).abs() < 0.5, "{cov}");
    assert_eq!(r2["resolution"]["clusters"], 2);
}

/// Gate 3 — simultaneous recorders: 2 externals over the same span with independent
/// degradation ⇒ `2 sources`, triangle closure within TRIANGLE_TOL_MS, best-z
/// default. (Also gate 8's sanity direction: same-master different-degradation MUST
/// classify as simultaneous.)
#[test]
fn gate3_simultaneous_recorders() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
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
    let r = pair_json(&dir, &cache, &[]);

    let row = row(&r, "video.mp4");
    assert_eq!(row["state"], "twoSources", "{row}");
    let sel = selected_audio(row).unwrap();
    let alts = alternates(row);
    assert_eq!(alts.len(), 1);
    assert_ne!(sel, alts[0]);
    // Best z selected.
    let sel_z = row["selected"]["z"].as_f64().unwrap();
    let alt_z = row["alternates"][0]["z"].as_f64().unwrap();
    assert!(sel_z >= alt_z, "selected z {sel_z} < alternate z {alt_z}");
    // Triangle closure (§3.4): A×B above floor AND lags close the triangle.
    let tri = &row["resolution"]["triangles"][0];
    assert_eq!(tri["simultaneous"], true, "{tri}");
    assert!(tri["abZ"].as_f64().unwrap() >= 8.5);
    assert!(tri["residualMs"].as_f64().unwrap() <= 250.0);
    assert_lag(&r, "video.mp4", 30.0 * 1000.0);
}

/// Gate 4 — unrelated audio (different master) never pairs and lands in the unused
/// tray; a video with only unrelated audio is `no match`.
#[test]
fn gate4_unrelated_audio() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 44,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 120.0, 40.0)],
        externals: vec![
            ext("ext_true", 0.0, 80.0), // covers video1 only
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
    let r = pair_json(&dir, &cache, &[]);

    let r1 = row(&r, "video1.mp4");
    assert_eq!(r1["state"], "confident", "{r1}");
    assert_eq!(selected_audio(r1), Some("ext_true.wav"));

    // video2's span is covered by NO related audio: same-master-different-span and
    // different-master must both stay below the floor.
    let r2 = row(&r, "video2.mp4");
    assert_eq!(r2["state"], "noMatch", "{r2}");
    assert!(r2["selected"].is_null());

    let unused: Vec<&str> = r["unusedAudio"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(unused, vec!["unrelated.wav"]);
    // The unrelated file cleared the floor against nothing.
    for cell in r["matrix"].as_array().unwrap() {
        if cell["audio"] == "unrelated.wav" {
            assert_eq!(cell["candidate"], false, "{cell}");
        }
    }
}

/// Gate 5 — mixed bag: non-media litter is skipped (surfaced once), an audio-less
/// video gets an honest `no match` row; no crash, no litter.
#[test]
fn gate5_mixed_bag() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
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
    let r = pair_json(&dir, &cache, &[]);

    let skipped: Vec<&str> = r["skipped"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert!(skipped.contains(&"notes.txt"), "{skipped:?}");

    let muted = row(&r, "muted.mp4");
    assert_eq!(muted["state"], "noMatch", "{muted}");
    assert_eq!(muted["resolution"]["note"], "no audio stream");

    let ok = row(&r, "video.mp4");
    assert_eq!(ok["state"], "confident", "{ok}");
}

/// Gate 6 — stability: adding one file changes only what the new file can change;
/// resolved rows for unchanged inputs are byte-identical in --json.
#[test]
fn gate6_stability() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 66,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 100.0, 40.0)],
        externals: vec![ext("external", 0.0, 150.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let r1 = pair_json(&dir, &cache, &[]);

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
    let r2 = pair_json(&dir, &cache, &[]);

    for v in ["video1.mp4", "video2.mp4"] {
        let a = serde_json::to_string(row(&r1, v)).unwrap();
        let b = serde_json::to_string(row(&r2, v)).unwrap();
        assert_eq!(a, b, "row {v} changed after an unrelated file was added");
    }
    let unused: Vec<&str> = r2["unusedAudio"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(unused, vec!["newfile.wav"]);
}

/// Gate 7 — budget: 6 video × 6 audio candidate matrix ≤ 5 s on top of imports.
#[test]
fn gate7_matrix_budget() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
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
    let spec = SessionSpec {
        seed: 77,
        cameras,
        externals,
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();
    let r = pair_json(&dir, &cache, &[]);

    assert_eq!(r["matrix"].as_array().unwrap().len(), 36);
    let matrix_ms = r["timingMs"]["matrix"].as_f64().unwrap();
    eprintln!("gate7: 36-cell candidate matrix in {matrix_ms:.1} ms (budget 5000)");
    assert!(matrix_ms <= 5000.0, "candidate matrix took {matrix_ms} ms (> 5 s budget)");
    // All six recorders are simultaneous: every row selects SOME source confidently.
    for k in 0..6 {
        let row = row(&r, &format!("video{k}.mp4"));
        assert_eq!(row["state"], "twoSources", "{row}");
        assert!(row["selected"].is_object());
    }
}

/// Gate 8 (best-effort, V2-F2) — honesty floor: a different "take" (same event
/// plan, transient micro-timing jittered ±400 ms, different noise) must never be
/// confidently selected over — or instead of — the true take. The recorded limit:
/// synthetic fixtures cannot faithfully produce real same-piece-different-take
/// material; real calibration waits for the owner corpus.
#[test]
fn gate8_honesty_floor() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
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
    let r = pair_json(&dir, &cache, &[]);

    let row = row(&r, "video.mp4");
    // Never confident-and-wrong: any selection must be the true take; `ambiguous`
    // (no selection) is also acceptable. A confident decoy selection fails.
    if let Some(sel) = selected_audio(row) {
        assert_eq!(sel, "ext_true.wav", "decoy selected: {row}");
    } else {
        assert_eq!(row["state"], "ambiguous", "{row}");
    }
    // Record the raw numbers for the future corpus calibration. Measured at
    // ±400 ms jitter: decoy z 14.8 vs true 16.8 (margin 2.0), A×B z 19.6,
    // residual 53 ms ⇒ classified simultaneous, true take selected by best-z.
    // At ±900 ms: decoy 12.8, A×B z 16.3, residual 11 ms — still "simultaneous"
    // (both lags shift together, and 200 ms stage-1 bins integrate over the
    // jitter). Synthetic same-plan takes cannot fail the simultaneity check;
    // the §3.5 ambiguous path is exercised in margin_rule_prefers_ambiguous.
    eprintln!("gate8 row: {row}");
    for cell in r["matrix"].as_array().unwrap() {
        eprintln!(
            "gate8 matrix: {} x {} z={} lag={}",
            cell["video"], cell["audio"], cell["z"], cell["lagMs"]
        );
    }
}

/// §3.5 margin rule, ambiguous branch (V2-F2: prefer `ambiguous` over a coin-flip).
/// Synthetic fixtures cannot make the simultaneity check fail honestly (see
/// gate8_honesty_floor), so this drives the conflict path via constants: closure
/// tolerance pinned to zero forces "not simultaneous", and a huge Z_MARGIN makes
/// the close-z conflict indecisive ⇒ NO selection, row floats as `ambiguous`.
#[test]
fn margin_rule_prefers_ambiguous() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
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
    let r = pair_json(
        &dir,
        &cache,
        &["--params", "triangle-tol-ms=0.000001", "--params", "z-margin=1000"],
    );

    let row = row(&r, "video.mp4");
    assert_eq!(row["state"], "ambiguous", "{row}");
    assert!(row["selected"].is_null(), "ambiguous row must not select (V2-F2)");
    assert_eq!(alternates(row).len(), 2, "all candidates listed for the picker");
    assert!(row["resolution"]["margin"].as_f64().is_some());
}

/// Finish line (real data): `couple pair` over the canonical 1-video + 1-audio pair
/// yields exactly one confident row whose lag matches the frozen offset within
/// stage-1 tolerance. Gated on COUPLE_REALDATA_DIR like the §9.4 suites.
#[test]
fn realdata_pair_confident_row() {
    let Some(dir) = std::env::var_os("COUPLE_REALDATA_DIR").map(std::path::PathBuf::from) else {
        eprintln!("COUPLE_REALDATA_DIR unset; skipping");
        return;
    };
    let video = dir.join("schumann-video.mp4");
    let audio = dir.join("schumann-audio.wav");
    if !video.is_file() || !audio.is_file() {
        eprintln!("real pair missing; skipping");
        return;
    }
    let tmp = tempfile::tempdir().unwrap();
    let session = tmp.path().join("session");
    std::fs::create_dir(&session).unwrap();
    std::os::unix::fs::symlink(&video, session.join("schumann-video.mp4")).unwrap();
    std::os::unix::fs::symlink(&audio, session.join("schumann-audio.wav")).unwrap();
    // Default cache dir: reuse the real pair's existing import caches.
    let out = Command::new(env!("CARGO_BIN_EXE_couple"))
        .args(["--json", "--quiet", "pair"])
        .arg(&session)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let r: Value = serde_json::from_slice(&out.stdout).unwrap();

    let rows = r["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["state"], "confident", "{row}");
    assert_eq!(selected_audio(row), Some("schumann-audio.wav"));
    let confident = rows.iter().filter(|r| r["state"] == "confident").count();
    assert_eq!(confident, 1);

    // Frozen container-time offset (PROGRESS 2026-06-11, §9.4 freeze): −5536.639 ms.
    // pair_correlate is the §5.3 stage ladder without the drift fit; the +3.57 ppm
    // drift over the ~24 min overlap puts the stage lag within a few ms of the
    // fitted intercept — ±25 ms is the stage-tolerance window.
    let lag = row["selected"]["lagMs"].as_f64().unwrap();
    assert!((lag - -5536.639).abs() <= 25.0, "lag {lag} vs frozen −5536.639 ± 25");
    eprintln!("realdata row: {row}");
}
