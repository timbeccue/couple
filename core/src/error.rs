//! §4.7 — failure is data, error is plumbing.

use serde::Serialize;
use std::path::PathBuf;

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "kind", content = "detail")]
pub enum CoupleError {
    #[error("file not found: {path}")]
    FileNotFound { path: PathBuf },
    #[error("unsupported format: {path}: {detail}")]
    UnsupportedFormat { path: PathBuf, detail: String },
    #[error("no audio stream: {path}")]
    NoAudioStream { path: PathBuf },
    #[error("ambiguous role: {path}: {detail}")]
    AmbiguousRole { path: PathBuf, detail: String },
    #[error("ffmpeg/ffprobe not found; searched: {searched}")]
    FfmpegMissing { searched: String },
    #[error("ffmpeg failed at {stage} (exit {exit_code:?}): {stderr_tail}")]
    FfmpegFailed {
        stage: String,
        exit_code: Option<i32>,
        stderr_tail: String,
    },
    #[error("decode failed: {path}: {detail}")]
    DecodeFailed { path: PathBuf, detail: String },
    #[error("cache corrupt: {path}")]
    CacheCorrupt { path: PathBuf },
    #[error("disk full: need {needed_bytes} bytes, {available_bytes} available")]
    DiskFull {
        needed_bytes: u64,
        available_bytes: u64,
    },
    #[error("invalid argument: {detail}")]
    InvalidArgument { detail: String },
    #[error("cancelled")]
    Cancelled,
    #[error("export failed at {stage}: {detail}")]
    ExportFailed { stage: String, detail: String },
    #[error("internal error: {detail}")]
    Internal { detail: String },
}

pub type Result<T> = std::result::Result<T, CoupleError>;

impl CoupleError {
    pub fn internal(detail: impl Into<String>) -> Self {
        CoupleError::Internal {
            detail: detail.into(),
        }
    }

    pub fn invalid(detail: impl Into<String>) -> Self {
        CoupleError::InvalidArgument {
            detail: detail.into(),
        }
    }

    /// CLI exit-code mapping (§10.3). Sync-failed (5) and insufficient-overlap (6)
    /// are SyncStatus outcomes, mapped by the CLI, not errors.
    pub fn exit_code(&self) -> i32 {
        use CoupleError::*;
        match self {
            FileNotFound { .. } | UnsupportedFormat { .. } | NoAudioStream { .. }
            | AmbiguousRole { .. } => 3,
            FfmpegMissing { .. } | FfmpegFailed { .. } | DecodeFailed { .. }
            | CacheCorrupt { .. } | DiskFull { .. } => 4,
            InvalidArgument { .. } => 2,
            Cancelled => 8,
            ExportFailed { .. } => 7,
            Internal { .. } => 9,
        }
    }
}
