// Alignment mapping math (UX §2.1, single source of truth):
//   u(t) = t·(1 + driftPpm·1e−6) + offsetMs/1000
//   L(t) = offsetMs + t·driftPpm·1e−3   (ms)

import Foundation

struct Mapping: Equatable {
    var offsetMs: Double
    var driftPpm: Double

    var rate: Double { 1 + driftPpm * 1e-6 }

    /// Video time → external-file time (seconds).
    func u(_ t: Double) -> Double {
        t * rate + offsetMs / 1000
    }

    /// Local lag in ms at video time t.
    func localLagMs(_ t: Double) -> Double {
        offsetMs + t * driftPpm * 1e-3
    }

    /// External-file time → video time (seconds).
    func tForU(_ u: Double) -> Double {
        (u - offsetMs / 1000) / rate
    }

    /// Overlap on the video timeline, nil when empty.
    func overlap(camDur: Double, extDur: Double) -> ClosedRange<Double>? {
        let t0 = max(0, tForU(0))
        let t1 = min(camDur, tForU(extDur))
        return t1 > t0 ? t0...t1 : nil
    }
}
