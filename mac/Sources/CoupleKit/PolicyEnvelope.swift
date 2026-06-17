// Shared decoder for the pure-policy FFI envelope (PLAN-CORE-SHELL-BOUNDARY Phase E).
//
// Every `cpl_policy_*` function returns the UNIFORM envelope
//   { "ok": true,  "value": { ... } }   on success
//   { "ok": false, "error": { "kind": "invalid", "message": "..." } }  on a malformed/stale request
// (deliberately NOT the older null-on-malformed contract of `cpl_triangle_closes`). These wrappers
// consume the core-allocated C string, parse the envelope, and return the `value` payload only on
// `ok:true`. A nil result means "core reported an error OR the call/parse failed" — every policy
// wrapper maps that to its own documented CONSERVATIVE fallback (never a silent wrong answer; PLAN
// §3.2 risk "Silent policy fallback"). The user-facing types/strings stay in each policy's file.

import CCouple
import Foundation

enum PolicyEnvelope {
    /// Consume a `cpl_policy_*` result, free it, and return the success `value` object, or nil when
    /// the envelope is an error / the payload is missing / the string could not be read. The caller
    /// maps nil to its conservative fallback.
    static func decodeValue(_ p: UnsafeMutablePointer<CChar>?) -> [String: Any]? {
        guard let s = Engine.takeString(p),
              let data = s.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              obj["ok"] as? Bool == true
        else { return nil }
        return obj["value"] as? [String: Any]
    }

    /// As `decodeValue`, but for policies whose `value` is a JSON ARRAY (e.g. anchor times). Returns
    /// the success array, or nil on an error envelope / missing payload / read failure.
    static func decodeValueArray(_ p: UnsafeMutablePointer<CChar>?) -> [Any]? {
        guard let s = Engine.takeString(p),
              let data = s.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              obj["ok"] as? Bool == true
        else { return nil }
        return obj["value"] as? [Any]
    }

    /// As `decodeValue`, but for policies whose `value` is a SCALAR that may be JSON `null` (e.g.
    /// the "+ anchor" time). Returns the success `value` (an NSNumber, or nil when the envelope is
    /// an error, the value is JSON null, or the read/parse failed). The caller maps nil to its
    /// conservative fallback (for "+ anchor": add nothing). `allowFragments` lets the top-level
    /// envelope object parse normally; the scalar lives inside it, so no fragment parsing is needed.
    static func decodeScalar(_ p: UnsafeMutablePointer<CChar>?) -> Any? {
        guard let s = Engine.takeString(p),
              let data = s.data(using: .utf8),
              let obj = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
              obj["ok"] as? Bool == true
        else { return nil }
        let value = obj["value"]
        // JSON null decodes to NSNull; treat it (and a missing key) as "no value".
        if value is NSNull { return nil }
        return value
    }
}
