//! Session + AlignmentState (§4.5; wire schema = UX §4.6 plus version field).
//! Core owns the canonical state; hot setters are atomic/lock-free.

use crate::error::{CoupleError, Result};
use crate::media::{SessionId, TrackId};
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Mutex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DriftMode {
    Resample,
    None,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Frame {
    pub win_ms: f64,
    pub auto: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMeasurement {
    pub lag_ms: f64,
    pub ncc: f64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DriftWindowState {
    pub t: f64,
    pub lag_ms: f64,
    pub ncc: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoupeLayout {
    Grid,
    Stack,
}

/// Wire form of the full state (§4.5). `offsetMs`/`driftPpm`/`driftMode` are stored in
/// atomics on `Session`; the rest lives behind a mutex.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlignmentState {
    pub version: u32,
    pub offset_ms: f64,
    pub drift_ppm: f64,
    pub drift_mode: DriftMode,
    pub saved_drift_ppm: f64,
    pub anchors: Vec<f64>,
    pub frames: Vec<Frame>,
    pub lock_zoom: bool,
    pub per_anchor_measurements: Vec<Option<LocalMeasurement>>,
    pub drift_windows: Vec<DriftWindowState>,
    pub verified: bool,
    pub loupe_layout: LoupeLayout,
}

impl Default for AlignmentState {
    fn default() -> Self {
        Self {
            version: 1,
            offset_ms: 0.0,
            drift_ppm: 0.0,
            drift_mode: DriftMode::Resample,
            saved_drift_ppm: 0.0,
            anchors: Vec::new(),
            frames: Vec::new(),
            lock_zoom: true,
            per_anchor_measurements: Vec::new(),
            drift_windows: Vec::new(),
            verified: false,
            loupe_layout: LoupeLayout::Grid,
        }
    }
}

#[derive(Debug, Default)]
struct ColdState {
    saved_drift_ppm: f64,
    anchors: Vec<f64>,
    frames: Vec<Frame>,
    lock_zoom: bool,
    per_anchor_measurements: Vec<Option<LocalMeasurement>>,
    drift_windows: Vec<DriftWindowState>,
    verified: bool,
    loupe_layout_grid: bool,
}

pub struct Session {
    pub id: SessionId,
    pub camera: TrackId,
    pub external: TrackId,
    offset_ms_bits: AtomicU64,
    drift_ppm_bits: AtomicU64,
    /// Drift as last committed (UX §2.7: audition renders the *committed* drift;
    /// the live value only drives loupes/readouts until commit_drift()).
    committed_drift_ppm_bits: AtomicU64,
    drift_mode: AtomicU8, // 0 = resample, 1 = none
    cold: Mutex<ColdState>,
}

fn f64_to_bits(v: f64) -> u64 {
    v.to_bits()
}
fn bits_to_f64(b: u64) -> f64 {
    f64::from_bits(b)
}

impl Session {
    pub fn new(id: SessionId, camera: TrackId, external: TrackId) -> Self {
        Self {
            id,
            camera,
            external,
            offset_ms_bits: AtomicU64::new(f64_to_bits(0.0)),
            drift_ppm_bits: AtomicU64::new(f64_to_bits(0.0)),
            committed_drift_ppm_bits: AtomicU64::new(f64_to_bits(0.0)),
            drift_mode: AtomicU8::new(0),
            cold: Mutex::new(ColdState {
                lock_zoom: true,
                loupe_layout_grid: true,
                ..Default::default()
            }),
        }
    }

    pub fn offset_ms(&self) -> f64 {
        bits_to_f64(self.offset_ms_bits.load(Ordering::Acquire))
    }

    pub fn drift_ppm(&self) -> f64 {
        bits_to_f64(self.drift_ppm_bits.load(Ordering::Acquire))
    }

    pub fn drift_mode(&self) -> DriftMode {
        if self.drift_mode.load(Ordering::Acquire) == 1 {
            DriftMode::None
        } else {
            DriftMode::Resample
        }
    }

    /// The mapping (UX §2.1): u(t) = t·(1 + driftPpm·1e−6) + offsetMs/1000.
    pub fn u_of_t(&self, t: f64) -> f64 {
        t * (1.0 + self.drift_ppm() * 1e-6) + self.offset_ms() / 1000.0
    }

    /// Local lag L(t) = offsetMs + t·driftPpm·1e−3 (ms).
    pub fn local_lag_ms(&self, t: f64) -> f64 {
        self.offset_ms() + t * self.drift_ppm() * 1e-3
    }

    // ---- hot setters (gesture rate, §4.5) ----

    pub fn set_offset_ms(&self, v: f64) {
        self.offset_ms_bits.store(f64_to_bits(v), Ordering::Release);
    }

    /// Any drift write while in "none" flips the mode to "resample" (UX §1.5, core-enforced).
    pub fn set_drift_ppm_live(&self, v: f64) {
        if self.drift_mode() == DriftMode::None && v != 0.0 {
            self.drift_mode.store(0, Ordering::Release);
        }
        self.drift_ppm_bits.store(f64_to_bits(v), Ordering::Release);
    }

    pub fn committed_drift_ppm(&self) -> f64 {
        bits_to_f64(self.committed_drift_ppm_bits.load(Ordering::Acquire))
    }

    /// Audition re-render point (UX §2.7/§4.7): the live drift becomes the
    /// value audition renders; the rest of the mapping is already live.
    pub fn commit_drift(&self) {
        self.committed_drift_ppm_bits
            .store(self.drift_ppm_bits.load(Ordering::Acquire), Ordering::Release);
    }

    // ---- cold setters ----

    /// UX §1.5: None zeroes the live mapping but retains savedDriftPpm; Resample restores it.
    pub fn set_drift_mode(&self, mode: DriftMode) {
        let mut cold = self.cold.lock().unwrap();
        match (self.drift_mode(), mode) {
            (DriftMode::Resample, DriftMode::None) => {
                cold.saved_drift_ppm = self.drift_ppm();
                self.drift_ppm_bits.store(f64_to_bits(0.0), Ordering::Release);
                self.drift_mode.store(1, Ordering::Release);
            }
            (DriftMode::None, DriftMode::Resample) => {
                self.drift_ppm_bits
                    .store(f64_to_bits(cold.saved_drift_ppm), Ordering::Release);
                self.committed_drift_ppm_bits
                    .store(f64_to_bits(cold.saved_drift_ppm), Ordering::Release);
                self.drift_mode.store(0, Ordering::Release);
            }
            _ => {}
        }
    }

    pub fn set_verified(&self, v: bool) {
        self.cold.lock().unwrap().verified = v;
    }

    pub fn set_lock_zoom(&self, v: bool) {
        self.cold.lock().unwrap().lock_zoom = v;
    }

    pub fn set_loupe_layout(&self, layout: LoupeLayout) {
        self.cold.lock().unwrap().loupe_layout_grid = layout == LoupeLayout::Grid;
    }

    /// Invariants (§4.5): anchors ascending, at most 4; frames/measurements kept parallel.
    pub fn set_anchors(&self, anchors: Vec<f64>, frames: Vec<Frame>) -> Result<()> {
        validate_anchors(&anchors)?;
        if frames.len() != anchors.len() {
            return Err(CoupleError::invalid("frames must parallel anchors"));
        }
        let mut cold = self.cold.lock().unwrap();
        cold.per_anchor_measurements = vec![None; anchors.len()];
        cold.anchors = anchors;
        cold.frames = frames;
        Ok(())
    }

    pub fn set_frames(&self, frames: Vec<Frame>) -> Result<()> {
        let mut cold = self.cold.lock().unwrap();
        if frames.len() != cold.anchors.len() {
            return Err(CoupleError::invalid("frames must parallel anchors"));
        }
        cold.frames = frames;
        Ok(())
    }

    pub fn set_per_anchor_measurements(&self, m: Vec<Option<LocalMeasurement>>) -> Result<()> {
        let mut cold = self.cold.lock().unwrap();
        if m.len() != cold.anchors.len() {
            return Err(CoupleError::invalid("measurements must parallel anchors"));
        }
        cold.per_anchor_measurements = m;
        Ok(())
    }

    pub fn set_drift_windows(&self, w: Vec<DriftWindowState>) {
        self.cold.lock().unwrap().drift_windows = w;
    }

    pub fn snapshot(&self) -> AlignmentState {
        let cold = self.cold.lock().unwrap();
        AlignmentState {
            version: 1,
            offset_ms: self.offset_ms(),
            drift_ppm: self.drift_ppm(),
            drift_mode: self.drift_mode(),
            saved_drift_ppm: cold.saved_drift_ppm,
            anchors: cold.anchors.clone(),
            frames: cold.frames.clone(),
            lock_zoom: cold.lock_zoom,
            per_anchor_measurements: cold.per_anchor_measurements.clone(),
            drift_windows: cold.drift_windows.clone(),
            verified: cold.verified,
            loupe_layout: if cold.loupe_layout_grid {
                LoupeLayout::Grid
            } else {
                LoupeLayout::Stack
            },
        }
    }

    pub fn state_json(&self) -> String {
        serde_json::to_string_pretty(&self.snapshot()).expect("state serializes")
    }

    pub fn set_state_json(&self, json: &str) -> Result<()> {
        let s: AlignmentState = serde_json::from_str(json)
            .map_err(|e| CoupleError::invalid(format!("state JSON: {e}")))?;
        if s.version != 1 {
            return Err(CoupleError::invalid(format!("unsupported state version {}", s.version)));
        }
        validate_anchors(&s.anchors)?;
        if s.frames.len() != s.anchors.len() || s.per_anchor_measurements.len() != s.anchors.len() {
            return Err(CoupleError::invalid(
                "frames/perAnchorMeasurements must parallel anchors",
            ));
        }
        let mut cold = self.cold.lock().unwrap();
        self.offset_ms_bits.store(f64_to_bits(s.offset_ms), Ordering::Release);
        // driftMode "none" implies live drift 0 while savedDriftPpm retains the measurement.
        let live_ppm = match s.drift_mode {
            DriftMode::None => 0.0,
            DriftMode::Resample => s.drift_ppm,
        };
        self.drift_ppm_bits.store(f64_to_bits(live_ppm), Ordering::Release);
        self.committed_drift_ppm_bits
            .store(f64_to_bits(live_ppm), Ordering::Release);
        self.drift_mode
            .store(matches!(s.drift_mode, DriftMode::None) as u8, Ordering::Release);
        cold.saved_drift_ppm = s.saved_drift_ppm;
        cold.anchors = s.anchors;
        cold.frames = s.frames;
        cold.lock_zoom = s.lock_zoom;
        cold.per_anchor_measurements = s.per_anchor_measurements;
        cold.drift_windows = s.drift_windows;
        cold.verified = s.verified;
        cold.loupe_layout_grid = s.loupe_layout == LoupeLayout::Grid;
        Ok(())
    }

    /// Auto-sync success path (§4.4): write offset/drift/windows, set driftMode +
    /// savedDriftPpm per the suggestion, leave verified = false.
    pub fn apply_sync(
        &self,
        offset_ms: f64,
        drift_ppm: f64,
        windows: Vec<DriftWindowState>,
        suggested: DriftMode,
    ) {
        let mut cold = self.cold.lock().unwrap();
        self.offset_ms_bits.store(f64_to_bits(offset_ms), Ordering::Release);
        cold.saved_drift_ppm = drift_ppm;
        match suggested {
            DriftMode::Resample => {
                self.drift_ppm_bits.store(f64_to_bits(drift_ppm), Ordering::Release);
                self.committed_drift_ppm_bits
                    .store(f64_to_bits(drift_ppm), Ordering::Release);
                self.drift_mode.store(0, Ordering::Release);
            }
            DriftMode::None => {
                self.drift_ppm_bits.store(f64_to_bits(0.0), Ordering::Release);
                self.committed_drift_ppm_bits.store(f64_to_bits(0.0), Ordering::Release);
                self.drift_mode.store(1, Ordering::Release);
            }
        }
        cold.drift_windows = windows;
        cold.verified = false;
    }
}

/// Standalone invariant check (§4.5) — used by `couple state validate`.
pub fn validate_state(s: &AlignmentState) -> Result<()> {
    if s.version != 1 {
        return Err(CoupleError::invalid(format!("unsupported state version {}", s.version)));
    }
    validate_anchors(&s.anchors)?;
    if s.frames.len() != s.anchors.len() || s.per_anchor_measurements.len() != s.anchors.len() {
        return Err(CoupleError::invalid(
            "frames/perAnchorMeasurements must parallel anchors",
        ));
    }
    if s.drift_mode == DriftMode::None && s.drift_ppm != 0.0 {
        return Err(CoupleError::invalid(
            "driftMode none implies driftPpm 0 (savedDriftPpm retains the measurement)",
        ));
    }
    Ok(())
}

fn validate_anchors(anchors: &[f64]) -> Result<()> {
    if anchors.len() == 1 || anchors.len() > 4 {
        return Err(CoupleError::invalid("anchors: 2–4 required (or none)"));
    }
    if anchors.windows(2).any(|w| w[0] >= w[1]) {
        return Err(CoupleError::invalid("anchors must be strictly ascending"));
    }
    Ok(())
}

/// Persistence wrapper (§4.5).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFile {
    pub schema: String,
    pub saved_at: String,
    pub camera: SessionFileTrack,
    pub external: SessionFileTrack,
    pub alignment: AlignmentState,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionFileTrack {
    pub path: String,
    pub content_key: String,
    pub duration_s: f64,
}

pub const SESSION_SCHEMA: &str = "couple-session/1";
