//! ffmpeg/ffprobe as child processes only (§3.3): discovery and probe.

use crate::error::{CoupleError, Result};
use crate::media::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock, PoisonError};

/// A candidate that exists can still fail to start — e.g. a bundled copy of a
/// Homebrew ffmpeg whose versioned Cellar dylibs a later `brew upgrade`
/// removed. Returning it would surface every import as "unsupported format",
/// so a candidate must also launch (`-version` exits 0) before resolution
/// accepts it; a broken one is skipped and resolution falls through.
/// Paths that passed once are cached for the life of the process.
fn launches(path: &Path) -> bool {
    static OK: OnceLock<Mutex<BTreeSet<PathBuf>>> = OnceLock::new();
    let ok = OK.get_or_init(|| Mutex::new(BTreeSet::new()));
    if ok.lock().unwrap_or_else(PoisonError::into_inner).contains(path) {
        return true;
    }
    let success = Command::new(path)
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if success {
        ok.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(path.to_path_buf());
    }
    success
}

/// Accept `cand` only if it exists and launches; otherwise record why in `searched`.
fn usable(cand: PathBuf, searched: &mut Vec<String>) -> Option<PathBuf> {
    if !cand.is_file() {
        searched.push(cand.display().to_string());
        return None;
    }
    if launches(&cand) {
        return Some(cand);
    }
    searched.push(format!("{} (present but failed to launch)", cand.display()));
    None
}

/// Resolution order (§3.3): explicit config > COUPLE_FFMPEG env > bundled helper > $PATH.
pub fn resolve_tool(name: &str, configured: Option<&Path>) -> Result<PathBuf> {
    let mut searched = Vec::new();

    if let Some(p) = configured {
        let p = if p.is_dir() { p.join(name) } else { p.to_path_buf() };
        if let Some(p) = usable(p, &mut searched) {
            return Ok(p);
        }
    }

    if let Ok(env) = std::env::var("COUPLE_FFMPEG") {
        let env = PathBuf::from(env);
        // Env var may point at the ffmpeg binary or its directory; siblings resolve ffprobe.
        let cand = if env.is_dir() {
            env.join(name)
        } else if name == "ffmpeg" {
            env.clone()
        } else {
            env.parent().map(|d| d.join(name)).unwrap_or(env.clone())
        };
        if let Some(cand) = usable(cand, &mut searched) {
            return Ok(cand);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            for cand in [dir.join(name), dir.join("../Helpers").join(name)] {
                if let Some(cand) = usable(cand, &mut searched) {
                    return Ok(cand);
                }
            }
        }
    }

    if let Some(paths) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&paths) {
            let cand = dir.join(name);
            if cand.is_file() {
                if launches(&cand) {
                    return Ok(cand);
                }
                searched.push(format!("{} (present but failed to launch)", cand.display()));
            }
        }
        searched.push("$PATH".into());
    }

    Err(CoupleError::FfmpegMissing {
        searched: searched.join(", "),
    })
}

fn parse_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(|v| v.as_str()).and_then(|s| s.parse().ok())
}

fn parse_fps(s: Option<&serde_json::Value>) -> f64 {
    let Some(s) = s.and_then(|v| v.as_str()) else {
        return 0.0;
    };
    let mut it = s.splitn(2, '/');
    let num: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
    let den: f64 = it.next().and_then(|x| x.parse().ok()).unwrap_or(1.0);
    if den == 0.0 {
        0.0
    } else {
        num / den
    }
}

/// `ffprobe -v error -print_format json -show_format -show_streams` → MediaInfo (§7.1).
/// Rejects files without an audio stream (v1 contract: nothing downstream of import
/// can use them).
pub fn probe(ffprobe: &Path, path: &Path) -> Result<MediaInfo> {
    let info = probe_lenient(ffprobe, path)?;
    if info.audio.is_empty() {
        return Err(CoupleError::NoAudioStream {
            path: path.to_path_buf(),
        });
    }
    Ok(info)
}

/// Probe WITHOUT the audio-stream gate (SPEC-V2 §2 pool classification: a video file
/// with a stripped audio stream is still a video — it gets an honest `no match` row,
/// not a silent rejection). Rust-side only; not part of the C ABI.
pub fn probe_lenient(ffprobe: &Path, path: &Path) -> Result<MediaInfo> {
    if !path.is_file() {
        return Err(CoupleError::FileNotFound {
            path: path.to_path_buf(),
        });
    }
    let out = Command::new(ffprobe)
        .args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"])
        .arg(path)
        .output()
        .map_err(|e| CoupleError::Internal {
            detail: format!("spawn ffprobe: {e}"),
        })?;
    if !out.status.success() {
        return Err(CoupleError::UnsupportedFormat {
            path: path.to_path_buf(),
            detail: String::from_utf8_lossy(&out.stderr)
                .lines()
                .last()
                .unwrap_or("ffprobe failed")
                .to_string(),
        });
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| CoupleError::UnsupportedFormat {
            path: path.to_path_buf(),
            detail: format!("ffprobe JSON: {e}"),
        })?;

    let format = &v["format"];
    let container = format["format_name"].as_str().unwrap_or("").to_string();
    let duration_s = parse_f64(Some(&format["duration"])).unwrap_or(0.0);

    let mut tags = BTreeMap::new();
    if let Some(map) = format["tags"].as_object() {
        for (k, val) in map {
            if let Some(s) = val.as_str() {
                tags.insert(k.clone(), s.to_string());
            }
        }
    }
    let creation_time_utc = tags.get("creation_time").cloned();

    // BWF bext fields surface as WAV format tags (verified on the H4n file).
    let bwf = if container == "wav"
        && ["time_reference", "coding_history", "encoded_by"]
            .iter()
            .any(|k| tags.contains_key(*k))
    {
        Some(BwfInfo {
            date: tags.get("date").cloned(),
            time: tags.get("creation_time").cloned(),
            time_reference_samples: tags.get("time_reference").and_then(|s| s.parse().ok()),
            encoded_by: tags.get("encoded_by").cloned(),
            coding_history: tags.get("coding_history").cloned(),
        })
    } else {
        None
    };

    let mut video = None;
    let mut audio = Vec::new();
    for s in v["streams"].as_array().map(|a| a.as_slice()).unwrap_or(&[]) {
        match s["codec_type"].as_str() {
            Some("video") => {
                // mjpeg attached pictures etc. are not camera video
                if s["disposition"]["attached_pic"].as_i64() == Some(1) {
                    continue;
                }
                if video.is_none() {
                    video = Some(VideoStreamInfo {
                        codec: s["codec_name"].as_str().unwrap_or("").to_string(),
                        width: s["width"].as_u64().unwrap_or(0) as u32,
                        height: s["height"].as_u64().unwrap_or(0) as u32,
                        nominal_fps: parse_fps(Some(&s["r_frame_rate"])),
                        avg_fps: parse_fps(Some(&s["avg_frame_rate"])),
                        start_time_s: parse_f64(Some(&s["start_time"])).unwrap_or(0.0),
                        bit_rate: s["bit_rate"].as_str().and_then(|x| x.parse().ok()),
                    });
                }
            }
            Some("audio") => audio.push(AudioStreamInfo {
                codec: s["codec_name"].as_str().unwrap_or("").to_string(),
                sample_rate: s["sample_rate"]
                    .as_str()
                    .and_then(|x| x.parse().ok())
                    .unwrap_or(0),
                channels: s["channels"].as_u64().unwrap_or(0) as u32,
                bits_per_sample: {
                    let b = s["bits_per_sample"].as_u64().unwrap_or(0);
                    let braw: u64 = s["bits_per_raw_sample"]
                        .as_str()
                        .and_then(|x| x.parse().ok())
                        .unwrap_or(0);
                    let bits = if b > 0 { b } else { braw };
                    (bits > 0).then_some(bits as u32)
                },
                start_time_s: parse_f64(Some(&s["start_time"])).unwrap_or(0.0),
                duration_s: parse_f64(Some(&s["duration"])),
            }),
            _ => {}
        }
    }

    Ok(MediaInfo {
        path: path.display().to_string(),
        container,
        duration_s,
        video,
        audio,
        creation_time_utc,
        bwf,
        tags,
    })
}

/// Classify per §7.1: camera = has a video stream; external = audio-only.
pub fn classify(info: &MediaInfo) -> TrackRole {
    if info.video.is_some() {
        TrackRole::Camera
    } else {
        TrackRole::External
    }
}

/// Tail of stderr for FfmpegFailed errors.
pub fn stderr_tail(stderr: &[u8]) -> String {
    let s = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = s.lines().collect();
    let n = lines.len().saturating_sub(6);
    lines[n..].join("\n")
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// An executable that exists but cannot run usefully — stands in for a
    /// bundled ffmpeg whose dylibs were removed by a Homebrew upgrade.
    fn broken_tool(dir: &Path, name: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let p = dir.join(name);
        std::fs::write(&p, "#!/bin/sh\nexit 7\n").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn broken_configured_tool_falls_through_to_path() {
        let dir = tempfile::tempdir().unwrap();
        let broken = broken_tool(dir.path(), "ffmpeg");
        let resolved = resolve_tool("ffmpeg", Some(dir.path())).expect("ffmpeg on PATH");
        assert_ne!(resolved, broken, "resolver returned the non-launching candidate");
        assert!(launches(&resolved));
    }

    #[test]
    fn broken_tool_with_no_fallback_reports_launch_failure() {
        let dir = tempfile::tempdir().unwrap();
        let broken = broken_tool(dir.path(), "couple-test-no-such-tool");
        let err = resolve_tool("couple-test-no-such-tool", Some(&broken)).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("failed to launch"),
            "error should say the candidate was present but broken: {msg}"
        );
    }
}
