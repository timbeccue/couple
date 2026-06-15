//! Fingerprint cache lifecycle (PLAN-SPECTRAL-FINGERPRINT-MATCHING §8.3).
//!
//! Glue between the pure analysis pipeline ([`super::builder`], [`super::landmarks`]), the pure
//! on-disk codec ([`super::file`]), and the content-addressed track cache. It owns exactly two
//! responsibilities and nothing else:
//!
//! - [`build_artifact_bytes`]: turn a track's canonical mono 48 kHz PCM into the complete
//!   `fingerprint-v1.cpf` byte image, deterministically. Same PCM ⇒ same bytes (§3 invariant 2).
//!   This is the single builder shared by the in-decode-pass generation ([`crate::import`]) and
//!   the lazy build-from-mmap-PCM path ([`crate::track::TrackData::fingerprint_v1`]), so the two
//!   can never diverge.
//! - [`load_or_build`]: produce a validated [`FingerprintReader`] for a cache entry, reading a
//!   present-and-valid `.cpf`, or (re)building from PCM and atomically installing it when the
//!   sidecar is absent, corrupt, or written under an incompatible analysis format.
//!
//! ## Invariants this module upholds (§3, §8.3, §20)
//!
//! - It NEVER touches `pcm48f32.raw`, `pyramid.cpk`, `onsets.json`, or `meta.json`. A missing or
//!   corrupt fingerprint is repaired by rewriting ONLY the `.cpf`; the rest of the entry is
//!   untouched, so a fingerprint rebuild cannot harm PCM/pyramid/onsets (§8.3.6, §20 "do not
//!   delete old cache entries merely because a fingerprint is missing").
//! - A missing `.cpf` is NOT corruption: an old cache entry without one stays valid and is
//!   lazily upgraded on first request (§3 invariant 7, §8.3.2).
//! - The write is temp-file + atomic rename, reusing the entry's established idiom; a build or
//!   write error leaves NO `.tmp` behind (§8.3.4).
//! - Determinism: the builder is fed the PCM in one slice and reads no RNG / hash order / wall
//!   clock; `params_hash` is carried as integer/bit-exact bytes; no NaN/Infinity is serialized
//!   (the codec carries frequencies as integer milli-hertz — §20).

use super::builder::FingerprintBuilder;
use super::file::{self, FingerprintReader};
use super::params::FingerprintFormatV1;
use crate::error::{CoupleError, Result};
use std::path::Path;

/// Build the complete `fingerprint-v1.cpf` byte image for a track from its canonical mono 48 kHz
/// PCM, under the frozen v1 format ([`FingerprintFormatV1::fixed`]).
///
/// `pcm` is the canonical 48 kHz f32 sample slice; `duration_frames` is the track's canonical
/// 48 kHz frame count (`pcm.len()` for a fully-decoded track, carried explicitly so the header
/// records the source duration even if a caller ever passes a windowed view). The builder runs
/// the deterministic 48k→12k decimation + STFT + peak + landmark pipeline; the result is sorted
/// by `(hash, anchor_frame)` and encoded with header/body BLAKE3 checksums.
///
/// Determinism (§3 invariant 2): feeding the whole slice at once is the canonical chunking, and
/// the pipeline is chunk-invariant regardless; two calls on equal PCM return byte-identical
/// output. No value is RNG-, hash-order-, or wall-clock-dependent, and none is NaN/Infinity.
pub fn build_artifact_bytes(pcm: &[f32], duration_frames: u64) -> Vec<u8> {
    let fmt = FingerprintFormatV1::fixed();
    let mut builder = FingerprintBuilder::with_format(fmt.clone());
    builder.push(pcm);
    let landmarks = builder.finalize_landmarks();
    file::encode(&fmt, duration_frames, &landmarks)
}

/// Outcome of [`load_or_build`] beyond the reader itself — lets the caller (and tests) see WHY a
/// build happened without re-deriving it. Purely informational; not persisted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FingerprintSource {
    /// A present, valid `.cpf` was read and decoded without a rebuild.
    LoadedExisting,
    /// No `.cpf` existed; one was built from PCM and installed (lazy upgrade of an old entry).
    BuiltMissing,
    /// A `.cpf` existed but was corrupt or written under an incompatible format; it was deleted
    /// and rebuilt from PCM. PCM/pyramid/onsets were not touched.
    RebuiltInvalid,
}

/// Produce a validated [`FingerprintReader`] for the cache entry at `cpf_path`, building from
/// `pcm` when needed (§8.3). `duration_frames` is the canonical 48 kHz frame count for the header.
///
/// Decision order:
///
/// 1. If `cpf_path` exists and [`FingerprintReader::parse`] accepts it under the current frozen
///    format, return it ([`FingerprintSource::LoadedExisting`]). An incompatible `params_hash`,
///    bad checksum, truncation, unsorted body, or out-of-range anchor is a parse REJECTION — it
///    is treated as "needs rebuild", never as a fatal error (§8.3.2/§8.3.6, §8.1 reject table).
/// 2. If the file is absent, build from `pcm` and install it ([`FingerprintSource::BuiltMissing`]).
/// 3. If the file was present but rejected, delete ONLY the `.cpf` and rebuild
///    ([`FingerprintSource::RebuiltInvalid`]). PCM/pyramid/onsets are never deleted.
///
/// The freshly built bytes are written to `cpf_path.tmp` and atomically renamed; on any build or
/// write error the `.tmp` is removed so no partial sidecar is left (§8.3.4). The just-written
/// bytes are re-parsed before returning, so a reader is always validated identically to a
/// load-from-disk (and a hypothetical builder defect surfaces immediately rather than as silent
/// bad data).
pub fn load_or_build(
    cpf_path: &Path,
    pcm: &[f32],
    duration_frames: u64,
) -> Result<(FingerprintReader, FingerprintSource)> {
    let fmt = FingerprintFormatV1::fixed();

    // 1. Try an existing artifact. A read error (absent file) ⇒ build-missing; a present file
    //    that fails to parse ⇒ rebuild-invalid. We deliberately distinguish "no bytes" from
    //    "bytes that won't parse" so the source reason and the delete-before-rebuild are correct.
    let existing = std::fs::read(cpf_path);
    let rebuild_reason = match &existing {
        Ok(bytes) => match FingerprintReader::parse(bytes, &fmt) {
            Ok(reader) => return Ok((reader, FingerprintSource::LoadedExisting)),
            // Rejected: corrupt, truncated, or written under an incompatible analysis format
            // (mismatched params hash). Rebuild — never surface as an error.
            Err(_) => FingerprintSource::RebuiltInvalid,
        },
        // Absent (or unreadable) ⇒ lazy first build for an old entry.
        Err(_) => FingerprintSource::BuiltMissing,
    };

    // 2/3. (Re)build. If we are replacing a rejected artifact, remove the stale `.cpf` first so a
    //      crash mid-rebuild never leaves the old bad bytes masquerading as current. PCM/pyramid/
    //      onsets/meta are NOT touched — only this one sidecar.
    if rebuild_reason == FingerprintSource::RebuiltInvalid {
        let _ = std::fs::remove_file(cpf_path);
    }

    let bytes = build_artifact_bytes(pcm, duration_frames);
    write_atomic(cpf_path, &bytes)?;

    // Re-parse the bytes we just wrote: a reader is always a VALIDATED artifact, identical to the
    // load-from-disk path, and a builder/codec defect fails loudly here instead of downstream.
    let reader = FingerprintReader::parse(&bytes, &fmt).map_err(|e| {
        CoupleError::internal(format!("freshly built fingerprint failed to parse: {e}"))
    })?;
    Ok((reader, rebuild_reason))
}

/// Write `bytes` to `<path>.tmp` then atomically rename onto `path` (§8.3.4). Mirrors the cache's
/// established temp-rename idiom ([`crate::import`]). On ANY failure the temp file is removed so a
/// failed build/write leaves no `.tmp` litter. The temp suffix is APPENDED (not
/// `with_extension`) so it never collides with the `.cpf` itself and is unambiguously this
/// artifact's scratch file.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut tmp = path.as_os_str().to_os_string();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);

    let write_then_rename = || -> std::io::Result<()> {
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    };
    if let Err(e) = write_then_rename() {
        // Best-effort cleanup of a partial temp file; the original error is what we report.
        let _ = std::fs::remove_file(&tmp);
        return Err(CoupleError::internal(format!(
            "write fingerprint {path:?}: {e}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // A deterministic broadband-ish PCM buffer the peak picker can lock onto (a few decaying
    // multi-tone transients over a faint bed), keyed to the absolute sample index so it is pure.
    // Mirrors the integration-test signal shape but kept tiny and local to the unit test.
    fn pcm(seed: u64, n: usize) -> Vec<f32> {
        let sr = 48_000.0f64;
        let mut h = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(0x55);
        let mut rnd = || {
            h ^= h >> 30;
            h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
            h ^= h >> 27;
            h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
            h ^= h >> 31;
            (h >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut events: Vec<(f64, [f64; 4], f64, f64)> = Vec::new();
        let mut t = 0.0;
        let dur = n as f64 / sr;
        loop {
            t += -(rnd().max(1e-9)).ln() / 4.0;
            if t >= dur {
                break;
            }
            let mut f = [0f64; 4];
            for x in &mut f {
                *x = 300.0 + rnd() * 5000.0;
            }
            events.push((t, f, 0.3 + 0.6 * rnd(), 0.03 + 0.12 * rnd()));
        }
        (0..n)
            .map(|i| {
                let tt = i as f64 / sr;
                let mut s = 0.0;
                for &(t0, f, amp, tau) in &events {
                    let dt = tt - t0;
                    if dt >= 0.0 && dt < 8.0 * tau {
                        let env = amp * (-dt / tau).exp();
                        let mut burst = 0.0;
                        for &fr in &f {
                            burst += (2.0 * std::f64::consts::PI * fr * dt).sin();
                        }
                        s += env * burst / 2.0;
                    }
                }
                s.clamp(-0.999, 0.999) as f32
            })
            .collect()
    }

    // build_artifact_bytes is deterministic and round-trips through the reader (§3 invariant 2).
    #[test]
    fn build_is_deterministic_and_parses() {
        let samples = pcm(1, 48_000 * 3);
        let frames = samples.len() as u64;
        let a = build_artifact_bytes(&samples, frames);
        let b = build_artifact_bytes(&samples, frames);
        assert_eq!(a, b, "same PCM must yield byte-identical .cpf");
        let fmt = FingerprintFormatV1::fixed();
        let r = FingerprintReader::parse(&a, &fmt).expect("built artifact must parse");
        assert_eq!(r.duration_frames, frames);
        assert_eq!(r.params_hash, fmt.params_hash());
    }

    // First load with no file builds; a second load reads the existing one (no rebuild).
    #[test]
    fn load_builds_then_reuses() {
        let dir = tempfile::tempdir().unwrap();
        let cpf = dir.path().join("fingerprint-v1.cpf");
        let samples = pcm(2, 48_000 * 3);
        let frames = samples.len() as u64;

        let (r1, src1) = load_or_build(&cpf, &samples, frames).unwrap();
        assert_eq!(src1, FingerprintSource::BuiltMissing);
        assert!(cpf.is_file(), "build must install the .cpf");
        let on_disk = std::fs::read(&cpf).unwrap();

        let (r2, src2) = load_or_build(&cpf, &samples, frames).unwrap();
        assert_eq!(src2, FingerprintSource::LoadedExisting);
        assert_eq!(r1.landmarks, r2.landmarks);
        // The file was not rewritten (re-load path does not touch it).
        assert_eq!(std::fs::read(&cpf).unwrap(), on_disk);
        // No .tmp litter.
        assert!(!cpf.with_file_name("fingerprint-v1.cpf.tmp").exists());
    }

    // A corrupt .cpf is deleted and rebuilt; the rebuilt bytes match a clean build.
    #[test]
    fn corrupt_artifact_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let cpf = dir.path().join("fingerprint-v1.cpf");
        let samples = pcm(3, 48_000 * 3);
        let frames = samples.len() as u64;
        let good = build_artifact_bytes(&samples, frames);

        // Write garbage where a .cpf belongs.
        std::fs::write(&cpf, b"not a fingerprint at all").unwrap();
        let (r, src) = load_or_build(&cpf, &samples, frames).unwrap();
        assert_eq!(src, FingerprintSource::RebuiltInvalid);
        // The rebuilt on-disk artifact equals a clean build of the same PCM.
        assert_eq!(std::fs::read(&cpf).unwrap(), good);
        assert_eq!(file::encode(&FingerprintFormatV1::fixed(), frames, &r.landmarks), good);
    }

    // An artifact written under an incompatible format (different params hash) rebuilds under the
    // current frozen format rather than failing.
    #[test]
    fn incompatible_params_hash_rebuilds() {
        let dir = tempfile::tempdir().unwrap();
        let cpf = dir.path().join("fingerprint-v1.cpf");
        let samples = pcm(4, 48_000 * 3);
        let frames = samples.len() as u64;

        // Encode under a DIFFERENT format (changed hop ⇒ different params hash). It is a valid
        // CPF1 file, just not for the format the loader uses.
        let mut other = FingerprintFormatV1::fixed();
        other.hop_size = 128;
        let mut ob = FingerprintBuilder::with_format(other.clone());
        ob.push(&samples);
        let other_lms = ob.finalize_landmarks();
        let stale = file::encode(&other, frames, &other_lms);
        std::fs::write(&cpf, &stale).unwrap();

        let (r, src) = load_or_build(&cpf, &samples, frames).unwrap();
        assert_eq!(src, FingerprintSource::RebuiltInvalid);
        // Now parses under the CURRENT frozen format.
        assert_eq!(r.params_hash, FingerprintFormatV1::fixed().params_hash());
        assert_eq!(std::fs::read(&cpf).unwrap(), build_artifact_bytes(&samples, frames));
    }
}
