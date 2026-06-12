// Job bridging (SPEC §11.1): core jobs report through a C callback from worker
// threads; CoupleKit exposes them as an AsyncStream of progress plus an awaited
// decoded result.

import CCouple
import Foundation

public struct ProgressUpdate: Sendable {
    /// Core phase name (SPEC §4.6): "Probe", "Decode", "Pyramid", "Finalize",
    /// "SyncCoarse", "SyncRefine", "SyncWindows", "SyncFit", "ExportRender", "ExportMux".
    public let phase: String
    public let fraction: Double
}

public enum JobError: Error, Sendable {
    case cancelled
    case core(kind: String, message: String)
    case startFailed(message: String)
    case badPayload
}

/// One in-flight core job. `progress` may be consumed by at most one listener;
/// `value()` awaits the terminal event and decodes the "done" payload.
public final class Job<Output: Decodable & Sendable>: @unchecked Sendable {
    public let id: UInt64
    public let progress: AsyncStream<ProgressUpdate>

    private let engine: Engine
    private let box: JobBox

    init(engine: Engine, box: JobBox, id: UInt64) {
        self.engine = engine
        self.box = box
        self.id = id
        self.progress = box.progressStream
    }

    public func cancel() {
        cpl_job_cancel(engine.ctx, id)
    }

    public func value() async throws -> Output {
        let payload = try await box.awaitTerminal()
        guard let data = payload.data(using: .utf8),
              let out = try? JSONDecoder().decode(Output.self, from: data) else {
            throw JobError.badPayload
        }
        return out
    }
}

/// Shared state between the C callback (worker thread) and Swift awaiters.
final class JobBox: @unchecked Sendable {
    private let lock = NSLock()
    private var terminal: Result<String, JobError>?
    private var waiter: CheckedContinuation<String, Error>?
    private var progressCont: AsyncStream<ProgressUpdate>.Continuation!
    let progressStream: AsyncStream<ProgressUpdate>

    init() {
        var cont: AsyncStream<ProgressUpdate>.Continuation!
        progressStream = AsyncStream { cont = $0 }
        progressCont = cont
    }

    func awaitTerminal() async throws -> String {
        try await withCheckedThrowingContinuation { c in
            lock.lock()
            if let t = terminal {
                lock.unlock()
                c.resume(with: t.mapError { $0 as Error })
            } else {
                waiter = c
                lock.unlock()
            }
        }
    }

    func handle(phase: String, fraction: Double, payload: String?) {
        switch phase {
        case "done":
            finish(.success(payload ?? ""))
        case "cancelled":
            finish(.failure(.cancelled))
        case "error":
            finish(.failure(Self.decodeError(payload)))
        default:
            progressCont.yield(ProgressUpdate(phase: phase, fraction: fraction))
        }
    }

    private func finish(_ result: Result<String, JobError>) {
        progressCont.finish()
        lock.lock()
        terminal = result
        let w = waiter
        waiter = nil
        lock.unlock()
        w?.resume(with: result.mapError { $0 as Error })
    }

    static func decodeError(_ payload: String?) -> JobError {
        guard let data = payload?.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return .core(kind: "internal", message: payload ?? "unknown error")
        }
        let err = obj["error"]
        let kind: String
        if let s = err as? String {
            kind = s
        } else if let d = err as? [String: Any], let k = d.keys.first {
            kind = k
        } else {
            kind = "internal"
        }
        return .core(kind: kind, message: (obj["message"] as? String) ?? String(describing: err))
    }
}

/// C trampoline: `user` is an unbalanced retain of the JobBox, released on the
/// terminal call (core guarantees exactly one terminal callback per job).
let jobTrampoline: @convention(c) (
    UInt64, UnsafePointer<CChar>?, Double, UnsafePointer<CChar>?, UnsafeMutableRawPointer?
) -> Void = { _, phasePtr, fraction, payloadPtr, user in
    guard let user, let phasePtr else { return }
    let phase = String(cString: phasePtr)
    let payload = payloadPtr.map { String(cString: $0) }
    let isTerminal = phase == "done" || phase == "error" || phase == "cancelled"
    let box: JobBox = isTerminal
        ? Unmanaged<JobBox>.fromOpaque(user).takeRetainedValue()
        : Unmanaged<JobBox>.fromOpaque(user).takeUnretainedValue()
    box.handle(phase: phase, fraction: fraction, payload: payload)
}
