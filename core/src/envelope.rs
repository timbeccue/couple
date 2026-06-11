//! Envelope definition (§5.1): span s = max − min in the int domain (0..254),
//! e = ln(1 + s). Log compression is part of the validated design [DECIDED].

use crate::pyramid::PyramidReader;

/// Envelope values for bins [b0, b1) of a pyramid level.
pub fn envelope(p: &PyramidReader, spb: u32, b0: u64, b1: u64) -> Vec<f64> {
    let Some(bins) = p.bins(spb, b0, b1) else {
        return Vec::new();
    };
    bins.chunks_exact(2)
        .map(|pair| {
            let span = (pair[1] as i16 - pair[0] as i16) as f64;
            (1.0 + span).ln()
        })
        .collect()
}

/// Linear span domain (0..254) for bins [b0, b1) — onset ranking input (§5.7).
pub fn spans(p: &PyramidReader, spb: u32, b0: u64, b1: u64) -> Vec<u8> {
    let Some(bins) = p.bins(spb, b0, b1) else {
        return Vec::new();
    };
    bins.chunks_exact(2)
        .map(|pair| (pair[1] as i16 - pair[0] as i16) as u8)
        .collect()
}

/// Full-track envelope at a level.
pub fn envelope_full(p: &PyramidReader, spb: u32) -> Vec<f64> {
    envelope(p, spb, 0, p.n_bins(spb))
}
