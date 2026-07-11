//! Auto-sync seed selection — the PURE decision behind seeding the full sync
//! (PLAN-SPECTRAL-FINGERPRINT-MATCHING §11.3 wiring). The §5 stage-1 global envelope scan is a
//! BLIND search guarded by a z acceptance gate; when the pairing resolver has ALREADY accepted a
//! spectral-landmark match, that gate can reject a true pair the fingerprint evidence proved
//! (quiet/short clips: the difficult-match test-3 case, z 8.0 < z_min 8.5). This policy decides
//! WHETHER a sync should begin from a known offset instead of rediscovering it, and from WHICH
//! source. The shell relays the decision into [`crate::sync::auto_sync_tracks_seeded`]; it never
//! invents a seed itself.
//!
//! The product rules (UX-DECISIONS 2026-07-03):
//!   - A row whose selection carries accepted fingerprint evidence syncs FROM that evidence lag
//!     (the §11.3 contract: begin from the accepted offset/drift; never rerun blind discovery just
//!     to fail its gate).
//!   - An EXPLICIT user re-sync after a FAILED sync, when the user has coarse-dragged the external
//!     lane away from 0, seeds from that manual placement — the "coarse-drag to rescue, then
//!     re-sync" toast finally means what it says. Manual rescue outranks evidence: the user
//!     dragged BECAUSE the automatic result (if any) was wrong or absent.
//!   - Ambient (non-explicit) syncs never read the current offset — batch re-resolution must not
//!     be steered by whatever placement a half-inspected row happens to have.
//!   - No evidence and no rescue ⇒ unseeded: the blind search + z gate is unchanged.
//!
//! CONTRACT (the testable invariants):
//!   1. Explicit + last-failed + non-zero current offset ⇒ Manual seed at that offset, drift 0.
//!   2. Otherwise, finite evidence lag WITH a positive acceptance score (`evidence_z > 0`) ⇒
//!      Evidence seed (lag, drift; a non-finite/absent drift is recorded as 0 —
//!      [`crate::sync::SyncSeed::drift_ppm`] is telemetry, the drift pass re-measures
//!      regardless). A manual-re-pair placeholder candidate (z 0, lag 0) is NOT evidence.
//!   3. Otherwise ⇒ no seed (byte-identical unseeded behavior).
//!   4. Non-finite inputs never seed: a NaN offset/lag/z disqualifies its rule, it is never
//!      passed through.

use serde::{Deserialize, Serialize};

/// Where a seed came from — shells may surface this (toast copy, telemetry); it changes no math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SeedSource {
    /// The resolver-accepted fingerprint evidence lag of the row's selected candidate.
    Evidence,
    /// The user's coarse-dragged placement, adopted on an explicit re-sync after a failure.
    Manual,
}

/// The seed the sync should begin from. Mirrors [`crate::sync::SyncSeed`] plus provenance.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeedPlan {
    pub offset_ms: f64,
    pub drift_ppm: f64,
    pub source: SeedSource,
}

/// Decide the sync seed. Pure; no IO.
///
/// - `evidence_lag_ms` / `evidence_z` / `evidence_drift_ppm`: the selected candidate's lag, its
///   resolver acceptance score, and drift. A candidate is EVIDENCE only when `evidence_z > 0`:
///   resolver-accepted candidates always carry a positive score (spectral score or envelope z),
///   while a placeholder fabricated by a manual re-pair (fresh pool drag) carries `z == 0` and
///   `lag == 0` — its lag is a fabrication and must never steer a sync.
/// - `explicit`: true for a user-invoked re-sync; false for ambient queue syncs.
/// - `last_sync_failed`: whether the couple's most recent sync ended `Failed`.
/// - `current_offset_ms`: the editor's current external-lane offset (0 when never placed).
pub fn plan(
    evidence_lag_ms: Option<f64>,
    evidence_z: Option<f64>,
    evidence_drift_ppm: Option<f64>,
    explicit: bool,
    last_sync_failed: bool,
    current_offset_ms: f64,
) -> Option<SeedPlan> {
    // (1) Manual rescue: an explicit re-sync after a failure adopts the user's placement. Gated on
    // explicit so ambient batch syncs can never be steered by a stray drag; gated on non-zero so
    // "never touched" (the reset value) is not mistaken for a placement.
    if explicit && last_sync_failed && current_offset_ms != 0.0 && current_offset_ms.is_finite() {
        return Some(SeedPlan {
            offset_ms: current_offset_ms,
            drift_ppm: 0.0,
            source: SeedSource::Manual,
        });
    }
    // (2) Accepted evidence seeds every sync of the row (§11.3). `z > 0` distinguishes a
    // resolver-accepted candidate from a manual-re-pair placeholder (z 0, lag 0).
    if let (Some(lag), Some(z)) = (evidence_lag_ms, evidence_z) {
        if lag.is_finite() && z.is_finite() && z > 0.0 {
            return Some(SeedPlan {
                offset_ms: lag,
                drift_ppm: evidence_drift_ppm.filter(|d| d.is_finite()).unwrap_or(0.0),
                source: SeedSource::Evidence,
            });
        }
    }
    // (3) Blind search, unchanged.
    None
}

// ============================ FFI request dispatch ============================

/// The request `cpl_policy_sync_seed` decodes — the inputs of [`plan`], camelCase on the wire.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Request {
    pub evidence_lag_ms: Option<f64>,
    pub evidence_z: Option<f64>,
    pub evidence_drift_ppm: Option<f64>,
    pub explicit: bool,
    pub last_sync_failed: bool,
    pub current_offset_ms: f64,
}

impl Default for Request {
    fn default() -> Self {
        Self {
            evidence_lag_ms: None,
            evidence_z: None,
            evidence_drift_ppm: None,
            explicit: false,
            last_sync_failed: false,
            current_offset_ms: 0.0,
        }
    }
}

/// The envelope `value`: always an OBJECT so the Swift wrapper decodes one shape; `seed` is
/// `null` for the unseeded decision.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub seed: Option<SeedPlan>,
}

/// Dispatch a decoded `Request` to [`plan`].
pub fn dispatch(req: &Request) -> Response {
    Response {
        seed: plan(
            req.evidence_lag_ms,
            req.evidence_z,
            req.evidence_drift_ppm,
            req.explicit,
            req.last_sync_failed,
            req.current_offset_ms,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // (2) Evidence seeds an ordinary (ambient, never-synced) run.
    #[test]
    fn evidence_seeds_ambient_sync() {
        let p = plan(Some(-82006.8), Some(8.17), Some(-477.8), false, false, 0.0).unwrap();
        assert_eq!(p.source, SeedSource::Evidence);
        assert_eq!(p.offset_ms, -82006.8);
        assert_eq!(p.drift_ppm, -477.8);
    }

    // (3) No evidence, no rescue ⇒ unseeded.
    #[test]
    fn no_inputs_no_seed() {
        assert_eq!(plan(None, None, None, false, false, 0.0), None);
        assert_eq!(plan(None, None, None, true, false, 0.0), None);
        // Failed but never dragged (offset still 0) ⇒ still blind.
        assert_eq!(plan(None, None, None, true, true, 0.0), None);
    }

    // (2) A manual-re-pair placeholder (z 0, lag 0) is not evidence; nor is a lag with no z.
    #[test]
    fn placeholder_candidate_is_not_evidence() {
        assert_eq!(plan(Some(0.0), Some(0.0), None, false, false, 0.0), None);
        assert_eq!(plan(Some(-82006.8), Some(0.0), None, false, false, 0.0), None);
        assert_eq!(plan(Some(-82006.8), None, None, false, false, 0.0), None);
    }

    // (1) The manual rescue: explicit + failed + dragged.
    #[test]
    fn manual_rescue_on_explicit_resync_after_failure() {
        let p = plan(None, None, None, true, true, -82500.0).unwrap();
        assert_eq!(p.source, SeedSource::Manual);
        assert_eq!(p.offset_ms, -82500.0);
        assert_eq!(p.drift_ppm, 0.0);
    }

    // (1) Manual rescue outranks evidence (the user dragged because automatic was wrong).
    #[test]
    fn manual_rescue_outranks_evidence() {
        let p = plan(Some(-82006.8), Some(8.17), Some(100.0), true, true, -70000.0).unwrap();
        assert_eq!(p.source, SeedSource::Manual);
        assert_eq!(p.offset_ms, -70000.0);
    }

    // Ambient syncs never read the current offset, even after a failure.
    #[test]
    fn ambient_sync_ignores_manual_placement() {
        assert_eq!(plan(None, None, None, false, true, -70000.0), None);
        // ...but evidence still applies ambiently.
        let p = plan(Some(-82006.8), Some(8.17), None, false, true, -70000.0).unwrap();
        assert_eq!(p.source, SeedSource::Evidence);
    }

    // An explicit re-sync after SUCCESS re-seeds from evidence, not the (now non-zero) offset.
    #[test]
    fn explicit_resync_after_success_uses_evidence() {
        let p = plan(Some(-82006.8), Some(8.17), None, true, false, -82005.0).unwrap();
        assert_eq!(p.source, SeedSource::Evidence);
        assert_eq!(p.offset_ms, -82006.8);
    }

    // (4) Non-finite inputs disqualify their rule instead of leaking through.
    #[test]
    fn non_finite_inputs_never_seed() {
        assert_eq!(plan(Some(f64::NAN), Some(8.0), None, false, false, 0.0), None);
        assert_eq!(plan(Some(-10.0), Some(f64::NAN), None, false, false, 0.0), None);
        assert_eq!(plan(None, None, None, true, true, f64::INFINITY), None);
        // A NaN drift is dropped to 0, not forwarded.
        let p = plan(Some(-10.0), Some(8.0), Some(f64::NAN), false, false, 0.0).unwrap();
        assert_eq!(p.drift_ppm, 0.0);
    }

    // Wire shapes: camelCase request with defaults; response object with null/object seed.
    #[test]
    fn request_and_response_wire_shapes() {
        let req: Request = serde_json::from_str(
            r#"{"evidenceLagMs":-82006.8,"evidenceZ":8.17,"explicit":true,"lastSyncFailed":false,"currentOffsetMs":0}"#,
        )
        .unwrap();
        let resp = dispatch(&req);
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""offsetMs":-82006.8"#) && json.contains(r#""source":"evidence""#));

        let none = dispatch(&serde_json::from_str::<Request>("{}").unwrap());
        assert_eq!(serde_json::to_string(&none).unwrap(), r#"{"seed":null}"#);
    }
}
