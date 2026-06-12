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
    public func autoSyncStart() throws -> Job<SyncResult> {
        let box = JobBox()
        let user = Unmanaged.passRetained(box).toOpaque()
        let jobId = cpl_autosync_start(engine.ctx, id, nil, jobTrampoline, user)
        guard jobId != 0 else {
            Unmanaged<JobBox>.fromOpaque(user).release()
            throw engine.takeError()
        }
        return Job(engine: engine, box: box, id: jobId)
    }

    /// measure_local (§5.4): nil = no confident measurement (silence/degenerate peak).
    public func measureLocal(tc: Double, halfWinS: Double = 9, centerMs: Double, spanMs: Double) -> LocalMeasurement? {
        (try? engine.decodeOptionalJSON(
            cpl_measure_local_json(engine.ctx, id, tc, halfWinS, centerMs, spanMs),
            as: LocalMeasurement.self)) ?? nil
    }
}
