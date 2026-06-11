//! Media identity and probe types (§4.2). JSON field names are the wire format.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub type TrackId = u64;
pub type SessionId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TrackRole {
    Camera,
    External,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoStreamInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub nominal_fps: f64,
    pub avg_fps: f64,
    pub start_time_s: f64,
    pub bit_rate: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStreamInfo {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
    pub bits_per_sample: Option<u32>,
    pub start_time_s: f64,
    pub duration_s: Option<f64>,
}

/// BWF `bext` fields (§1.4: preserved for v2 pairing even though v1 never trusts them).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BwfInfo {
    pub date: Option<String>,
    pub time: Option<String>,
    pub time_reference_samples: Option<u64>,
    pub encoded_by: Option<String>,
    pub coding_history: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    pub path: String,
    pub container: String,
    pub duration_s: f64,
    pub video: Option<VideoStreamInfo>,
    pub audio: Vec<AudioStreamInfo>,
    pub creation_time_utc: Option<String>,
    pub bwf: Option<BwfInfo>,
    pub tags: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackStats {
    pub abs_peak: f64,
    /// p95 of active envelope bins (> 12% of track max) at 9600 spb, in sample units (UX §4.1).
    pub active_p95_9600: f64,
    /// RMS (dBFS) over active bins at 2400 spb (§4.8 loudness matching input).
    pub active_rms_dbfs: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackMeta {
    pub id: TrackId,
    pub role: TrackRole,
    pub info: MediaInfo,
    /// Decoded audio stream's container start_time (camera); 0.0 for plain audio files (§1.2).
    pub epoch_s: f64,
    /// Decoded duration on the canonical timeline (includes epoch lead).
    pub duration_s: f64,
    /// Analysis rate, always 48_000.
    pub sample_rate: u32,
    /// Track absolute peak (UX §4.1).
    pub normalization: f64,
    pub stats: TrackStats,
    pub content_key: String,
}
