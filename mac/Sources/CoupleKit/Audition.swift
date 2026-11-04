// Audition streams (SPEC §4.8). The fill path is real-time-safe: the shell's
// audio render callback should call `cpl_audition_fill` directly via the raw
// (ctx, stream) handles exposed here — no Swift allocation on that path.

import CCouple
import Foundation

public enum MonitorMode: Int32, CaseIterable, Sendable {
    case pan = 0
    case overlay = 1
    /// Difference monitor (cam − ext): exact-cancellation test oracle only —
    /// excluded from `allCases` so the UI never offers it.
    case null = 2
    /// A/B solo (UX-DECISIONS 2026-11-03): only the camera track, in both ears.
    case camera = 3
    /// A/B solo: only the external track, in both ears.
    case external = 4

    /// Picker order: the two solos bracket the two both-modes, so the ends are
    /// the A/B sides and `a` hops between them.
    public static var allCases: [MonitorMode] { [.camera, .pan, .overlay, .external] }

    public var label: String {
        switch self {
        case .pan: return "Pan L/R"
        case .overlay: return "Overlay"
        case .null: return "Null"
        case .camera: return "Camera"
        case .external: return "External"
        }
    }

    /// True for the two A/B solos (one track audible).
    public var isSolo: Bool { self == .camera || self == .external }

    /// Wire name shared with core serde (`policy::monitor` requests).
    public var wireName: String {
        switch self {
        case .pan: return "pan"
        case .overlay: return "overlay"
        case .null: return "null"
        case .camera: return "camera"
        case .external: return "external"
        }
    }

    public init?(wireName: String) {
        switch wireName {
        case "pan": self = .pan
        case "overlay": self = .overlay
        case "null": self = .null
        case "camera": self = .camera
        case "external": self = .external
        default: return nil
        }
    }
}

public final class AuditionHandle: @unchecked Sendable {
    public let id: UInt64
    /// Raw ctx handle for direct C calls from the audio render thread.
    public let rawCtx: UInt64
    private let engine: Engine

    init(engine: Engine, id: UInt64) {
        self.engine = engine
        self.rawCtx = engine.ctx
        self.id = id
    }

    /// Non-RT convenience fill (tests, offline rendering).
    @discardableResult
    public func fill(into buffer: UnsafeMutablePointer<Float>, capacityFrames: UInt32)
        -> (frames: UInt32, streamT: Double) {
        var frames: UInt32 = 0
        var streamT: Double = 0
        _ = cpl_audition_fill(rawCtx, id, buffer, capacityFrames, &frames, &streamT)
        return (frames, streamT)
    }

    public func seek(to t: Double) {
        _ = cpl_audition_seek(rawCtx, id, t)
    }

    public func setMode(_ mode: MonitorMode) {
        _ = cpl_audition_set_mode(rawCtx, id, mode.rawValue)
    }

    /// Per-track monitor gains (1 = unity, 0 = mute), on top of the core's
    /// loudness matching. Click-free.
    public func setGains(cam: Float, ext: Float) {
        _ = cpl_audition_set_gains(rawCtx, id, cam, ext)
    }

    public func close() {
        cpl_audition_close(rawCtx, id)
    }
}

extension SessionHandle {
    public func auditionOpen(mode: MonitorMode, tStart: Double) throws -> AuditionHandle {
        let id = cpl_audition_open(engine.ctx, self.id, mode.rawValue, tStart)
        guard id != 0 else { throw engine.takeError() }
        return AuditionHandle(engine: engine, id: id)
    }
}

extension SessionHandle {
    /// Start an export job (§4.9); value = ExportReport JSON (decoded loosely).
    public func exportStart(optionsJSON: String) throws -> Job<ExportReport> {
        try engine.startJob { user in
            cpl_export_start(engine.ctx, id, optionsJSON, jobTrampoline, user)
        }
    }
}

/// Loose decode of the export job's terminal payload.
public struct ExportReport: Decodable, Sendable {
    public var outputPath: String?
    public var bytes: UInt64?

    private enum CodingKeys: String, CodingKey {
        case outputPath, bytes
    }

    public init(from decoder: Decoder) throws {
        let c = try? decoder.container(keyedBy: CodingKeys.self)
        outputPath = try? c?.decodeIfPresent(String.self, forKey: .outputPath)
        bytes = try? c?.decodeIfPresent(UInt64.self, forKey: .bytes)
    }
}
