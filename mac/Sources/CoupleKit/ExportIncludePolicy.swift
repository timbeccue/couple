// Include-for-export decision (PLAN-COMPACT-MODE §3, D1/D8; UX-DECISIONS V2.12) — the PURE
// core behind `CoupleRow.included`, the "Export N files" count and the export rail's Files
// list. D1 replaces the opt-in "approve ≡ queued" flag with an opt-OUT inclusion flag whose
// default is DERIVED, so the derivation must live in one place both shells share:
//
//   included    = verdict is good ∧ (explicit ?? ¬exported)
//   inFilesList = included ∪ exported
//
//   - `verdict` is the row's match quality; only `.confident` ("good") is includable. After D1
//     the verdict is the only gate before a file leaves the app, so a non-good row is NEVER
//     included — not even by an explicit toggle.
//   - `explicit` is the user's checkbox (`true`/`false`), or nil when untouched.
//   - `exported` is "already exported" (`exportedAt != nil`, persisted per couple — D8). An
//     exported couple drops out of the default so the next export does not re-emit it;
//     `explicit == true` re-includes it. It stays in the Files list with its ✓ time.
//
// Inclusion is DERIVED shell state with the rule in core: the app stores only `explicit` and
// `exportedAt` on `CoupleRow` and re-derives on every verdict change, undo, restore or sync
// completion (no recompute points). Repair clears both stored optionals shell-side.
//
// The DECISION lives in `couple_core::policy::export_include` and is reached over the FFI
// (`cpl_policy_export_include`); this type is a thin WRAPPER. On a core error (malformed
// request — unreachable with this in-process call) the fallback re-applies the SAME rule
// shell-side, so it is EXACT, not merely conservative: the policy is a total two-line
// predicate, and the usual "exclude everything" fallback would silently empty the export
// button instead of describing the queue. The core stays the authority — the Rust body and
// `fallback(rows:)` are locked equal by the mirror test, and a deliberate rule change lands
// in core first and fails that test here until this file follows.

import CCouple
import Foundation

/// One row's inclusion inputs (mirrors `couple_core::policy::export_include::IncludeInput`).
public struct ExportIncludeRow: Sendable, Equatable {
    /// The row's match quality (`CoupleRow.verdict`); only `.confident` is includable.
    public var verdict: MatchQuality
    /// The user's checkbox, or nil when untouched (⇒ the default rule applies).
    public var explicit: Bool?
    /// The couple has been exported already (`exportedAt != nil`, D8).
    public var exported: Bool

    public init(verdict: MatchQuality, explicit: Bool?, exported: Bool) {
        self.verdict = verdict
        self.explicit = explicit
        self.exported = exported
    }
}

/// One row's inclusion outcome (mirrors `IncludeDecision`).
public struct ExportIncludeDecision: Sendable, Equatable {
    /// The row goes out on the next export (drives the checkbox and the Ready-section count).
    public var included: Bool
    /// The row appears in the export rail's Files list (included ∪ exported).
    public var inFilesList: Bool

    public init(included: Bool, inFilesList: Bool) {
        self.included = included
        self.inFilesList = inFilesList
    }
}

public enum ExportIncludePolicy {
    /// Decide every row at once (one FFI round trip): one decision per input row, in input
    /// order. The N in "Export N files" is the number of `included` decisions.
    public static func evaluate(rows: [ExportIncludeRow]) -> [ExportIncludeDecision] {
        let wireRows: [[String: Any]] = rows.map { row in
            [
                "verdict": row.verdict.wireValue,
                "explicit": row.explicit.map { $0 as Any } ?? NSNull(),
                "exported": row.exported,
            ]
        }
        let req: [String: Any] = ["rows": wireRows]
        let value: [String: Any]?
        if let data = try? JSONSerialization.data(withJSONObject: req),
           let json = String(data: data, encoding: .utf8) {
            value = PolicyEnvelope.decodeValue(cpl_policy_export_include(json))
        } else {
            value = nil
        }
        return decisions(fromEnvelopeValue: value, rows: rows)
    }

    /// Convenience for one row: `verdict is good ∧ (explicit ?? ¬exported)`, decided in core.
    public static func included(verdict: MatchQuality, explicit: Bool?, exported: Bool) -> Bool {
        let row = ExportIncludeRow(verdict: verdict, explicit: explicit, exported: exported)
        return evaluate(rows: [row]).first?.included ?? fallback(rows: [row])[0].included
    }

    /// Whether a row with this verdict may be included at all (D1: good only) — the checkbox is
    /// shown for these rows only. Decided in core as "included when explicitly opted in and not
    /// yet exported", which the rule reduces to the verdict gate alone.
    public static func canInclude(_ verdict: MatchQuality) -> Bool {
        included(verdict: verdict, explicit: true, exported: false)
    }

    /// Map a decoded envelope `value` to the decisions, applying the EXACT fallback. A nil value
    /// (error envelope / failed call), a `rows` array of the wrong length, or a malformed row ⇒
    /// `fallback(rows:)` for the whole batch (never a partially-decoded mix). Internal so a test
    /// can drive the error path without reaching the C ABI (PLAN §3.2). `@inline(never)` keeps a
    /// standalone symbol so the test can link it.
    @inline(never)
    static func decisions(fromEnvelopeValue value: [String: Any]?,
                          rows: [ExportIncludeRow]) -> [ExportIncludeDecision] {
        guard let wire = value?["rows"] as? [[String: Any]], wire.count == rows.count else {
            return fallback(rows: rows)
        }
        var out: [ExportIncludeDecision] = []
        out.reserveCapacity(wire.count)
        for w in wire {
            guard let included = w["included"] as? Bool, let inFilesList = w["inFilesList"] as? Bool else {
                return fallback(rows: rows)
            }
            out.append(ExportIncludeDecision(included: included, inFilesList: inFilesList))
        }
        return out
    }

    /// The rule re-applied shell-side — byte-equal to `couple_core::policy::export_include`
    /// (`included`, `in_files_list`). Used ONLY when the envelope is unusable; the mirror test
    /// asserts it agrees with core on every input.
    @inline(never)
    static func fallback(rows: [ExportIncludeRow]) -> [ExportIncludeDecision] {
        rows.map { row in
            let included = row.verdict == .confident && (row.explicit ?? !row.exported)
            return ExportIncludeDecision(included: included, inFilesList: included || row.exported)
        }
    }
}
