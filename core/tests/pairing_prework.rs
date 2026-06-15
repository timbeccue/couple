//! PLAN-AUDIO-MATCHING-PREWORK Phase 0.3 + Phase 1 (§6.8) cross-cutting gates.
//!
//! - Determinism snapshot (§5.3): `pair_correlate` over a true and an unrelated pair
//!   serializes byte-identically across runs — protects §3 invariant 3 while later phases
//!   move parameters and window placement.
//! - Override visibly changes behavior (§6.8.4): the SAME path the FFI `*_params` symbols
//!   feed (`verify_pair` with a parsed `PairingParams`) changes its result when a param
//!   changes — proving a parameterized override drives the parameterized behavior.
//! - Source-search gate (§6.8.6): no verifier default literal may reappear in the CLI/Swift
//!   resolvers; core is the sole owner of those defaults (§2.2).

use couple_core::pair::pair_correlate;
use couple_core::pyramid::{PyramidBuilder, PyramidReader, SAMPLE_RATE};
use couple_core::verify::{is_match, verify_pair};
use couple_core::PairingParams;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const SR: f64 = SAMPLE_RATE as f64;
const PLAN_LO: f64 = -3.0;
const PLAN_HI: f64 = 120.0;

/// Deterministic broadband master keyed to the ABSOLUTE 48 kHz grid index, so any two
/// slices of the same seed agree where they overlap (mirrors the validated generator in
/// the core verify/sync unit tests). White-noise carrier × randomly-placed
/// exponential-decay bursts — sharp, well-timed features the envelope NCC locks onto.
fn master(seed: u64, t0: f64, n: usize) -> Vec<f32> {
    let mut rb = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0xb0_57));
    let mut bursts: Vec<(f64, f64, f64)> = Vec::new();
    let mut u = PLAN_LO;
    loop {
        u += -(rb.gen_range(1e-9..1.0f64)).ln() / 6.0;
        if u >= PLAN_HI {
            break;
        }
        bursts.push((u, rb.gen_range(0.010..0.060), rb.gen_range(0.4..1.0)));
    }
    let carrier = |idx: i64| {
        let mut z = (idx as u64)
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(seed.wrapping_mul(0xD1B5_4A32_D192_ED03));
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        ((z >> 40) as f32 / (1u32 << 24) as f32) * 2.0 - 1.0
    };
    let base = (t0 * SR).round() as i64;
    (0..n)
        .map(|k| {
            let idx = base + k as i64;
            let t = idx as f64 / SR;
            let mut env = 0.02f64;
            for &(o, tau, a) in &bursts {
                if o <= t && t - o < 8.0 * tau {
                    let dt = t - o;
                    env += a * (1.0 - (-dt / 0.0005).exp()) * (-dt / tau).exp();
                }
            }
            (carrier(idx) as f64 * env).clamp(-0.999, 0.999) as f32
        })
        .collect()
}

fn pyramid_of(samples: &[f32], dir: &tempfile::TempDir, name: &str) -> PyramidReader {
    let mut b = PyramidBuilder::new();
    b.push(samples);
    let fin = b.finalize();
    let path = dir.path().join(name);
    fin.write(&path).expect("write pyramid");
    PyramidReader::open(&path).expect("open pyramid")
}

// Phase 0.3 — `pair_correlate` is byte-stable across runs for both a true 30 s pair and an
// unrelated pair, so the snapshot protects determinism while later phases move params.
#[test]
fn determinism_pair_correlate_byte_stable() {
    let dir = tempfile::tempdir().unwrap();
    let params = PairingParams::default();

    // 30 s true pair (same master, known lag).
    let lag_ms = 1200.0;
    let off_s = lag_ms / 1000.0;
    let p_cam = pyramid_of(&master(7, 0.0, (30.0 * SR) as usize), &dir, "cam.cpk");
    let p_ext = pyramid_of(&master(7, -off_s, (35.0 * SR) as usize), &dir, "ext.cpk");

    // Unrelated pair (different masters, both long enough to scan).
    let p_cam2 = pyramid_of(&master(7, 0.0, (30.0 * SR) as usize), &dir, "cam2.cpk");
    let p_ext2 = pyramid_of(&master(999, 0.0, (35.0 * SR) as usize), &dir, "ext2.cpk");

    for (a, b, label) in [
        (&p_cam, &p_ext, "true"),
        (&p_cam2, &p_ext2, "unrelated"),
    ] {
        let j1 = serde_json::to_string(&pair_correlate(a, b, &params)).unwrap();
        let j2 = serde_json::to_string(&pair_correlate(a, b, &params)).unwrap();
        assert_eq!(
            j1, j2,
            "pair_correlate {label} pair must serialize byte-identically across runs"
        );
    }

    // Sanity: the snapshot is of a real measurement, not a degenerate insufficient-overlap cell.
    let m = pair_correlate(&p_cam, &p_ext, &params);
    assert!(!m.insufficient_overlap, "true 30 s pair has scannable overlap");
}

/// Bins → seconds on the 9600-spb stage-1 grid (one bin = 200 ms).
const BIN_S: f64 = 9600.0 / SR;

/// Build a true pair (same master) with EXACTLY `overlap_bins` of stage-1 overlap at a known
/// integer-bin lag. The stage-1 envelope quantizes to 200 ms bins, so the overlap floor and
/// every test overlap are expressed in whole bins (the only granularity the floor can resolve):
/// the default 8.0 s floor is `ceil(8.0 · 48000 / 9600) = 40` bins, so 39 bins (7.8 s) is the
/// largest sub-floor overlap and 40 bins (8.0 s) is exactly the floor.
///
/// Geometry: at lag `lag_bins` (> 0) A is `overlap_bins` long and B is `overlap_bins + lag_bins`
/// long, so the overlap interval is `[0, overlap_bins)` = exactly `overlap_bins`. Content matches
/// there (lag convention: A-content at t shows at t + off in B), and `lag_ms` is exact because
/// the offset is a whole number of 200 ms bins.
fn true_pair_bins(
    dir: &tempfile::TempDir,
    seed: u64,
    overlap_bins: u64,
    lag_bins: u64,
    tag: &str,
) -> (PyramidReader, PyramidReader, f64) {
    let lag_ms = lag_bins as f64 * BIN_S * 1000.0;
    let off_s = lag_ms / 1000.0;
    let n_a = (overlap_bins * 9600) as usize;
    let n_b = ((overlap_bins + lag_bins) * 9600) as usize;
    let p_a = pyramid_of(&master(seed, 0.0, n_a), dir, &format!("a_{tag}.cpk"));
    let p_b = pyramid_of(&master(seed, -off_s, n_b), dir, &format!("b_{tag}.cpk"));
    (p_a, p_b, lag_ms)
}

// §7.4 — short overlaps are mechanically reachable. Generated true pairs bracketing the default
// 8.0 s pairing floor (= 40 stage-1 bins) and spanning the 8–20 s band the legacy 20 s (100-bin)
// floor used to reject. Because the stage-1 envelope is quantized to 200 ms bins, the floor can
// only resolve whole bins, so the plan's 7.9/8.0/…/20.0 s points are realized on that grid: 39
// bins (7.8 s, the largest sub-floor overlap) is insufficient; 40 (8.0 s), 60 (12.0 s), 99
// (19.8 s) and 100 (20.0 s) bins are scannable. Asserts: sub-floor ⇒ insufficient; floor and up
// ⇒ scannable; a genuine 8–20 s pair gets a refined lag once its NCC clears the prefilter; legacy
// candidacy stays z-governed (z < legacy_z_min here ⇒ the default-off resolver gate is unchanged).
#[test]
fn short_overlap_reachable_phase2() {
    let dir = tempfile::tempdir().unwrap();
    let params = PairingParams::default();
    let lag_bins = 6; // 6 · 200 ms = 1200 ms — content at t in A shows at t+1.2 s in B
    // The §3.3/§9.3 clean-fixture offset tolerance (the gates use 2.0 ms; core sync lands
    // sub-ms). The refined ladder must land the short-pair lag inside this.
    const LAG_TOL_MS: f64 = 2.0;
    let floor_bins = ((params.min_overlap_s * SR / 9600.0).ceil() as u64).max(2);
    assert_eq!(floor_bins, 40, "default 8.0 s floor is 40 stage-1 bins");

    // 39 bins (7.8 s) < the 40-bin (8.0 s) floor ⇒ insufficient: no scannable lag, no
    // refinement, neither candidacy. (A's 39-bin length trips the duration floor directly.)
    let (a_sub, b_sub, lag_ms) = true_pair_bins(&dir, 7, floor_bins - 1, lag_bins, "sub");
    let m_sub = pair_correlate(&a_sub, &b_sub, &params);
    assert!(
        m_sub.insufficient_overlap,
        "39-bin (7.8 s) overlap is below the 8.0 s pairing floor ⇒ insufficient: {m_sub:?}"
    );
    assert!(!m_sub.refined && !m_sub.prefilter_candidate && !m_sub.legacy_candidate, "{m_sub:?}");

    // 40 (8.0 s) / 60 (12.0 s) / 99 (19.8 s) / 100 (20.0 s) bins ⇒ all scannable; the genuine
    // pair refines and clears the loose prefilter, but z (≤ NCC·√bins, ~6–10 here) stays below
    // legacy_z_min = 8.5 at the short end, so legacy candidacy does NOT silently turn on.
    for &ov in &[floor_bins, 60, 99, 100] {
        let (a, b, _) = true_pair_bins(&dir, 7, ov, lag_bins, &format!("ov{ov}"));
        let m = pair_correlate(&a, &b, &params);
        let secs = ov as f64 * BIN_S;
        assert!(!m.insufficient_overlap, "{ov} bins ({secs:.1} s) must be scannable: {m:?}");
        assert!(
            m.prefilter_candidate && m.refined,
            "genuine {ov}-bin ({secs:.1} s) pair clears the NCC prefilter and refines: {m:?}"
        );
        assert!(
            (m.lag_ms - lag_ms).abs() <= LAG_TOL_MS,
            "{ov}-bin refined lag {:.3} ms off true {lag_ms} ms (tol {LAG_TOL_MS}): {m:?}",
            m.lag_ms
        );
        // Legacy candidacy is z-governed and matches its definition exactly (invariant 10) —
        // the §7.2 guarantee that the default-off resolver gate is unchanged. `z` itself is
        // untouched by Phase 2.
        assert_eq!(m.legacy_candidate, m.z >= params.legacy_z_min, "{m:?}");
        // At the genuinely-short end (≤ 60 bins / 12 s), even this "too clean" synthetic
        // content (NCC ≈ 1.0) caps z = NCC·√n below legacy_z_min = 8.5, so the floor change
        // does NOT silently create a legacy candidate there. (Above ~72 bins the synthetic
        // NCC ≈ 1.0 does clear 8.5 — a fixture artifact, NOT a Phase-2 change: `z` and
        // `legacy_z_min` are identical to before; the real-content NCC of 0.6–0.8 keeps these
        // mid-length pairs well under the gate. The end-to-end guard is the flag-OFF pair_gates
        // staying byte-identical, asserted there.)
        if ov <= 60 {
            assert!(
                !m.legacy_candidate,
                "{ov}-bin pair must NOT become a legacy candidate (z {:.2} < {}): {m:?}",
                m.z, params.legacy_z_min
            );
        }
    }

    // The verifier can now RECEIVE an 8–20 s candidate (its flag, not this floor, governs
    // whether it runs): a 60-bin (12 s) genuine pair clears verify's own 8 s hard floor and locks.
    let (a12, b12, _) = true_pair_bins(&dir, 7, 60, lag_bins, "v12");
    let v = verify_pair(&a12, &b12, lag_ms, &params)
        .expect("12 s genuine pair clears the verifier's 8 s hard floor");
    assert!(
        is_match(&v),
        "a 12 s genuine same-master pair must verify as a match once reachable: {v:?}"
    );
    assert!(
        (v.off_ms - lag_ms).abs() <= LAG_TOL_MS,
        "verified 12 s offset {:.3} ms off true {lag_ms} ms: {v:?}",
        v.off_ms
    );

    // §7.3 anchor: the verifier's own absolute floor is still 8 s — a 7.8 s overlap returns
    // None (no candidate), independent of the stage-1 floor change.
    let (a_sv, b_sv, _) = true_pair_bins(&dir, 7, floor_bins - 1, lag_bins, "vsub");
    assert!(
        verify_pair(&a_sv, &b_sv, lag_ms, &params).is_none(),
        "7.8 s overlap is below verify_min_overlap_s ⇒ no verifier candidate"
    );
}

// §6.8.4 — a PairingParams override visibly changes `verify_pair`'s result. This is the exact
// path the FFI `cpl_*_params` symbols feed (parse+validate a PairingParams, then
// ctx.verify_batch → verify_pair), so a parameterized override demonstrably changes the
// parameterized behavior.
#[test]
fn override_visibly_changes_verify() {
    let dir = tempfile::tempdir().unwrap();
    let lag_ms = 1500.0;
    let off_s = lag_ms / 1000.0;
    // ~25 s same-master pair: a clean lock under the defaults.
    let p_cam = pyramid_of(&master(7, 0.0, (25.0 * SR) as usize), &dir, "cam.cpk");
    let p_ext = pyramid_of(&master(7, -off_s, (30.0 * SR) as usize), &dir, "ext.cpk");

    let base = verify_pair(&p_cam, &p_ext, lag_ms, &PairingParams::default())
        .expect("overlap clears the 8 s floor under defaults");
    assert!(is_match(&base), "clean 25 s same-master pair matches under defaults: {base:?}");

    // (1) Raise the overlap floor above the clip ⇒ verify_pair returns None (no candidate).
    let high_floor = PairingParams { min_overlap_s: 100.0, ..PairingParams::default() };
    assert!(
        verify_pair(&p_cam, &p_ext, lag_ms, &high_floor).is_none(),
        "min_overlap_s override above the overlap must drop the candidate"
    );

    // (2) Tighten the RMS trust floor to 0 ⇒ the SAME clean lock is no longer a match.
    let strict_rms = PairingParams { verify_rms_max_ms: 0.0, ..PairingParams::default() };
    let v = verify_pair(&p_cam, &p_ext, lag_ms, &strict_rms).expect("overlap still clears floor");
    assert!(!is_match(&v), "verify_rms_max_ms=0 override must reject the lock: {v:?}");
}

// §8.5(4) — a majority of SCATTERED points must NOT be rescued into matched=true by the
// robust fit. A different-master pair at a plausible coarse lag locks windows whose per-window
// lags scatter (no common line); the §8.2 MAD rejection cannot conjure a tight line from
// noise, so either too few inliers survive (rms = +∞) or the lock_fraction/rms gate fails.
// The honesty guarantee (invariant 7): robust rejection tightens the predicate, it never
// loosens it — dropping outliers cannot turn a scatter into a confident match.
#[test]
fn scattered_majority_not_matched_phase3() {
    let dir = tempfile::tempdir().unwrap();
    let dur = 40.0; // well over the 8 s floor; plenty of windows
    let lag_ms = 1000.0;
    // Different masters ⇒ the locked per-window lags do not share a line (scatter).
    let p_a = pyramid_of(&master(7, 0.0, (dur * SR) as usize), &dir, "scat_a.cpk");
    let p_b = pyramid_of(&master(54321, 0.0, (dur * SR) as usize), &dir, "scat_b.cpk");
    let v = verify_pair(&p_a, &p_b, lag_ms, &PairingParams::default())
        .expect("40 s overlap clears the 8 s hard floor");
    assert!(
        !is_match(&v),
        "a scattered different-master majority must not be matched (robust fit cannot rescue it): {v:?}"
    );
    // And the robust survivors never exceed the raw locked set (inliers ⊆ locked, §8.4).
    assert!(v.inliers <= v.locked, "inliers {} must not exceed locked {}: {v:?}", v.inliers, v.locked);
}

// §8.5(7) — a clean true pair's verifier values stay within the documented tolerances after
// the Phase-3 robust rewrite: offset within the §3.3 clean-fixture tolerance (2 ms), a tight
// sub-bin RMS, in-bound drift, and ≥ verify_min_locked robust inliers all agreeing. This is
// the "true-pair decisions hold" anchor — for a clean pair every locked window IS an inlier,
// so robust rejection leaves the numbers essentially unchanged.
#[test]
fn clean_pair_values_within_tolerance_phase3() {
    let dir = tempfile::tempdir().unwrap();
    let p = PairingParams::default();
    let lag_ms = 1500.0;
    let off_s = lag_ms / 1000.0;
    // 30 s contained in 40 s, same master ⇒ a clean full lock.
    let p_a = pyramid_of(&master(7, 0.0, (30.0 * SR) as usize), &dir, "clean_a.cpk");
    let p_b = pyramid_of(&master(7, -off_s, (40.0 * SR) as usize), &dir, "clean_b.cpk");
    let v = verify_pair(&p_a, &p_b, lag_ms, &p).expect("clean pair clears the floor");
    assert!(is_match(&v), "clean 30 s same-master pair must match: {v:?}");
    assert!((v.off_ms - lag_ms).abs() <= 2.0, "offset {:.3} off true {lag_ms} (tol 2 ms): {v:?}", v.off_ms);
    assert!(v.rms_ms < 5.0, "clean-pair rms {:.3} ms should be tight: {v:?}", v.rms_ms);
    assert!(!v.clamped, "clean-pair drift must be in bound: {v:?}");
    assert!(v.inliers >= p.verify_min_locked, "clean pair must keep ≥ verify_min_locked inliers: {v:?}");
    assert!(v.inliers == v.locked, "on a clean pair every locked window is an inlier: {v:?}");
    assert!(v.lock_fraction >= p.verify_lock_fraction_min, "clean lock_fraction {:.3}: {v:?}", v.lock_fraction);
    // p95 of the inlier residuals is finite and small; temporal coverage spans the lock. The
    // centers fill the feasible interval [hw, dur−hw−margin]; with hw ≈ overlap/4 that span is
    // ≈ half the overlap, so coverage lands at ~0.5 for a contained clip (telemetry only, §8.3).
    assert!(v.residual_p95_ms.is_finite() && v.residual_p95_ms < 10.0, "p95 {:.3}: {v:?}", v.residual_p95_ms);
    assert!(v.temporal_coverage >= 0.5, "inliers should span ≥ half the overlap: {v:?}");
}

// §6.8.6 — verifier default literals must NOT reappear in the CLI/Swift resolvers; core is the
// sole owner of those defaults (§2.2). A curated set of DISTINCTIVE verifier defaults — chosen
// to NOT collide with kept resolver values (z 8.5, z-margin 6.0, triangle 250.0, coverage
// 0.98, disjoint 0.5, rms-trust 9.0) — acts as a tripwire: re-hardcoding the verifier params
// would bring several of these back.
#[test]
fn no_verifier_default_literals_in_resolvers() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root");
    let files = [
        root.join("mac/Sources/Couple/Resolver.swift"),
        root.join("cli/src/pair.rs"),
    ];
    // verify_rms_max_ms=12.0, verify_lock_fraction_min=0.66, drift_max_ppm=600.0,
    // refine_ncc_min=0.15, verify_ncc_window_min=0.25 — now owned ONLY by core PairingParams.
    let banned = ["12.0", "0.66", "600.0", "0.15", "0.25"];
    for f in &files {
        let src = std::fs::read_to_string(f)
            .unwrap_or_else(|e| panic!("read {}: {e}", f.display()));
        for lit in banned {
            assert!(
                !src.contains(lit),
                "verifier default literal `{lit}` reappeared in {} — it must live ONLY in core \
                 PairingParams (§6.8.6); the resolver reads core defaults via \
                 pairingParamsDefault()/PairParams.pairing",
                f.display()
            );
        }
    }
}
