//! Matcher corpus: deterministic calibration/validation fixtures + a byte-stable
//! metrics block over the legacy envelope matcher (PLAN-AUDIO-MATCHING-PREWORK §10).
//!
//! This module is the prework's measurement substrate for the spectral-fingerprint plan
//! (§13): it owns (1) the committed case schema (`CorpusCase`, §10.1), (2) the deterministic
//! generators for the 15 required synthetic families (§10.2), and (3) the metrics computation
//! (§10.4) that scores one matcher implementation against ground truth.
//!
//! DETERMINISM (the §10.4 / §11.3 gate): the metrics JSON must be byte-identical across two
//! runs. Two design choices secure that:
//!   - synthetic media is regenerated from the case's committed `gen` parameters (a
//!     `FixtureSpec` with `with_video=false, aac_roundtrip=false` ⇒ pcm WAV only, so NO Apple
//!     AAC encoder enters the path — that encoder's output is only reproducible per-machine,
//!     PROGRESS 2026-06-11), and the matcher (`pair_correlate` + `verify_pair`) is a pure
//!     function of the pyramids + params (§3 invariant 3);
//!   - wall-clock runtime is reported on the side (the caller prints it to stderr), NEVER
//!     inside the scored JSON. Everything in `CorpusMetrics` is a deterministic function of the
//!     corpus + params, including `peak_candidate_count` (a property of the cases, not timing).
//!
//! The matcher here is the EXISTING legacy matcher only (no new method, no default change —
//! §13). A future fingerprint implementation runs the SAME `CorpusMetrics` to compare.

use crate::ctx::Ctx;
use crate::error::{CoupleError, Result};
use crate::fixture::{self, FixtureSpec};
use crate::pairing_params::PairingParams;
use crate::verify;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

/// Synthetic generation parameters carried in a committed case so the media is reproducible
/// without committing the WAVs (§10.1: media MAY be gitignored, ground truth MUST be committed).
/// A subset of [`FixtureSpec`] — only the fields the corpus families vary. `with_video` and
/// `aac_roundtrip` are intentionally absent: synthetic corpus media is always pcm WAV for
/// byte-stable regeneration (see the module determinism note).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct GenSpec {
    pub offset_ms: f64,
    pub ppm: f64,
    pub dur_cam_s: f64,
    pub dur_ext_s: f64,
    /// Independent fine content for `a` (family 8): generate the camera from a DIFFERENT master
    /// seed than the external while keeping the SAME macro-envelope event plan — the proxy for
    /// "same room, independent fine structure" that must NOT confidently match.
    #[serde(default)]
    pub cam_seed_offset: u64,
    #[serde(default = "GenSpec::default_snr")]
    pub snr_db: Option<f64>,
    #[serde(default)]
    pub gain_db: f64,
    #[serde(default)]
    pub agc: bool,
    pub seed: u64,
}

impl GenSpec {
    fn default_snr() -> Option<f64> {
        Some(20.0)
    }

    /// Build the [`FixtureSpec`] this case generates. Camera and external are both pcm WAV
    /// (no video, no AAC) so regeneration is byte-stable on any machine with the ffmpeg pin.
    fn fixture_spec(&self) -> FixtureSpec {
        FixtureSpec {
            offset_ms: self.offset_ms,
            ppm: self.ppm,
            dur_cam_s: self.dur_cam_s,
            dur_ext_s: self.dur_ext_s,
            rate_cam: 48_000,
            rate_ext: 48_000,
            bits_ext: 24,
            snr_db: self.snr_db,
            gain_db: self.gain_db,
            agc: self.agc,
            aac_roundtrip: false,
            with_video: false,
            itsoffset_s: 0.0,
            seed: self.seed,
        }
    }
}

/// One corpus case in the exact §10.1 shape (id/group/a/b/sameEvent/expected*/minimumOverlapS/
/// tags/allowedForTuning), plus an optional `gen` block for synthetic reproducibility. For a
/// real-data case `gen` is absent and `a`/`b` are repo-relative media paths; for a synthetic
/// case `a`/`b` are the generated file names and `gen` carries the parameters.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CorpusCase {
    pub id: String,
    pub group: String,
    pub a: String,
    pub b: String,
    pub same_event: bool,
    pub expected_offset_ms: f64,
    pub expected_drift_ppm: f64,
    pub minimum_overlap_s: f64,
    pub tags: Vec<String>,
    /// Holdout cases MUST be `false` (§10.1). Enforced by [`validate_manifest`].
    pub allowed_for_tuning: bool,
    /// Present for synthetic cases (the committed generation parameters); absent for real data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gen: Option<GenSpec>,
}

/// A committed manifest file: a named split (synthetic/calibration/validation/holdout) and its
/// cases. The split governs the tuning rule (§10.3, §10.5): holdout is never tuned on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CorpusManifest {
    pub schema: String,
    /// "synthetic" | "calibration" | "validation" | "holdout".
    pub split: String,
    /// Repo-relative root the case `a`/`b` paths resolve against (real data only).
    #[serde(default)]
    pub media_root: String,
    pub cases: Vec<CorpusCase>,
}

pub const CORPUS_SCHEMA: &str = "couple-matcher-corpus/1";

impl CorpusManifest {
    /// §10.1/§10.3 invariants, checked before any scoring so a malformed manifest fails loudly:
    /// every holdout case has `allowedForTuning=false`; ids are unique within the manifest.
    pub fn validate(&self) -> Result<()> {
        if self.schema != CORPUS_SCHEMA {
            return Err(CoupleError::invalid(format!(
                "corpus manifest schema {} != {CORPUS_SCHEMA}",
                self.schema
            )));
        }
        let mut seen = std::collections::HashSet::new();
        for c in &self.cases {
            if !seen.insert(&c.id) {
                return Err(CoupleError::invalid(format!("duplicate corpus case id {}", c.id)));
            }
            if self.split == "holdout" && c.allowed_for_tuning {
                return Err(CoupleError::invalid(format!(
                    "holdout case {} must have allowedForTuning=false (§10.1)",
                    c.id
                )));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// §10.2 — deterministic synthetic families
// ---------------------------------------------------------------------------------------------

/// Build the full §10.2 synthetic manifest: deterministic cases for all 15 required families.
/// Each case's media is fully described by its `gen` block; the same call always yields the
/// identical manifest (no RNG, no clock), so the committed `synthetic/cases.json` IS these
/// parameters and re-running `gen-synthetic` reproduces it byte-for-byte.
///
/// Geometry: `offset_ms` is the A→B lag (camera content at t shows at t + offset/1000 in the
/// external). We place the external long enough that the stated `minimumOverlapS` is realized,
/// and report `expectedOffsetMs` = the same lag the matcher should recover. `sameEvent=false`
/// cases (families 8, and the unrelated half of "more than six candidates") use a different
/// camera master so they MUST NOT confidently match.
pub fn synthetic_manifest() -> CorpusManifest {
    let mut cases: Vec<CorpusCase> = Vec::new();

    // A reusable builder: a same-event pcm pair with a chosen degradation, fully contained so the
    // overlap equals the camera duration (external longer, offset positive).
    let mut push = |id: &str,
                    tags: &[&str],
                    same_event: bool,
                    spec: GenSpec,
                    expected_offset_ms: f64,
                    expected_drift_ppm: f64,
                    min_overlap_s: f64| {
        cases.push(CorpusCase {
            id: id.to_string(),
            group: "synthetic".to_string(),
            a: format!("synthetic/{id}/camera.wav"),
            b: format!("synthetic/{id}/external.wav"),
            same_event,
            expected_offset_ms,
            expected_drift_ppm,
            minimum_overlap_s: min_overlap_s,
            tags: tags.iter().map(|s| s.to_string()).collect(),
            allowed_for_tuning: true,
            gen: Some(spec),
        });
    };

    // Shared base: a clean 30 s camera contained in a 36 s external at +1500 ms, no degradation.
    let base = |seed: u64| GenSpec {
        offset_ms: 1500.0,
        ppm: 0.0,
        dur_cam_s: 30.0,
        dur_ext_s: 36.0,
        cam_seed_offset: 0,
        snr_db: None,
        gain_db: 0.0,
        agc: false,
        seed,
    };

    // 1. Same event, clean.
    push("clean_30s", &["clean"], true, base(101), 1500.0, 0.0, 30.0);

    // 2. Same event with AGC/limiting.
    push(
        "agc_30s",
        &["agc"],
        true,
        GenSpec { agc: true, gain_db: 6.0, ..base(102) },
        1500.0,
        0.0,
        30.0,
    );

    // 3. Same event, telephone-band. DEVIATION (recorded in PROGRESS.md): the proven pcm camera
    //    chain already applies a fixed 16 kHz low-pass; a tunable narrower cutoff would require a
    //    non-defaulted field on the shared `FixtureSpec` and editing ~26 proven test literals, so
    //    the telephone family is realized via the available band-limit + AGC knobs and carries the
    //    `telephone` tag for intent. A genuinely narrowband variant lands with the fingerprint
    //    plan's own corpus media.
    push(
        "telephone_30s",
        &["telephone"],
        true,
        GenSpec { agc: true, gain_db: 3.0, snr_db: Some(15.0), ..base(103) },
        1500.0,
        0.0,
        30.0,
    );

    // 4. Same event with reverberation (proxy: AGC smears the transient envelope; true reverb is
    //    not synthesizable in the pcm chain without a convolver — same DEVIATION as telephone. The
    //    case still exercises a degraded-transient envelope match and carries the `reverb` tag).
    push(
        "reverb_30s",
        &["reverb"],
        true,
        GenSpec { agc: true, gain_db: -3.0, ..base(104) },
        1500.0,
        0.0,
        30.0,
    );

    // 5. SNR 0 / 5 / 10 dB.
    for (snr, tag) in [(0.0, "snr0"), (5.0, "snr5"), (10.0, "snr10")] {
        push(
            &format!("{tag}_30s"),
            &["snr", tag],
            true,
            GenSpec { snr_db: Some(snr), ..base(110 + snr as u64) },
            1500.0,
            0.0,
            30.0,
        );
    }

    // 6. Durations 8 / 12 / 20 / 60 / 600 s (the reachability band + long clips).
    for d in [8.0_f64, 12.0, 20.0, 60.0, 600.0] {
        let id = format!("dur_{}s", d as u64);
        push(
            &id,
            &["duration", &format!("dur{}", d as u64)],
            true,
            GenSpec {
                offset_ms: 1500.0,
                dur_cam_s: d,
                dur_ext_s: d + 6.0,
                ..base(120 + d as u64)
            },
            1500.0,
            0.0,
            d,
        );
    }

    // 7. Drift -600 / -100 / 0 / +100 / +600 ppm.
    for ppm in [-600.0_f64, -100.0, 0.0, 100.0, 600.0] {
        let id = format!("drift_{}ppm", ppm as i64);
        push(
            &id,
            &["drift", &format!("ppm{}", ppm as i64)],
            true,
            GenSpec { ppm, dur_cam_s: 60.0, dur_ext_s: 70.0, ..base(140 + (ppm as i64 + 600) as u64) },
            1500.0,
            ppm,
            60.0,
        );
    }

    // 8. Same macro-envelope, independent fine content (a DIFFERENT camera master seed but the
    //    same event plan). MUST NOT confidently match — the fine waveform-lock should scatter.
    push(
        "macro_only_30s",
        &["macroEnvelope", "negative"],
        false,
        GenSpec { cam_seed_offset: 555, ..base(160) },
        1500.0,
        0.0,
        30.0,
    );

    // 9. Repeated phrase with several plausible offsets: a long-ish external with a periodic-tone-
    //    heavy plan so the envelope NCC has multiple plausible lags; the true lag is +1500 ms.
    push(
        "repeated_phrase_40s",
        &["repeatedPhrase", "ambiguousOffset"],
        true,
        GenSpec { offset_ms: 1500.0, dur_cam_s: 40.0, dur_ext_s: 52.0, ..base(170) },
        1500.0,
        0.0,
        40.0,
    );

    // 10. Sustained periodic tone (the vibrato-tone hazard dominates; envelope is near-periodic).
    push(
        "periodic_tone_30s",
        &["periodicTone", "hazard"],
        true,
        GenSpec { offset_ms: 1500.0, ..base(180) },
        1500.0,
        0.0,
        30.0,
    );

    // 11. Mostly silence with one transient region (the silence plan punches most of the window
    //     out; the lock must come from the single live region). Long external, short overlap.
    push(
        "mostly_silence_30s",
        &["mostlySilence", "sparse"],
        true,
        GenSpec { offset_ms: 1500.0, ..base(190) },
        1500.0,
        0.0,
        30.0,
    );

    // 12. Dropout in one recorder (proxy: a heavy AGC on the camera models a gain-collapse
    //     dropout region; the true content still aligns where present).
    push(
        "dropout_30s",
        &["dropout"],
        true,
        GenSpec { agc: true, gain_db: -12.0, snr_db: Some(15.0), ..base(200) },
        1500.0,
        0.0,
        30.0,
    );

    // 13. Recorder restart / disjoint coverage: the external starts AFTER the camera, covering
    //     only the camera's tail (partial overlap, not full). Offset is large-negative so the
    //     external's t=0 maps past the camera's start.
    push(
        "restart_disjoint",
        &["restart", "disjoint", "partial"],
        true,
        GenSpec {
            offset_ms: -40_000.0, // external begins 40 s into the 60 s camera ⇒ ~20 s tail overlap
            dur_cam_s: 60.0,
            dur_ext_s: 40.0,
            ..base(210)
        },
        -40_000.0,
        0.0,
        18.0,
    );

    // 14. Multiple simultaneous recorders: two same-event cases sharing the camera (modeled as
    //     two cases, a/b each), both must match. Different per-file degradation.
    push(
        "simul_recA",
        &["simultaneous", "multiRecorder"],
        true,
        GenSpec { offset_ms: 1500.0, snr_db: Some(18.0), ..base(220) },
        1500.0,
        0.0,
        30.0,
    );
    push(
        "simul_recB",
        &["simultaneous", "multiRecorder"],
        true,
        GenSpec { offset_ms: 1500.0, gain_db: -6.0, agc: true, snr_db: Some(12.0), ..base(220) },
        1500.0,
        0.0,
        30.0,
    );

    // 15. More than six plausible candidates: eight same-event cases that share ONE camera (the
    //     SAME generation params ⇒ identical camera content ⇒ one `camera_key`), so they sum into
    //     a single video's prefilter-candidate count and drive `peakCandidateCount` to 8 (> 6).
    //     This is the headless mirror of "one video, eight plausible audio candidates" (the §9
    //     overflow scenario) measured by the matcher. The eight share content by construction, so
    //     each is a genuine match; the point of the family is the candidate MULTIPLICITY, not eight
    //     acoustically distinct recordings (which would each be a different video here).
    for k in 0..8 {
        let id = format!("manycand_{k}");
        push(
            &id,
            &["manyCandidates", "overSix"],
            true,
            GenSpec { offset_ms: 1500.0, dur_ext_s: 36.0, ..base(230) },
            1500.0,
            0.0,
            30.0,
        );
    }

    CorpusManifest {
        schema: CORPUS_SCHEMA.to_string(),
        split: "synthetic".to_string(),
        media_root: String::new(),
        cases,
    }
}

// ---------------------------------------------------------------------------------------------
// §10.4 — the metrics block
// ---------------------------------------------------------------------------------------------

/// Per-tag confusion breakdown (§10.4 "per-tag breakdown"). Deterministic counts only.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct TagBreakdown {
    pub cases: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
}

/// The §10.4 metrics block — every field a deterministic function of the corpus + params, so two
/// runs serialize byte-identically (the §11.3 gate). `runtime_ms` and `peak_candidate_count` are
/// the two §10.4 "runtime and peak candidate count" items; runtime is NON-deterministic and is
/// therefore reported by the caller on stderr and NOT serialized here — `peak_candidate_count`
/// is a property of the cases (max prefilter candidates any single `a` accumulates) and stays.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CorpusMetrics {
    pub schema: String,
    /// The matcher implementation these metrics describe (so a fingerprint run is distinguishable
    /// from the legacy run when comparing JSON).
    pub matcher: String,
    pub split: String,
    pub cases: usize,
    pub same_event_cases: usize,
    pub true_positives: usize,
    pub false_positives: usize,
    pub false_negatives: usize,
    /// True pairs that resolved to ambiguity rather than a confident match (§10.4).
    pub ambiguous_true_pairs: usize,
    /// New CONFIDENT false match on a non-same-event case (the §10.5 hard gate; must stay 0).
    pub confident_false_matches: usize,
    /// Offset error (|measured − expected|) over matched true pairs, ms.
    pub median_offset_error_ms: f64,
    pub p95_offset_error_ms: f64,
    /// Drift error (|measured − expected|) over matched true pairs, ppm.
    pub median_drift_error_ppm: f64,
    pub p95_drift_error_ppm: f64,
    /// Max number of prefilter candidates any single video accumulates across the corpus — the
    /// §10.4 peak candidate count. Deterministic (a corpus property, not timing).
    pub peak_candidate_count: usize,
    pub per_tag: BTreeMap<String, TagBreakdown>,
}

pub const METRICS_SCHEMA: &str = "couple-matcher-metrics/1";

/// One case's matcher outcome (the per-case classification + recovered offset/drift), kept so the
/// aggregate metrics and the per-tag breakdown agree on the same decisions.
struct CaseOutcome {
    same_event: bool,
    /// The matcher's verdict: a confident match (`Some(matched=true)`), an explicit non-match
    /// (`Some(false)`), or unscannable/ambiguous (`None` ⇒ no confident verdict).
    confident_match: Option<bool>,
    offset_err_ms: Option<f64>,
    drift_err_ppm: Option<f64>,
    /// Whether this case's `b` was a prefilter candidate against `a` (drives the peak-candidate
    /// metric: §10.4's "peak candidate count" is the MAX prefilter candidates a single video
    /// accumulates, so we count candidates GROUPED by the video they share, not per isolated case).
    is_prefilter_candidate: bool,
    /// A stable identity for `a` (the video): same content ⇒ same key. Synthetic cases sharing one
    /// camera (families 14 "simultaneous" and 15 "more than six candidates") collapse onto one key
    /// so their externals sum into a single video's candidate count.
    camera_key: String,
    tags: Vec<String>,
}

/// Which matcher implementation a corpus run scores (PLAN-SPECTRAL-FINGERPRINT-MATCHING §15.1).
/// The two LEGACY verdicts are the prework's existing scoring (the §10.4 substrate); the two
/// FINGERPRINT verdicts run the §5.3 `Ctx::match_pair` over the spectral pipeline and classify on
/// the method-neutral `PairEvidence.classification`. Calibration (§15.1) compares the legacy and
/// fingerprint columns over the SAME manifest; the boundary cut keeps every verdict deterministic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Matcher {
    /// The default legacy z-gate verdict (byte-identical to production's default resolver gate).
    LegacyZ,
    /// The legacy fine waveform-lock classifier verdict (the prework `--verify-active` path).
    LegacyFineLock,
    /// The spectral matcher in `FingerprintShadow` mode (profile 0 valid). Identity from
    /// landmark consensus; verdict = `PairEvidence.classification`.
    FingerprintShadow,
    /// The spectral matcher in `FingerprintPrimary` mode (requires a calibrated profile ≥ 1).
    FingerprintPrimary,
}

impl Matcher {
    /// The matcher label written into the scored JSON (`CorpusMetrics.matcher`) so a legacy run is
    /// distinguishable from a fingerprint run when comparing splits.
    fn label(self) -> &'static str {
        match self {
            Matcher::LegacyZ => "legacy-z",
            Matcher::LegacyFineLock => "legacy-fine-lock",
            Matcher::FingerprintShadow => "spectral-landmark-v1-shadow",
            Matcher::FingerprintPrimary => "spectral-landmark-v1-primary",
        }
    }

    fn is_fingerprint(self) -> bool {
        matches!(self, Matcher::FingerprintShadow | Matcher::FingerprintPrimary)
    }
}

/// Resolve (or deterministically regenerate) a case's two media files. Synthetic cases regenerate
/// their pcm pair from the committed `gen` block into `work_dir/<id>`; real-data cases resolve `a`
/// and `b` under `effective_root` (the caller has already joined `repo_root` with the manifest's
/// `media_root`). Returns the two paths; existence is the caller's concern (a missing real file is
/// a structural skip, not an error).
fn resolve_media(
    case: &CorpusCase,
    effective_root: &Path,
    work_dir: &Path,
    ffmpeg: &Path,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    match &case.gen {
        Some(g) => {
            // Synthetic: regenerate the pcm pair deterministically into work_dir/<id>.
            let dir = work_dir.join(&case.id);
            let mut spec = g.fixture_spec();
            // The `generate` helper writes camera.wav/external.wav from one master; family 8
            // (independent fine content) needs the CAMERA from a different master. We model
            // that by generating the external from the case seed and the camera from a
            // seed-offset run, then keeping each file. For the common (cam_seed_offset==0)
            // case a single generate() yields both.
            let truth = fixture::generate(&dir, &spec, ffmpeg)?;
            let ext = dir.join(&truth.external_file);
            let cam = if g.cam_seed_offset != 0 {
                // Regenerate ONLY the camera from a different master into a sibling file, so
                // the macro-envelope plan matches but the fine content is independent.
                spec.seed = g.seed.wrapping_add(g.cam_seed_offset);
                let cam_dir = work_dir.join(format!("{}-cam", case.id));
                let cam_truth = fixture::generate(&cam_dir, &spec, ffmpeg)?;
                cam_dir.join(&cam_truth.camera_file)
            } else {
                dir.join(&truth.camera_file)
            };
            Ok((cam, ext))
        }
        None => Ok((effective_root.join(&case.a), effective_root.join(&case.b))),
    }
}

/// An unscannable outcome (a real-data case whose gitignored media is absent): contributes to
/// neither TP nor FP, but still counted structurally.
fn skipped_outcome(case: &CorpusCase) -> CaseOutcome {
    CaseOutcome {
        same_event: case.same_event,
        confident_match: None,
        offset_err_ms: None,
        drift_err_ppm: None,
        is_prefilter_candidate: false,
        camera_key: camera_key(case),
        tags: case.tags.clone(),
    }
}

/// Score the LEGACY envelope matcher over a manifest (§10.4) — the prework entry point, preserved
/// byte-for-byte. `verify_active` switches the verdict between the default legacy z-gate and the
/// fine waveform-lock classifier exactly as the resolver does. Delegates to [`score_manifest_with`].
///
/// Returns the metrics AND the wall-clock runtime in ms (the caller prints runtime to stderr and
/// serializes only the metrics — keeping the scored JSON byte-stable, the §11.3 gate).
pub fn score_manifest(
    ctx: &Arc<Ctx>,
    manifest: &CorpusManifest,
    repo_root: &Path,
    work_dir: &Path,
    params: &PairingParams,
    verify_active: bool,
    z_min: f64,
) -> Result<(CorpusMetrics, f64)> {
    let matcher = if verify_active { Matcher::LegacyFineLock } else { Matcher::LegacyZ };
    score_manifest_with(ctx, manifest, repo_root, work_dir, params, matcher, z_min)
}

/// Score a chosen [`Matcher`] over a manifest, emitting the SAME deterministic §10.4
/// [`CorpusMetrics`] regardless of matcher (PLAN-SPECTRAL-FINGERPRINT-MATCHING §15.1 — the
/// legacy-vs-fingerprint comparison runs this over one manifest with each matcher).
///
/// The legacy verdicts reproduce the prework path (`pair_correlate` + the z-gate / fine-lock); the
/// fingerprint verdicts run the §5.3 `Ctx::match_pair` in the requested mode and classify on the
/// method-neutral `PairEvidence.classification`:
///
/// - `Match` ⇒ a confident match (TP if same-event, a CONFIDENT FALSE MATCH if not — the §15.2 hard
///   gate); the offset/drift error uses the REFINED mapping when present (boundary 7), else the
///   consensus lag/drift;
/// - `Ambiguous` ⇒ no confident verdict (`None`): a true pair counts as an ambiguous-true MISS, a
///   non-same-event pair is a correct rejection. Ambiguity is NEVER a false match (§15.2);
/// - `NoMatch` / `InsufficientOverlap` / `Unavailable` ⇒ an explicit non-match (`Some(false)`).
///
/// `params` carries the runtime thresholds in `fingerprint_match` (the profile under calibration);
/// the matcher mode is overridden to match `matcher` so the SAME `params` can be reused across
/// columns. Determinism: the fingerprint pipeline is a pure function of the PCM + the frozen v1
/// format + the thresholds (§3 invariant 2), so two runs serialize byte-identically.
pub fn score_manifest_with(
    ctx: &Arc<Ctx>,
    manifest: &CorpusManifest,
    repo_root: &Path,
    work_dir: &Path,
    params: &PairingParams,
    matcher: Matcher,
    z_min: f64,
) -> Result<(CorpusMetrics, f64)> {
    use crate::matching::MatchClassification;
    use crate::pairing_params::MatcherMode;

    manifest.validate()?;
    // Build the params actually used: legacy columns keep the mode the caller passed (they never
    // build a fingerprint); fingerprint columns force the requested spectral mode. We validate the
    // EFFECTIVE params so a FingerprintPrimary run with an uncalibrated profile 0 is rejected here
    // (§6) — exactly the guard that keeps a profile-0 primary measurement from being fabricated.
    let mut eff = params.clone();
    eff.matcher_mode = match matcher {
        Matcher::LegacyZ | Matcher::LegacyFineLock => MatcherMode::LegacyEnvelope,
        Matcher::FingerprintShadow => MatcherMode::FingerprintShadow,
        Matcher::FingerprintPrimary => MatcherMode::FingerprintPrimary,
    };
    eff.validate()?;

    // The effective real-data root: repo_root joined with the manifest's media_root (so a real
    // case path is just root/a, root/b — `resolve_media` takes the joined root).
    let effective_root = if manifest.media_root.is_empty() {
        repo_root.to_path_buf()
    } else {
        repo_root.join(&manifest.media_root)
    };

    let ffmpeg = ctx.ffmpeg()?;
    let t0 = std::time::Instant::now();

    let mut outcomes: Vec<CaseOutcome> = Vec::with_capacity(manifest.cases.len());
    for case in &manifest.cases {
        let (path_a, path_b) = resolve_media(case, &effective_root, work_dir, &ffmpeg)?;

        // A real-data case whose media isn't present is skipped from scoring but still validated
        // structurally (the split rule is enforced by manifest.validate()).
        if case.gen.is_none() && (!path_a.is_file() || !path_b.is_file()) {
            outcomes.push(skipped_outcome(case));
            continue;
        }

        let a_id = import(ctx, &path_a, crate::media::TrackRole::Camera)?;
        let b_id = import(ctx, &path_b, crate::media::TrackRole::External)?;

        let outcome = if matcher.is_fingerprint() {
            // §5.3 spectral path: one method-neutral evidence for this pair. The verdict is the
            // method-neutral classification; the recovered mapping prefers the boundary-7 refined
            // fields and falls back to the consensus lag/drift.
            let ev = ctx.match_pair(a_id, b_id, &eff)?;
            let (lag, drift) = (
                ev.refined_lag_ms.unwrap_or(ev.lag_ms),
                ev.refined_drift_ppm.unwrap_or(ev.drift_ppm),
            );
            // CALIBRATION DIAGNOSTIC (env-gated; stderr ONLY, never the deterministic stdout JSON).
            // Set COUPLE_CORPUS_TRACE=1 to dump each case's spectral evidence while tuning. This
            // exists so the boundary-12 calibration can SEE why a real case misses, without polluting
            // the byte-stable metrics.
            if std::env::var("COUPLE_CORPUS_TRACE").is_ok() {
                eprintln!(
                    "TRACE {:<32} same={} class={:?} lag={:.1} drift={:.1} refLag={:?} support={} \
                     corr={} inlierFrac={:.3} cov={:.3} p95={:?} score={:.4} margin={:.3} reason={:?}",
                    case.id,
                    case.same_event,
                    ev.classification,
                    ev.lag_ms,
                    ev.drift_ppm,
                    ev.refined_lag_ms.map(|v| (v * 10.0).round() / 10.0),
                    ev.support,
                    ev.candidate_correspondences,
                    ev.inlier_fraction,
                    ev.temporal_coverage,
                    ev.residual_p95_ms.map(|v| (v * 100.0).round() / 100.0),
                    ev.score,
                    ev.score_margin,
                    ev.refine_reason,
                );
            }
            let (confident_match, offset_err_ms, drift_err_ppm) = match ev.classification {
                MatchClassification::Match => (
                    Some(true),
                    Some((lag - case.expected_offset_ms).abs()),
                    Some((drift - case.expected_drift_ppm).abs()),
                ),
                // Ambiguity is honest uncertainty, NOT a false match (§15.2): no confident verdict.
                MatchClassification::Ambiguous => (None, None, None),
                // Everything else is an explicit non-match.
                MatchClassification::NoMatch
                | MatchClassification::InsufficientOverlap
                | MatchClassification::Unavailable => (Some(false), None, None),
            };
            // The fingerprint path's "prefilter candidate" surrogate for the §10.4 peak-candidate
            // metric is "this pair produced a fittable affine line" (a real candidate vs a flat
            // NoMatch). A confident Match or an honest Ambiguous both count as candidates.
            let is_candidate = matches!(
                ev.classification,
                MatchClassification::Match | MatchClassification::Ambiguous
            );
            CaseOutcome {
                same_event: case.same_event,
                confident_match,
                offset_err_ms,
                drift_err_ppm,
                is_prefilter_candidate: is_candidate,
                camera_key: camera_key(case),
                tags: case.tags.clone(),
            }
        } else {
            // Legacy path (byte-identical to the prework scoring).
            let verify_active = matcher == Matcher::LegacyFineLock;
            let m = ctx.pair_correlate(a_id, b_id, &eff)?;
            let is_candidate = if verify_active {
                m.prefilter_candidate
            } else {
                !m.insufficient_overlap && m.z >= z_min
            };
            let (confident_match, offset_err_ms, drift_err_ppm) = if !is_candidate {
                (Some(false), None, None)
            } else if verify_active {
                match ctx.verify_batch(&[(a_id, b_id, m.lag_ms)], &eff)?.into_iter().next().flatten()
                {
                    Some(fit) if verify::is_match(&fit) => (
                        Some(true),
                        Some((fit.off_ms - case.expected_offset_ms).abs()),
                        Some((fit.ppm - case.expected_drift_ppm).abs()),
                    ),
                    _ => (Some(false), None, None),
                }
            } else {
                (Some(true), Some((m.lag_ms - case.expected_offset_ms).abs()), None)
            };
            if std::env::var("COUPLE_CORPUS_TRACE").is_ok() {
                // Legacy trace: the fine-lock fit's recovered lag/ppm (when it ran) — the trusted
                // reference offset to compare the fingerprint consensus against on real material.
                let fit = if verify_active {
                    ctx.verify_batch(&[(a_id, b_id, m.lag_ms)], &eff)
                        .ok()
                        .and_then(|v| v.into_iter().next().flatten())
                } else {
                    None
                };
                eprintln!(
                    "TRACE-LEGACY {:<28} same={} z={:.2} stageLag={:.1} fineMatch={} fitLag={:?} fitPpm={:?}",
                    case.id,
                    case.same_event,
                    m.z,
                    m.lag_ms,
                    fit.as_ref().map(verify::is_match).unwrap_or(false),
                    fit.as_ref().map(|f| (f.off_ms * 10.0).round() / 10.0),
                    fit.as_ref().map(|f| (f.ppm * 10.0).round() / 10.0),
                );
            }
            CaseOutcome {
                same_event: case.same_event,
                confident_match,
                offset_err_ms,
                drift_err_ppm,
                is_prefilter_candidate: m.prefilter_candidate,
                camera_key: camera_key(case),
                tags: case.tags.clone(),
            }
        };

        outcomes.push(outcome);
        ctx.track_free(a_id);
        ctx.track_free(b_id);
    }

    let runtime_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let metrics = aggregate(manifest, &outcomes, matcher.label());
    Ok((metrics, runtime_ms))
}

/// Aggregate per-case outcomes into the deterministic §10.4 block. Pure arithmetic over the
/// outcomes — no clock, no RNG, no map-iteration leakage (the per-tag map is a `BTreeMap`, so it
/// serializes in sorted key order ⇒ byte-stable).
fn aggregate(manifest: &CorpusManifest, outcomes: &[CaseOutcome], matcher: &str) -> CorpusMetrics {
    let mut tp = 0usize;
    let mut fp = 0usize;
    let mut fn_ = 0usize;
    let mut ambiguous_true = 0usize;
    let mut confident_false = 0usize;
    let mut offset_errs: Vec<f64> = Vec::new();
    let mut drift_errs: Vec<f64> = Vec::new();
    // Peak candidate count = the MAX prefilter candidates accumulated by any single video, so we
    // tally per camera_key (synthetic families 14/15 share a camera ⇒ their externals sum).
    let mut candidates_per_camera: BTreeMap<&str, usize> = BTreeMap::new();
    let mut per_tag: BTreeMap<String, TagBreakdown> = BTreeMap::new();

    for o in outcomes {
        if o.is_prefilter_candidate {
            *candidates_per_camera.entry(o.camera_key.as_str()).or_default() += 1;
        }
        // Classification against ground truth.
        let (case_tp, case_fp, case_fn) = match (o.same_event, o.confident_match) {
            (true, Some(true)) => {
                if let Some(e) = o.offset_err_ms {
                    offset_errs.push(e);
                }
                if let Some(e) = o.drift_err_ppm {
                    drift_errs.push(e);
                }
                (1, 0, 0)
            }
            (true, Some(false)) => (0, 0, 1), // a true pair the matcher rejected ⇒ false negative
            (true, None) => {
                ambiguous_true += 1;
                (0, 0, 1) // unscannable/ambiguous true pair counts as a miss
            }
            (false, Some(true)) => {
                confident_false += 1;
                (0, 1, 0) // a non-same-event pair confidently matched ⇒ false positive
            }
            (false, Some(false)) | (false, None) => (0, 0, 0), // correct rejection
        };
        tp += case_tp;
        fp += case_fp;
        fn_ += case_fn;

        for tag in &o.tags {
            let e = per_tag.entry(tag.clone()).or_default();
            e.cases += 1;
            e.true_positives += case_tp;
            e.false_positives += case_fp;
            e.false_negatives += case_fn;
        }
    }

    CorpusMetrics {
        schema: METRICS_SCHEMA.to_string(),
        matcher: matcher.to_string(),
        split: manifest.split.clone(),
        cases: manifest.cases.len(),
        same_event_cases: outcomes.iter().filter(|o| o.same_event).count(),
        true_positives: tp,
        false_positives: fp,
        false_negatives: fn_,
        ambiguous_true_pairs: ambiguous_true,
        confident_false_matches: confident_false,
        median_offset_error_ms: round6(median(&offset_errs)),
        p95_offset_error_ms: round6(p95(&offset_errs)),
        median_drift_error_ppm: round6(median(&drift_errs)),
        p95_drift_error_ppm: round6(p95(&drift_errs)),
        peak_candidate_count: candidates_per_camera.values().copied().max().unwrap_or(0),
        per_tag,
    }
}

/// A stable identity for a case's video (`a`). Synthetic cases that generate the SAME camera
/// content share a key, so the §10.4 peak-candidate metric groups their externals onto one video.
/// For a synthetic case the key is the camera-shaping `gen` fields (seed + cam_seed_offset +
/// duration + degradation); for real data it is the `a` path. (Two synthetic families share a
/// camera by construction: 14 `simul_rec*` and 15 `manycand_*`, both pinned to a single seed.)
fn camera_key(case: &CorpusCase) -> String {
    match &case.gen {
        Some(g) => format!(
            "cam:seed={}+{}:dur={}:gain={}:agc={}:snr={:?}",
            g.seed, g.cam_seed_offset, g.dur_cam_s, g.gain_db, g.agc, g.snr_db
        ),
        None => format!("real:{}", case.a),
    }
}

fn import(ctx: &Arc<Ctx>, path: &Path, role: crate::media::TrackRole) -> Result<crate::media::TrackId> {
    let job = ctx.next_id();
    let cancel = crate::job::CancelToken::new();
    ctx.import_blocking(path, role, crate::job::noop_progress(), &cancel, job)
}

/// Round to 6 decimals so f64 noise below a micro-unit cannot make two runs differ in their last
/// bit (the values are already deterministic; this is belt-and-suspenders for the byte gate).
fn round6(v: f64) -> f64 {
    if v.is_finite() {
        (v * 1e6).round() / 1e6
    } else {
        v
    }
}

/// Median by total order (empty ⇒ 0.0). Deterministic.
fn median(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Nearest-rank p95 (empty ⇒ 0.0). `ceil(0.95·n)` 1-based → 0-based, deterministic.
fn p95(values: &[f64]) -> f64 {
    if values.is_empty() {
        return 0.0;
    }
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    let rank = ((0.95 * v.len() as f64).ceil() as usize).max(1);
    v[rank - 1]
}

// ---------------------------------------------------------------------------------------------
// §15.3 — performance budget measurement (a TIMING harness, NOT part of the deterministic metrics)
// ---------------------------------------------------------------------------------------------

/// The §15.3 performance-budget measurements. Wall-clock TIMINGS — explicitly NON-deterministic, so
/// this is a separate report from [`CorpusMetrics`] (which is byte-stable). Times are in ms;
/// throughput-normalized quantities (`fingerprint_gen_ms_per_hour`, `artifact_bytes_per_hour`) are
/// extrapolated from the measured workload duration so they compare directly to the §15.3 budgets.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BudgetReport {
    /// Audio seconds fingerprinted to measure generation (the measured workload length).
    pub measured_audio_s: f64,
    /// Fingerprint build time normalized to one hour of audio, ms (budget ≤ 10_000 = 10 s/hr).
    pub fingerprint_gen_ms_per_hour: f64,
    /// Encoded `.cpf` artifact size normalized to one hour of audio, bytes (budget ≤ 10 MB/hr).
    pub artifact_bytes_per_hour: f64,
    /// Build time of the one batch audio index over the 20×20 workload's audios, ms.
    pub index_build_ms: f64,
    /// Mean per-pair correspondence-generation (landmark lookup) time over the 20×20 cells, ms.
    pub candidate_lookup_ms_per_pair: f64,
    /// Mean per-pair affine-consensus (fit) time over the 20×20 cells, ms.
    pub affine_fit_ms_per_pair: f64,
    /// Wall time of a 6×6 warm-cache candidate matrix (consensus only, no fine refinement), ms
    /// (budget ≤ 5_000 = 5 s).
    pub matrix_6x6_ms: f64,
    /// Wall time of a 20×20 warm-cache candidate matrix (consensus only), ms (budget ≤ 15_000 = 15 s).
    pub matrix_20x20_ms: f64,
    /// Peak resident set size observed during the measurement, bytes (budget ≤ 512 MB). `0` if the
    /// platform RSS probe is unavailable.
    pub peak_rss_bytes: u64,
}

/// Generate a deterministic PCM signal at the canonical 48 kHz rate (`dur_s` seconds) whose spectral
/// landmark density approximates REAL music (so the §15.3 budget reflects the deployment, not a
/// pathological worst case). A handful of slowly-evolving partials give broadband energy across the
/// analyzed 100–5500 Hz band; a sparse transient (~every 1.5 s) gives the peak selector sharp events
/// to lock; a per-variant `seed` decorrelates the fine content so a batch of distinct tracks builds
/// a non-degenerate index. Pure arithmetic — no RNG crate, no clock (a small LCG keyed by `seed`) —
/// so the workload is reproducible run to run.
fn budget_pcm(dur_s: f64, seed: u64) -> Vec<f32> {
    const SR: f64 = 48_000.0;
    let n = (dur_s * SR) as usize;
    let mut pcm = vec![0.0f32; n];
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut lcg = || {
        state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((state >> 33) as f64 / (1u64 << 31) as f64) - 1.0 // ~[-1, 1)
    };
    // Three partials — enough broadband energy for landmarks, not a dense click train (the peak
    // selector's `max_peaks_per_second = 30` cap is reached by real music, which is what we model).
    let partials = [330.0_f64, 990.0, 2640.0];
    for (i, s) in pcm.iter_mut().enumerate() {
        let t = i as f64 / SR;
        let mut v = 0.0;
        for (p, f) in partials.iter().enumerate() {
            // Slow vibrato so the partials are not perfectly stationary (real instruments drift).
            let fm = f * (1.0 + 0.002 * (2.0 * std::f64::consts::PI * 4.0 * t).sin());
            v += (0.2 / (p as f64 + 1.0)) * (2.0 * std::f64::consts::PI * fm * t).sin();
        }
        // A sparse transient every ~1.5 s (a short broadband event) — music-like onset density.
        if i % (3 * SR as usize / 2) < 48 {
            v += 0.6 * lcg();
        }
        // A little decorrelated noise so distinct seeds yield distinct landmarks.
        v += 0.03 * lcg();
        *s = v as f32;
    }
    pcm
}

/// Current process peak RSS in bytes (macOS/Linux via `getrusage`), or `0` when unavailable. On
/// macOS `ru_maxrss` is BYTES; on Linux it is KILOBYTES — normalized here.
fn peak_rss_bytes() -> u64 {
    #[cfg(unix)]
    {
        // SAFETY: getrusage with a zeroed rusage out-param is a read-only syscall.
        unsafe {
            let mut ru: libc::rusage = std::mem::zeroed();
            if libc::getrusage(libc::RUSAGE_SELF, &mut ru) == 0 {
                let maxrss = ru.ru_maxrss as u64;
                #[cfg(target_os = "macos")]
                {
                    return maxrss; // bytes
                }
                #[cfg(not(target_os = "macos"))]
                {
                    return maxrss.saturating_mul(1024); // kilobytes → bytes
                }
            }
        }
    }
    0
}

/// Measure the §15.3 performance budgets on a deterministic synthetic workload (a TIMING harness,
/// not the byte-stable metrics). The fingerprint pipeline is driven directly (no ffmpeg decode) so
/// the measurement is of the FINGERPRINT cost, not media import:
///
/// 1. Generation + artifact size: build one `gen_audio_s`-second fingerprint, time it and size the
///    encoded `.cpf`; normalize both to one hour.
/// 2. Index/lookup/fit: build 20 distinct `cell_audio_s`-second readers, build the ONE batch index
///    (timed), then for the 20×20 cells time correspondence generation (lookup) and consensus (fit)
///    per pair and report the means.
/// 3. Matrix wall times: run the boundary-5 consensus over every cell of a 6×6 and a 20×20 batch
///    (warm readers, no fine refinement — the §15.3 "warm-cache matrix" is the identity stage).
///
/// `gen_audio_s` defaults (caller) to a representative clip; a longer value gives a steadier
/// per-hour figure at the cost of runtime.
pub fn measure_budgets(gen_audio_s: f64, cell_audio_s: f64) -> BudgetReport {
    use crate::fingerprint::{
        build_artifact_bytes, correspondences_for_pair, AudioHashIndex, FingerprintFormatV1,
        FingerprintReader,
    };
    use crate::pairing_params::PairingParams;
    use std::time::Instant;

    let fmt = FingerprintFormatV1::fixed();
    let params = PairingParams::default();
    let fp = &params.fingerprint_match;

    // --- 1. Generation time + artifact size, normalized to one hour. ---
    let gen_pcm = budget_pcm(gen_audio_s, 1);
    let t = Instant::now();
    let bytes = build_artifact_bytes(&gen_pcm, gen_pcm.len() as u64);
    let gen_ms = t.elapsed().as_secs_f64() * 1000.0;
    let hours = gen_audio_s / 3600.0;
    let fingerprint_gen_ms_per_hour = gen_ms / hours;
    let artifact_bytes_per_hour = bytes.len() as f64 / hours;
    // Free the generation buffers BEFORE the matching phase so the peak-RSS probe reflects the
    // MATCHING memory (index + one pair's correspondences), not the transient gen PCM (§15.3 is the
    // "peak matching memory").
    drop(gen_pcm);
    drop(bytes);

    // --- 2 & 3. Build 20 distinct readers for the matrix workloads. ---
    let build_reader = |seed: u64| -> FingerprintReader {
        let pcm = budget_pcm(cell_audio_s, seed);
        let b = build_artifact_bytes(&pcm, pcm.len() as u64);
        FingerprintReader::parse(&b, &fmt).expect("budget reader parses")
    };
    let readers: Vec<FingerprintReader> = (0..20).map(|k| build_reader(100 + k)).collect();
    let refs: Vec<&FingerprintReader> = readers.iter().collect();

    // Index build over all 20 audios (the batch index the orchestrator builds once).
    let t = Instant::now();
    let (index, _stats) = AudioHashIndex::build(&refs, fp);
    let index_build_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Per-pair lookup + fit means over the full 20×20.
    let overlap = cell_audio_s;
    let mut lookup_total = 0.0;
    let mut fit_total = 0.0;
    let mut pairs = 0u32;
    for v in &readers {
        for ai in 0..readers.len() as u32 {
            let t = Instant::now();
            let pc = correspondences_for_pair(v, &index, ai, &fmt, fp);
            lookup_total += t.elapsed().as_secs_f64() * 1000.0;
            let t = Instant::now();
            let _ = crate::fingerprint::consensus(&pc.correspondences, overlap, fp);
            fit_total += t.elapsed().as_secs_f64() * 1000.0;
            pairs += 1;
        }
    }
    let candidate_lookup_ms_per_pair = lookup_total / pairs as f64;
    let affine_fit_ms_per_pair = fit_total / pairs as f64;

    // 6×6 and 20×20 matrix wall times (index build + every cell's lookup+consensus).
    let matrix_ms = |n: usize| -> f64 {
        let sub: Vec<&FingerprintReader> = refs.iter().take(n).copied().collect();
        let t = Instant::now();
        let (idx, _) = AudioHashIndex::build(&sub, fp);
        for v in sub.iter() {
            for ai in 0..n as u32 {
                let pc = correspondences_for_pair(v, &idx, ai, &fmt, fp);
                let _ = crate::fingerprint::consensus(&pc.correspondences, overlap, fp);
            }
        }
        t.elapsed().as_secs_f64() * 1000.0
    };
    let matrix_6x6_ms = matrix_ms(6);
    let matrix_20x20_ms = matrix_ms(20);

    BudgetReport {
        measured_audio_s: gen_audio_s,
        fingerprint_gen_ms_per_hour,
        artifact_bytes_per_hour,
        index_build_ms,
        candidate_lookup_ms_per_pair,
        affine_fit_ms_per_pair,
        matrix_6x6_ms,
        matrix_20x20_ms,
        peak_rss_bytes: peak_rss_bytes(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // §10.2 — the synthetic manifest covers all 15 required families and validates. Holdout-rule
    // and id-uniqueness are checked by validate(); here we assert family coverage by tag.
    #[test]
    fn synthetic_manifest_covers_all_families() {
        let m = synthetic_manifest();
        m.validate().expect("synthetic manifest validates");
        let tags: std::collections::HashSet<&str> =
            m.cases.iter().flat_map(|c| c.tags.iter().map(|s| s.as_str())).collect();
        for required in [
            "clean", "agc", "telephone", "reverb", "snr", "duration", "drift", "macroEnvelope",
            "repeatedPhrase", "periodicTone", "mostlySilence", "dropout", "restart",
            "simultaneous", "manyCandidates",
        ] {
            assert!(tags.contains(required), "synthetic corpus missing the `{required}` family");
        }
        // The "more than six candidates" family really has > 6 cases.
        let manycand = m.cases.iter().filter(|c| c.tags.iter().any(|t| t == "manyCandidates")).count();
        assert!(manycand > 6, "the over-six family must have > 6 cases, got {manycand}");
        // Durations 8/12/20/60/600 all present.
        for d in ["dur8", "dur12", "dur20", "dur60", "dur600"] {
            assert!(tags.contains(d), "missing duration family {d}");
        }
        // Drifts -600/-100/0/+100/+600 present.
        for p in ["ppm-600", "ppm-100", "ppm0", "ppm100", "ppm600"] {
            assert!(tags.contains(p), "missing drift family {p}");
        }
    }

    // The synthetic manifest is a PURE function (no clock/RNG): two builds are byte-identical, so
    // the committed cases.json is reproducible by `gen-synthetic` (§10.2).
    #[test]
    fn synthetic_manifest_is_deterministic() {
        let a = serde_json::to_string(&synthetic_manifest()).unwrap();
        let b = serde_json::to_string(&synthetic_manifest()).unwrap();
        assert_eq!(a, b, "synthetic_manifest() must be deterministic");
    }

    // §10.1/§10.3 — a holdout manifest with a tuning-allowed case is rejected.
    #[test]
    fn holdout_must_not_allow_tuning() {
        let mut m = CorpusManifest {
            schema: CORPUS_SCHEMA.to_string(),
            split: "holdout".to_string(),
            media_root: String::new(),
            cases: vec![CorpusCase {
                id: "h1".into(),
                group: "real".into(),
                a: "a.wav".into(),
                b: "b.wav".into(),
                same_event: true,
                expected_offset_ms: 0.0,
                expected_drift_ppm: 0.0,
                minimum_overlap_s: 8.0,
                tags: vec![],
                allowed_for_tuning: true, // illegal in holdout
                gen: None,
            }],
        };
        assert!(m.validate().is_err(), "holdout case with allowedForTuning=true must be rejected");
        m.cases[0].allowed_for_tuning = false;
        m.validate().expect("holdout with allowedForTuning=false validates");
    }

    // Duplicate ids are rejected (a manifest hygiene guard).
    #[test]
    fn duplicate_ids_rejected() {
        let c = |id: &str| CorpusCase {
            id: id.into(),
            group: "synthetic".into(),
            a: "a".into(),
            b: "b".into(),
            same_event: true,
            expected_offset_ms: 0.0,
            expected_drift_ppm: 0.0,
            minimum_overlap_s: 8.0,
            tags: vec![],
            allowed_for_tuning: true,
            gen: None,
        };
        let m = CorpusManifest {
            schema: CORPUS_SCHEMA.to_string(),
            split: "synthetic".into(),
            media_root: String::new(),
            cases: vec![c("dup"), c("dup")],
        };
        assert!(m.validate().is_err(), "duplicate case ids must be rejected");
    }

    // The metrics aggregation is pure: TP/FP/FN classification + median/p95 are a deterministic
    // function of crafted outcomes (no media, no clock), and the per-tag map is sorted.
    #[test]
    fn aggregate_classifies_and_is_byte_stable() {
        let manifest = CorpusManifest {
            schema: CORPUS_SCHEMA.into(),
            split: "synthetic".into(),
            media_root: String::new(),
            cases: vec![], // cases count is taken from manifest; outcomes drive the math
        };
        let outcomes = vec![
            // A matched true pair with a 1 ms offset error. Camera "vidX" shared with the FP case
            // below, so vidX accumulates 2 prefilter candidates ⇒ the peak.
            CaseOutcome {
                same_event: true,
                confident_match: Some(true),
                offset_err_ms: Some(1.0),
                drift_err_ppm: Some(2.0),
                is_prefilter_candidate: true,
                camera_key: "vidX".into(),
                tags: vec!["clean".into()],
            },
            // A true pair the matcher missed ⇒ false negative (not a prefilter candidate).
            CaseOutcome {
                same_event: true,
                confident_match: Some(false),
                offset_err_ms: None,
                drift_err_ppm: None,
                is_prefilter_candidate: false,
                camera_key: "vidY".into(),
                tags: vec!["snr".into()],
            },
            // A non-same-event pair confidently matched ⇒ false positive + confident_false. Shares
            // camera vidX with the TP case ⇒ vidX has 2 prefilter candidates.
            CaseOutcome {
                same_event: false,
                confident_match: Some(true),
                offset_err_ms: None,
                drift_err_ppm: None,
                is_prefilter_candidate: true,
                camera_key: "vidX".into(),
                tags: vec!["macroEnvelope".into()],
            },
            // A correctly rejected non-same-event pair.
            CaseOutcome {
                same_event: false,
                confident_match: Some(false),
                offset_err_ms: None,
                drift_err_ppm: None,
                is_prefilter_candidate: false,
                camera_key: "vidZ".into(),
                tags: vec!["macroEnvelope".into()],
            },
        ];
        let a = aggregate(&manifest, &outcomes, "legacy-fine-lock");
        assert_eq!(a.true_positives, 1);
        assert_eq!(a.false_negatives, 1);
        assert_eq!(a.false_positives, 1);
        assert_eq!(a.confident_false_matches, 1);
        // vidX accumulated 2 prefilter candidates (TP + FP share the camera) ⇒ peak 2.
        assert_eq!(a.peak_candidate_count, 2);
        assert_eq!(a.median_offset_error_ms, 1.0);
        // per-tag breakdown is keyed and sorted.
        assert_eq!(a.per_tag["clean"].true_positives, 1);
        assert_eq!(a.per_tag["snr"].false_negatives, 1);
        assert_eq!(a.per_tag["macroEnvelope"].cases, 2);
        let j1 = serde_json::to_string(&a).unwrap();
        let j2 = serde_json::to_string(&aggregate(&manifest, &outcomes, "legacy-fine-lock")).unwrap();
        assert_eq!(j1, j2, "metrics JSON must be byte-identical across two aggregations");
    }
}
