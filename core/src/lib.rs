//! couple-core — the Couple sync engine (SPEC.md §3–§9).
//!
//! No UI, no audio hardware, no video rendering. ffmpeg/ffprobe run as child
//! processes only. The canonical timeline for everything is the video container's
//! presentation timeline (§1.2).

pub mod audition;
pub mod cache;
pub mod ctx;
pub mod envelope;
pub mod error;
pub mod export;
pub mod ffmpeg;
pub mod fit;
pub mod fixture;
pub mod import;
pub mod job;
pub mod measure;
pub mod media;
pub mod onsets;
pub mod pyramid;
pub mod session;
pub mod sync;
pub mod resample;
pub mod track;

pub use ctx::{Ctx, CtxConfig};
pub use error::{CoupleError, Result};
pub use job::{CancelToken, JobId, Phase, Progress, ProgressFn};
pub use media::*;
pub use onsets::Onset;
pub use audition::{AuditionStream, FillInfo, MonitorMode};
pub use pyramid::{band, Band};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
