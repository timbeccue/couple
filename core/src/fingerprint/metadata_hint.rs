//! Metadata time hints (PLAN-SPECTRAL-FINGERPRINT-MATCHING §12).
//!
//! Boundary 10. Container/BWF metadata is ADVISORY: it MAY contribute one affine hypothesis to the
//! robust-consensus candidate list and MAY break an EXACT audio-score tie, but it NEVER creates a
//! match without audio support, NEVER vetoes strong audio evidence, and NEVER restricts the landmark
//! lookup (§3 invariant 3, §12.3, §19 DoD #7, §20 "use timestamps as a hard search window").
//!
//! This module is pure parsing + the reliability policy. It produces a [`TimeHint`] from the two
//! tracks' metadata; the [`consensus`](crate::fingerprint::consensus) function decides, from the
//! hint's [`HintReliability`], whether to seed a hypothesis ([`HintReliability::can_rank`]) and/or
//! to allow it as an exact-tie order key ([`HintReliability::can_order`]). The hint NEVER reaches a
//! gate or threshold.
//!
//! ## What a hint means (§12.1)
//!
//! A per-pair hint's `lag_ms` is the DIFFERENCE of the two tracks' ABSOLUTE metadata timestamps —
//! `t_A_abs − t_B_abs`, in the §3.1 sense that content at A-time `t` occurs at B-time
//! `t + lag_ms/1000`. If both clocks are correct, A's recording started `lag_ms` ms before B's, so
//! the same wall-clock instant sits at A-time `t` and B-time `t − lag_ms/1000`… i.e. the affine
//! `intercept_s = lag_ms/1000` (scale 1) that boundary 5 already speaks. The absolute timestamp of a
//! track is the start of its recording on the wall clock; we derive it from, in priority order:
//!
//! 1. BWF `time_reference_samples / source_sample_rate` — sample-accurate offset of the recording
//!    start from the BWF day-origin (midnight), seconds. Sample-accurate but day-relative, so two
//!    tracks are only comparable this way when their BWF DATES agree (same recording day).
//! 2. BWF date+time (`bext` origination date/time) — second-resolution wall clock.
//! 3. Container `creation_time` (ISO-8601 UTC) — second-resolution wall clock, often a different
//!    device's clock than the recorder's.
//!
//! ## Determinism & finiteness (§3 invariant 2, §20)
//!
//! Parsing is a pure function of the two metadata records; there is no clock read, no RNG, no
//! map-iteration order. Every numeric the hint carries is finite (a parse that would yield a
//! non-finite or absurd value yields `None` instead). The hint can therefore enter the deterministic
//! consensus without perturbing its bit-stability.

use crate::media::MediaInfo;

/// Which metadata source produced a [`TimeHint`] (§12.1). Carried for debug explanations (§12.3c)
/// and to drive the [`HintReliability`] policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeHintSource {
    /// BWF `time_reference_samples / source_sample_rate`, with compatible BWF dates (§12.1 #1).
    BwfSampleReference,
    /// BWF `bext` origination date + time, second resolution (§12.1 #2).
    BwfDateTime,
    /// Container `creation_time` (ISO-8601 UTC), second resolution (§12.1 #3).
    ContainerCreationTime,
    /// Same-device monotonic ordering from a stable device tag (§12.1 #4). Carries an ORDERING
    /// only — never a usable lag for ranking.
    SameDeviceOrdering,
}

/// How much a [`TimeHint`] may influence consensus (§12.2). This is the crux of the non-vetoing
/// guarantee: it gates ONLY whether the hint seeds a hypothesis and/or breaks an exact tie. It can
/// never lower a threshold, accept, or reject.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HintReliability {
    /// BWF sample reference WITH compatible dates (§12.2). May seed a hypothesis AND order an exact
    /// tie.
    Medium,
    /// Creation-time-only cross-device hint (§12.2). May seed a hypothesis AND order an exact tie
    /// (it still only wins by earning audio inliers; a wrong low-reliability lag simply loses).
    Low,
    /// Implausible date disagreement (§12.2). Kept as DEBUG data ONLY — it must NOT seed a
    /// hypothesis or order anything. This is the 11.6-year clock-misconfig case: the hint exists for
    /// explanation, never for ranking.
    DebugOnly,
    /// Same-device monotonic ordering (§12.2). May ORDER equal audio-evidence candidates (exact tie)
    /// but must NEVER seed an accept/reject hypothesis — there is no trustworthy absolute lag, only
    /// "A before B".
    OrderOnly,
}

impl HintReliability {
    /// Whether this hint may add an affine hypothesis to the consensus candidate list (§12.3a). A
    /// seeded hypothesis still competes on the SAME §10.3 audio scoring and only wins by earning
    /// audio inliers — so `true` here never weakens acceptance, it only offers one more candidate.
    pub fn can_rank(self) -> bool {
        matches!(self, HintReliability::Medium | HintReliability::Low)
    }

    /// Whether this hint may break an EXACT audio-score tie (§12.3b) by ordering an
    /// otherwise-identical-quality candidate first. `OrderOnly` (same-device) gets this but NOT
    /// `can_rank`; `DebugOnly` gets neither.
    pub fn can_order(self) -> bool {
        matches!(
            self,
            HintReliability::Medium | HintReliability::Low | HintReliability::OrderOnly
        )
    }
}

/// A metadata-derived time hint for one (A, B) pair (§12.1). `lag_ms` is the difference of the two
/// tracks' absolute metadata timestamps (A − B), interpretable directly as a scale-1 affine
/// `intercept_s = lag_ms/1000`. `tolerance_ms` bounds how close an audio hypothesis must sit to count
/// as "metadata-consistent" for the exact-tie order (it reflects the source's resolution — sub-ms
/// for sample reference, ~1 s for second-resolution wall clocks). ALL fields are finite.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TimeHint {
    /// A − B absolute-timestamp difference, ms. Finite. For `SameDeviceOrdering` this is the SIGN of
    /// the ordering scaled to a nominal value; `reliability` is `OrderOnly` so it is used only as an
    /// order key, never as a lag to seed.
    pub lag_ms: f64,
    /// Consistency tolerance, ms (the source's resolution). Finite, `> 0`.
    pub tolerance_ms: f64,
    /// Which metadata source produced this hint.
    pub source: TimeHintSource,
    /// How much it may influence consensus (§12.2).
    pub reliability: HintReliability,
}

/// The minimal, parsed view of one track's metadata that the hint parser needs (§12.1). Extracted
/// from a [`MediaInfo`] by [`MetadataView::from_media`]; for synthetic fixtures with no metadata,
/// every field is `None`/empty and no hint is produced (graceful absence).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MetadataView {
    /// BWF `time_reference_samples` (§12.1 #1).
    pub bwf_time_reference_samples: Option<u64>,
    /// Source audio stream sample rate (Hz) — the denominator for the sample reference (§12.1 #1).
    pub source_sample_rate: Option<u32>,
    /// BWF origination date `YYYY-MM-DD` (§12.1 #2).
    pub bwf_date: Option<String>,
    /// BWF origination time `HH:MM:SS` (§12.1 #2).
    pub bwf_time: Option<String>,
    /// Container `creation_time` ISO-8601 UTC (§12.1 #3).
    pub creation_time_utc: Option<String>,
    /// A stable device identifier for same-device ordering (§12.1 #4), e.g. an encoder/make tag.
    pub device_tag: Option<String>,
}

impl MetadataView {
    /// Extract the hint-relevant metadata from a probed [`MediaInfo`] (§12.1). Pure: just reads the
    /// already-parsed BWF/creation-time/tag fields; it performs no I/O and no clock read. A track
    /// with no such metadata (synthetic fixtures) yields an all-`None` view.
    pub fn from_media(info: &MediaInfo) -> Self {
        let (bwf_time_reference_samples, bwf_date, bwf_time) = match &info.bwf {
            Some(b) => (b.time_reference_samples, b.date.clone(), b.time.clone()),
            None => (None, None, None),
        };
        // The recorder's source sample rate is the FIRST audio stream's rate (the BWF sample
        // reference is in that domain). Absent ⇒ the sample-reference hint cannot be formed.
        let source_sample_rate = info.audio.first().map(|a| a.sample_rate).filter(|&r| r > 0);
        // A stable device tag for same-device ordering: prefer an explicit encoder/device identity
        // tag, falling back to common container tags. Purely a string identity; never a clock.
        let device_tag = info
            .tags
            .get("encoder")
            .or_else(|| info.tags.get("device"))
            .or_else(|| info.tags.get("make"))
            .or_else(|| info.tags.get("com.apple.quicktime.make"))
            .or_else(|| info.tags.get("model"))
            .cloned()
            .or_else(|| info.bwf.as_ref().and_then(|b| b.encoded_by.clone()));
        Self {
            bwf_time_reference_samples,
            source_sample_rate,
            bwf_date,
            bwf_time,
            creation_time_utc: info.creation_time_utc.clone(),
            device_tag,
        }
    }
}

/// A nominal lag magnitude (ms) used as the `SameDeviceOrdering` order-key value. The value itself
/// is meaningless (the reliability is `OrderOnly`, so it never seeds a hypothesis); only its SIGN
/// participates, in the exact-tie order. Kept small and finite.
const SAME_DEVICE_ORDER_NOMINAL_MS: f64 = 1.0;

/// The §12.2 implausibility threshold: a wall-clock disagreement beyond this is "implausible" (a
/// clock misconfiguration, not a real start-time offset) and the hint is demoted to `DebugOnly`. One
/// year of seconds; the 11.6-year camera/Zoom case is ~366× over this, so it can NEVER rank (§12.3
/// required test). A genuine multi-take session offset is seconds-to-minutes, far under this.
const IMPLAUSIBLE_DISAGREEMENT_MS: f64 = 365.0 * 24.0 * 3600.0 * 1000.0;

/// Tolerance (ms) for a BWF sample-reference hint — sub-sample at the source rate, reported as a
/// tight fixed bound so an audio hypothesis must agree closely to be "metadata-consistent" (§12.1).
const TOL_SAMPLE_REF_MS: f64 = 2.0;
/// Tolerance (ms) for a second-resolution wall-clock hint (BWF date/time, container creation time).
/// One second: the wall clock cannot localise tighter than its resolution (§12.1).
const TOL_WALLCLOCK_MS: f64 = 1000.0;
/// Tolerance (ms) for the same-device ordering order key. Generous: it only orders an exact tie.
const TOL_ORDER_MS: f64 = 1000.0;

impl TimeHint {
    /// Parse the per-pair time hint from the two tracks' metadata (§12.1), or `None` when no source
    /// yields a usable hint (the common synthetic-fixture case). `a` is the A (video) side, `b` the
    /// B (audio) side; `lag_ms` is the A − B absolute-timestamp difference.
    ///
    /// Priority (§12.1): BWF sample reference (with compatible dates) → BWF date+time → container
    /// creation time → same-device ordering. The FIRST source that yields a finite, well-formed lag
    /// wins; its [`HintReliability`] follows §12.2, including the implausible-disagreement demotion
    /// to [`HintReliability::DebugOnly`].
    pub fn from_views(a: &MetadataView, b: &MetadataView) -> Option<TimeHint> {
        // ---- §12.1 #1: BWF sample reference, day-relative, comparable only with matching dates ----
        if let (Some(sa), Some(sb)) = (
            sample_reference_seconds(a),
            sample_reference_seconds(b),
        ) {
            // Day-relative offsets are directly comparable ONLY when the BWF dates agree (same
            // recording day). With a wall-clock day offset available we could in principle add it,
            // but a date MISMATCH here is exactly the implausible/clock-misconfig signal §12.2 wants
            // demoted, so we treat the sample reference as rankable only with compatible dates.
            let dates_compatible = bwf_dates_compatible(a, b);
            let lag_ms = (sa - sb) * 1000.0;
            if lag_ms.is_finite() {
                let reliability = if !dates_compatible {
                    // BWF present but dates disagree ⇒ not trustworthy as an absolute lag.
                    // Fall through to the wall-clock sources rather than rank a day-relative lag.
                    None
                } else {
                    Some(HintReliability::Medium)
                };
                if let Some(reliability) = reliability {
                    return Some(TimeHint {
                        lag_ms,
                        tolerance_ms: TOL_SAMPLE_REF_MS,
                        source: TimeHintSource::BwfSampleReference,
                        reliability,
                    });
                }
            }
        }

        // ---- §12.1 #2: BWF date + time (second-resolution wall clock) ----
        if let (Some(ta), Some(tb)) = (bwf_wallclock_seconds(a), bwf_wallclock_seconds(b)) {
            if let Some(hint) = wallclock_hint(ta, tb, TimeHintSource::BwfDateTime, TOL_WALLCLOCK_MS) {
                return Some(hint);
            }
        }

        // ---- §12.1 #3: container creation_time (second-resolution wall clock, often cross-device)
        if let (Some(ta), Some(tb)) = (
            a.creation_time_utc.as_deref().and_then(parse_iso8601_utc_seconds),
            b.creation_time_utc.as_deref().and_then(parse_iso8601_utc_seconds),
        ) {
            // Cross-device creation time is LOW reliability when plausible; implausible ⇒ DebugOnly.
            if let Some(mut hint) =
                wallclock_hint(ta, tb, TimeHintSource::ContainerCreationTime, TOL_WALLCLOCK_MS)
            {
                if hint.reliability == HintReliability::Medium {
                    // wallclock_hint assigns Medium for a plausible disagreement; container creation
                    // time across devices is only LOW (§12.2). (Implausible stays DebugOnly.)
                    hint.reliability = HintReliability::Low;
                }
                return Some(hint);
            }
        }

        // ---- §12.1 #4: same-device ordering from a stable device tag ----
        // Only when BOTH sides carry the SAME device tag AND some monotonic order signal exists.
        // We cannot derive an absolute lag, only "A before B" / "B before A"; the order signal is the
        // sign of whatever coarse timestamp both carry (sample reference, or a wall clock). This is
        // ORDER-ONLY (§12.2): it may order an exact tie, never accept/reject.
        if let (Some(da), Some(db)) = (a.device_tag.as_deref(), b.device_tag.as_deref()) {
            if !da.is_empty() && da == db {
                if let Some(sign) = same_device_order_sign(a, b) {
                    if sign != 0.0 {
                        return Some(TimeHint {
                            lag_ms: sign * SAME_DEVICE_ORDER_NOMINAL_MS,
                            tolerance_ms: TOL_ORDER_MS,
                            source: TimeHintSource::SameDeviceOrdering,
                            reliability: HintReliability::OrderOnly,
                        });
                    }
                }
            }
        }

        None
    }

    /// Convenience: parse directly from two [`MediaInfo`] records (§12.1).
    pub fn from_media(a: &MediaInfo, b: &MediaInfo) -> Option<TimeHint> {
        Self::from_views(&MetadataView::from_media(a), &MetadataView::from_media(b))
    }
}

/// Build a wall-clock hint from two absolute second-resolution timestamps `ta`, `tb` (§12.2). A
/// plausible disagreement ⇒ `Medium` (callers may demote, e.g. container creation time ⇒ `Low`); an
/// implausible disagreement (beyond [`IMPLAUSIBLE_DISAGREEMENT_MS`]) ⇒ `DebugOnly` (kept for debug,
/// never ranked — the 11.6-year case). `None` only if the resulting lag is non-finite (defensive).
fn wallclock_hint(ta: f64, tb: f64, source: TimeHintSource, tolerance_ms: f64) -> Option<TimeHint> {
    let lag_ms = (ta - tb) * 1000.0;
    if !lag_ms.is_finite() {
        return None;
    }
    let reliability = if lag_ms.abs() > IMPLAUSIBLE_DISAGREEMENT_MS {
        HintReliability::DebugOnly
    } else {
        HintReliability::Medium
    };
    Some(TimeHint {
        lag_ms,
        tolerance_ms,
        source,
        reliability,
    })
}

/// BWF sample-reference start-of-recording offset from the BWF day origin, seconds (§12.1 #1):
/// `time_reference_samples / source_sample_rate`. `None` unless both are present and the rate is
/// positive. Day-relative (resets at midnight); comparability across tracks needs matching dates.
fn sample_reference_seconds(v: &MetadataView) -> Option<f64> {
    let samples = v.bwf_time_reference_samples?;
    let sr = v.source_sample_rate?;
    if sr == 0 {
        return None;
    }
    let s = samples as f64 / sr as f64;
    s.is_finite().then_some(s)
}

/// Whether two BWF dates are "compatible" for comparing day-relative sample references (§12.1 #1):
/// both present and equal. Absent dates ⇒ not compatible (we cannot assume the same day). A
/// normalized exact-string compare; BWF dates are `YYYY-MM-DD` or `YYYY:MM:DD` (both accepted).
fn bwf_dates_compatible(a: &MetadataView, b: &MetadataView) -> bool {
    match (a.bwf_date.as_deref(), b.bwf_date.as_deref()) {
        (Some(da), Some(db)) => normalize_date(da) == normalize_date(db) && !normalize_date(da).is_empty(),
        _ => false,
    }
}

/// Absolute wall-clock seconds for a BWF date+time pair (§12.1 #2). Combines the day count (days
/// since a fixed civil epoch, via [`days_from_civil`]) with the intra-day seconds. `None` unless
/// both a parseable date and time are present. The epoch is arbitrary but FIXED, so DIFFERENCES
/// between two tracks are exact regardless of epoch.
fn bwf_wallclock_seconds(v: &MetadataView) -> Option<f64> {
    let date = v.bwf_date.as_deref()?;
    let time = v.bwf_time.as_deref()?;
    let (y, mo, d) = parse_ymd(date)?;
    let (h, mi, s) = parse_hms(time)?;
    let days = days_from_civil(y, mo, d)?;
    let secs = days as f64 * 86400.0 + (h as f64 * 3600.0 + mi as f64 * 60.0 + s as f64);
    secs.is_finite().then_some(secs)
}

/// The order-sign for same-device ordering (§12.1 #4): `+1` if A is LATER than B, `-1` if earlier,
/// `0`/`None` if no monotonic signal exists. Uses, in order, BWF sample reference (same device ⇒
/// same date/rate domain), then BWF wall clock, then container creation time. Only the SIGN is used.
fn same_device_order_sign(a: &MetadataView, b: &MetadataView) -> Option<f64> {
    let cmp = |x: f64, y: f64| -> f64 {
        if x > y {
            1.0
        } else if x < y {
            -1.0
        } else {
            0.0
        }
    };
    if let (Some(sa), Some(sb)) = (sample_reference_seconds(a), sample_reference_seconds(b)) {
        return Some(cmp(sa, sb));
    }
    if let (Some(ta), Some(tb)) = (bwf_wallclock_seconds(a), bwf_wallclock_seconds(b)) {
        return Some(cmp(ta, tb));
    }
    if let (Some(ta), Some(tb)) = (
        a.creation_time_utc.as_deref().and_then(parse_iso8601_utc_seconds),
        b.creation_time_utc.as_deref().and_then(parse_iso8601_utc_seconds),
    ) {
        return Some(cmp(ta, tb));
    }
    None
}

// ===================================================================================================
// Deterministic date/time parsing (no external `chrono`/`time` dependency)
// ===================================================================================================

/// Normalize a BWF date string to `YYYY-MM-DD` for comparison: accept `:` or `-` separators and trim
/// whitespace. A non-date string normalizes to itself (so two equal garbage strings still compare
/// equal, but a date vs garbage do not).
fn normalize_date(s: &str) -> String {
    s.trim().replace(':', "-")
}

/// Parse a `YYYY-MM-DD` or `YYYY:MM:DD` date into `(year, month, day)`. `None` on any malformed
/// field or out-of-range month/day. Pure integer parsing.
fn parse_ymd(s: &str) -> Option<(i64, u32, u32)> {
    let norm = s.trim().replace(':', "-");
    let mut it = norm.split('-');
    let y: i64 = it.next()?.parse().ok()?;
    let mo: u32 = it.next()?.parse().ok()?;
    let d: u32 = it.next()?.parse().ok()?;
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) {
        return None;
    }
    Some((y, mo, d))
}

/// Parse an `HH:MM:SS` (optionally `HH:MM`) time into `(hour, minute, second)`. Trailing fractional
/// seconds or zone suffixes are ignored on the seconds field. `None` on malformed/out-of-range.
fn parse_hms(s: &str) -> Option<(u32, u32, u32)> {
    let t = s.trim();
    let mut it = t.split(':');
    let h: u32 = it.next()?.parse().ok()?;
    let mi: u32 = it.next()?.parse().ok()?;
    // Seconds may carry a fractional part or a zone; take the leading integer run.
    let s_field = it.next().unwrap_or("0");
    let sec_int: String = s_field.chars().take_while(|c| c.is_ascii_digit()).collect();
    let se: u32 = if sec_int.is_empty() { 0 } else { sec_int.parse().ok()? };
    if h >= 24 || mi >= 60 || se >= 61 {
        return None;
    }
    Some((h, mi, se))
}

/// Parse an ISO-8601 UTC timestamp (`YYYY-MM-DDThh:mm:ss[.fff][Z]`, the ffmpeg `creation_time`
/// form) to absolute seconds from the fixed civil epoch. `None` on malformed input. Accepts a space
/// or `T` date/time separator and tolerates a trailing `Z` / fractional seconds.
fn parse_iso8601_utc_seconds(s: &str) -> Option<f64> {
    let t = s.trim();
    // Split on 'T' (or a space) into date and time halves.
    let (date_part, time_part) = if let Some(idx) = t.find('T') {
        (&t[..idx], &t[idx + 1..])
    } else if let Some(idx) = t.find(' ') {
        (&t[..idx], &t[idx + 1..])
    } else {
        // Date only ⇒ midnight.
        (t, "00:00:00")
    };
    let (y, mo, d) = parse_ymd(date_part)?;
    // Strip a trailing zone marker for the time field (Z or +hh:mm); we treat the stamp as UTC and
    // only need DIFFERENCES, so a uniform zone handling suffices for two same-form stamps.
    let time_trim = time_part.trim_end_matches('Z');
    let (h, mi, se) = parse_hms(time_trim)?;
    let days = days_from_civil(y, mo, d)?;
    let secs = days as f64 * 86400.0 + (h as f64 * 3600.0 + mi as f64 * 60.0 + se as f64);
    secs.is_finite().then_some(secs)
}

/// Days since the civil epoch `0000-03-01` for a proleptic-Gregorian `(y, m, d)`, via Howard
/// Hinnant's branch-free `days_from_civil` algorithm. The epoch is arbitrary but FIXED; only
/// differences between two dates are used, so the choice of epoch cancels. `None` only defensively
/// (the month range is already validated by the parsers). Pure integer arithmetic — deterministic.
fn days_from_civil(y: i64, m: u32, d: u32) -> Option<i64> {
    if !(1..=12).contains(&m) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let m = m as i64;
    let d = d as i64;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    Some(era * 146097 + doe - 719468)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view_bwf(samples: u64, sr: u32, date: &str, time: &str) -> MetadataView {
        MetadataView {
            bwf_time_reference_samples: Some(samples),
            source_sample_rate: Some(sr),
            bwf_date: Some(date.to_string()),
            bwf_time: Some(time.to_string()),
            ..Default::default()
        }
    }

    // ---- §12.1: lag is the A − B absolute-timestamp difference ----
    #[test]
    fn sample_reference_lag_is_a_minus_b() {
        // A starts 2.0 s into the day, B starts 0.5 s in (same date, same rate) ⇒ lag +1.5 s.
        let a = view_bwf(96_000, 48_000, "2026-06-14", "10:00:02");
        let b = view_bwf(24_000, 48_000, "2026-06-14", "10:00:00");
        let h = TimeHint::from_views(&a, &b).expect("sample-ref hint");
        assert_eq!(h.source, TimeHintSource::BwfSampleReference);
        assert_eq!(h.reliability, HintReliability::Medium, "BWF sample-ref + matching dates ⇒ medium");
        assert!((h.lag_ms - 1500.0).abs() < 1e-9, "lag {} ms ≠ +1500", h.lag_ms);
        assert!(h.lag_ms.is_finite() && h.tolerance_ms.is_finite());
    }

    // ---- §12.2: BWF sample-ref + compatible dates ⇒ medium ----
    #[test]
    fn bwf_with_compatible_dates_is_medium() {
        let a = view_bwf(0, 48_000, "2026:06:14", "00:00:00"); // ':' separator accepted
        let b = view_bwf(48_000, 48_000, "2026-06-14", "00:00:01");
        let h = TimeHint::from_views(&a, &b).unwrap();
        assert_eq!(h.reliability, HintReliability::Medium);
        assert!(h.reliability.can_rank() && h.reliability.can_order());
    }

    // BWF sample-ref but dates DISAGREE ⇒ not ranked via sample-ref; falls back to wall clock, which
    // here is a plausible disagreement ⇒ Medium BwfDateTime (still rankable, but via a DIFFERENT,
    // date-aware source). The day-relative sample lag is NOT used.
    #[test]
    fn bwf_sample_ref_incompatible_dates_falls_back_to_wallclock() {
        let a = view_bwf(96_000, 48_000, "2026-06-14", "10:00:02");
        let b = view_bwf(24_000, 48_000, "2026-06-15", "10:00:00"); // next day
        let h = TimeHint::from_views(&a, &b).unwrap();
        assert_eq!(h.source, TimeHintSource::BwfDateTime, "incompatible dates ⇒ not sample-ref");
        // A is 2026-06-14 10:00:02, B is 2026-06-15 10:00:00 ⇒ A is ~1 day earlier ⇒ large negative.
        assert!(h.lag_ms < 0.0 && h.lag_ms.is_finite());
    }

    // ---- §12.2: creation-time-only cross-device ⇒ low ----
    #[test]
    fn creation_time_only_is_low() {
        let a = MetadataView {
            creation_time_utc: Some("2026-06-14T10:00:05Z".to_string()),
            ..Default::default()
        };
        let b = MetadataView {
            creation_time_utc: Some("2026-06-14T10:00:00Z".to_string()),
            ..Default::default()
        };
        let h = TimeHint::from_views(&a, &b).unwrap();
        assert_eq!(h.source, TimeHintSource::ContainerCreationTime);
        assert_eq!(h.reliability, HintReliability::Low, "cross-device creation time ⇒ low");
        assert!((h.lag_ms - 5000.0).abs() < 1e-9, "A 5 s after B ⇒ +5000 ms, got {}", h.lag_ms);
        assert!(h.reliability.can_rank() && h.reliability.can_order());
    }

    // ---- §12.2: IMPLAUSIBLE date disagreement ⇒ DebugOnly, not ranked (the 11.6-year case) ----
    #[test]
    fn implausible_disagreement_is_debug_only() {
        // Camera clock at the 1904 QuickTime epoch vs a 2016 Zoom recorder ⇒ ~11.6-year gap. BWF
        // dates differ wildly, so the sample-ref path declines; the wall-clock difference is the
        // 11.6-year gap ⇒ DebugOnly.
        let a = view_bwf(0, 48_000, "2004-01-01", "00:00:00");
        let b = view_bwf(0, 48_000, "2015-08-01", "12:00:00"); // ~11.6 years later
        let h = TimeHint::from_views(&a, &b).unwrap();
        assert_eq!(h.reliability, HintReliability::DebugOnly, "11.6-year gap ⇒ debug-only");
        assert!(!h.reliability.can_rank(), "DebugOnly must NOT seed a hypothesis");
        assert!(!h.reliability.can_order(), "DebugOnly must NOT order a tie");
        // The lag is still finite and present (for debug explanation), just not usable.
        assert!(h.lag_ms.is_finite() && h.lag_ms.abs() > IMPLAUSIBLE_DISAGREEMENT_MS);
    }

    // ---- §12.2: same-device monotonic ordering ⇒ order-only ----
    #[test]
    fn same_device_ordering_is_order_only() {
        // Same device tag, no usable absolute lag beyond ordering: A's sample reference is later.
        let a = MetadataView {
            bwf_time_reference_samples: Some(480_000),
            source_sample_rate: Some(48_000),
            bwf_date: Some("2026-06-14".into()),
            device_tag: Some("ZOOM H4n".into()),
            ..Default::default()
        };
        let mut b = a.clone();
        b.bwf_time_reference_samples = Some(240_000); // B earlier in the day
                                                      // Give them DIFFERENT dates so the sample-ref ranking path declines and we reach §12.1 #4.
        b.bwf_date = Some("2026-06-13".into());
        let h = TimeHint::from_views(&a, &b).unwrap();
        assert_eq!(h.source, TimeHintSource::SameDeviceOrdering);
        assert_eq!(h.reliability, HintReliability::OrderOnly);
        assert!(!h.reliability.can_rank(), "OrderOnly must NOT seed a hypothesis");
        assert!(h.reliability.can_order(), "OrderOnly MAY order an exact tie");
        // The sign reflects A-later (sample ref via §12.1 #4 sign, both on same device).
        // (Dates differ, so wall-clock sign would also apply; sample-ref sign is checked first.)
        assert!(h.lag_ms > 0.0, "A later ⇒ positive order sign, got {}", h.lag_ms);
    }

    // ---- determinism + graceful absence ----
    #[test]
    fn no_metadata_yields_no_hint() {
        // The synthetic-fixture case: nothing present ⇒ None (no hint, graceful absence).
        let a = MetadataView::default();
        let b = MetadataView::default();
        assert!(TimeHint::from_views(&a, &b).is_none());
    }

    #[test]
    fn parsing_is_deterministic() {
        let a = view_bwf(96_000, 48_000, "2026-06-14", "10:00:02");
        let b = view_bwf(24_000, 48_000, "2026-06-14", "10:00:00");
        let h1 = TimeHint::from_views(&a, &b).unwrap();
        let h2 = TimeHint::from_views(&a, &b).unwrap();
        assert_eq!(h1, h2, "hint parsing must be deterministic");
    }

    // days_from_civil sanity: a one-day and a one-year difference are exact integers.
    #[test]
    fn days_from_civil_differences_exact() {
        let d0 = days_from_civil(2026, 6, 14).unwrap();
        let d1 = days_from_civil(2026, 6, 15).unwrap();
        assert_eq!(d1 - d0, 1, "consecutive days differ by 1");
        let dy0 = days_from_civil(2024, 1, 1).unwrap();
        let dy1 = days_from_civil(2025, 1, 1).unwrap();
        assert_eq!(dy1 - dy0, 366, "2024 is a leap year ⇒ 366 days");
    }

    // ISO-8601 creation_time parsing tolerates 'T'/space/Z and fractional seconds.
    #[test]
    fn iso8601_variants_parse() {
        let a = parse_iso8601_utc_seconds("2026-06-14T10:00:00.500000Z").unwrap();
        let b = parse_iso8601_utc_seconds("2026-06-14 10:00:00").unwrap();
        assert!((a - b).abs() < 1.0, "fractional/Z vs plain differ < 1 s");
        assert!(parse_iso8601_utc_seconds("not-a-date").is_none());
    }
}
