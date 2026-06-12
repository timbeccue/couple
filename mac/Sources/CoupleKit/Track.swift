// TrackHandle: hot read paths (band, raw_window) with caller-owned buffers
// (SPEC §4.3 — tens of µs, no per-call allocation), plus onset queries.

import CCouple
import Foundation

/// Reusable buffer for band() results. Allocate once per rendering surface and
/// pass to `TrackHandle.band(into:)` every frame.
public final class BandBuffer {
    public internal(set) var mins: [Int8]
    public internal(set) var maxs: [Int8]
    public private(set) var count: Int = 0
    public private(set) var levelSpb: UInt32 = 0
    public private(set) var colT0: Double = 0
    public private(set) var colDt: Double = 0

    public init(capacity: Int = 4096) {
        mins = [Int8](repeating: 0, count: capacity)
        maxs = [Int8](repeating: 0, count: capacity)
    }

    func ensureCapacity(_ n: Int) {
        if mins.count < n {
            mins = [Int8](repeating: 0, count: n)
            maxs = [Int8](repeating: 0, count: n)
        }
    }

    func setResult(count: Int, levelSpb: UInt32, colT0: Double, colDt: Double) {
        self.count = count
        self.levelSpb = levelSpb
        self.colT0 = colT0
        self.colDt = colDt
    }
}

public final class TrackHandle: @unchecked Sendable {
    public let meta: TrackMeta
    let engine: Engine

    public var id: UInt64 { meta.id }
    public var durationS: Double { meta.durationS }

    init(engine: Engine, meta: TrackMeta) {
        self.engine = engine
        self.meta = meta
    }

    public func free() {
        cpl_track_free(engine.ctx, meta.id)
    }

    /// band() into a reusable buffer; returns false on error (buffer left stale).
    @discardableResult
    public func band(t0: Double, t1: Double, nCols: Int, into buf: BandBuffer) -> Bool {
        guard nCols > 0 else { return false }
        buf.ensureCapacity(nCols)
        var levelSpb: UInt32 = 0
        var colT0 = 0.0
        var colDt = 0.0
        let status = buf.mins.withUnsafeMutableBufferPointer { mins in
            buf.maxs.withUnsafeMutableBufferPointer { maxs in
                cpl_band(engine.ctx, meta.id, t0, t1, UInt32(nCols),
                         mins.baseAddress, maxs.baseAddress, &levelSpb, &colT0, &colDt)
            }
        }
        guard status == 0 else {
            _ = engine.takeError()
            return false
        }
        buf.setResult(count: nCols, levelSpb: levelSpb, colT0: colT0, colDt: colDt)
        return true
    }

    /// Mono 48 kHz f32 from the PCM cache (canonical timeline). Returns frames written.
    public func rawWindow(t0: Double, t1: Double, into out: inout [Float]) -> Int {
        var frames: UInt32 = 0
        let status = out.withUnsafeMutableBufferPointer { buf in
            cpl_raw_window(engine.ctx, meta.id, t0, t1, buf.baseAddress, UInt32(buf.count), &frames)
        }
        guard status == 0 else {
            _ = engine.takeError()
            return 0
        }
        return Int(frames)
    }

    public func rms(t0: Double, t1: Double) -> Double? {
        var dbfs = 0.0
        guard cpl_rms(engine.ctx, meta.id, t0, t1, &dbfs) == 0 else {
            _ = engine.takeError()
            return nil
        }
        return dbfs
    }

    public func onsets(t0: Double, t1: Double, limit: UInt32 = 100) -> [Onset] {
        (try? engine.decodeJSON(cpl_onsets_json(engine.ctx, meta.id, t0, t1, limit), as: [Onset].self)) ?? []
    }

    public func bestOnset(in t0: Double, _ t1: Double) -> Onset? {
        (try? engine.decodeOptionalJSON(cpl_best_onset_in_json(engine.ctx, meta.id, t0, t1), as: Onset.self)) ?? nil
    }

    /// Snap query: best onset within ±halfSpan of t (UX §2.2, ±4 s default).
    public func bestOnset(near t: Double, halfSpan: Double = 4) -> Onset? {
        (try? engine.decodeOptionalJSON(cpl_best_onset_near_json(engine.ctx, meta.id, t, halfSpan), as: Onset.self)) ?? nil
    }
}
