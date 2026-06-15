//! Loaded track: cache-backed pyramid + PCM + onsets, and the read queries (§4.3).

use crate::cache::{CacheEntry, CacheMeta};
use crate::error::{CoupleError, Result};
use crate::fingerprint::{self, FingerprintReader};
use crate::media::TrackMeta;
use crate::onsets::Onset;
use crate::pyramid::{PyramidReader, SAMPLE_RATE};
use memmap2::Mmap;
use std::sync::{Arc, Mutex, OnceLock};

pub const RAW_FETCH_MAX_S: f64 = 8.0; // [SPEC] §4.3

pub struct TrackData {
    pub meta: TrackMeta,
    pub pyramid: PyramidReader,
    pcm: Mmap,
    pub onsets: Vec<Onset>,
    pub cache: CacheEntry,
    /// Lazily-built spectral fingerprint reader (§8.3). Cached here so a track builds/reads its
    /// `fingerprint-v1.cpf` at most once per loaded handle. `OnceLock` holds the successful
    /// result; `fp_build_lock` serializes the FALLIBLE build so concurrent first-callers build at
    /// most once in-process (`OnceLock::get_or_init` cannot host a `Result`-returning build).
    fingerprint: OnceLock<Arc<FingerprintReader>>,
    fp_build_lock: Mutex<()>,
}

impl TrackData {
    pub fn open(cache: CacheEntry, cmeta: &CacheMeta, meta: TrackMeta) -> Result<Self> {
        let pyramid = PyramidReader::open(&cache.pyramid_path())?;
        let f = std::fs::File::open(cache.pcm_path()).map_err(|_| CoupleError::CacheCorrupt {
            path: cache.pcm_path(),
        })?;
        let pcm = unsafe { Mmap::map(&f) }.map_err(|_| CoupleError::CacheCorrupt {
            path: cache.pcm_path(),
        })?;
        if pcm.len() as u64 != cmeta.frames * 4 || pyramid.duration_samples != cmeta.frames {
            return Err(CoupleError::CacheCorrupt {
                path: cache.dir.clone(),
            });
        }
        let onsets: Vec<Onset> = serde_json::from_slice(
            &std::fs::read(cache.onsets_path()).map_err(|_| CoupleError::CacheCorrupt {
                path: cache.onsets_path(),
            })?,
        )
        .map_err(|_| CoupleError::CacheCorrupt {
            path: cache.onsets_path(),
        })?;
        Ok(Self {
            meta,
            pyramid,
            pcm,
            onsets,
            cache,
            fingerprint: OnceLock::new(),
            fp_build_lock: Mutex::new(()),
        })
    }

    pub fn frames(&self) -> u64 {
        (self.pcm.len() / 4) as u64
    }

    pub(crate) fn pcm_samples(&self) -> &[f32] {
        // f32le on all supported targets (little-endian); mmap is 4-byte... not guaranteed
        // aligned, so go through bytemuck-free manual check.
        let bytes: &[u8] = &self.pcm;
        debug_assert_eq!(bytes.as_ptr() as usize % 4, 0, "mmap is page-aligned");
        unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const f32, bytes.len() / 4) }
    }

    /// The track's versioned spectral fingerprint (PLAN-SPECTRAL-FINGERPRINT-MATCHING §8.3),
    /// built lazily from the MMAP PCM cache — never by re-decoding the source media (DoD item 2).
    ///
    /// Lifecycle, in order:
    ///
    /// - returns the in-handle cached reader if this track has already produced one;
    /// - otherwise reads `fingerprint-v1.cpf`; if present and valid under the frozen v1 format it
    ///   is decoded and cached;
    /// - if the sidecar is ABSENT, it is built from PCM and atomically installed (the lazy upgrade
    ///   of an old cache entry that predates fingerprints — a missing `.cpf` is NOT corruption);
    /// - if the sidecar is CORRUPT or written under an INCOMPATIBLE format (mismatched params
    ///   hash), only the `.cpf` is deleted and rebuilt — PCM, pyramid, and onsets are untouched.
    ///
    /// Concurrency: the successful reader is memoized in a `OnceLock`; the fallible build is
    /// serialized by a per-track mutex so two callers racing the FIRST request build AT MOST ONCE
    /// in-process (the second waits, then observes the freshly-stored result). After the first
    /// success this is a lock-free `OnceLock` read.
    ///
    /// Determinism (§3 invariant 2): the build is a pure function of the PCM (fed in one slice);
    /// repeated builds of the same PCM produce byte-identical `.cpf` bytes.
    pub fn fingerprint_v1(&self) -> Result<Arc<FingerprintReader>> {
        // Fast path: already built/loaded on this handle.
        if let Some(fp) = self.fingerprint.get() {
            return Ok(Arc::clone(fp));
        }

        // Serialize the build so concurrent first-callers do not both decode/rebuild. The lock is
        // held only for the (bounded) build; the result is published through the OnceLock.
        let _guard = self
            .fp_build_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());

        // Re-check under the lock: a racing caller may have just published the reader.
        if let Some(fp) = self.fingerprint.get() {
            return Ok(Arc::clone(fp));
        }

        let frames = self.frames();
        let (reader, _src) =
            fingerprint::load_or_build(&self.cache.fingerprint_path_v1(), self.pcm_samples(), frames)?;
        let arc = Arc::new(reader);
        // `set` only fails if another thread already published; we hold the build lock, so it
        // cannot — but fall back to the stored value defensively rather than asserting.
        let stored = match self.fingerprint.set(Arc::clone(&arc)) {
            Ok(()) => arc,
            Err(_) => Arc::clone(self.fingerprint.get().expect("set raced means value present")),
        };
        Ok(stored)
    }

    /// §4.3 raw_window: mono 48 kHz f32 from the PCM cache, canonical timeline.
    /// Out-of-track regions are zero-filled. Returns frames written.
    pub fn raw_window(&self, t0: f64, t1: f64, out: &mut [f32]) -> Result<u32> {
        if !(t1 > t0) {
            return Err(CoupleError::invalid("raw_window: need t1 > t0"));
        }
        if t1 - t0 > RAW_FETCH_MAX_S {
            return Err(CoupleError::invalid(format!(
                "raw_window: span {:.3} s exceeds RAW_FETCH_MAX_S = {RAW_FETCH_MAX_S} s",
                t1 - t0
            )));
        }
        let sr = SAMPLE_RATE as f64;
        let n = (((t1 - t0) * sr).round() as usize).min(out.len());
        let start = (t0 * sr).round() as i64;
        let samples = self.pcm_samples();
        let total = samples.len() as i64;
        for (j, o) in out.iter_mut().take(n).enumerate() {
            let idx = start + j as i64;
            *o = if idx >= 0 && idx < total {
                samples[idx as usize]
            } else {
                0.0
            };
        }
        Ok(n as u32)
    }

    /// §4.3 rms: dBFS over active 2400-spb bins in [t0, t1]. Active = bin amplitude
    /// > 12% of track max (UX §4.1 rule). Computed from the PCM cache on demand.
    pub fn rms(&self, t0: f64, t1: f64) -> Result<f64> {
        if !(t1 > t0) {
            return Err(CoupleError::invalid("rms: need t1 > t0"));
        }
        let sr = SAMPLE_RATE as f64;
        let spb = 2400usize;
        let samples = self.pcm_samples();
        let b0 = ((t0 * sr / spb as f64).floor().max(0.0)) as usize;
        let b1 = (((t1 * sr / spb as f64).ceil()) as usize).min(samples.len().div_ceil(spb));
        let gate = (0.12 * self.meta.normalization) as f32;
        let mut sum_sq = 0f64;
        let mut count = 0u64;
        for b in b0..b1 {
            let a = b * spb;
            let e = ((b + 1) * spb).min(samples.len());
            let chunk = &samples[a..e];
            let amp = chunk.iter().fold(0f32, |m, &v| m.max(v.abs()));
            if amp > gate {
                sum_sq += chunk.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>();
                count += (e - a) as u64;
            }
        }
        if count == 0 {
            return Ok(f64::NEG_INFINITY);
        }
        Ok(20.0 * (sum_sq / count as f64).sqrt().log10())
    }
}
