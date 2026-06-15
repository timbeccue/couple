//! PLAN-AUDIO-MATCHING-PREWORK §11.2 — cross-stack parity over the committed candidate-matrix
//! fixture (`fixtures/matcher-corpus/parity/candidate-matrix.json`), which was PRODUCED BY CORE
//! (`couple pair` over a synthetic session) plus two hand-authored edge cells.
//!
//! The Rust side of the parity contract: the CLI decodes every `PairCorrelation` field and its
//! flag-ON / flag-OFF candidacy reads the CORE-COMPUTED booleans (`prefilterCandidate`,
//! `legacyCandidate`), never a local numeric gate — byte-for-byte the same decision the resolver
//! (`cli/src/pair.rs`) and CoupleKit (`Resolver.swift`) make. The Swift mirror of this test lives
//! in `mac/Tests/CoupleKitTests` so `swift test` runs it too.

use couple_core::pair::PairCorrelation;
use serde::Deserialize;

/// The committed fixture's cell = a core `PairCorrelation` (flattened) plus the video/audio names.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatrixCell {
    #[allow(dead_code)]
    video: String,
    audio: String,
    #[serde(flatten)]
    corr: PairCorrelation,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MatrixFixture {
    schema: String,
    cells: Vec<MatrixCell>,
}

fn load_fixture() -> MatrixFixture {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("fixtures/matcher-corpus/parity/candidate-matrix.json");
    let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_slice(&raw).expect("candidate-matrix.json decodes into core PairCorrelation cells")
}

/// The flag-ON candidacy predicate, IDENTICAL to `cli/src/pair.rs` (the loose Stage-A pre-filter
/// reads `prefilter_candidate`) — no local threshold here.
fn candidate_flag_on(m: &PairCorrelation) -> bool {
    m.prefilter_candidate
}

/// The flag-OFF candidacy predicate, IDENTICAL to `cli/src/pair.rs` (the default z-gate against
/// the resolver z_min) — `z` is read, the boolean meaning matches `legacy_candidate`'s definition.
fn candidate_flag_off(m: &PairCorrelation, z_min: f64) -> bool {
    !m.insufficient_overlap && m.z >= z_min
}

// Every field decodes, and the core booleans carry the exact contract the resolver consumes.
#[test]
fn decodes_every_field_and_booleans() {
    let fx = load_fixture();
    assert_eq!(fx.schema, "couple-pair-matrix/1");
    assert_eq!(fx.cells.len(), 4, "fixture has four cells");

    let by = |name: &str| -> &PairCorrelation {
        &fx.cells.iter().find(|c| c.audio == name).expect("cell present").corr
    };

    // match.wav: a clean match ⇒ both booleans true.
    let m = by("match.wav");
    assert!(m.legacy_candidate && m.prefilter_candidate, "matched cell: both booleans true: {m:?}");
    assert!(m.refined && !m.insufficient_overlap);
    assert!(m.fine_ncc.is_some(), "a refined cell carries fineNcc");

    // unrelated.wav: the booleans DIVERGE (legacy false, prefilter true) — the cell that makes
    // this a real parity test (a local z gate would reject it; the prefilter admits it).
    let u = by("unrelated.wav");
    assert!(!u.legacy_candidate, "unrelated z below legacyZMin ⇒ legacyCandidate false: {u:?}");
    assert!(u.prefilter_candidate, "unrelated ncc clears the loose prefilter ⇒ prefilterCandidate true: {u:?}");

    // short_genuine.wav: the 8–20 s reachability cell — legacy false, prefilter true.
    let s = by("short_genuine.wav");
    assert!(!s.legacy_candidate && s.prefilter_candidate, "short genuine: legacy false, prefilter true: {s:?}");

    // tiny_clip.wav: below the floor ⇒ both false, fineNcc null.
    let t = by("tiny_clip.wav");
    assert!(t.insufficient_overlap && !t.legacy_candidate && !t.prefilter_candidate, "{t:?}");
    assert!(t.fine_ncc.is_none(), "an insufficient-overlap cell has no fineNcc");
}

// The flag-ON pre-filter selects by `prefilter_candidate`; the flag-OFF gate by z/`legacy`. On
// THIS fixture the two predicates select DIFFERENT candidate sets, proving the shell consumes the
// core booleans rather than re-deriving a single threshold.
#[test]
fn flag_on_and_off_select_via_core_booleans() {
    let fx = load_fixture();
    const Z_MIN: f64 = 8.5; // the resolver z_min (a resolution constant, mirrored in Swift PairParams)

    let on: Vec<&str> = fx
        .cells
        .iter()
        .filter(|c| candidate_flag_on(&c.corr))
        .map(|c| c.audio.as_str())
        .collect();
    let off: Vec<&str> = fx
        .cells
        .iter()
        .filter(|c| candidate_flag_off(&c.corr, Z_MIN))
        .map(|c| c.audio.as_str())
        .collect();

    // Flag ON admits every prefilter candidate (match + unrelated + short_genuine); tiny_clip out.
    assert_eq!(on, vec!["match.wav", "unrelated.wav", "short_genuine.wav"], "flag-ON prefilter set");
    // Flag OFF admits only the legacy z-passer (match); the others fall below z_min or the floor.
    assert_eq!(off, vec!["match.wav"], "flag-OFF legacy z-gate set");
    // The two sets DIFFER ⇒ the booleans are doing real work (not a tautology).
    assert_ne!(on, off, "flag-ON and flag-OFF must select different sets on this fixture");

    // The flag-ON membership equals each cell's own core `prefilter_candidate` — consumed, not
    // re-derived (PLAN §7.2 invariant 8).
    for c in &fx.cells {
        assert_eq!(
            candidate_flag_on(&c.corr),
            c.corr.prefilter_candidate,
            "flag-ON membership for {} must equal its core prefilterCandidate",
            c.audio
        );
    }
}
