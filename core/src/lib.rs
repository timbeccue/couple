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
pub mod fingerprint;
pub mod fit;
pub mod fixture;
pub mod import;
pub mod job;
pub mod match_graph;
pub mod match_refine;
pub mod matcher_corpus;
pub mod matching;
pub mod measure;
pub mod media;
pub mod onsets;
pub mod pair;
pub mod pairing_params;
pub mod policy;
pub mod pyramid;
pub mod resolve;
pub mod robust_fit;
pub mod session;
pub mod sync;
pub mod resample;
pub mod track;
pub mod verify;

pub use ctx::{Ctx, CtxConfig};
pub use error::{CoupleError, Result};
pub use matching::{MatchClassification, MatchMethod, PairEvidence};
pub use pairing_params::{MatcherMode, PairingParams};
pub use resolve::{
    CandidateOverflow, Coverage, MatcherChoice, ResolutionDetail, ResolveMatrix, ResolveParams, ResolvedRow,
    RowCandidate, RowState, TriangleCheck,
};
pub use job::{CancelToken, JobId, Phase, Progress, ProgressFn};
pub use media::*;
pub use onsets::Onset;
pub use audition::{AuditionStream, FillInfo, MonitorMode};
pub use pyramid::{band, Band};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
