// Engine: the CoupleKit handle over cpl_ctx (SPEC §4.9). Thread-safe; hot reads
// are callable from any thread, jobs report from worker threads.

import CCouple
import Foundation

public struct CoreError: Error, Sendable {
    public let kind: String
    public let message: String
}

public final class Engine: @unchecked Sendable {
    let ctx: UInt64

    public init(ffmpegPath: String? = nil, ffprobePath: String? = nil, cacheDir: String? = nil) throws {
        var config: [String: String] = [:]
        config["ffmpegPath"] = ffmpegPath
        config["ffprobePath"] = ffprobePath
        config["cacheDir"] = cacheDir
        let json = String(data: try JSONEncoder().encode(config), encoding: .utf8)!
        ctx = cpl_ctx_new(json)
        guard ctx != 0 else { throw Engine.staticTakeError() }
    }

    deinit {
        cpl_ctx_free(ctx)
    }

    public static var coreVersion: String {
        takeString(cpl_version()) ?? "?"
    }

    public static var abiVersion: UInt32 {
        cpl_abi_version()
    }

    // ---------- error plumbing ----------

    /// Consume a core-allocated C string.
    static func takeString(_ p: UnsafeMutablePointer<CChar>?) -> String? {
        guard let p else { return nil }
        defer { cpl_string_free(p) }
        return String(cString: p)
    }

    func takeError() -> CoreError {
        Engine.staticTakeError(ctx: ctx)
    }

    static func staticTakeError(ctx: UInt64 = 0) -> CoreError {
        guard let json = takeString(cpl_take_error_json(ctx)),
              let data = json.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            return CoreError(kind: "internal", message: "unknown core error")
        }
        var kind = "internal"
        if let e = obj["error"] as? String {
            kind = e
        } else if let e = obj["error"] as? [String: Any], let k = e.keys.first {
            kind = k
        }
        return CoreError(kind: kind, message: (obj["message"] as? String) ?? json)
    }

    func decodeJSON<T: Decodable>(_ p: UnsafeMutablePointer<CChar>?, as type: T.Type) throws -> T {
        guard let s = Engine.takeString(p) else { throw takeError() }
        guard let data = s.data(using: .utf8) else {
            throw CoreError(kind: "internal", message: "non-UTF8 core JSON")
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    /// Decode a JSON payload that may be the literal `null` (e.g. measure_local misses).
    func decodeOptionalJSON<T: Decodable>(_ p: UnsafeMutablePointer<CChar>?, as type: T.Type) throws -> T? {
        guard let s = Engine.takeString(p) else { throw takeError() }
        if s == "null" { return nil }
        guard let data = s.data(using: .utf8) else { return nil }
        return try JSONDecoder().decode(T.self, from: data)
    }

    // ---------- media ----------

    public func probe(_ path: String) throws -> MediaInfo {
        try decodeJSON(cpl_probe_json(ctx, path), as: MediaInfo.self)
    }

    /// Start an import job (§4.3); the job's value is the imported TrackHandle.
    public func importStart(_ path: String, role: TrackRole) throws -> Job<TrackMeta> {
        let box = JobBox()
        let user = Unmanaged.passRetained(box).toOpaque()
        let id = cpl_import_start(ctx, path, role == .camera ? 0 : 1, jobTrampoline, user)
        guard id != 0 else {
            Unmanaged<JobBox>.fromOpaque(user).release()
            throw takeError()
        }
        return Job(engine: self, box: box, id: id)
    }

    public func track(_ meta: TrackMeta) -> TrackHandle {
        TrackHandle(engine: self, meta: meta)
    }

    public func sessionNew(camera: TrackHandle, external: TrackHandle) throws -> SessionHandle {
        let id = cpl_session_new(ctx, camera.id, external.id)
        guard id != 0 else { throw takeError() }
        return SessionHandle(engine: self, id: id)
    }

    /// fit_ls over (t_s, lag_ms) points; nil when < 2 points (§4.4).
    public func fitLS(_ points: [(Double, Double)]) -> Fit? {
        let arr = points.map { [$0.0, $0.1] }
        guard let json = try? JSONEncoder().encode(arr),
              let s = String(data: json, encoding: .utf8) else { return nil }
        return try? decodeOptionalJSON(cpl_fit_ls_json(s), as: Fit.self)
    }

    public func cacheGC(maxBytes: UInt64) -> UInt64 {
        cpl_cache_gc(ctx, maxBytes)
    }
}
