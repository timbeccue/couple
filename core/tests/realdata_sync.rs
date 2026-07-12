//! §9.4 real-data validation: full pipeline on the example_data pair, the F2 offset
//! freeze procedure, and the §5.7 onset zone property.
//! #[ignore]-gated: default runs print "SKIPPED (no realdata)"; run via scripts/realdata-sweep.sh or --include-ignored.

mod common;

use couple_core::job::{noop_progress, CancelToken, ProgressEmitter};
use couple_core::sync::*;
use couple_core::TrackRole;
use serde::{Deserialize, Serialize};

fn cache_dir() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../.couple-cache")
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Expected {
    frozen_at: String,
    owner_verified: bool,
    offset_ms: f64,
    tolerance_ms: f64,
    drift_ppm: f64,
    engine_version: String,
    note: String,
}

#[test]
#[ignore = "SKIPPED (no realdata): set COUPLE_REALDATA_DIR and run scripts/realdata-sweep.sh (or -- --include-ignored)"]
fn real_pair_full_sync_and_freeze() {
    let dir = common::realdata_dir();
    let ctx = common::test_ctx(&cache_dir());
    let cam_id = common::import(&ctx, &dir.join("schumann-video.mp4"), TrackRole::Camera).unwrap();
    let ext_id = common::import(&ctx, &dir.join("schumann-audio.wav"), TrackRole::External).unwrap();
    let cam = ctx.track(cam_id).unwrap();
    let ext = ctx.track(ext_id).unwrap();

    let mut prog = ProgressEmitter::new(0, noop_progress());
    let r = auto_sync_tracks(
        &cam.pyramid,
        &ext.pyramid,
        &SyncParams::default(),
        &mut prog,
        &CancelToken::new(),
    )
    .unwrap();
    eprintln!(
        "real pair: offset {:.3} ms, drift {:.3} ppm, rms {:?}, z {:.1}, accepted {}/12",
        r.offset_ms,
        r.drift_ppm,
        r.fit_rms_ms,
        r.coarse.z,
        r.windows.iter().filter(|w| w.accepted).count()
    );

    // Epoch-invariant assertions, asserted unconditionally from day one (§9.4).
    assert!((r.drift_ppm - 3.07).abs() <= 0.75, "drift {} ppm", r.drift_ppm);
    let rms = r.fit_rms_ms.expect("fit present");
    assert!(rms <= 0.7, "fit rms {rms}");
    let accepted = r.windows.iter().filter(|w| w.accepted).count();
    assert!(accepted >= 8, "accepted {accepted}/12");
    assert!(matches!(r.status, SyncStatus::Synced), "{:?}", r.status);
    assert_eq!(
        r.suggested_drift_mode,
        couple_core::session::DriftMode::Resample
    );
    assert!(r.overlap.fraction_of_video >= 0.99, "overlap fraction {}", r.overlap.fraction_of_video);
    // The spurious 42 s boundary peak must not be chosen (§9.3): overlap ≈ 99.6 % of video.

    // Offset freeze procedure (F2): sanity window first, then frozen ± 1 ms.
    let expected_path = dir.join("expected.json");
    assert!(
        (r.offset_ms - (-5535.5)).abs() <= 25.0,
        "offset {} outside the §1.3 sanity window",
        r.offset_ms
    );
    if expected_path.is_file() {
        let exp: Expected =
            serde_json::from_slice(&std::fs::read(&expected_path).unwrap()).unwrap();
        assert!(
            (r.offset_ms - exp.offset_ms).abs() <= exp.tolerance_ms,
            "offset {} vs frozen {} (± {})",
            r.offset_ms,
            exp.offset_ms,
            exp.tolerance_ms
        );
    } else {
        let exp = Expected {
            frozen_at: "2026-06-11".into(),
            owner_verified: false,
            offset_ms: r.offset_ms,
            tolerance_ms: 1.0,
            drift_ppm: r.drift_ppm,
            engine_version: couple_core::version().into(),
            note: "Frozen by Session C per §9.4. Container-time offset; predicted \
                   −5535.5 ± 25 ms (§1.3). Owner: please verify by ear/loupe once the \
                   shell exists, then set ownerVerified=true."
                .into(),
        };
        std::fs::write(&expected_path, serde_json::to_vec_pretty(&exp).unwrap()).unwrap();
        eprintln!("FROZE offset {} ms into {}", exp.offset_ms, expected_path.display());
    }

    // Full auto-sync budget on the real pair (cache warm): trivially inside 10 s.
    assert!(r.compute_ms <= 10_000.0, "compute {} ms", r.compute_ms);
}

/// §5.7 onset zone property (from the UX provenance): in each anchor zone, the winning
/// onset is an attack-from-quiet — for ≥ 2 of the 3 zone winners, preceding level
/// < 0.25·s_max and local peak ≥ 0.6·s_max.
#[test]
#[ignore = "SKIPPED (no realdata): set COUPLE_REALDATA_DIR and run scripts/realdata-sweep.sh (or -- --include-ignored)"]
fn real_pair_onset_zone_property() {
    let dir = common::realdata_dir();
    let ctx = common::test_ctx(&cache_dir());
    let cam_id = common::import(&ctx, &dir.join("schumann-video.mp4"), TrackRole::Camera).unwrap();
    let cam = ctx.track(cam_id).unwrap();

    // Overlap on the video timeline (from the frozen/measured mapping ≈ [5.54, end]).
    let (t0, t1) = (5.54, cam.meta.duration_s);
    let span = t1 - t0;
    let zones = [(0.01, 0.30), (0.38, 0.62), (0.70, 0.99)];

    let spans = couple_core::envelope::spans(&cam.pyramid, 480, 0, cam.pyramid.n_bins(480));
    let s_max = *spans.iter().max().unwrap() as f64;
    let bin_s = 480.0 / 48_000.0;

    let mut good = 0;
    for (a, b) in zones {
        let onset = couple_core::onsets::best_in(&cam.onsets, t0 + a * span, t0 + b * span)
            .expect("zone winner");
        let i = (onset.t / bin_s).round() as usize;
        let prec_bins = &spans[i.saturating_sub(30)..i.saturating_sub(3).max(1)];
        let prec = prec_bins.iter().map(|&s| s as f64).sum::<f64>() / prec_bins.len() as f64;
        let peak = spans[i..(i + 30).min(spans.len())]
            .iter()
            .map(|&s| s as f64)
            .fold(0.0, f64::max);
        eprintln!(
            "zone {a:.2}-{b:.2}: onset t={:.2}s prec={:.1} ({:.2}·max) peak={:.1} ({:.2}·max)",
            onset.t,
            prec,
            prec / s_max,
            peak,
            peak / s_max
        );
        if prec < 0.25 * s_max && peak >= 0.6 * s_max {
            good += 1;
        }
    }
    assert!(good >= 2, "only {good}/3 zone winners are attacks-from-quiet");
}
