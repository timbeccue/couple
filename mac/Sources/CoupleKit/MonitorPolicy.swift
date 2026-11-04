// Monitor A/B decisions (UX-DECISIONS 2026-11-03) — WHICH monitor mode the `a` (flip) /
// `b` (both) keys and the 4-way HUD picker land on, plus the two remembered modes the
// shell stores for the next call (the solo heard last; the both-mode used last).
//
// The DECISION lives in `couple_core::policy::monitor` and is reached over the FFI
// (`cpl_policy_monitor`); this type is a thin WRAPPER, style-matched to `SyncSeedPolicy`.
// On a core error (malformed request — unreachable with this in-process call) it maps to
// the CONSERVATIVE fallback: keep the current mode and the memory as they were. A failure
// can never change what the user hears.

import CCouple
import Foundation

/// The outcome of one monitor request: the mode to set and the memory to keep.
public struct MonitorPlan: Sendable, Equatable {
    public var next: MonitorMode
    public var lastSide: MonitorMode
    public var lastBoth: MonitorMode

    public init(next: MonitorMode, lastSide: MonitorMode, lastBoth: MonitorMode) {
        self.next = next
        self.lastSide = lastSide
        self.lastBoth = lastBoth
    }
}

public enum MonitorPolicy {
    public enum Action: String, Sendable {
        case flip
        case both
        case select
    }

    /// Decide the next monitor mode. `lastSide`/`lastBoth` are the memories the previous
    /// plan returned (nil before the first call); `chosen` is the picker's selection for
    /// `.select` only.
    public static func plan(_ action: Action,
                            current: MonitorMode,
                            chosen: MonitorMode? = nil,
                            lastSide: MonitorMode?,
                            lastBoth: MonitorMode?) -> MonitorPlan {
        var req: [String: Any] = [
            "action": action.rawValue,
            "current": current.wireName,
        ]
        if let chosen { req["chosen"] = chosen.wireName }
        if let lastSide { req["lastSide"] = lastSide.wireName }
        if let lastBoth { req["lastBoth"] = lastBoth.wireName }
        return planFromEnvelope(callPolicy(req),
                                fallback: MonitorPlan(next: current,
                                                      lastSide: lastSide ?? .camera,
                                                      lastBoth: lastBoth ?? .overlay))
    }

    /// Map a decoded envelope `value` to the plan; any missing/unknown field ⇒ `fallback`
    /// (the conservative keep-everything plan). Internal so a test can drive the error
    /// path without reaching the C ABI (PLAN §3.2).
    @inline(never)
    static func planFromEnvelope(_ value: [String: Any]?, fallback: MonitorPlan) -> MonitorPlan {
        guard let next = (value?["next"] as? String).flatMap(MonitorMode.init(wireName:)),
              let lastSide = (value?["lastSide"] as? String).flatMap(MonitorMode.init(wireName:)),
              let lastBoth = (value?["lastBoth"] as? String).flatMap(MonitorMode.init(wireName:))
        else { return fallback }
        return MonitorPlan(next: next, lastSide: lastSide, lastBoth: lastBoth)
    }

    private static func callPolicy(_ req: [String: Any]) -> [String: Any]? {
        guard let data = try? JSONSerialization.data(withJSONObject: req),
              let json = String(data: data, encoding: .utf8) else { return nil }
        return PolicyEnvelope.decodeValue(cpl_policy_monitor(json))
    }
}
