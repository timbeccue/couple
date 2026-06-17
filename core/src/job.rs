//! Jobs, progress, cancellation (§3.5, §4.6).

use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub type JobId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Phase {
    Probe,
    Decode,
    Pyramid,
    Finalize,
    SyncCoarse,
    SyncRefine,
    SyncWindows,
    SyncFit,
    MatchFingerprint,
    MatchCorrelate,
    MatchGraph,
    Verify,
    Resolve,
    ExportRender,
    ExportMux,
}

impl Phase {
    /// UI copy per UX §3 / §4.6.
    pub fn label(&self) -> &'static str {
        match self {
            Phase::Probe => "Probing…",
            Phase::Decode => "Decoding…",
            Phase::Pyramid => "Building waveform…",
            Phase::Finalize => "Finishing…",
            Phase::SyncCoarse => "Searching…",
            Phase::SyncRefine => "Refining…",
            Phase::SyncWindows => "Measuring drift…",
            Phase::SyncFit => "Fitting…",
            Phase::MatchFingerprint => "Fingerprinting…",
            Phase::MatchCorrelate => "Comparing…",
            Phase::MatchGraph => "Cross-checking…",
            Phase::Verify => "Verifying…",
            Phase::Resolve => "Resolving…",
            Phase::ExportRender => "Rendering audio…",
            Phase::ExportMux => "Writing file…",
        }
    }

    /// Cumulative 0–1 band this phase occupies within its job's single progress bar
    /// (spec §4.6). Defined here beside `label()` so a phase's display/cost metadata
    /// lives in ONE place: adding or reordering a phase is a single edit and the
    /// compiler enforces completeness. Shells render `overall(fraction)` — they never
    /// re-derive the per-phase weighting (e.g. fingerprinting dominates first-match cost).
    fn bar_range(self) -> (f64, f64) {
        match self {
            Phase::Probe => (0.0, 0.02),
            Phase::Decode => (0.02, 0.80),
            Phase::Pyramid => (0.80, 0.95),
            Phase::Finalize => (0.95, 1.0),
            Phase::SyncCoarse => (0.0, 0.70),
            Phase::SyncRefine => (0.70, 0.75),
            Phase::SyncWindows => (0.75, 0.98),
            Phase::SyncFit => (0.98, 1.0),
            // Match job: fingerprinting is the dominant first-match cost, then per-row
            // correlation, then the §13 graph-consistency pass ends at 1.0. (The standalone
            // `cpl_match_candidates_graph_start` job terminates on `MatchGraph` at 1.0 — these
            // three bands are UNCHANGED so that job's bar is unaffected.)
            Phase::MatchFingerprint => (0.0, 0.70),
            Phase::MatchCorrelate => (0.70, 0.95),
            Phase::MatchGraph => (0.95, 1.0),
            // Resolve job (couple_core::resolve): the matrix build above carries the bar to
            // ~1.0; the cheap row pass ends on `Resolve` at 1.0 (a terminal tick, monotone with
            // MatchGraph's 1.0). The LEGACY verify-active / RMS-escalation path does real work
            // AFTER the matrix (which terminates the legacy build on `MatchCorrelate` at 0.95),
            // so `Verify` (chunked verify_batch) and `Resolve` (batched pair_fit_batch + the row
            // pass) occupy the 0.95→1.0 tail honestly rather than hiding under MatchGraph.
            Phase::Verify => (0.95, 0.98),
            Phase::Resolve => (0.98, 1.0),
            Phase::ExportRender => (0.0, 0.70),
            Phase::ExportMux => (0.70, 1.0),
        }
    }

    /// This phase's within-phase `fraction` projected onto the job's overall 0–1 bar (§4.6).
    pub fn overall(self, fraction: f64) -> f64 {
        let (lo, hi) = self.bar_range();
        lo + (hi - lo) * fraction.clamp(0.0, 1.0)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Progress {
    pub job: JobId,
    pub phase: Phase,
    pub fraction: f64,
    pub detail: Option<String>,
}

/// Thread-safe progress callback; invoked from worker threads at ≤ 30 Hz.
pub type ProgressFn = Arc<dyn Fn(&Progress) + Send + Sync>;

pub fn noop_progress() -> ProgressFn {
    Arc::new(|_| {})
}

#[derive(Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
    pub fn check(&self) -> crate::error::Result<()> {
        if self.is_cancelled() {
            Err(crate::error::CoupleError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// Rate-limits progress emission to ≤ 30 Hz and keeps fractions monotone within a phase.
pub struct ProgressEmitter {
    job: JobId,
    f: ProgressFn,
    last: Option<(Phase, f64, Instant)>,
    min_interval: Duration,
}

impl ProgressEmitter {
    pub fn new(job: JobId, f: ProgressFn) -> Self {
        Self {
            job,
            f,
            last: None,
            min_interval: Duration::from_millis(34),
        }
    }

    pub fn emit(&mut self, phase: Phase, fraction: f64) {
        let now = Instant::now();
        let fraction = fraction.clamp(0.0, 1.0);
        if let Some((p, frac, t)) = self.last {
            if p == phase {
                if fraction <= frac {
                    return; // monotone within phase
                }
                if now - t < self.min_interval && fraction < 1.0 {
                    return; // ≤ 30 Hz
                }
            }
        }
        self.last = Some((phase, fraction, now));
        (self.f)(&Progress {
            job: self.job,
            phase,
            fraction,
            detail: None,
        });
    }
}
