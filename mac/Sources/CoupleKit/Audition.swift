// Audition streams (SPEC §4.8). The fill path is real-time-safe: the shell's
// audio render callback should call `cpl_audition_fill` directly via the raw
// (ctx, stream) handles exposed here — no Swift allocation on that path.

import CCouple
import Foundation

public enum MonitorMode: Int32, CaseIterable, Sendable {
    case pan = 0
    case overlay = 1
    case null = 2

    public var label: String {
        switch self {
        case .pan: return "Pan L/R"
        case .overlay: return "Overlay"
        case .null: return "Null"
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
        let box = JobBox()
        let user = Unmanaged.passRetained(box).toOpaque()
        let jobId = cpl_export_start(engine.ctx, id, optionsJSON, jobTrampoline, user)
        guard jobId != 0 else {
            Unmanaged<JobBox>.fromOpaque(user).release()
            throw engine.takeError()
        }
        return Job(engine: engine, box: box, id: jobId)
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
