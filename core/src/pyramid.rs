//! Peak pyramid: streaming builder, `pyramid.cpk` binary format, mmap reader, band() (§6.1, §4.3).

use crate::error::{CoupleError, Result};
use memmap2::Mmap;
use std::path::Path;

pub const LEVELS_SPB: [u32; 6] = [24, 96, 480, 2400, 9600, 48000];
pub const SAMPLE_RATE: u32 = 48_000;
const MAGIC: &[u8; 8] = b"CPLPK1\0\0";

/// Streaming min/max builder. Accumulates f32 min/max per bin; quantizes to int8 once the
/// track absolute peak (normalization) is known at finalize.
pub struct PyramidBuilder {
    fine_mins: Vec<f32>,
    fine_maxs: Vec<f32>,
    cur_min: f32,
    cur_max: f32,
    cur_fill: u32,
    n_samples: u64,
    abs_peak: f32,
}

impl Default for PyramidBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl PyramidBuilder {
    pub fn new() -> Self {
        Self {
            fine_mins: Vec::new(),
            fine_maxs: Vec::new(),
            cur_min: f32::INFINITY,
            cur_max: f32::NEG_INFINITY,
            cur_fill: 0,
            n_samples: 0,
            abs_peak: 0.0,
        }
    }

    pub fn push(&mut self, samples: &[f32]) {
        for &s in samples {
            if s < self.cur_min {
                self.cur_min = s;
            }
            if s > self.cur_max {
                self.cur_max = s;
            }
            let a = s.abs();
            if a > self.abs_peak {
                self.abs_peak = a;
            }
            self.cur_fill += 1;
            if self.cur_fill == LEVELS_SPB[0] {
                self.flush_bin();
            }
        }
        self.n_samples += samples.len() as u64;
    }

    /// Epoch lead (§7.2): zeros, but cheap — no per-sample loop.
    pub fn push_zeros(&mut self, count: u64) {
        let mut remaining = count;
        while remaining > 0 {
            if self.cur_min > 0.0 {
                self.cur_min = 0.0;
            }
            if self.cur_max < 0.0 {
                self.cur_max = 0.0;
            }
            let room = (LEVELS_SPB[0] - self.cur_fill) as u64;
            let take = room.min(remaining);
            self.cur_fill += take as u32;
            if self.cur_fill == LEVELS_SPB[0] {
                self.flush_bin();
            }
            remaining -= take;
        }
        self.n_samples += count;
    }

    fn flush_bin(&mut self) {
        self.fine_mins.push(self.cur_min);
        self.fine_maxs.push(self.cur_max);
        self.cur_min = f32::INFINITY;
        self.cur_max = f32::NEG_INFINITY;
        self.cur_fill = 0;
    }

    pub fn n_samples(&self) -> u64 {
        self.n_samples
    }
    pub fn abs_peak(&self) -> f64 {
        self.abs_peak as f64
    }

    /// Fold all levels, quantize to int8 against `normalization` = abs peak (UX §4.1).
    pub fn finalize(mut self) -> FinishedPyramid {
        if self.cur_fill > 0 {
            self.flush_bin();
        }
        let norm = self.abs_peak as f64;
        let q = |v: f32| -> i8 {
            if norm <= 0.0 {
                return 0;
            }
            ((v as f64 / norm * 127.0).round() as i32).clamp(-127, 127) as i8
        };

        // Float min/max per level, coarser folded from finer.
        let mut float_levels: Vec<(Vec<f32>, Vec<f32>)> =
            vec![(std::mem::take(&mut self.fine_mins), std::mem::take(&mut self.fine_maxs))];
        for w in 1..LEVELS_SPB.len() {
            let factor = (LEVELS_SPB[w] / LEVELS_SPB[w - 1]) as usize;
            let (pmins, pmaxs) = &float_levels[w - 1];
            let n = pmins.len().div_ceil(factor);
            let mut mins = Vec::with_capacity(n);
            let mut maxs = Vec::with_capacity(n);
            for i in 0..n {
                let a = i * factor;
                let b = ((i + 1) * factor).min(pmins.len());
                let mut mn = f32::INFINITY;
                let mut mx = f32::NEG_INFINITY;
                for j in a..b {
                    mn = mn.min(pmins[j]);
                    mx = mx.max(pmaxs[j]);
                }
                mins.push(mn);
                maxs.push(mx);
            }
            float_levels.push((mins, maxs));
        }

        let levels = float_levels
            .iter()
            .zip(LEVELS_SPB)
            .map(|((mins, maxs), spb)| QuantLevel {
                spb,
                mins: mins.iter().map(|&v| q(v)).collect(),
                maxs: maxs.iter().map(|&v| q(v)).collect(),
            })
            .collect();

        FinishedPyramid {
            sample_rate: SAMPLE_RATE,
            duration_samples: self.n_samples,
            normalization: norm,
            levels,
            float_levels,
        }
    }
}

pub struct QuantLevel {
    pub spb: u32,
    pub mins: Vec<i8>,
    pub maxs: Vec<i8>,
}

pub struct FinishedPyramid {
    pub sample_rate: u32,
    pub duration_samples: u64,
    pub normalization: f64,
    pub levels: Vec<QuantLevel>,
    /// Pre-quantization float min/max per level (import-time stats only; not persisted).
    pub float_levels: Vec<(Vec<f32>, Vec<f32>)>,
}

impl FinishedPyramid {
    pub fn level(&self, spb: u32) -> Option<&QuantLevel> {
        self.levels.iter().find(|l| l.spb == spb)
    }

    /// Serialize to `pyramid.cpk` (§6.1), little-endian, mmap-friendly.
    pub fn write(&self, path: &Path) -> Result<()> {
        let mut buf: Vec<u8> = Vec::new();
        buf.extend_from_slice(MAGIC);
        buf.extend_from_slice(&1u32.to_le_bytes()); // version
        buf.extend_from_slice(&0u32.to_le_bytes()); // flags
        buf.extend_from_slice(&self.sample_rate.to_le_bytes());
        buf.extend_from_slice(&self.duration_samples.to_le_bytes());
        buf.extend_from_slice(&self.normalization.to_le_bytes());
        buf.extend_from_slice(&(self.levels.len() as u32).to_le_bytes());
        buf.resize(64, 0); // pad header to 64 B

        let table_off = buf.len();
        buf.resize(table_off + self.levels.len() * 24, 0);

        // Level data, finest first, each 64-B aligned.
        let mut offsets = Vec::new();
        for l in &self.levels {
            let aligned = buf.len().div_ceil(64) * 64;
            buf.resize(aligned, 0);
            offsets.push(aligned as u64);
            let mut data = Vec::with_capacity(l.mins.len() * 2);
            for i in 0..l.mins.len() {
                data.push(l.mins[i] as u8);
                data.push(l.maxs[i] as u8);
            }
            buf.extend_from_slice(&data);
        }
        for (i, l) in self.levels.iter().enumerate() {
            let e = table_off + i * 24;
            buf[e..e + 4].copy_from_slice(&l.spb.to_le_bytes());
            buf[e + 8..e + 16].copy_from_slice(&(l.mins.len() as u64).to_le_bytes());
            buf[e + 16..e + 24].copy_from_slice(&offsets[i].to_le_bytes());
        }

        let tmp = path.with_extension("cpk.tmp");
        std::fs::write(&tmp, &buf).map_err(|e| CoupleError::internal(format!("write cpk: {e}")))?;
        std::fs::rename(&tmp, path).map_err(|e| CoupleError::internal(format!("rename cpk: {e}")))?;
        Ok(())
    }
}

struct LevelView {
    spb: u32,
    n_bins: u64,
    byte_offset: u64,
}

/// mmap-backed pyramid reader.
pub struct PyramidReader {
    map: Mmap,
    pub sample_rate: u32,
    pub duration_samples: u64,
    pub normalization: f64,
    levels: Vec<LevelView>,
}

impl PyramidReader {
    pub fn open(path: &Path) -> Result<Self> {
        let corrupt = || CoupleError::CacheCorrupt {
            path: path.to_path_buf(),
        };
        let f = std::fs::File::open(path).map_err(|_| corrupt())?;
        let map = unsafe { Mmap::map(&f) }.map_err(|_| corrupt())?;
        if map.len() < 64 || &map[..8] != MAGIC {
            return Err(corrupt());
        }
        let u32_at = |o: usize| u32::from_le_bytes(map[o..o + 4].try_into().unwrap());
        let u64_at = |o: usize| u64::from_le_bytes(map[o..o + 8].try_into().unwrap());
        if u32_at(8) != 1 {
            return Err(corrupt());
        }
        let sample_rate = u32_at(16);
        let duration_samples = u64_at(20);
        let normalization = f64::from_le_bytes(map[28..36].try_into().unwrap());
        let n_levels = u32_at(36) as usize;
        if map.len() < 64 + n_levels * 24 {
            return Err(corrupt());
        }
        let mut levels = Vec::with_capacity(n_levels);
        for i in 0..n_levels {
            let e = 64 + i * 24;
            let lv = LevelView {
                spb: u32_at(e),
                n_bins: u64_at(e + 8),
                byte_offset: u64_at(e + 16),
            };
            if lv.byte_offset + lv.n_bins * 2 > map.len() as u64 {
                return Err(corrupt());
            }
            levels.push(lv);
        }
        Ok(Self {
            map,
            sample_rate,
            duration_samples,
            normalization,
            levels,
        })
    }

    pub fn duration_s(&self) -> f64 {
        self.duration_samples as f64 / self.sample_rate as f64
    }

    pub fn level_spbs(&self) -> Vec<u32> {
        self.levels.iter().map(|l| l.spb).collect()
    }

    pub fn n_bins(&self, spb: u32) -> u64 {
        self.levels.iter().find(|l| l.spb == spb).map(|l| l.n_bins).unwrap_or(0)
    }

    /// Interleaved (min,max) int8 pairs for `bins [b0, b1)` of the given level.
    pub fn bins(&self, spb: u32, b0: u64, b1: u64) -> Option<&[i8]> {
        let l = self.levels.iter().find(|l| l.spb == spb)?;
        let b1 = b1.min(l.n_bins);
        if b0 >= b1 {
            return Some(&[]);
        }
        let start = (l.byte_offset + b0 * 2) as usize;
        let end = (l.byte_offset + b1 * 2) as usize;
        let bytes = &self.map[start..end];
        // i8 and u8 share layout
        Some(unsafe { std::slice::from_raw_parts(bytes.as_ptr() as *const i8, bytes.len()) })
    }
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Band {
    pub level_spb: u32,
    pub col_t0: f64,
    pub col_dt: f64,
    pub mins: Vec<i8>,
    pub maxs: Vec<i8>,
}

/// §4.3: coarsest level still yielding ≥ n_cols bins over [t0,t1]; aggregate bins to
/// columns by min/max. Columns outside the track are (0,0).
pub fn band(p: &PyramidReader, t0: f64, t1: f64, n_cols: u32) -> Result<Band> {
    if !(t1 > t0) || n_cols == 0 {
        return Err(CoupleError::invalid("band: need t1 > t0 and n_cols > 0"));
    }
    let sr = p.sample_rate as f64;
    let span_samples = (t1 - t0) * sr;
    let spb = p
        .levels
        .iter()
        .map(|l| l.spb)
        .filter(|&spb| span_samples / spb as f64 >= n_cols as f64)
        .max()
        .unwrap_or_else(|| p.levels.iter().map(|l| l.spb).min().unwrap_or(24));

    let lvl = p.levels.iter().find(|l| l.spb == spb).unwrap();
    let n_bins = lvl.n_bins as i64;
    let dt = (t1 - t0) / n_cols as f64;
    let mut mins = vec![0i8; n_cols as usize];
    let mut maxs = vec![0i8; n_cols as usize];
    let bins = p.bins(spb, 0, lvl.n_bins).unwrap();

    for c in 0..n_cols as usize {
        let ct0 = t0 + c as f64 * dt;
        let ct1 = ct0 + dt;
        let b0 = (ct0 * sr / spb as f64).floor() as i64;
        let b1 = (ct1 * sr / spb as f64).ceil() as i64;
        let b0c = b0.max(0);
        let b1c = b1.min(n_bins);
        if b0c >= b1c {
            continue; // outside the track: (0,0)
        }
        let mut mn = i8::MAX;
        let mut mx = i8::MIN;
        for b in b0c..b1c {
            let i = (b as usize) * 2;
            mn = mn.min(bins[i]);
            mx = mx.max(bins[i + 1]);
        }
        mins[c] = mn;
        maxs[c] = mx;
    }

    Ok(Band {
        level_spb: spb,
        col_t0: t0,
        col_dt: dt,
        mins,
        maxs,
    })
}
