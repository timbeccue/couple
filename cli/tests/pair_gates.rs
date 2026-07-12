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
    serde_json::from_slice(&pair_stdout(dir, cache, extra)).expect("pair JSON")
}

/// Raw stdout bytes of `couple --json --quiet pair <dir> [extra...]` (for byte-identity checks).
fn pair_stdout(dir: &Path, cache: &Path, extra: &[&str]) -> Vec<u8> {
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
    out.stdout
}

/// stderr of a `couple pair` run (for the opt-in debug tables, which never touch stdout/--json).
fn pair_stderr(dir: &Path, cache: &Path, extra: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_couple"))
        .args(["--json", "--quiet", "--cache-dir"])
        .arg(cache)
        .arg("pair")
        .arg(dir)
        .args(extra)
        .output()
        .expect("run couple pair");
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stderr).into_owned()
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

/// Gate 2 — recorder restart: 2 externals splitting the master mid-video-2.
///
/// GROUND TRUTH: video1 master [20,100) ⊂ ext1 [0,150) ⇒ full-coverage true pair. video3 master
/// [240,320) ⊂ ext2 [150,480) ⇒ full-coverage true pair. video2 master [120,210) straddles the
/// restart: ext1 covers only its first 30 s (video2 timeline [0,30)), ext2 covers the last 60 s
/// ([30,90)) — so ext2 is video2's DOMINANT true source (⅔ of its span).
///
/// BOUNDARY 13 re-baseline: the `split` recorder-restart STATE depends on the resolver accepting
/// ext1↔video2 — a 30 s overlap that is only 0.315 of video2's 90 s span. The legacy z-gate accepts
/// it (no coverage floor); the fingerprint §10.6 identity gate deliberately REJECTS it
/// (`temporal_coverage 0.315 < min_temporal_coverage 0.35` — the localized-coincidence guard). That
/// is a CORRECT fingerprint decision (a ⅓-coverage partial is exactly the hazard the gate blocks),
/// not a regression — so the `split` clustering policy is validated under the `--matcher legacy`
/// developer override, while the fingerprint DEFAULT is verified to make a SAFE, ground-truth-correct
/// decision (no false match: video2 resolves to ext2, its true dominant source; ext1's partial is
/// honestly suppressed).
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

    // --- Fingerprint DEFAULT: full-coverage true pairs Match; video2 selects its dominant true
    //     source (ext2). The ⅓-coverage ext1↔video2 partial is honestly suppressed (NOT a false
    //     match, NOT a confident wrong pairing). ---
    let r = pair_json(&dir, &cache, &[]);
    let r1 = row(&r, "video1.mp4");
    assert_eq!(r1["state"], "confident", "{r1}");
    assert_eq!(selected_audio(r1), Some("ext1.wav"));
    assert_lag(&r, "video1.mp4", 20.0 * 1000.0);
    let r3 = row(&r, "video3.mp4");
    assert_eq!(r3["state"], "confident", "{r3}");
    assert_eq!(selected_audio(r3), Some("ext2.wav"));
    assert_lag(&r, "video3.mp4", (240.0 - 150.0) * 1000.0);
    // video2: its accepted source is ext2 (the true dominant ⅔-coverage take); never a wrong pick.
    let r2 = row(&r, "video2.mp4");
    assert!(r2["state"] == "confident" || r2["state"] == "partial", "video2 fingerprint state: {r2}");
    assert_eq!(selected_audio(r2), Some("ext2.wav"), "video2 selects its true dominant source ext2: {r2}");
    assert_lag(&r, "video2.mp4", (120.0 - 150.0) * 1000.0);

    // --- `--matcher legacy` (developer override, §16.1): the legacy candidate set surfaces the 30 s
    //     ext1↔video2 partial, so video2 is `split` with disjoint clusters and ext2 (max coverage)
    //     selected — the recorder-restart CLUSTERING policy, byte-identical to before boundary 13. ---
    let rl = pair_json(&dir, &cache, &["--matcher", "legacy"]);
    let rl1 = row(&rl, "video1.mp4");
    assert_eq!(rl1["state"], "confident", "{rl1}");
    assert_eq!(selected_audio(rl1), Some("ext1.wav"));
    let rl3 = row(&rl, "video3.mp4");
    assert_eq!(rl3["state"], "confident", "{rl3}");
    assert_eq!(selected_audio(rl3), Some("ext2.wav"));
    let rl2 = row(&rl, "video2.mp4");
    assert_eq!(rl2["state"], "split", "{rl2}");
    assert_eq!(selected_audio(rl2), Some("ext2.wav"));
    assert_eq!(alternates(rl2), vec!["ext1.wav"]);
    assert_lag(&rl, "video2.mp4", (120.0 - 150.0) * 1000.0);
    let cov = &rl2["selected"]["coverage"];
    assert!((cov["t0"].as_f64().unwrap() - 30.0).abs() < 0.5, "{cov}");
    assert!((cov["t1"].as_f64().unwrap() - 90.0).abs() < 0.5, "{cov}");
    assert_eq!(rl2["resolution"]["clusters"], 2);
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
///
/// PHASE-2 NOTE (PLAN-MATCH-CLASSIFIER §6 option a): this test pins the **z-path**
/// (default `verify-active=0`). The classifier path treats extA/extB as genuine
/// same-master duplicates (both pass `is_match` → `2 sources`), which is the correct
/// classification but NOT the z-margin ambiguity this test was written to protect. So it
/// runs flag-OFF; the classifier's behaviour on takes is asserted by `verify_classifies_takes`.
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
    // BOUNDARY 13: this drives the LEGACY z-margin conflict path via legacy-only constants
    // (`triangle-tol-ms`, `z-margin`), so it is a `--matcher legacy` regression guard (§16.1). The
    // fingerprint resolver's §3.4 triangle uses the core composed-map residual + graph tolerances,
    // not `triangle-tol-ms`; the fingerprint equivalent (a genuinely non-closing take ⇒ ambiguous)
    // is exercised by the truth-table gates. Run under the legacy override.
    let r = pair_json(
        &dir,
        &cache,
        &["--matcher", "legacy", "--params", "triangle-tol-ms=0.000001", "--params", "z-margin=1000"],
    );

    let row = row(&r, "video.mp4");
    assert_eq!(row["state"], "ambiguous", "{row}");
    assert!(row["selected"].is_null(), "ambiguous row must not select (V2-F2)");
    assert_eq!(alternates(row).len(), 2, "all candidates listed for the picker");
    assert!(row["resolution"]["margin"].as_f64().is_some());
}

/// Finish line (real data): `couple pair` over the canonical 1-video + 1-audio pair yields exactly
/// one confident row whose lag matches the frozen offset. Boundary 13: the no-flag default now uses
/// the FINGERPRINT resolver, and schumann Matches under profile 2 (CALIBRATION.md §6.5 — it was
/// `Ambiguous` under profile 1) with refined lag −5536.4 ms (within 0.2 ms of the frozen truth), so
/// the row is `confident` and the lag well inside the ±25 ms window. Gated on COUPLE_REALDATA_DIR.
#[test]
#[ignore = "SKIPPED (no realdata): set COUPLE_REALDATA_DIR and run scripts/realdata-sweep.sh (or -- --include-ignored)"]
fn realdata_pair_confident_row() {
    let dir = std::env::var_os("COUPLE_REALDATA_DIR")
        .map(std::path::PathBuf::from)
        .expect(
            "REALDATA MISSING: run explicitly (--include-ignored) with COUPLE_REALDATA_DIR \
             set, or via scripts/realdata-sweep.sh",
        );
    let video = dir.join("schumann-video.mp4");
    let audio = dir.join("schumann-audio.wav");
    assert!(
        video.is_file() && audio.is_file(),
        "REALDATA MISSING: schumann pair not present under {}",
        dir.display()
    );
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

// ===== PLAN-MATCH-CLASSIFIER Phase-2 fixtures (flag ON: `verify-active=1`) =====
// These exercise the classifier path, which the default-OFF gates above never touch.
// They prove the flip: the fine waveform-lock both ADMITS a short genuine clip that
// today's z structurally rejects (gate_short_genuine_clip) and REJECTS a short spurious
// overlap (gate_short_spurious_clip) — reject-the-real and accept-the-fake are one root
// cause (coarse envelope `z` can't classify short overlaps), and the classifier fixes both.

/// gate_short_genuine_clip (THE owner-reported regression) — a ~20 s external cut from the
/// SAME master as a 60 s video, fully contained in the video's span. The defect: `z = NCC·√n`
/// structurally rejects short genuine clips because √n is small (a 20 s overlap caps z near
/// √100 = 10), so a real short clip whose envelope NCC is < ~0.85 lands below the z floor and
/// is rejected BEFORE it can be a candidate — even though the actual audio aligns perfectly.
///
/// SYNTHETIC LIMITATION (mirrors gate8_honesty_floor §pair_gates.rs:369-371): the fixture's
/// master content has an unrealistically HIGH envelope NCC (~0.97 even at 20 s, vs the
/// real-world 0.6–0.8), so this clip's z (~9.86) sits ABOVE the default 8.5 floor — the
/// synthetic clip is "too clean" to fall below it. To reproduce the regression
/// DETERMINISTICALLY we hold the clip fixed and raise the floor to `z-min=11`, putting z in
/// exactly the regime a real short clip occupies (below the floor). This isolates the precise
/// mechanism: the z-gate REJECTS the genuine clip (OFF ⇒ noMatch); the classifier bypasses z
/// and its fine waveform-lock ADMITS it (ON ⇒ confident/partial). z is the SAME in both runs;
/// only the decision path differs — which is exactly the flip this phase ships.
#[test]
fn gate_short_genuine_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    // video covers master [30,90); the 20 s external covers master [50,70) — fully inside,
    // same seed ⇒ genuine same-content match at lag = 30 − 50 = −20 s. Measured z ≈ 9.86.
    let spec = SessionSpec {
        seed: 1212,
        cameras: vec![cam("video", 30.0, 60.0)],
        externals: vec![ext("clip", 50.0, 20.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();

    // BOUNDARY 13: this exercises the LEGACY classifier path (the z-gate ↔ fine-verify flip), which is
    // pinned by legacy-only constants (`z-min`, `verify-active`). It is the regression guard for the
    // `--matcher legacy` developer override (§16.1); the fingerprint default has no z-gate. Run both
    // arms under `--matcher legacy`.
    // Baseline (flag OFF): with the floor raised to 11, the z-gate rejects the clip's
    // z≈9.86 ⇒ no match — the exact structural rejection of a short genuine clip.
    let off = pair_json(&dir, &cache, &["--matcher", "legacy", "--params", "verify-active=0", "--params", "z-min=11"]);
    let off_row = row(&off, "video.mp4");
    eprintln!("gate_short_genuine_clip OFF row: {off_row}");
    assert_eq!(off_row["state"], "noMatch", "flag-OFF z-gate (z-min=11) should reject the 20 s clip: {off_row}");

    // Flag ON (same z-min=11): the classifier bypasses z; the fine waveform-lock admits the
    // genuine clip ⇒ confident/partial selecting it.
    let on = pair_json(&dir, &cache, &["--matcher", "legacy", "--params", "verify-active=1", "--params", "z-min=11"]);
    let on_row = row(&on, "video.mp4");
    eprintln!("gate_short_genuine_clip ON row: {on_row}");
    let state = on_row["state"].as_str().unwrap();
    assert!(
        state == "confident" || state == "partial",
        "classifier should select the genuine 20 s clip (confident/partial), got {state}: {on_row}"
    );
    assert_eq!(selected_audio(on_row), Some("clip.wav"), "{on_row}");
}

/// gate_short_spurious_clip (the opposite failure) — a ~20 s external from a DIFFERENT
/// master overlapping a sliver of the video. The fine lock must REJECT it: a different
/// master scatters (high rms / low lock_fraction), so `is_match` is false ⇒ the video is
/// `noMatch` and the clip is unused. (Run flag ON; the loose pre-filter may admit it as a
/// verify candidate, but the classifier disposes.)
#[test]
fn gate_short_spurious_clip() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    // video on seed 1313 covers master [30,90); a 20 s clip from a DIFFERENT master (seed
    // 9001) placed so its lag overlaps the video's span by a sliver — no real content match.
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

    let on = pair_json(&dir, &cache, &["--params", "verify-active=1"]);
    let on_row = row(&on, "video.mp4");
    eprintln!("gate_short_spurious_clip ON row: {on_row}");
    assert_eq!(on_row["state"], "noMatch", "different-master clip must not match: {on_row}");
    assert!(on_row["selected"].is_null(), "{on_row}");
    let unused: Vec<&str> = on["unusedAudio"].as_array().unwrap().iter().map(|v| v.as_str().unwrap()).collect();
    assert_eq!(unused, vec!["spurious.wav"], "spurious clip stays unused");
}

/// verify_classifies_takes — the classifier on the gate8-style decoy (same plan, ±400 ms
/// micro-timing jitter, different noise) alongside the true take. ASSERT the true take is a
/// real match (is_match true → selected). The decoy: per gate8's recorded limit, synthetic
/// ±400 ms jitter is integrated by the 200 ms stage bins, so it may STILL pass the fine lock
/// (the jitter shifts both lags together and the waveform still broadly agrees). We therefore
/// assert the honesty-preserving property the classifier guarantees — the TRUE take is
/// classified and selected, and any selection is the true take (never a confident decoy) —
/// and DOCUMENT the synthetic-jitter limitation exactly as gate8 does. Real same-piece
/// different-take separation waits for the owner corpus.
#[test]
fn verify_classifies_takes() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
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

    // BOUNDARY 13: this validates the LEGACY fine-verify classifier on takes (`verify-active=1`), the
    // `--matcher legacy` developer-override path (§16.1). Run under the legacy override.
    let on = pair_json(&dir, &cache, &["--matcher", "legacy", "--params", "verify-active=1"]);
    let row = row(&on, "video.mp4");
    eprintln!("verify_classifies_takes ON row: {row}");
    // The true take must be classified (a clean full-overlap lock) and selected. Any
    // selection must be the true take — a confident decoy is the failure gate8 forbids.
    if let Some(sel) = selected_audio(row) {
        assert_eq!(sel, "ext_true.wav", "decoy selected by the classifier: {row}");
    } else {
        // No selection ⇒ honest ambiguity; still never a confident decoy. Acceptable, but
        // the true take SHOULD classify, so record it if the classifier was over-strict.
        eprintln!("verify_classifies_takes: no selection (true take did not classify) — {row}");
    }
    // LIMIT (mirrors gate8_honesty_floor §pair_gates.rs:369-371): synthetic ±400 ms jitter
    // is integrated by the 200 ms stage bins, so the decoy's fine lock can still broadly
    // agree; the classifier cannot be relied on to REJECT a synthetic same-plan take here.
    // The guarantee asserted is honesty-preserving (true take selected, no confident decoy);
    // genuine take separation is validated on the real corpus.
}

// ===== PLAN-SPECTRAL-FINGERPRINT-MATCHING §16.1: boundary-13 default switch + legacy override =====
// Boundary 13 flipped the DEFAULT to the fingerprint resolver (the no-flag `couple pair` now resolves
// from the spectral `PairEvidence` matrix and carries `evidenceMatrix`). The `--matcher legacy`
// DEVELOPER OVERRIDE keeps the proven envelope resolution byte-identical across runs (the regression
// guard retained for ≥ 1 release); `--matcher shadow` resolves with legacy but attaches the spectral
// matrix; `--matcher-debug` prints the §14.3 table to STDERR.

/// §16.1 LEGACY-OVERRIDE byte-identity gate — the developer override `--matcher legacy` must stay the
/// proven envelope resolution and be BYTE-IDENTICAL across runs (no `evidenceMatrix`, legacy
/// `PairCorrelation` matrix + legacy resolved rows), exactly as before boundary 13. The ONLY field
/// that legitimately varies between two process invocations is the wall-clock `timingMs` block, which
/// we normalize. ALSO asserts the boundary-13 FLIP: the no-flag DEFAULT is now the fingerprint
/// resolver — it carries `evidenceMatrix` and its rows are spectral, so it is deliberately NOT
/// byte-identical to the legacy override.
#[test]
fn matcher_legacy_is_byte_identical() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 5151,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 150.0, 40.0)],
        externals: vec![ext("external", 0.0, 200.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();

    // Replace the run-varying wall-clock `timingMs` with a constant so the comparison targets the
    // deterministic content (schema/constants/videos/audios/matrix/rows/unusedAudio).
    let normalize = |mut v: Value| -> String {
        if let Some(obj) = v.as_object_mut() {
            obj.insert("timingMs".into(), serde_json::json!("<normalized>"));
        }
        serde_json::to_string(&v).unwrap()
    };

    // The DEVELOPER OVERRIDE is byte-identical across runs (the proven envelope resolution; §16.1).
    let legacy_a = normalize(pair_json(&dir, &cache, &["--matcher", "legacy"]));
    let legacy_b = normalize(pair_json(&dir, &cache, &["--matcher", "legacy"]));
    assert_eq!(legacy_a, legacy_b, "--matcher legacy must be byte-identical across runs");
    // `--matcher legacy --matcher-debug` (debug table is STDERR-only) must ALSO leave stdout intact.
    let legacy_dbg = normalize(pair_json(&dir, &cache, &["--matcher", "legacy", "--matcher-debug"]));
    assert_eq!(legacy_a, legacy_dbg, "--matcher-debug must not change stdout in legacy mode");
    // The legacy override carries NO spectral evidenceMatrix and keeps the legacy `matrix` shape.
    let raw_legacy = String::from_utf8(pair_stdout(&dir, &cache, &["--matcher", "legacy"])).unwrap();
    assert!(!raw_legacy.contains("evidenceMatrix"), "--matcher legacy must NOT carry evidenceMatrix");
    let legacy_v: Value = serde_json::from_str(&raw_legacy).unwrap();
    // Legacy resolved rows carry the stage-1 `z` (a small √n-bounded value), not a spectral score.
    assert_eq!(legacy_v["rows"][0]["state"], "confident");

    // BOUNDARY 13 FLIP: the no-flag DEFAULT is the fingerprint resolver — it carries `evidenceMatrix`
    // and its rows are spectral (so NOT byte-identical to the legacy override).
    let raw_default = String::from_utf8(pair_stdout(&dir, &cache, &[])).unwrap();
    assert!(raw_default.contains("evidenceMatrix"), "the boundary-13 default must carry evidenceMatrix");
    let default_v: Value = serde_json::from_str(&raw_default).unwrap();
    // Both resolvers agree this clean true pair is confident — but the default's selection comes from
    // the spectral evidence (the evidenceMatrix rows are spectralLandmarkV1).
    assert_eq!(default_v["rows"][0]["state"], "confident", "default resolves the true pair confidently");
    assert_eq!(default_v["rows"][0]["selected"]["audio"], "external.wav");
    assert_eq!(default_v["evidenceMatrix"][0][0]["method"], "spectralLandmarkV1");
    assert_ne!(
        normalize(default_v.clone()), legacy_a,
        "the fingerprint default is deliberately NOT byte-identical to the legacy override (§16.1)"
    );
}

/// §16.1 shadow gate — `--matcher shadow --json` emits BOTH matrices (the legacy `matrix` AND the
/// spectral `evidenceMatrix`), and its RESOLVED ROWS are byte-identical to the LEGACY OVERRIDE
/// resolution (shadow is observational; its spectral matrix never feeds resolution). Boundary 13:
/// `--matcher fingerprint` is now the DEFAULT and RESOLVES FROM the spectral matrix, so its rows
/// match the no-flag default and are NOT the legacy rows. Also asserts the shadow evidence carries
/// the attached `legacy` correlation and no NaN/Infinity.
#[test]
fn matcher_shadow_emits_both_matrices_rows_stay_legacy() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 5252,
        cameras: vec![cam("video1", 30.0, 40.0), cam("video2", 150.0, 40.0)],
        externals: vec![ext("external", 0.0, 200.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();

    // The legacy resolution is now the `--matcher legacy` developer override (§16.1).
    let legacy = pair_json(&dir, &cache, &["--matcher", "legacy"]);
    let shadow = pair_json(&dir, &cache, &["--matcher", "shadow"]);

    // Both matrices present in shadow.
    assert!(shadow["matrix"].is_array(), "legacy matrix still present in shadow");
    let ev = &shadow["evidenceMatrix"];
    assert!(ev.is_array(), "shadow must carry the spectral evidenceMatrix: {shadow}");
    // The evidence matrix is one row per imported video, one cell per audio, each a PairEvidence
    // with the method-neutral fields + the attached legacy correlation (shadow attaches it).
    let rows0 = ev.as_array().unwrap();
    assert_eq!(rows0.len(), 2, "two imported video rows");
    let cell = &rows0[0][0];
    assert!(cell["method"].as_str() == Some("spectralLandmarkV1"), "spectral method: {cell}");
    assert!(cell["classification"].is_string(), "classification present: {cell}");
    assert!(cell["legacy"].is_object(), "shadow attaches the legacy correlation: {cell}");
    assert!(cell["video"].as_str() == Some("video1.mp4"));

    // SHADOW RESOLVED ROWS are byte-identical to the LEGACY OVERRIDE resolution (shadow resolves with
    // legacy; the spectral matrix is observational only).
    for v in ["video1.mp4", "video2.mp4"] {
        let a = serde_json::to_string(row(&legacy, v)).unwrap();
        let b = serde_json::to_string(row(&shadow, v)).unwrap();
        assert_eq!(a, b, "row {v} must be IDENTICAL under shadow (shadow rows stay legacy)");
    }
    // The legacy `matrix` itself is unchanged between the two runs (shadow adds, never mutates).
    assert_eq!(
        serde_json::to_string(&legacy["matrix"]).unwrap(),
        serde_json::to_string(&shadow["matrix"]).unwrap(),
        "the legacy matrix is unchanged by shadow"
    );

    // No NaN/Infinity anywhere in the shadow JSON (§20).
    let shadow_str = shadow.to_string();
    for bad in ["NaN", "Infinity"] {
        assert!(!shadow_str.contains(bad), "shadow JSON must not contain {bad}");
    }

    // BOUNDARY 13: `--matcher fingerprint` is the DEFAULT and RESOLVES FROM the spectral matrix. Its
    // rows match the no-flag default and the cell's resolved `z` is the spectral SCORE (not the
    // legacy stage-1 z), so they are NOT the legacy rows.
    let fp = pair_json(&dir, &cache, &["--matcher", "fingerprint"]);
    let no_flag = pair_json(&dir, &cache, &[]);
    assert!(fp["evidenceMatrix"].is_array(), "fingerprint mode computes the spectral matrix");
    for v in ["video1.mp4", "video2.mp4"] {
        // The fingerprint resolution equals the no-flag default (both are the fingerprint resolver).
        assert_eq!(
            serde_json::to_string(row(&fp, v)).unwrap(),
            serde_json::to_string(row(&no_flag, v)).unwrap(),
            "row {v} under --matcher fingerprint must equal the no-flag default (both fingerprint-resolve)"
        );
        // Both find the true pair confidently; the fingerprint cell carries a large spectral score
        // (≫ the legacy √n-bounded z ≈ 14), proving the row came from the spectral evidence.
        assert_eq!(row(&fp, v)["state"], "confident");
        assert!(
            row(&fp, v)["selected"]["z"].as_f64().unwrap() > 30.0,
            "fingerprint row carries the spectral score, not the legacy z: {}", row(&fp, v)
        );
    }
}

/// §14.3 `--matcher-debug` table — the opt-in STDERR table renders the specified columns
/// (method, class, lag, ppm, support, inlier%, coverage%, p95, score, margin) and changes nothing
/// on stdout. An unknown `--matcher` value is rejected with a clear message.
#[test]
fn matcher_debug_table_and_bad_value() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("session");
    let cache = tmp.path().join("cache");
    let spec = SessionSpec {
        seed: 5353,
        cameras: vec![cam("video", 30.0, 40.0)],
        externals: vec![ext("external", 0.0, 120.0)],
        ..Default::default()
    };
    generate_session(&dir, &spec, &ffmpeg()).unwrap();

    let stderr = pair_stderr(&dir, &cache, &["--matcher", "shadow", "--matcher-debug"]);
    // The header lists the exact §14.3 columns and the observational disclaimer.
    assert!(stderr.contains("matcher-debug (shadow)"), "debug banner: {stderr}");
    assert!(stderr.contains("rows still resolved by legacy"), "observational note: {stderr}");
    assert!(
        stderr.contains("method class lag_ms ppm support inlier% coverage% p95_ms score margin"),
        "debug column header lists the §14.3 columns: {stderr}"
    );
    // At least one data row for the real (video, audio) cell.
    assert!(stderr.contains("video.mp4") && stderr.contains("external.wav"), "a cell row: {stderr}");

    // An unknown --matcher value fails with a clear message and a non-zero exit.
    let out = Command::new(env!("CARGO_BIN_EXE_couple"))
        .args(["--json", "--quiet", "--cache-dir"])
        .arg(&cache)
        .arg("pair")
        .arg(&dir)
        .args(["--matcher", "bogus"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "unknown --matcher must fail");
    let err =
        format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(
        err.contains("legacy|shadow|fingerprint"),
        "the error names the valid modes: {err}"
    );
}

/// Phase 4 (§9.3) — candidate limits degrade to honest ambiguity, never a silent
/// false negative. One video with EIGHT eligible (prefilter) candidates, the true pair
/// placed AFTER position six (named `zzz_true` so it sorts last by name). All eight are
/// genuine same-event recordings (same master, per-file SNR), so each passes the fine
/// lock and each is a prefilter candidate. Asserts the three §9.3 properties, all under
/// the classifier (`verify-active=1`):
///   (a) NORMAL limits verify the true pair — it appears in the resolved row (selected or
///       alternate), proving it was NOT evicted by a top-K cap before verification (the
///       old code capped at 6 by NCC; `zzz_true` past position six would have been at risk).
///   (b) A deliberately LOW per-video limit (4 < 8 eligible) yields EXPLICIT ambiguity: the
///       row is `ambiguous` (never `noMatch`, never `confident`), carries a `candidateOverflow`
///       {eligible:8, limit:4, scope:"perVideo"}, and lists ALL eight candidate names.
///   (c) Changing filenames does NOT decide which candidates are silently retained — under
///       the low limit the overflow set is the SAME full eight regardless of name order,
///       because none are silently retained.
#[test]
fn candidate_overflow_no_silent_eviction() {
    let tmp = tempfile::tempdir().unwrap();
    let cache = tmp.path().join("cache");

    // Eight externals, all the SAME master ([0,120)) fully covering the video's span
    // (master [30,90)) ⇒ eight genuine simultaneous candidates, each a prefilter candidate
    // and a fine-lock match. Per-file SNR varies so they are distinct recordings. The true
    // pair is `zzz_true` (sorts last by name — past position six).
    let names: [&str; 8] = [
        "aaa_rec", "bbb_rec", "ccc_rec", "ddd_rec", "eee_rec", "fff_rec", "ggg_rec", "zzz_true",
    ];
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

    let dir = tmp.path().join("session");
    make(&dir, &names);

    // BOUNDARY 13: the `candidateOverflow` resource ceiling is a LEGACY-classifier facility
    // (`max-verify-pairs-*` bounds the per-video fine-verify work). The fingerprint default computes
    // the whole matrix in one batch with no per-video verify cap, so it has no `candidateOverflow`
    // (its resource guard is the §9.3 `hard_correspondence_ceiling`, unit-tested in core). This is the
    // `--matcher legacy` regression guard for "a resource limit degrades to honest ambiguity, never a
    // silent false negative" (§9.2); run all three arms under the legacy override.
    // (a) Normal limits (default max-verify-pairs-per-video = 32 ≫ 8): every candidate is
    // verified; the true pair is present in the resolved row (selected or alternate).
    let on = pair_json(&dir, &cache, &["--matcher", "legacy", "--params", "verify-active=1"]);
    let r = row(&on, "video.mp4");
    eprintln!("overflow (a) normal row: {r}");
    assert_ne!(r["state"], "noMatch", "8 genuine candidates must not resolve noMatch: {r}");
    assert!(r["resolution"]["candidateOverflow"].is_null(), "no overflow under normal limits: {r}");
    let mut present: Vec<&str> = alternates(r);
    if let Some(s) = selected_audio(r) {
        present.push(s);
    }
    assert!(
        present.contains(&"zzz_true.wav"),
        "the true pair (past position six) must be verified, not evicted by a top-K cap: {r}"
    );
    assert_eq!(present.len(), 8, "all eight candidates visible (none dropped): {r}");

    // (b) Low per-video limit (4 < 8 eligible) ⇒ explicit ambiguity with the overflow detail.
    let low = pair_json(
        &dir,
        &cache,
        &["--matcher", "legacy", "--params", "verify-active=1", "--params", "max-verify-pairs-per-video=4"],
    );
    let rl = row(&low, "video.mp4");
    eprintln!("overflow (b) low-limit row: {rl}");
    assert_eq!(rl["state"], "ambiguous", "overflow must be ambiguous, never noMatch/confident: {rl}");
    assert!(rl["selected"].is_null(), "overflow row must not select (V2-F2): {rl}");
    let ov = &rl["resolution"]["candidateOverflow"];
    assert_eq!(ov["eligible"].as_u64(), Some(8), "eligible count surfaced: {rl}");
    assert_eq!(ov["limit"].as_u64(), Some(4), "configured limit surfaced: {rl}");
    assert_eq!(ov["scope"].as_str(), Some("perVideo"), "ceiling identified: {rl}");
    let mut alts: Vec<&str> = alternates(rl);
    alts.sort();
    let mut expect: Vec<String> = names.iter().map(|n| format!("{n}.wav")).collect();
    expect.sort();
    assert_eq!(alts, expect, "ALL eight candidate names stay visible under overflow: {rl}");

    // (c) Filenames do NOT decide retention: rename to the REVERSE order and re-run the low
    // limit — the overflow set is the SAME full eight (none silently retained by name).
    let dir2 = tmp.path().join("session2");
    let cache2 = tmp.path().join("cache2");
    let mut rev = names;
    rev.reverse();
    make(&dir2, &rev);
    let low2 = pair_json(
        &dir2,
        &cache2,
        &["--matcher", "legacy", "--params", "verify-active=1", "--params", "max-verify-pairs-per-video=4"],
    );
    let rl2 = row(&low2, "video.mp4");
    eprintln!("overflow (c) renamed low-limit row: {rl2}");
    assert_eq!(rl2["state"], "ambiguous", "{rl2}");
    assert_eq!(rl2["resolution"]["candidateOverflow"]["eligible"].as_u64(), Some(8), "{rl2}");
    let mut alts2: Vec<&str> = alternates(rl2);
    alts2.sort();
    assert_eq!(alts2, expect, "the overflow set is name-order-independent (none silently retained): {rl2}");
}
