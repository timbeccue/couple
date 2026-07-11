// Auto-sync seed selection (PLAN-SPECTRAL-FINGERPRINT-MATCHING §11.3 wiring) — decides
// WHETHER a full sync should begin from a known offset instead of the §5 stage-1 blind
// global scan, and from WHICH source. The blind scan's z acceptance gate can reject a
// true pair the pairing resolver ALREADY proved by fingerprint evidence (quiet/short
// clips: difficult-match test 3, z 8.0 < z_min 8.5), which used to strand the audio at
// 0:00 with "no plausible correlation peak" despite a correctly resolved row.
//
// The product rules (UX-DECISIONS 2026-07-03):
//   - A row whose selection carries resolver-accepted evidence (candidate z > 0) syncs
//     FROM that evidence lag.
//   - An EXPLICIT re-sync after a FAILED sync, when the user has coarse-dragged the
//     external lane away from 0, seeds from that manual placement (the rescue the
//     failure toast promises). Manual rescue outranks evidence.
//   - Ambient queue syncs never read the current offset.
//   - No evidence, no rescue ⇒ unseeded: blind search + z gate, unchanged.
//
// The DECISION lives in `couple_core::policy::sync_seed` and is reached over the FFI
// (`cpl_policy_sync_seed`); this type is a thin WRAPPER. On a core error (malformed
// request — unreachable with this in-process call) it maps to the CONSERVATIVE
// fallback: no seed, i.e. today's unseeded sync. A failure can never invent an offset.

import CCouple
import Foundation

/// The seed a sync should begin from, as decided by core. `source` is display/telemetry
/// only (it changes no math shell-side).
public struct SyncSeedPlan: Sendable, Equatable {
    public enum Source: String, Sendable {
        case evidence
        case manual
    }

    public var offsetMs: Double
    public var driftPpm: Double
    public var source: Source

    public init(offsetMs: Double, driftPpm: Double, source: Source) {
        self.offsetMs = offsetMs
        self.driftPpm = driftPpm
        self.source = source
    }
}

public enum SyncSeedPolicy {
    /// Decide the sync seed for one run. nil ⇒ run the unseeded sync (`autoSyncStart()`).
    ///
    /// - Parameters:
    ///   - evidenceLagMs / evidenceZ: the selected candidate's lag and acceptance score
    ///     (nil when the row has no selection). A manual-re-pair placeholder candidate
    ///     (z 0, lag 0) is not evidence — core requires z > 0.
    ///   - evidenceDriftPpm: the candidate's drift telemetry when known; recorded in the
    ///     seed only (the drift pass re-measures regardless).
    ///   - explicit: true for a user-invoked re-sync; false for ambient queue syncs.
    ///   - lastSyncFailed: whether this couple's most recent sync ended Failed.
    ///   - currentOffsetMs: the editor's current external-lane offset (0 = never placed).
    public static func plan(evidenceLagMs: Double?,
                            evidenceZ: Double?,
                            evidenceDriftPpm: Double?,
                            explicit: Bool,
                            lastSyncFailed: Bool,
                            currentOffsetMs: Double) -> SyncSeedPlan? {
        var req: [String: Any] = [
            "explicit": explicit,
            "lastSyncFailed": lastSyncFailed,
            "currentOffsetMs": currentOffsetMs,
        ]
        if let evidenceLagMs { req["evidenceLagMs"] = evidenceLagMs }
        if let evidenceZ { req["evidenceZ"] = evidenceZ }
        if let evidenceDriftPpm { req["evidenceDriftPpm"] = evidenceDriftPpm }
        return planFromEnvelope(callPolicy(req))
    }

    /// Map a decoded envelope `value` to the plan. A nil value (error envelope /
    /// call/parse failure), a JSON-null seed, or a malformed seed object all ⇒ nil —
    /// the unseeded sync, never a fabricated offset. Internal so a test can drive the
    /// error path without reaching the C ABI (PLAN §3.2).
    @inline(never)
    static func planFromEnvelope(_ value: [String: Any]?) -> SyncSeedPlan? {
        guard let seed = value?["seed"] as? [String: Any],
              let offsetMs = (seed["offsetMs"] as? NSNumber)?.doubleValue,
              let driftPpm = (seed["driftPpm"] as? NSNumber)?.doubleValue,
              let source = (seed["source"] as? String).flatMap(SyncSeedPlan.Source.init)
        else { return nil }
        return SyncSeedPlan(offsetMs: offsetMs, driftPpm: driftPpm, source: source)
    }

    /// Encode a request dict, call `cpl_policy_sync_seed`, and return the envelope
    /// `value` object (nil on any encode/call/parse failure or an error envelope).
    private static func callPolicy(_ req: [String: Any]) -> [String: Any]? {
        guard let data = try? JSONSerialization.data(withJSONObject: req),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return PolicyEnvelope.decodeValue(cpl_policy_sync_seed(json))
    }
}
