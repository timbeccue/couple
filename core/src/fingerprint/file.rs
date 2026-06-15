//! Versioned fingerprint binary artifact `fingerprint-v1.cpf` (PLAN-SPECTRAL-FINGERPRINT-MATCHING §8.1).
//!
//! A self-describing, checksummed, little-endian container holding a track's sorted landmark
//! set plus the analysis identity needed to decide whether the artifact is usable under the
//! current code. The format has an INDEPENDENT version lifecycle from `pyramid.cpk` and is
//! always rebuildable from `pcm48f32.raw` (§4), so a missing or rejected artifact is never
//! fatal — boundary 3 will rebuild it. This module is PURE encode/decode over byte buffers; it
//! does not touch the cache, PCM, or any matching logic.
//!
//! ## Layout
//!
//! Header (64 bytes, little-endian):
//!
//! ```text
//! off  size  field
//!   0     4  magic                 "CPF1"
//!   4     4  format_version        u32  (== 1)
//!   8     4  analysis_sample_rate  u32
//!  12     4  fft_size              u32
//!  16     4  hop_size              u32
//!  20     4  min_frequency_millihz u32  (min_frequency_hz × 1000, rounded)
//!  24     4  max_frequency_millihz u32
//!  28    16  params_hash           [u8; 16]   (§8.2)
//!  44     8  duration_frames       u64  (canonical 48 kHz frame count of the source)
//!  52     8  landmark_count        u64
//!  60     4  header_blake3_32      u32  (LE first 4 bytes of BLAKE3 over bytes 0..60)
//! ```
//!
//! Body: `landmark_count` records of `{ hash u32 LE, anchor_frame u32 LE }` (8 bytes each).
//!
//! Footer: `body_blake3_128` `[u8; 16]` — the first 16 BLAKE3 digest bytes over the body bytes.
//!
//! ## Rejection (§8.1)
//!
//! [`FingerprintReader::parse`] REJECTS, in order:
//!
//! - too-short buffer (cannot even hold the header);
//! - wrong magic or unsupported `format_version`;
//! - a header checksum mismatch (the header was corrupted);
//! - an `analysis_sample_rate` / `fft_size` / `hop_size` / band that disagrees with the format
//!   the caller loads under, OR a `params_hash` that does not match — i.e. the artifact was
//!   written by a different analysis format (incompatible params hash);
//! - an UNREASONABLE `landmark_count` (checked BEFORE any allocation: it must fit both an
//!   absolute ceiling and the bytes physically present in the buffer);
//! - a body that is truncated (fewer bytes than `landmark_count × 8 + 16` footer);
//! - a body checksum mismatch;
//! - landmarks not sorted by `(hash, anchor_frame)` (§7.4);
//! - an `anchor_frame` at or beyond `duration_frames`.
//!
//! ## Determinism (§3 invariant 2)
//!
//! [`encode`] is a pure function of the format + landmark list; two encodes of the same inputs
//! are byte-identical. No field depends on RNG, hash-map order, or a wall clock. No NaN/Infinity
//! is ever written: frequencies are carried as integer milli-hertz, not floats (§20).

use super::landmarks::Landmark;
use super::params::FingerprintFormatV1;
use crate::error::{CoupleError, Result};

/// Artifact magic. The trailing version digit is cosmetic; `format_version` is authoritative.
pub const CPF1_MAGIC: &[u8; 4] = b"CPF1";
/// The only supported artifact format version.
pub const CPF1_VERSION: u32 = 1;

/// Fixed header size in bytes (see the module layout table).
pub const HEADER_LEN: usize = 64;
/// Bytes of the header covered by `header_blake3_32` (everything before the checksum field).
const HEADER_CHECKSUM_PREFIX: usize = 60;
/// Body record size: `hash u32 ‖ anchor_frame u32`.
const RECORD_LEN: usize = 8;
/// Footer size: `body_blake3_128`.
const FOOTER_LEN: usize = 16;

/// Absolute sanity ceiling on `landmark_count`, enforced BEFORE allocation (§8.1 "unreasonable
/// landmark counts before allocation"). Landmark density is bounded by the §7.3 per-second cap
/// (≤ `max_peaks_per_second` anchors/s) times `fanout`, so even a multi-hour clip is far under
/// this. ~134 M landmarks ≈ 1 GiB of body — anything larger is corruption, not data. The buffer
/// length is the real bound; this guards against a self-consistent but absurd header before we
/// trust any size arithmetic.
pub const MAX_LANDMARKS: u64 = 128 * 1024 * 1024;

/// Convert a frequency in Hz to the header's integer milli-hertz, rounding to nearest. The
/// frozen band edges (100 Hz, 5500 Hz) are exact multiples of 1 mHz, so this is lossless for v1.
/// Carrying integers (not floats) guarantees no NaN/Infinity can reach the artifact (§20).
fn hz_to_millihz(hz: f64) -> u32 {
    (hz * 1000.0).round() as u32
}

/// Encode a fingerprint artifact (§8.1). `landmarks` MUST be sorted by `(hash, anchor_frame)`
/// (the order [`super::landmarks::generate_landmarks`] returns) and every `anchor_frame` MUST be
/// `< duration_frames`; both are debug-asserted here and re-checked on decode. `duration_frames`
/// is the canonical 48 kHz frame count of the source track.
///
/// Returns the complete artifact bytes (header ‖ body ‖ footer).
pub fn encode(fmt: &FingerprintFormatV1, duration_frames: u64, landmarks: &[Landmark]) -> Vec<u8> {
    debug_assert!(
        landmarks.windows(2).all(|w| (w[0].hash, w[0].anchor_frame) <= (w[1].hash, w[1].anchor_frame)),
        "encode requires landmarks sorted by (hash, anchor_frame)"
    );
    debug_assert!(
        landmarks.iter().all(|l| (l.anchor_frame as u64) < duration_frames || duration_frames == 0),
        "encode requires every anchor_frame < duration_frames"
    );

    let params_hash = fmt.params_hash();
    let body_len = landmarks.len() * RECORD_LEN;
    let mut out = Vec::with_capacity(HEADER_LEN + body_len + FOOTER_LEN);

    // ---- Header (bytes 0..60), then its checksum (bytes 60..64). ----
    out.extend_from_slice(CPF1_MAGIC);
    out.extend_from_slice(&CPF1_VERSION.to_le_bytes());
    out.extend_from_slice(&fmt.analysis_sample_rate.to_le_bytes());
    out.extend_from_slice(&(fmt.fft_size as u32).to_le_bytes());
    out.extend_from_slice(&(fmt.hop_size as u32).to_le_bytes());
    out.extend_from_slice(&hz_to_millihz(fmt.min_frequency_hz).to_le_bytes());
    out.extend_from_slice(&hz_to_millihz(fmt.max_frequency_hz).to_le_bytes());
    out.extend_from_slice(&params_hash);
    out.extend_from_slice(&duration_frames.to_le_bytes());
    out.extend_from_slice(&(landmarks.len() as u64).to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_CHECKSUM_PREFIX, "header prefix must be 60 bytes");
    let header_ck = blake3::hash(&out[..HEADER_CHECKSUM_PREFIX]);
    let header_ck32 = u32::from_le_bytes(header_ck.as_bytes()[..4].try_into().unwrap());
    out.extend_from_slice(&header_ck32.to_le_bytes());
    debug_assert_eq!(out.len(), HEADER_LEN, "header must be 64 bytes");

    // ---- Body. ----
    let body_start = out.len();
    for l in landmarks {
        out.extend_from_slice(&l.hash.to_le_bytes());
        out.extend_from_slice(&l.anchor_frame.to_le_bytes());
    }

    // ---- Footer: BLAKE3-128 over the body bytes. ----
    let body_ck = blake3::hash(&out[body_start..]);
    out.extend_from_slice(&body_ck.as_bytes()[..FOOTER_LEN]);
    out
}

/// A decoded, validated fingerprint artifact (§8.1). Holds the owned landmark set plus the
/// header identity. Decoded from a `&[u8]` so boundary 3 can hand it an `mmap`'d file directly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FingerprintReader {
    /// `analysis_sample_rate` from the header.
    pub analysis_sample_rate: u32,
    /// `fft_size` from the header.
    pub fft_size: u32,
    /// `hop_size` from the header.
    pub hop_size: u32,
    /// `min_frequency_hz × 1000` from the header.
    pub min_frequency_millihz: u32,
    /// `max_frequency_hz × 1000` from the header.
    pub max_frequency_millihz: u32,
    /// The §8.2 params hash the artifact was written under (already verified compatible).
    pub params_hash: [u8; 16],
    /// Canonical 48 kHz frame count of the source track.
    pub duration_frames: u64,
    /// The sorted landmark set (by `(hash, anchor_frame)`).
    pub landmarks: Vec<Landmark>,
}

impl FingerprintReader {
    /// Parse and FULLY validate an artifact against the format the caller loads under (§8.1).
    /// Every rejection condition is checked; allocation of the landmark vector happens only AFTER
    /// the count is proven reasonable and the body bytes are proven present.
    ///
    /// `expected` is the analysis format the running code uses (in production always
    /// [`FingerprintFormatV1::fixed`]); an artifact written under a different format — detected
    /// by mismatched header analysis fields OR a mismatched params hash — is rejected so the
    /// caller rebuilds it.
    pub fn parse(bytes: &[u8], expected: &FingerprintFormatV1) -> Result<Self> {
        // --- Header presence. ---
        if bytes.len() < HEADER_LEN {
            return Err(corrupt(format!(
                "buffer too short for CPF1 header: {} < {HEADER_LEN}",
                bytes.len()
            )));
        }

        // --- Magic + version. ---
        if &bytes[0..4] != CPF1_MAGIC {
            return Err(corrupt(format!("bad magic: {:?}", &bytes[0..4])));
        }
        let format_version = read_u32(bytes, 4);
        if format_version != CPF1_VERSION {
            return Err(corrupt(format!(
                "unsupported CPF format_version {format_version} (expected {CPF1_VERSION})"
            )));
        }

        // --- Header checksum (catch a corrupted header before trusting any field). ---
        let stored_header_ck = read_u32(bytes, HEADER_CHECKSUM_PREFIX);
        let computed_ck = blake3::hash(&bytes[..HEADER_CHECKSUM_PREFIX]);
        let computed_header_ck = u32::from_le_bytes(computed_ck.as_bytes()[..4].try_into().unwrap());
        if stored_header_ck != computed_header_ck {
            return Err(corrupt(format!(
                "header checksum mismatch: stored {stored_header_ck:#010x} vs computed {computed_header_ck:#010x}"
            )));
        }

        // --- Analysis identity must match the format we load under. ---
        let analysis_sample_rate = read_u32(bytes, 8);
        let fft_size = read_u32(bytes, 12);
        let hop_size = read_u32(bytes, 16);
        let min_frequency_millihz = read_u32(bytes, 20);
        let max_frequency_millihz = read_u32(bytes, 24);
        let mut params_hash = [0u8; 16];
        params_hash.copy_from_slice(&bytes[28..44]);
        let duration_frames = read_u64(bytes, 44);
        let landmark_count = read_u64(bytes, 52);

        let expected_hash = expected.params_hash();
        // The params hash is the authoritative compatibility gate; the individual header analysis
        // fields are also checked so a same-hash-but-mangled header (or a future hash collision)
        // still fails fast and with a precise message.
        if params_hash != expected_hash {
            return Err(corrupt(format!(
                "incompatible params hash: artifact {} vs current {}",
                hex16(&params_hash),
                hex16(&expected_hash)
            )));
        }
        if analysis_sample_rate != expected.analysis_sample_rate
            || fft_size != expected.fft_size as u32
            || hop_size != expected.hop_size as u32
            || min_frequency_millihz != hz_to_millihz(expected.min_frequency_hz)
            || max_frequency_millihz != hz_to_millihz(expected.max_frequency_hz)
        {
            return Err(corrupt(
                "artifact analysis header disagrees with the current fingerprint format".to_string(),
            ));
        }

        // --- Unreasonable count, checked BEFORE allocation (§8.1). ---
        if landmark_count > MAX_LANDMARKS {
            return Err(corrupt(format!(
                "unreasonable landmark_count {landmark_count} exceeds ceiling {MAX_LANDMARKS}"
            )));
        }
        // The body bytes must physically be present. Compute the needed length with checked
        // arithmetic so a huge count cannot overflow into a small "needed" value.
        let body_len = (landmark_count as usize)
            .checked_mul(RECORD_LEN)
            .ok_or_else(|| corrupt("landmark_count × record size overflows usize".to_string()))?;
        let needed = HEADER_LEN
            .checked_add(body_len)
            .and_then(|n| n.checked_add(FOOTER_LEN))
            .ok_or_else(|| corrupt("artifact length arithmetic overflows usize".to_string()))?;
        if bytes.len() < needed {
            return Err(corrupt(format!(
                "truncated body: have {} bytes, need {needed} for {landmark_count} landmarks + footer",
                bytes.len()
            )));
        }

        // --- Body checksum. ---
        let body = &bytes[HEADER_LEN..HEADER_LEN + body_len];
        let footer = &bytes[HEADER_LEN + body_len..HEADER_LEN + body_len + FOOTER_LEN];
        let body_ck = blake3::hash(body);
        if &body_ck.as_bytes()[..FOOTER_LEN] != footer {
            return Err(corrupt("body checksum mismatch".to_string()));
        }

        // --- Decode landmarks (count already trusted) and validate order + bounds. ---
        let mut landmarks = Vec::with_capacity(landmark_count as usize);
        let mut prev: Option<(u32, u32)> = None;
        for rec in body.chunks_exact(RECORD_LEN) {
            let hash = u32::from_le_bytes(rec[0..4].try_into().unwrap());
            let anchor_frame = u32::from_le_bytes(rec[4..8].try_into().unwrap());
            // Sorted strictly-non-decreasing by (hash, anchor_frame) (§7.4).
            if let Some(p) = prev {
                if (hash, anchor_frame) < p {
                    return Err(corrupt(format!(
                        "landmarks not sorted: ({hash:#010x}, {anchor_frame}) follows ({:#010x}, {})",
                        p.0, p.1
                    )));
                }
            }
            // Anchor frame must lie within the analyzed duration.
            if duration_frames != 0 && anchor_frame as u64 >= duration_frames {
                return Err(corrupt(format!(
                    "anchor_frame {anchor_frame} >= duration_frames {duration_frames}"
                )));
            }
            prev = Some((hash, anchor_frame));
            landmarks.push(Landmark { hash, anchor_frame });
        }

        Ok(Self {
            analysis_sample_rate,
            fft_size,
            hop_size,
            min_frequency_millihz,
            max_frequency_millihz,
            params_hash,
            duration_frames,
            landmarks,
        })
    }

    /// Number of landmarks in the artifact.
    pub fn len(&self) -> usize {
        self.landmarks.len()
    }

    /// Whether the artifact has no landmarks (a valid, if uninformative, fingerprint).
    pub fn is_empty(&self) -> bool {
        self.landmarks.is_empty()
    }
}

fn corrupt(detail: String) -> CoupleError {
    // A malformed artifact is an InvalidArgument at this layer (the cache layer in boundary 3
    // turns a decode failure into a rebuild; it does not surface as a user error). Carrying the
    // precise reason keeps the reject table and debugging honest.
    CoupleError::invalid(format!("CPF1: {detail}"))
}

#[inline]
fn read_u32(bytes: &[u8], off: usize) -> u32 {
    u32::from_le_bytes(bytes[off..off + 4].try_into().unwrap())
}

#[inline]
fn read_u64(bytes: &[u8], off: usize) -> u64 {
    u64::from_le_bytes(bytes[off..off + 8].try_into().unwrap())
}

fn hex16(b: &[u8; 16]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lm(hash: u32, anchor_frame: u32) -> Landmark {
        Landmark { hash, anchor_frame }
    }

    fn sample_landmarks() -> Vec<Landmark> {
        // Already sorted by (hash, anchor_frame); a couple share a hash.
        let mut v = vec![
            lm(0x0000_0001, 10),
            lm(0x0000_0001, 42),
            lm(0x0010_00ff, 5),
            lm(0x00ab_cdef, 100),
            lm(0x7fff_ffff, 7),
        ];
        v.sort();
        v
    }

    // Encode → decode round trip: the reader reproduces the inputs, and re-encoding the decoded
    // landmarks is byte-identical (the §3 invariant-2 / §8.1 round-trip gate).
    #[test]
    fn round_trip_byte_identical() {
        let fmt = FingerprintFormatV1::fixed();
        let lms = sample_landmarks();
        let dur = 1_000_000u64;
        let bytes = encode(&fmt, dur, &lms);
        let r = FingerprintReader::parse(&bytes, &fmt).expect("valid artifact must parse");
        assert_eq!(r.landmarks, lms, "decoded landmarks must equal the input");
        assert_eq!(r.duration_frames, dur);
        assert_eq!(r.params_hash, fmt.params_hash());
        assert_eq!(r.analysis_sample_rate, fmt.analysis_sample_rate);
        // Re-encoding the decoded set reproduces the exact bytes.
        let bytes2 = encode(&fmt, r.duration_frames, &r.landmarks);
        assert_eq!(bytes, bytes2, "re-encode must be byte-identical");
        // Header is exactly 64 bytes, body is 8/landmark, footer 16.
        assert_eq!(bytes.len(), HEADER_LEN + lms.len() * RECORD_LEN + FOOTER_LEN);
    }

    // Encode is deterministic.
    #[test]
    fn encode_is_deterministic() {
        let fmt = FingerprintFormatV1::fixed();
        let lms = sample_landmarks();
        assert_eq!(encode(&fmt, 500_000, &lms), encode(&fmt, 500_000, &lms));
    }

    // An empty landmark set is a valid artifact (header + empty body + footer over zero bytes).
    #[test]
    fn empty_landmarks_round_trip() {
        let fmt = FingerprintFormatV1::fixed();
        let bytes = encode(&fmt, 48_000, &[]);
        assert_eq!(bytes.len(), HEADER_LEN + FOOTER_LEN);
        let r = FingerprintReader::parse(&bytes, &fmt).expect("empty artifact must parse");
        assert!(r.is_empty());
        assert_eq!(r.len(), 0);
    }

    // §8.1 GOLDEN: a fully-pinned artifact byte vector for a tiny fixed landmark set. This is the
    // committed on-disk contract — any change to the header layout, field order, checksum, or the
    // params hash changes these bytes and fails here. (Mirrors the params-hash golden.)
    #[test]
    fn golden_artifact_bytes() {
        let fmt = FingerprintFormatV1::fixed();
        let lms = vec![lm(0x0000_0001, 0x0000_0002), lm(0x0000_0003, 0x0000_0004)];
        let bytes = encode(&fmt, 0x0102_0304_0506_0708, &lms);
        // Pinned bytes (filled from a one-shot run; see GOLDEN_ARTIFACT).
        assert_eq!(bytes, GOLDEN_ARTIFACT, "CPF1 golden artifact drifted");
        // And it round-trips.
        let r = FingerprintReader::parse(&bytes, &fmt).unwrap();
        assert_eq!(r.landmarks, lms);
    }

    // §8.1 REJECT TABLE: every rejection condition must be caught. We start from a valid artifact
    // and corrupt exactly one thing per case.
    #[test]
    fn reject_table() {
        let fmt = FingerprintFormatV1::fixed();
        let lms = sample_landmarks();
        let dur = 200u64; // small so we can test the anchor-frame bound
        // Rebuild with anchors all < dur.
        let lms: Vec<Landmark> = lms.iter().map(|l| lm(l.hash, (l.anchor_frame % 200).min(199))).collect();
        let mut lms = lms;
        lms.sort();
        let good = encode(&fmt, dur, &lms);
        // Sanity: the good artifact parses.
        assert!(FingerprintReader::parse(&good, &fmt).is_ok());

        // (1) buffer too short for the header.
        assert!(FingerprintReader::parse(&good[..HEADER_LEN - 1], &fmt).is_err());

        // (2) wrong magic.
        let mut b = good.clone();
        b[0] = b'X';
        assert!(FingerprintReader::parse(&b, &fmt).is_err(), "bad magic must reject");

        // (3) unsupported version. Bump version then REPAIR the header checksum so we isolate the
        // version rejection (otherwise the checksum would catch it first).
        let mut b = good.clone();
        b[4..8].copy_from_slice(&2u32.to_le_bytes());
        fix_header_checksum(&mut b);
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("format_version"), "version reject, got {e}");

        // (4) header checksum mismatch: flip a header byte WITHOUT repairing the checksum.
        let mut b = good.clone();
        b[12] ^= 0x01; // perturb fft_size
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("header checksum"), "header checksum reject, got {e}");

        // (5) incompatible params hash: corrupt the stored hash and repair the header checksum so
        // the params-hash gate (not the header checksum) is what rejects.
        let mut b = good.clone();
        b[28] ^= 0xff;
        fix_header_checksum(&mut b);
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("params hash"), "params-hash reject, got {e}");

        // (5b) analysis header disagreement WITH a matching params hash is impossible to forge
        // without also changing the hash, but we can simulate a future hash collision by parsing
        // under a DIFFERENT expected format whose params hash we force to match via a custom
        // format. Instead, assert the simpler contract: parsing a fixed()-format artifact under a
        // genuinely different format is rejected (covers the cross-format case end to end).
        let mut other = FingerprintFormatV1::fixed();
        other.hop_size = 128; // a real different format ⇒ different params hash
        assert!(FingerprintReader::parse(&good, &other).is_err(), "cross-format must reject");

        // (6) unreasonable landmark_count before allocation: set landmark_count to a huge value
        // and repair the header checksum so the COUNT ceiling (not the checksum) rejects, and no
        // allocation is attempted.
        let mut b = good.clone();
        b[52..60].copy_from_slice(&(MAX_LANDMARKS + 1).to_le_bytes());
        fix_header_checksum(&mut b);
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("unreasonable"), "count-ceiling reject, got {e}");

        // (7) truncated body: claim one MORE landmark than the bytes hold (still under the
        // ceiling), repair the header checksum so truncation is the rejecter.
        let mut b = good.clone();
        let claimed = lms.len() as u64 + 1;
        b[52..60].copy_from_slice(&claimed.to_le_bytes());
        fix_header_checksum(&mut b);
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("truncated"), "truncation reject, got {e}");

        // (8) body checksum mismatch: flip a body byte (leaves length intact; header checksum
        // still valid because we did not touch the header).
        let mut b = good.clone();
        b[HEADER_LEN] ^= 0x01;
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("body checksum"), "body checksum reject, got {e}");

        // (9) unsorted landmarks: swap two body records so order breaks, then recompute the BODY
        // checksum (footer) so the order check — not the checksum — is the rejecter.
        let mut b = good.clone();
        // Records are at HEADER_LEN + i*8. Swap record 0 and record 1 if they differ.
        let r0 = HEADER_LEN;
        let r1 = HEADER_LEN + RECORD_LEN;
        let (a, c) = b.split_at_mut(r1);
        a[r0..r0 + RECORD_LEN].swap_with_slice(&mut c[..RECORD_LEN]);
        // Only meaningful if the swap actually produced a descending pair.
        fix_body_checksum(&mut b, lms.len());
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("not sorted"), "unsorted reject, got {e}");

        // (10) anchor frame >= duration: encode a landmark whose anchor_frame == dur, bypassing
        // the encode debug-assert by building the bytes directly via a larger duration then
        // shrinking the stored duration_frames and repairing both checksums.
        let mut b = good.clone();
        // Shrink stored duration to just below an existing anchor. Find the max anchor.
        let max_anchor = lms.iter().map(|l| l.anchor_frame).max().unwrap();
        b[44..52].copy_from_slice(&(max_anchor as u64).to_le_bytes()); // == max anchor ⇒ that anchor is out of range
        fix_header_checksum(&mut b);
        let e = FingerprintReader::parse(&b, &fmt).unwrap_err();
        assert!(format!("{e}").contains("anchor_frame"), "anchor-bound reject, got {e}");
    }

    // Helper: recompute and write the header checksum over bytes 0..60.
    fn fix_header_checksum(b: &mut [u8]) {
        let ck = blake3::hash(&b[..HEADER_CHECKSUM_PREFIX]);
        let ck32 = u32::from_le_bytes(ck.as_bytes()[..4].try_into().unwrap());
        b[HEADER_CHECKSUM_PREFIX..HEADER_LEN].copy_from_slice(&ck32.to_le_bytes());
    }

    // Helper: recompute and write the body checksum (footer) for `n` landmarks.
    fn fix_body_checksum(b: &mut [u8], n: usize) {
        let body_len = n * RECORD_LEN;
        let ck = blake3::hash(&b[HEADER_LEN..HEADER_LEN + body_len]);
        let foot_start = HEADER_LEN + body_len;
        b[foot_start..foot_start + FOOTER_LEN].copy_from_slice(&ck.as_bytes()[..FOOTER_LEN]);
    }

    // Pinned golden artifact bytes (see golden_artifact_bytes). Filled from a one-shot run; if an
    // intentional format change makes these drift, regenerate alongside FIXED_PARAMS_HASH.
    const GOLDEN_ARTIFACT: &[u8] = &[
        0x43, 0x50, 0x46, 0x31, 0x01, 0x00, 0x00, 0x00, 0xe0, 0x2e, 0x00, 0x00, 0x00, 0x04, 0x00,
        0x00, 0x00, 0x01, 0x00, 0x00, 0xa0, 0x86, 0x01, 0x00, 0x60, 0xec, 0x53, 0x00, 0x42, 0x7e,
        0x9e, 0x29, 0x44, 0x6c, 0x2e, 0x4c, 0x8a, 0x9f, 0xb7, 0x8f, 0x34, 0x64, 0x6e, 0x10, 0x08,
        0x07, 0x06, 0x05, 0x04, 0x03, 0x02, 0x01, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x7a, 0x61, 0xcb, 0xd0, 0x01, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x00, 0x00,
        0x00, 0x04, 0x00, 0x00, 0x00, 0xca, 0x29, 0x6c, 0x6a, 0x32, 0x40, 0xcd, 0x16, 0xca, 0x5f,
        0xab, 0x77, 0xbe, 0xab, 0xd9, 0x87,
    ];
}
