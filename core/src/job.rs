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
            Phase::ExportRender => "Rendering audio…",
            Phase::ExportMux => "Writing file…",
        }
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
