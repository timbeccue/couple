import Foundation
import Testing
@testable import CoupleKit

/// Deterministic, media-free coverage of `ExportIncludePolicy` — the include-for-export rule
/// (PLAN-COMPACT-MODE §3, D1/D8; UX-DECISIONS V2.12) round-tripped through
/// `cpl_policy_export_include`, plus the EXACT fallback (the same rule re-applied shell-side).
/// The app target owns only the derived state (`explicit`, `exportedAt`) and the checkbox.
@Suite struct ExportIncludePolicyTests {
    private func row(_ verdict: MatchQuality, _ explicit: Bool?, _ exported: Bool) -> ExportIncludeRow {
        ExportIncludeRow(verdict: verdict, explicit: explicit, exported: exported)
    }

    /// Every input combination (3 verdicts × 3 checkbox states × 2 export states).
    private var allRows: [ExportIncludeRow] {
        var rows: [ExportIncludeRow] = []
        for verdict in [MatchQuality.confident, .questionable, .none] {
            for explicit in [Bool?.none, true, false] {
                for exported in [false, true] {
                    rows.append(row(verdict, explicit, exported))
                }
            }
        }
        return rows
    }

    // (3) DEFAULT = good ∧ not yet exported — a fresh good couple is in; an exported one drops out.
    @Test func defaultIsGoodAndNotYetExported() {
        #expect(ExportIncludePolicy.included(verdict: .confident, explicit: nil, exported: false))
        #expect(!ExportIncludePolicy.included(verdict: .confident, explicit: nil, exported: true))
    }

    // (2) EXPLICIT OVERRIDES THE DEFAULT WHILE GOOD — both ways, whatever `exported` says.
    @Test func explicitOverridesDefaultWhileGood() {
        #expect(!ExportIncludePolicy.included(verdict: .confident, explicit: false, exported: false))
        #expect(ExportIncludePolicy.included(verdict: .confident, explicit: true, exported: true))
        #expect(ExportIncludePolicy.included(verdict: .confident, explicit: true, exported: false))
        #expect(!ExportIncludePolicy.included(verdict: .confident, explicit: false, exported: true))
    }

    // (1) NON-GOOD IS NEVER INCLUDED — not even by an explicit toggle.
    @Test func nonGoodIsNeverIncluded() {
        for verdict in [MatchQuality.questionable, .none] {
            for explicit in [Bool?.none, true, false] {
                for exported in [false, true] {
                    #expect(!ExportIncludePolicy.included(verdict: verdict, explicit: explicit, exported: exported),
                            "\(verdict) \(String(describing: explicit)) \(exported)")
                }
            }
        }
        #expect(ExportIncludePolicy.canInclude(.confident))
        #expect(!ExportIncludePolicy.canInclude(.questionable))
        #expect(!ExportIncludePolicy.canInclude(.none))
    }

    // (4)/(5) One decision per row, in order; Files list = included ∪ exported; the count is the
    //         number of included rows.
    @Test func evaluateReturnsOneDecisionPerRowInOrder() {
        let rows = [
            row(.confident, nil, false),     // included (default)
            row(.confident, nil, true),      // exported, untouched ⇒ out, still listed
            row(.confident, true, true),     // re-included
            row(.confident, false, false),   // opted out ⇒ out, not listed
            row(.questionable, true, false), // never
            row(.none, nil, true),           // never included; listed because exported
        ]
        let d = ExportIncludePolicy.evaluate(rows: rows)
        #expect(d == [
            ExportIncludeDecision(included: true, inFilesList: true),
            ExportIncludeDecision(included: false, inFilesList: true),
            ExportIncludeDecision(included: true, inFilesList: true),
            ExportIncludeDecision(included: false, inFilesList: false),
            ExportIncludeDecision(included: false, inFilesList: false),
            ExportIncludeDecision(included: false, inFilesList: true),
        ])
        #expect(d.filter(\.included).count == 2)
        #expect(ExportIncludePolicy.evaluate(rows: []).isEmpty)
    }

    // FALLBACK EQUALITY (PLAN §3.2, Phase-E style) — the shell-side rule is byte-equal to core on
    // EVERY input, so an error envelope changes nothing. If core's rule ever changes, this fails
    // until the fallback follows.
    @Test func fallbackEqualsTheCoreRuleOnEveryInput() {
        let rows = allRows
        let viaCore = ExportIncludePolicy.evaluate(rows: rows)
        let viaFallback = ExportIncludePolicy.fallback(rows: rows)
        #expect(viaCore == viaFallback)
        // One at a time too (the convenience path).
        for r in rows {
            #expect(ExportIncludePolicy.included(verdict: r.verdict, explicit: r.explicit, exported: r.exported)
                    == ExportIncludePolicy.fallback(rows: [r])[0].included)
        }
    }

    // ERROR PATH IS EXACT — a nil value (error envelope / failed call), a malformed `rows`, a
    // wrong-length array or a malformed row all fall back to the rule for the WHOLE batch (never a
    // partially-decoded mix), and that equals what core returns.
    @Test func malformedEnvelopeValueFallsBackToTheRule() {
        let rows = allRows
        let expected = ExportIncludePolicy.evaluate(rows: rows)
        #expect(ExportIncludePolicy.decisions(fromEnvelopeValue: nil, rows: rows) == expected)
        #expect(ExportIncludePolicy.decisions(fromEnvelopeValue: [:], rows: rows) == expected)
        #expect(ExportIncludePolicy.decisions(fromEnvelopeValue: ["rows": "junk"], rows: rows) == expected)
        #expect(ExportIncludePolicy.decisions(fromEnvelopeValue: ["rows": [["included": true, "inFilesList": true]]],
                                              rows: rows) == expected)
        let malformedRow: [String: Any] = ["rows": Array(repeating: ["included": "yes"] as [String: Any],
                                                         count: rows.count)]
        #expect(ExportIncludePolicy.decisions(fromEnvelopeValue: malformedRow, rows: rows) == expected)
        // Sanity: a well-formed value still decodes verbatim (core's answer wins, not the fallback).
        let one = [row(.confident, nil, false)]
        #expect(ExportIncludePolicy.decisions(fromEnvelopeValue: ["rows": [["included": false, "inFilesList": true]]],
                                              rows: one)
                == [ExportIncludeDecision(included: false, inFilesList: true)])
    }
}
