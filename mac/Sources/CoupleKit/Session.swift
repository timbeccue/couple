// SessionHandle: alignment state lives in core (SPEC §4.5 ownership rule); the
// shell mutates through setters. Hot setters map 1:1; cold fields (anchors,
// frames, measurements, layout…) go through the state-JSON round trip — the
// C-ABI-conformant route per §4.9's inventory.

import CCouple
import Foundation

public final class SessionHandle: @unchecked Sendable {
    public let id: UInt64
    let engine: Engine

    init(engine: Engine, id: UInt64) {
        self.engine = engine
        self.id = id
    }

    public func free() {
        cpl_session_free(engine.ctx, id)
    }

    // ---------- hot setters (gesture rate) ----------

    public func setOffsetMs(_ v: Double) {
        _ = cpl_set_offset_ms(engine.ctx, id, v)
    }

    /// A drift write while driftMode == .none flips the mode to .resample (core-enforced).
    public func setDriftPpmLive(_ v: Double) {
        _ = cpl_set_drift_ppm_live(engine.ctx, id, v)
    }

    public func commitDrift() {
        _ = cpl_commit_drift(engine.ctx, id)
    }

    // ---------- cold setters ----------

    public func setDriftMode(_ mode: DriftMode) {
        _ = cpl_set_drift_mode(engine.ctx, id, mode == .none ? 1 : 0)
    }

    public func setVerified(_ v: Bool) {
        _ = cpl_set_verified(engine.ctx, id, v)
    }

    public func state() throws -> AlignmentState {
        try engine.decodeJSON(cpl_state_json(engine.ctx, id), as: AlignmentState.self)
    }

    public func setState(_ s: AlignmentState) throws {
        let json = String(data: try JSONEncoder().encode(s), encoding: .utf8)!
        guard cpl_set_state_json(engine.ctx, id, json) == 0 else {
            throw engine.takeError()
        }
    }

    /// Read-modify-write for the cold state fields.
    public func updateState(_ mutate: (inout AlignmentState) -> Void) throws {
        var s = try state()
        mutate(&s)
        try setState(s)
    }

    // ---------- sync ----------

    /// Full auto-sync job (§4.4). On Synced/LowConfidence core writes the mapping
    /// + driftWindows into the session state; on Failed the state is untouched.
    ///
    /// `seed` (PLAN-SPECTRAL-FINGERPRINT-MATCHING §11.3): when non-nil the sync begins
    /// from that accepted offset/drift instead of the stage-1 blind global scan + its z
    /// gate — pass `SyncSeedPolicy.plan(...)`'s decision, never an invented value. nil
    /// is the unseeded sync, byte-identical to before the seed existed.
    public func autoSyncStart(seed: SyncSeedPlan? = nil) throws -> Job<SyncResult> {
        try engine.startJob { user in
            if let seed {
                return cpl_autosync_start_seeded(engine.ctx, id, nil, seed.offsetMs, seed.driftPpm,
                                               jobTrampoline, user)
            } else {
                return cpl_autosync_start(engine.ctx, id, nil, jobTrampoline, user)
            }
        }
    }

    /// measure_local (§5.4): nil = no confident measurement (silence/degenerate peak).
    public func measureLocal(tc: Double, halfWinS: Double = 9, centerMs: Double, spanMs: Double) -> LocalMeasurement? {
        (try? engine.decodeOptionalJSON(
            cpl_measure_local_json(engine.ctx, id, tc, halfWinS, centerMs, spanMs),
            as: LocalMeasurement.self)) ?? nil
    }
}
