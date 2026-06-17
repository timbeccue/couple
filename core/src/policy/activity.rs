//! Activity-strip decision (TODO-005) — the PURE, testable MATH behind the single global activity
//! strip beneath the queue/session header. PLAN-CORE-SHELL-BOUNDARY Phase F; relocated from the
//! Swift `ActivitySummaryPolicy` (a RELOCATION, not a behavior change).
//!
//! Several operations do meaningful background work (import, candidate pairing, queued auto-sync,
//! batch export), and the strip answers ONE question as a pure function: "given the current job
//! SIGNALS (and how long the current operation has run), what is the SINGLE summary line the strip
//! should show, and does it have a TRUSTWORTHY fraction?"
//!
//! ## The core/shell split (Phase F)
//! This module owns ONLY the pure decision math:
//!   - PRIORITY — which signal wins the one summary line (failure ▸ ingest ▸ export ▸ import ▸
//!     pairing ▸ sync), and which line-KIND it produces.
//!   - DEBOUNCE — the decision `elapsedMs >= debounceMs` (a failure skips it). `elapsedMs` is passed
//!     in as an EXPLICIT field — core never reads a clock; the shell holds the debounce TIMER.
//!   - FRACTION / WEIGHTING — the determinate 0…1 (import per-file re-join, export per-row, the
//!     weighted+renormalized unified ingest bar, the sync position), all clamped to 0…1.
//!   - MONOTONIC CLAMP — `max(fraction, phaseFloor)` within a phase. `phaseFloor` is passed in;
//!     the shell carries the per-`phaseKey` floor across recomputes.
//!   - phaseKey — the stable token for the determinate phase ("ingest", "export.N", or none).
//!
//! The shell keeps PRESENTATION + LOCALIZATION: it maps the structured `Decision` (a tagged line
//! KIND + the counts/percentage/names it needs) into the localized strings the UI renders, and owns
//! the side effects (the debounce timer, the per-phase floor, drawing the bar). The user-facing copy
//! never crosses the FFI line.
//!
//! ## HONESTY (TODO-005 acceptance criteria, preserved verbatim)
//!   - A determinate `fraction` is returned ONLY for work backed by trustworthy job progress: IMPORT
//!     (the OVERALL batch position) and batch EXPORT (per-row %), plus the unified INGEST bar that
//!     composes them. Everything else is indeterminate (`fraction == None` ⇒ a moving bar).
//!   - PAIRING has no real fraction (one synchronous FFI call) ⇒ ALWAYS indeterminate.
//!   - A determinate fraction is MONOTONIC within a phase (`phaseFloor`).
//!   - The strip appears only AFTER a debounce (~250 ms) so quick tasks don't flash; a `.failed`
//!     signal is shown IMMEDIATELY (an error is a result to act on, not "quick work").
//!
//! ## CONTRACT (the testable invariants, mirrored from the Swift tests)
//!   1. IDLE ⇒ NO LINE — no active signal ⇒ `None`.
//!   2. DEBOUNCE — an active job younger than `debounceMs` ⇒ `None`; at/after it ⇒ a line. A failure
//!      is shown immediately.
//!   3. PAIRING IS INDETERMINATE — a pairing signal ALWAYS yields `fraction == None`.
//!   4. IMPORT / EXPORT ARE DETERMINATE — clamped to `phaseFloor` for monotonicity; import is ONE
//!      continuous bar across the batch (a single `phaseKey`).
//!   5. QUEUED WORK SHOWS POSITION — multiple queued sync couples yield a position line.
//!   6. PRIORITY WHEN SEVERAL RUN — exactly one line wins (failure ▸ ingest ▸ export ▸ import ▸
//!      pairing ▸ sync).

use serde::{Deserialize, Serialize};

/// The debounce before any non-error strip appears (TODO-005: "~200–300 ms so quick tasks do not
/// flash"). The midpoint of the spec window. The shell reads this to drive its timer; core uses the
/// caller-supplied `debounceMs` (defaulting to this) so the decision stays a pure function of inputs.
pub const DEBOUNCE_MS: f64 = 250.0;

/// One stable phase token for the whole IMPORT/INGEST phase, so the determinate bar is monotonic
/// across every file and sub-phase (the shell carries its floor across the batch and resets it only
/// on idle). MATCHING within a standalone pairing is a SEPARATE, indeterminate phase.
pub const INGEST_PHASE_KEY: &str = "ingest";

/// Default top-level INGEST stage weights (Open Decision 2), summed only over the PRESENT stages.
const IMPORT_WEIGHT: f64 = 0.50;
const MATCH_WEIGHT: f64 = 0.20;
const SYNC_WEIGHT: f64 = 0.30;

// ---------- signal snapshot (the Request) ----------

/// The three core matching phases — each maps a core phase string EXACTLY: `MatchFingerprint` ⇒
/// `fingerprinting`, `MatchCorrelate` ⇒ `comparing`, `MatchGraph` ⇒ `crosschecking`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MatchPhase {
    Fingerprinting,
    Comparing,
    Crosschecking,
}

/// Honest import phases (TODO-005). Only phases the core import job reports DISTINCTLY get a distinct
/// re-join weight; the rest collapse into waveform/finishing rather than inventing precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportPhase {
    Probing,
    Decoding,
    Waveform,
    Finishing,
}

/// Import / analysis / extraction (MediaPool's import job). Determinate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Importing {
    pub phase: ImportPhase,
    /// 0…1 within `phase`, trustworthy (from the core job).
    pub fraction: f64,
    /// 1-based file position when a batch is known.
    #[serde(default)]
    pub index: Option<u32>,
    /// Batch size when known.
    #[serde(default)]
    pub total: Option<u32>,
}

/// Candidate PAIRING (the resolver). Indeterminate by construction — COUNTS only, never a fraction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pairing {
    pub video_count: u32,
    pub audio_count: u32,
    /// `true` once the matrix is built and rows are being resolved.
    #[serde(default)]
    pub resolving: bool,
    /// `true` when this is a re-resolve after a later import.
    #[serde(default)]
    pub after_import: bool,
    /// `true` when this pairing is the MATCHING TAIL of an import operation (reads "Matching
    /// couples…" as an indeterminate moving bar). A standalone re-resolve leaves it false.
    #[serde(default)]
    pub import_tail: bool,
}

/// Queued / running per-couple auto-sync. Summary-level indeterminate (position, not a partial bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Syncing {
    /// 1-based position of the running couple.
    pub index: u32,
    /// Couples in this sync batch (running + queued).
    pub total: u32,
}

/// Batch export (BatchExport + per-row status). Determinate (per-row %).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exporting {
    /// 1-based row being exported.
    pub index: u32,
    /// Approved couples in the batch.
    pub total: u32,
    /// 0…1 of the current row (from the core job).
    pub row_fraction: f64,
    /// Disk preflight before the first row.
    #[serde(default)]
    pub preflight: bool,
}

/// Incremental MATCHING progress (the determinate match job). Used ONLY by the unified ingest
/// descriptor; the standalone pairing line stays indeterminate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Matching {
    pub phase: MatchPhase,
    /// 0…1 within `phase` (from the core job).
    pub fraction: f64,
}

/// Which ingest STAGES are planned (unified-bar boundary). Modeled as three booleans (the Swift
/// `Stages` OptionSet) so the weighted bar renormalizes over exactly the present stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stages {
    #[serde(default)]
    pub import_stage: bool,
    #[serde(default)]
    pub match_stage: bool,
    #[serde(default)]
    pub sync_stage: bool,
}

/// The ONE in-flight INGEST operation (import → match → sync), composed into a single 0→100% bar.
/// Set ONLY by the shell's ingest orchestration; a standalone op leaves it absent.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Ingest {
    #[serde(default)]
    pub importing: Option<Importing>,
    #[serde(default)]
    pub matching: Option<Matching>,
    #[serde(default)]
    pub syncing: Option<Syncing>,
    pub stages_planned: Stages,
}

/// The activity-decision request: a snapshot of every long-running signal, plus the EXPLICIT time
/// inputs (no clock is read in core). Absent (`None`) members mean "that kind of work is not
/// happening". (Not `Copy`: `failure` carries an owned `String`.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    #[serde(default)]
    pub importing: Option<Importing>,
    #[serde(default)]
    pub pairing: Option<Pairing>,
    #[serde(default)]
    pub syncing: Option<Syncing>,
    #[serde(default)]
    pub exporting: Option<Exporting>,
    /// The single unified ingest descriptor; absent for standalone operations.
    #[serde(default)]
    pub ingest: Option<Ingest>,
    /// An active operation FAILED — surfaced immediately as an actionable error (no bar). The shell
    /// supplies an already-actionable message; core carries it through verbatim.
    #[serde(default)]
    pub failure: Option<String>,
    /// How long the CURRENT foreground operation has been active, in ms (EXPLICIT — no clock in
    /// core). Below `debounceMs` a non-error operation yields no line.
    pub elapsed_ms: f64,
    /// The debounce window in ms. Optional; defaults to `DEBOUNCE_MS` so the shell may omit it.
    #[serde(default = "default_debounce")]
    pub debounce_ms: f64,
    /// The highest determinate fraction already shown for the WINNING phase, or `None` if the phase
    /// just changed / is indeterminate. The result's `fraction` is clamped UP to this (monotonic).
    #[serde(default)]
    pub phase_floor: Option<f64>,
}

fn default_debounce() -> f64 {
    DEBOUNCE_MS
}

// ---------- structured decision (the Response) ----------

/// The line-KIND the shell formats. Each variant carries exactly the structured data the shell needs
/// to assemble the localized string; the shell owns ALL copy. (Pairing collapses into the
/// presentation-relevant cases so the shell does not re-derive the priority/debounce decision.)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum LineKind {
    /// A failure to act on (the message is carried verbatim; shell shows it as an error).
    Failure { message: String },
    /// Import line (batch position + percentage). `index`/`total`/`filename` are presentation hints
    /// the shell already has; only the math (the overall fraction → `percent`) is core's.
    Import { index: Option<u32>, total: Option<u32>, percent: u32 },
    /// Export, mid-row: "Exporting i of n[: file] — p%".
    Export { index: u32, total: u32, percent: u32 },
    /// Export preflight ("Preparing export…").
    ExportPreflight,
    /// Export row finished ("Finishing export…", a full-bar end state, not a stuck 100%).
    ExportFinishing,
    /// Pairing, matching tail of an import ("Matching couples…", indeterminate).
    PairingMatchingTail,
    /// Pairing, resolve sub-phase ("Resolving matches…").
    PairingResolving,
    /// Pairing, a re-resolve after a later import ("Updating matches after import…").
    PairingAfterImport,
    /// Pairing, the V×A comparison counts ("Comparing V videos with A audio files…").
    #[serde(rename_all = "camelCase")]
    PairingComparing { video_count: u32, audio_count: u32 },
    /// Pairing, only one role present ("Analyzing N files for matches…").
    PairingAnalyzing { count: u32 },
    /// Queued sync position ("Syncing couple i of n[: name]").
    Syncing { index: u32, total: u32 },
    /// The unified ingest line whose ACTIVE stage is import (text follows the import line).
    IngestImport { index: Option<u32>, total: Option<u32>, percent: u32 },
    /// The unified ingest line whose active stage is match ("Matching couples…").
    IngestMatching,
    /// The unified ingest line whose active stage is sync ("Syncing couple i of n[: name]").
    IngestSyncing { index: u32, total: u32 },
    /// The unified ingest line during a hand-off gap (no sub-stage live). The shell shows a neutral
    /// "Working…" line; the floor holds the fraction.
    IngestWorking,
}

/// The structured decision: which line to show + the computed determinate fraction / phaseKey /
/// error flag. The shell maps `line` to localized text. (`None` from `decide` means "show nothing".)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Decision {
    /// The structured line to render.
    pub line: LineKind,
    /// Trustworthy 0…1 for a determinate bar, or `None` for an indeterminate one. Already clamped to
    /// `phaseFloor` (monotonic). Never fabricated — only import/export/ingest ever populate this.
    pub fraction: Option<f64>,
    /// This decision is a failure to act on (the shell renders an error treatment, no bar).
    pub is_error: bool,
    /// A stable token for the determinate PHASE ("ingest", "export.N"), or `None` for an
    /// indeterminate/error line. The shell carries a per-phase monotonic floor keyed on this.
    #[serde(default)]
    pub phase_key: Option<String>,
}

/// The decision output: either a line to show, or `None` ("show nothing"). Wrapped in a struct so
/// the FFI envelope `value` is a JSON object (`{"summary": Decision|null}`) rather than a bare null,
/// keeping the envelope contract uniform with the other policies.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    /// The line to show, or `None` for "show nothing".
    pub summary: Option<Decision>,
}

// ---------- the decision ----------

/// Decide the single activity-strip line for the snapshot (TODO-005). Pure; no ctx, no clock — time
/// and debounce are EXPLICIT request fields.
pub fn decide(req: &Request) -> Response {
    Response { summary: summarize(req) }
}

fn summarize(req: &Request) -> Option<Decision> {
    // (2)/(failure) A failure is a result to act on — surface it immediately, ahead of any debounce
    // and any concurrent progress (this holds mid-ingest too).
    if let Some(failure) = &req.failure {
        return Some(Decision {
            line: LineKind::Failure { message: failure.clone() },
            fraction: None,
            is_error: true,
            phase_key: None,
        });
    }

    // (6) Exactly one foreground operation wins, most user-relevant first. Compute the RAW line…
    let raw = winning_line(req)?; // (1) idle ⇒ nothing

    // (2) …then gate non-error lines on the debounce so quick work never flashes.
    if req.elapsed_ms < req.debounce_ms {
        return None;
    }

    // (4) Monotonic clamp for determinate phases only.
    if let (Some(f), Some(floor)) = (raw.fraction, req.phase_floor) {
        return Some(Decision { fraction: Some(f.max(floor)), ..raw });
    }
    Some(raw)
}

/// The winning raw line (no debounce / no monotonic clamp), given the priority order. When an INGEST
/// operation is in flight it OWNS the line (the unified bar under `phaseKey = "ingest"`); standalone
/// export/import/pairing/sync keep their own lines.
fn winning_line(s: &Request) -> Option<Decision> {
    if let Some(ingest) = &s.ingest {
        return Some(unified_line(ingest));
    }
    if let Some(e) = &s.exporting {
        return Some(export_line(e));
    }
    if let Some(i) = &s.importing {
        return Some(import_line(i));
    }
    if let Some(p) = &s.pairing {
        return Some(pairing_line(p));
    }
    if let Some(y) = &s.syncing {
        return Some(sync_line(y));
    }
    None
}

// ---------- unified ingest bar ----------

/// Compose an INGEST operation into ONE 0→100% line. The bar is a WEIGHTED sub-range blend across the
/// planned stages — import 50% / match 20% / sync 30%, renormalized over exactly the present stages.
/// The KIND follows the currently-active (latest-started present) stage; the FRACTION is the blended
/// overall. Always carries `phaseKey = "ingest"`. A present ingest ALWAYS yields a line (even with no
/// sub-stage momentarily live — a hand-off gap), so the bar stays visible for the whole span.
fn unified_line(ingest: &Ingest) -> Decision {
    let overall = unified_fraction(ingest);
    let line = unified_kind(ingest);
    Decision {
        line,
        fraction: Some(overall),
        is_error: false,
        phase_key: Some(INGEST_PHASE_KEY.to_string()),
    }
}

/// The weighted, renormalized 0…1 across the planned ingest stages.
fn unified_fraction(ingest: &Ingest) -> f64 {
    let stages = ingest.stages_planned;
    let weight = |present: bool, w: f64| if present { w } else { 0.0 };
    let total = weight(stages.import_stage, IMPORT_WEIGHT)
        + weight(stages.match_stage, MATCH_WEIGHT)
        + weight(stages.sync_stage, SYNC_WEIGHT);
    if total <= 0.0 {
        return 0.0;
    }

    // The latest-started present stage is "active"; every EARLIER present stage reads complete (1.0).
    let match_active = ingest.matching.is_some();
    let sync_active = ingest.syncing.is_some();

    let stage_fraction = |stage: char| -> f64 {
        match stage {
            'i' => match ingest.importing {
                Some(i) => batch_import_fraction(&i),
                // Import planned but no longer reporting ⇒ a later stage started ⇒ import done.
                None => {
                    if match_active || sync_active {
                        1.0
                    } else {
                        0.0
                    }
                }
            },
            'm' => match ingest.matching {
                Some(m) => per_match_fraction(m.phase, m.fraction),
                // Matching planned, not yet active: 0 while import runs; 1.0 once sync started.
                None => {
                    if sync_active {
                        1.0
                    } else {
                        0.0
                    }
                }
            },
            's' => match ingest.syncing {
                // Position-only (Open Decision 4): index/total, clamped 0…1.
                Some(y) if y.total > 0 => (y.index as f64 / y.total as f64).clamp(0.0, 1.0),
                _ => 0.0,
            },
            _ => 0.0,
        }
    };

    let mut sum = 0.0;
    if stages.import_stage {
        sum += (IMPORT_WEIGHT / total) * stage_fraction('i');
    }
    if stages.match_stage {
        sum += (MATCH_WEIGHT / total) * stage_fraction('m');
    }
    if stages.sync_stage {
        sum += (SYNC_WEIGHT / total) * stage_fraction('s');
    }
    sum.clamp(0.0, 1.0)
}

/// The active-stage KIND for the unified ingest line (the active stage is the latest-started present
/// stage; import ▸ match ▸ sync hand off in order).
fn unified_kind(ingest: &Ingest) -> LineKind {
    if let Some(y) = ingest.syncing {
        return LineKind::IngestSyncing { index: y.index, total: y.total };
    }
    if ingest.matching.is_some() {
        return LineKind::IngestMatching;
    }
    if let Some(i) = ingest.importing {
        // The import-stage text reuses the import line's fields (batch position + overall %).
        let percent = (batch_import_fraction(&i) * 100.0).round() as u32;
        return LineKind::IngestImport { index: i.index, total: i.total, percent };
    }
    LineKind::IngestWorking
}

// ---------- per-kind lines ----------

/// Export (determinate). The per-row fraction is the trustworthy number; the row position gives the
/// batch context.
fn export_line(e: &Exporting) -> Decision {
    if e.preflight {
        return Decision {
            line: LineKind::ExportPreflight,
            fraction: None,
            is_error: false,
            phase_key: None,
        };
    }
    let f = clamp01(e.row_fraction);
    // A finished-but-not-yet-idle row reads "Finishing export…" rather than a stuck 100% bar.
    if f >= 1.0 {
        return Decision {
            line: LineKind::ExportFinishing,
            fraction: Some(1.0),
            is_error: false,
            phase_key: Some(format!("export.{}", e.index)),
        };
    }
    let percent = (f * 100.0).round() as u32;
    Decision {
        line: LineKind::Export { index: e.index, total: e.total, percent },
        fraction: Some(f),
        is_error: false,
        phase_key: Some(format!("export.{}", e.index)),
    }
}

/// Import — ONE determinate bar for the whole batch, not a per-file reset. The fraction is the OVERALL
/// import progress across the dropped batch, under a single stable `phaseKey` so the shell carries its
/// monotonic floor across every file and phase.
fn import_line(i: &Importing) -> Decision {
    let overall = batch_import_fraction(i);
    let percent = (overall * 100.0).round() as u32;
    Decision {
        line: LineKind::Import { index: i.index, total: i.total, percent },
        fraction: Some(overall),
        is_error: false,
        phase_key: Some(INGEST_PHASE_KEY.to_string()),
    }
}

/// Overall determinate progress of the IMPORT phase (0…1 across the whole batch): the files already
/// finished (`index - 1`) plus the current file's progress, over the batch size. With an unknown batch
/// size it falls back to the single file's progress. Reaches 1.0 only when the last file truly
/// finishes.
fn batch_import_fraction(i: &Importing) -> f64 {
    let pf = per_file_fraction(i.phase, clamp01(i.fraction));
    let v = match (i.index, i.total) {
        (Some(idx), Some(total)) if total > 0 => (f64::from(idx) - 1.0 + pf) / f64::from(total),
        _ => pf,
    };
    v.clamp(0.0, 1.0)
}

/// One file's import progress 0…1, re-joining the core's per-phase 0→1 emissions back into the single
/// per-file 0→1 using the core's own split: decode is 80%, waveform the next 15%, finishing the last
/// 5% (core/src/import.rs). (Probe precedes the 0→1 work ⇒ 0.)
fn per_file_fraction(phase: ImportPhase, f: f64) -> f64 {
    match phase {
        ImportPhase::Probing => 0.0,
        ImportPhase::Decoding => 0.80 * f, // overall 0 → 0.80
        ImportPhase::Waveform => 0.80 + 0.15 * f, // overall 0.80 → 0.95
        ImportPhase::Finishing => 0.95 + 0.05 * f, // overall 0.95 → 1.0
    }
}

/// One matching stage's progress 0…1, re-joining the three core match phases into a single 0→1 over
/// the whole match stage (fingerprint 60% / compare 35% / cross-check 5%, Open Decision 1) — the
/// analogue of `per_file_fraction` for matching.
pub fn per_match_fraction(phase: MatchPhase, f: f64) -> f64 {
    let f = clamp01(f);
    match phase {
        MatchPhase::Fingerprinting => 0.60 * f, // overall 0 → 0.60
        MatchPhase::Comparing => 0.60 + 0.35 * f, // overall 0.60 → 0.95
        MatchPhase::Crosschecking => 0.95 + 0.05 * f, // overall 0.95 → 1.0
    }
}

/// Pairing — ALWAYS indeterminate (the candidate matrix is one synchronous FFI call with no live %).
/// The MATCHING TAIL of an import reads "Matching couples…"; a STANDALONE pairing is a count summary.
fn pairing_line(p: &Pairing) -> Decision {
    let line = if p.import_tail {
        LineKind::PairingMatchingTail
    } else if p.resolving {
        LineKind::PairingResolving
    } else if p.after_import {
        LineKind::PairingAfterImport
    } else if p.video_count > 0 && p.audio_count > 0 {
        LineKind::PairingComparing { video_count: p.video_count, audio_count: p.audio_count }
    } else {
        LineKind::PairingAnalyzing { count: p.video_count + p.audio_count }
    };
    Decision { line, fraction: None, is_error: false, phase_key: None }
}

/// Queued auto-sync (summary-level indeterminate). The per-couple line keeps the phase detail.
fn sync_line(y: &Syncing) -> Decision {
    Decision {
        line: LineKind::Syncing { index: y.index, total: y.total },
        fraction: None,
        is_error: false,
        phase_key: None,
    }
}

/// Clamp a reported fraction into 0…1 so a slightly-out-of-range job value can never produce a >100%
/// / negative bar.
fn clamp01(v: f64) -> f64 {
    v.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHOWN: f64 = DEBOUNCE_MS + 10.0;

    fn req() -> Request {
        Request {
            importing: None,
            pairing: None,
            syncing: None,
            exporting: None,
            ingest: None,
            failure: None,
            elapsed_ms: SHOWN,
            debounce_ms: DEBOUNCE_MS,
            phase_floor: None,
        }
    }

    // (1) IDLE ⇒ NO LINE — no active signal ⇒ None, regardless of elapsed time.
    #[test]
    fn idle_shows_nothing() {
        let r = Request { elapsed_ms: 99999.0, ..req() };
        assert!(decide(&r).summary.is_none());
    }

    // (2) DEBOUNCE — an active job younger than the debounce flashes nothing; at/after it appears.
    #[test]
    fn debounce_suppresses_quick_work() {
        let base = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: 0.4,
                index: None,
                total: None,
            }),
            ..req()
        };
        let at = |ms: f64| decide(&Request { elapsed_ms: ms, ..base.clone() }).summary;
        assert!(at(100.0).is_none());
        assert!(at(249.0).is_none());
        assert!(at(250.0).is_some());
        assert!(at(5000.0).is_some());
    }

    // (3) PAIRING IS INDETERMINATE — counts, NEVER a fraction; the right structured kind each way.
    #[test]
    fn pairing_is_indeterminate_with_counts() {
        let comparing = Request {
            pairing: Some(Pairing {
                video_count: 4,
                audio_count: 8,
                resolving: false,
                after_import: false,
                import_tail: false,
            }),
            ..req()
        };
        let d = decide(&comparing).summary.unwrap();
        assert!(d.fraction.is_none());
        assert!(!d.is_error);
        assert_eq!(d.line, LineKind::PairingComparing { video_count: 4, audio_count: 8 });

        // Only one role present ⇒ the generic "analyzing" kind with the summed count.
        let analyzing = Request {
            pairing: Some(Pairing {
                video_count: 0,
                audio_count: 12,
                resolving: false,
                after_import: false,
                import_tail: false,
            }),
            ..req()
        };
        assert_eq!(
            decide(&analyzing).summary.unwrap().line,
            LineKind::PairingAnalyzing { count: 12 }
        );

        // Resolve sub-phase.
        let resolving = Request {
            pairing: Some(Pairing {
                video_count: 4,
                audio_count: 8,
                resolving: true,
                after_import: false,
                import_tail: false,
            }),
            ..req()
        };
        let r = decide(&resolving).summary.unwrap();
        assert_eq!(r.line, LineKind::PairingResolving);
        assert!(r.fraction.is_none());

        // A re-resolve after a later import.
        let update = Request {
            pairing: Some(Pairing {
                video_count: 2,
                audio_count: 3,
                resolving: false,
                after_import: true,
                import_tail: false,
            }),
            ..req()
        };
        assert_eq!(decide(&update).summary.unwrap().line, LineKind::PairingAfterImport);

        // The import-tail matching reads the tail kind, still indeterminate.
        let tail = Request {
            pairing: Some(Pairing {
                video_count: 2,
                audio_count: 3,
                resolving: false,
                after_import: false,
                import_tail: true,
            }),
            ..req()
        };
        let t = decide(&tail).summary.unwrap();
        assert_eq!(t.line, LineKind::PairingMatchingTail);
        assert!(t.fraction.is_none());
    }

    // (4) IMPORT ⇒ ONE determinate bar for the whole batch, under a single phaseKey.
    #[test]
    fn import_is_one_determinate_batch_bar() {
        // Mid-batch: file 3 of 12, 42% through DECODE ⇒ overall ≈ (2 + 0.80·0.42)/12.
        let decoding = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: 0.42,
                index: Some(3),
                total: Some(12),
            }),
            ..req()
        };
        let d = decide(&decoding).summary.unwrap();
        assert!((d.fraction.unwrap() - (2.0 + 0.80 * 0.42) / 12.0).abs() < 1e-9);
        assert_eq!(d.line, LineKind::Import { index: Some(3), total: Some(12), percent: 19 });
        assert_eq!(d.phase_key.as_deref(), Some("ingest"));

        // The very first file just starting ⇒ 0%.
        let first = Request {
            importing: Some(Importing {
                phase: ImportPhase::Probing,
                fraction: 0.0,
                index: Some(1),
                total: Some(4),
            }),
            ..req()
        };
        let p = decide(&first).summary.unwrap();
        assert_eq!(p.fraction, Some(0.0));
        assert_eq!(p.line, LineKind::Import { index: Some(1), total: Some(4), percent: 0 });

        // A single file at the END OF DECODE is only 80%, not "done".
        let mid_solo = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: 1.0,
                index: Some(1),
                total: Some(1),
            }),
            ..req()
        };
        let ms = decide(&mid_solo).summary.unwrap();
        assert_eq!(ms.fraction, Some(0.80));
        assert_eq!(ms.line, LineKind::Import { index: Some(1), total: Some(1), percent: 80 });

        // …and reaches 100% only when that last file TRULY finishes.
        let done_solo = Request {
            importing: Some(Importing {
                phase: ImportPhase::Finishing,
                fraction: 1.0,
                index: Some(1),
                total: Some(1),
            }),
            ..req()
        };
        let s = decide(&done_solo).summary.unwrap();
        assert_eq!(s.fraction, Some(1.0));
        assert_eq!(s.line, LineKind::Import { index: Some(1), total: Some(1), percent: 100 });
    }

    // (unified bar) the import-tail matching is a DETERMINATE slice of the unified bar.
    #[test]
    fn matching_tail_is_determinate_in_unified_bar() {
        let ingest = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: Some(Matching { phase: MatchPhase::Comparing, fraction: 0.5 }),
                syncing: None,
                stages_planned: Stages {
                    import_stage: true,
                    match_stage: true,
                    sync_stage: true,
                },
            }),
            ..req()
        };
        let t = decide(&ingest).summary.unwrap();
        assert_eq!(t.line, LineKind::IngestMatching);
        assert!(t.fraction.is_some());
        assert_eq!(t.phase_key.as_deref(), Some("ingest"));
        assert!(!t.is_error);
        // import (50%) done = 0.50, plus match weight (20%) × perMatch(comparing,0.5)=0.775.
        let expected = 0.50 + 0.20 * (0.60 + 0.35 * 0.5);
        assert!((t.fraction.unwrap() - expected).abs() < 1e-9);
    }

    // (unified bar: per-match split) the three match phases re-join into one 0→1.
    #[test]
    fn per_match_fraction_splits_the_phases() {
        assert_eq!(per_match_fraction(MatchPhase::Fingerprinting, 0.5), 0.60 * 0.5);
        assert_eq!(per_match_fraction(MatchPhase::Fingerprinting, 1.0), 0.60);
        assert_eq!(per_match_fraction(MatchPhase::Comparing, 0.0), 0.60);
        assert_eq!(per_match_fraction(MatchPhase::Comparing, 1.0), 0.95);
        assert_eq!(per_match_fraction(MatchPhase::Crosschecking, 1.0), 1.0);
        assert_eq!(per_match_fraction(MatchPhase::Fingerprinting, -1.0), 0.0);
    }

    // (unified bar: stage-weight math at known inputs).
    #[test]
    fn unified_stage_weight_math() {
        let stages = Stages { import_stage: true, match_stage: true, sync_stage: true };
        // Mid-IMPORT: file 1 of 2 at decode 0.5 ⇒ batch import (0 + 0.80·0.5)/2 = 0.20.
        let importing = Request {
            ingest: Some(Ingest {
                importing: Some(Importing {
                    phase: ImportPhase::Decoding,
                    fraction: 0.5,
                    index: Some(1),
                    total: Some(2),
                }),
                matching: None,
                syncing: None,
                stages_planned: stages,
            }),
            ..req()
        };
        let i = decide(&importing).summary.unwrap();
        assert!((i.fraction.unwrap() - 0.50 * 0.20).abs() < 1e-9);
        assert_eq!(i.line, LineKind::IngestImport { index: Some(1), total: Some(2), percent: 20 });

        // Mid-SYNC: import+match done, syncing couple 3 of 6 ⇒ 0.50·1 + 0.20·1 + 0.30·0.5 = 0.85.
        let syncing = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: None,
                syncing: Some(Syncing { index: 3, total: 6 }),
                stages_planned: stages,
            }),
            ..req()
        };
        let y = decide(&syncing).summary.unwrap();
        assert!((y.fraction.unwrap() - (0.50 + 0.20 + 0.30 * 0.5)).abs() < 1e-9);
        assert_eq!(y.line, LineKind::IngestSyncing { index: 3, total: 6 });
    }

    // (unified bar: monotonic across stages) import 99% → matching just started must NOT regress.
    #[test]
    fn unified_bar_is_monotonic_across_stages() {
        let stages = Stages { import_stage: true, match_stage: true, sync_stage: true };
        let late_import = Request {
            ingest: Some(Ingest {
                importing: Some(Importing {
                    phase: ImportPhase::Finishing,
                    fraction: 0.8,
                    index: Some(1),
                    total: Some(1),
                }),
                matching: None,
                syncing: None,
                stages_planned: stages,
            }),
            ..req()
        };
        let li = decide(&late_import).summary.unwrap().fraction.unwrap();
        assert!((li - 0.50 * (0.95 + 0.05 * 0.8)).abs() < 1e-9);
        let match_start = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: Some(Matching { phase: MatchPhase::Fingerprinting, fraction: 0.1 }),
                syncing: None,
                stages_planned: stages,
            }),
            ..req()
        };
        let ms = decide(&match_start).summary.unwrap().fraction.unwrap();
        assert!(ms >= li - 1e-9, "matching start must not regress below late import");
        assert!(ms >= 0.50);
    }

    // (unified bar: missing-stage renormalization).
    #[test]
    fn unified_missing_stage_renormalizes() {
        // Import-only (all-duplicates): import done ⇒ 1.0.
        let import_only = Request {
            ingest: Some(Ingest {
                importing: Some(Importing {
                    phase: ImportPhase::Finishing,
                    fraction: 1.0,
                    index: Some(2),
                    total: Some(2),
                }),
                matching: None,
                syncing: None,
                stages_planned: Stages {
                    import_stage: true,
                    match_stage: false,
                    sync_stage: false,
                },
            }),
            ..req()
        };
        assert_eq!(decide(&import_only).summary.unwrap().fraction, Some(1.0));

        // Re-resolve [match, sync]: weights 20/30 → 40/60. Match done + sync 1 of 2 ⇒ 0.40 + 0.30.
        let reresolve = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: None,
                syncing: Some(Syncing { index: 1, total: 2 }),
                stages_planned: Stages {
                    import_stage: false,
                    match_stage: true,
                    sync_stage: true,
                },
            }),
            ..req()
        };
        let rr = decide(&reresolve).summary.unwrap().fraction.unwrap();
        assert!((rr - (0.40 + 0.60 * 0.5)).abs() < 1e-9);

        // Matching mid (comparing 0.5) BEFORE sync ⇒ only the renormalized match sub-range fills.
        let matching_only = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: Some(Matching { phase: MatchPhase::Comparing, fraction: 0.5 }),
                syncing: None,
                stages_planned: Stages {
                    import_stage: false,
                    match_stage: true,
                    sync_stage: true,
                },
            }),
            ..req()
        };
        let mo = decide(&matching_only).summary.unwrap().fraction.unwrap();
        assert!((mo - 0.40 * (0.60 + 0.35 * 0.5)).abs() < 1e-9);
    }

    // (unified bar: reaches 1.0 only at sync end; hand-off gap holds; true idle clears).
    #[test]
    fn unified_reaches_100_only_at_sync_end_and_clears_idle() {
        let stages = Stages { import_stage: true, match_stage: true, sync_stage: true };
        let near_end = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: None,
                syncing: Some(Syncing { index: 6, total: 7 }),
                stages_planned: stages,
            }),
            ..req()
        };
        assert!(decide(&near_end).summary.unwrap().fraction.unwrap() < 1.0);
        let done = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: None,
                syncing: Some(Syncing { index: 7, total: 7 }),
                stages_planned: stages,
            }),
            ..req()
        };
        assert_eq!(decide(&done).summary.unwrap().fraction, Some(1.0));
        // A hand-off GAP (ingest in flight, no sub-stage live) HOLDS the bar — it does NOT clear.
        let gap = Request {
            ingest: Some(Ingest {
                importing: None,
                matching: None,
                syncing: None,
                stages_planned: stages,
            }),
            ..req()
        };
        let g = decide(&gap).summary.unwrap();
        assert_eq!(g.line, LineKind::IngestWorking);
        // TRUE idle = no descriptor at all ⇒ clears.
        assert!(decide(&req()).summary.is_none());
    }

    // (5) QUEUED SYNC ⇒ position kind; indeterminate at the summary level.
    #[test]
    fn queued_sync_shows_position() {
        let s = Request { syncing: Some(Syncing { index: 2, total: 7 }), ..req() };
        let y = decide(&s).summary.unwrap();
        assert_eq!(y.line, LineKind::Syncing { index: 2, total: 7 });
        assert!(y.fraction.is_none());
        assert!(!y.is_error);
    }

    // (export ⇒ determinate) row position + per-row %; preflight + finishing edges.
    #[test]
    fn export_is_determinate() {
        let e = Request {
            exporting: Some(Exporting {
                index: 2,
                total: 5,
                row_fraction: 0.61,
                preflight: false,
            }),
            ..req()
        };
        let x = decide(&e).summary.unwrap();
        assert_eq!(x.fraction, Some(0.61));
        assert_eq!(x.line, LineKind::Export { index: 2, total: 5, percent: 61 });
        assert_eq!(x.phase_key.as_deref(), Some("export.2"));

        // Preflight ⇒ indeterminate.
        let pre = Request {
            exporting: Some(Exporting {
                index: 0,
                total: 3,
                row_fraction: 0.0,
                preflight: true,
            }),
            ..req()
        };
        let pf = decide(&pre).summary.unwrap();
        assert_eq!(pf.line, LineKind::ExportPreflight);
        assert!(pf.fraction.is_none());

        // A row at 100% reads the finishing kind.
        let done = Request {
            exporting: Some(Exporting {
                index: 3,
                total: 3,
                row_fraction: 1.0,
                preflight: false,
            }),
            ..req()
        };
        assert_eq!(decide(&done).summary.unwrap().line, LineKind::ExportFinishing);
    }

    // (failure ⇒ error state) shown immediately (skips the debounce), no fraction, isError set.
    #[test]
    fn failure_is_error_not_stuck_bar() {
        let s = Request {
            failure: Some("2 exported, 1 failed — disk full".to_string()),
            elapsed_ms: 0.0,
            ..req()
        };
        let f = decide(&s).summary.unwrap();
        assert!(f.is_error);
        assert!(f.fraction.is_none());
        assert_eq!(f.line, LineKind::Failure { message: "2 exported, 1 failed — disk full".to_string() });
        // A failure wins even when other work is concurrently active.
        let concurrent = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: 0.5,
                index: None,
                total: None,
            }),
            failure: Some("import failed: bad file".to_string()),
            ..req()
        };
        assert!(decide(&concurrent).summary.unwrap().is_error);
    }

    // (monotonic) a regressing fraction is clamped UP to the floor; a higher reported fraction wins.
    #[test]
    fn determinate_fraction_is_monotonic_within_phase() {
        // 0.30 through decode ⇒ 0.24; a floor of 0.55 already shown ⇒ never regress.
        let s = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: 0.30,
                index: None,
                total: None,
            }),
            phase_floor: Some(0.55),
            ..req()
        };
        assert_eq!(decide(&s).summary.unwrap().fraction, Some(0.55));
        // A higher reported fraction wins over the floor (waveform 0.5 ⇒ 0.875 > 0.55).
        let ahead = Request {
            importing: Some(Importing {
                phase: ImportPhase::Waveform,
                fraction: 0.5,
                index: None,
                total: None,
            }),
            phase_floor: Some(0.55),
            ..req()
        };
        assert_eq!(decide(&ahead).summary.unwrap().fraction, Some(0.80 + 0.15 * 0.5));
        // No floor ⇒ the raw overall fraction.
        let s2 = Request { phase_floor: None, ..s.clone() };
        assert_eq!(decide(&s2).summary.unwrap().fraction, Some(0.80 * 0.30));
    }

    // (out-of-range fractions clamp to 0…1).
    #[test]
    fn fractions_clamp_to_unit_range() {
        let over = Request {
            exporting: Some(Exporting {
                index: 1,
                total: 1,
                row_fraction: 1.4,
                preflight: false,
            }),
            ..req()
        };
        assert_eq!(decide(&over).summary.unwrap().fraction, Some(1.0));
        let under = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: -0.2,
                index: None,
                total: None,
            }),
            ..req()
        };
        assert_eq!(decide(&under).summary.unwrap().fraction, Some(0.0));
    }

    // (6) PRIORITY WHEN SEVERAL RUN — exactly one line wins: export ▸ import ▸ pairing ▸ sync.
    #[test]
    fn priority_picks_the_most_relevant_single_line() {
        let all = Request {
            importing: Some(Importing {
                phase: ImportPhase::Decoding,
                fraction: 0.5,
                index: None,
                total: None,
            }),
            pairing: Some(Pairing {
                video_count: 2,
                audio_count: 2,
                resolving: false,
                after_import: false,
                import_tail: false,
            }),
            syncing: Some(Syncing { index: 1, total: 2 }),
            exporting: Some(Exporting {
                index: 1,
                total: 1,
                row_fraction: 0.3,
                preflight: false,
            }),
            ..req()
        };
        // Export wins.
        assert_eq!(
            decide(&all).summary.unwrap().line,
            LineKind::Export { index: 1, total: 1, percent: 30 }
        );

        // No export ⇒ import wins over pairing/sync.
        let no_export = Request { exporting: None, ..all.clone() };
        assert_eq!(
            decide(&no_export).summary.unwrap().line,
            LineKind::Import { index: None, total: None, percent: 40 }
        );

        // No import ⇒ pairing wins over sync.
        let pair_then_sync = Request { importing: None, ..no_export.clone() };
        assert_eq!(
            decide(&pair_then_sync).summary.unwrap().line,
            LineKind::PairingComparing { video_count: 2, audio_count: 2 }
        );
    }

    // Request/Response JSON round-trips (the camelCase wire the FFI envelope carries).
    #[test]
    fn request_deserializes_camel_case() {
        // debounceMs and phaseFloor are optional (defaulted); a minimal snapshot parses.
        let req: Request = serde_json::from_str(
            r#"{"pairing":{"videoCount":1,"audioCount":1},"elapsedMs":300}"#,
        )
        .unwrap();
        let d = decide(&req).summary.unwrap();
        assert_eq!(d.line, LineKind::PairingComparing { video_count: 1, audio_count: 1 });
        // The structured decision serializes the tagged kind + fields.
        let json = serde_json::to_string(&decide(&req)).unwrap();
        assert!(json.contains("\"kind\":\"pairingComparing\""), "kind tag: {json}");

        // A full ingest snapshot with explicit time fields round-trips too.
        let req2: Request = serde_json::from_str(
            r#"{"ingest":{"matching":{"phase":"comparing","fraction":0.5},
                "stagesPlanned":{"importStage":true,"matchStage":true,"syncStage":true}},
                "elapsedMs":300,"debounceMs":250,"phaseFloor":0.1}"#,
        )
        .unwrap();
        assert_eq!(decide(&req2).summary.unwrap().line, LineKind::IngestMatching);
    }
}
