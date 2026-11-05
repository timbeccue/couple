//! Include-for-export decision (PLAN-COMPACT-MODE §3, decisions D1/D8; UX-DECISIONS V2.12) — the
//! PURE, testable core behind `CoupleRow.included`, the "Export N files" count and the Files-list
//! membership. A NEW policy (not a relocation): D1 replaces the opt-in "approve ≡ queued" flag with
//! an opt-OUT inclusion flag whose default is derived, and the derivation lives here so both shells
//! agree.
//!
//! THE RULE (PLAN-COMPACT-MODE §3 / UX-DECISIONS V2.12):
//!
//!   included = verdict is good ∧ (explicit ?? ¬exported)
//!
//!   - `verdict` is the row's match quality (`policy::match_quality::MatchQuality`; "good" ≡
//!     `Confident`). After D1 the verdict is the only gate before a file leaves the app, so a
//!     non-good row can NEVER be included — not even by an explicit toggle (`can_include`).
//!   - `explicit` is the user's checkbox (`Some(true/false)`), or `None` when the user has not
//!     touched the row. An explicit value overrides the default while the row stays includable.
//!   - `exported` is "this couple has been exported already" (D8: `exportedAt` is persisted per
//!     couple). With no explicit toggle, an exported couple drops out of the default so the next
//!     export does not re-emit it; `explicit == Some(true)` re-includes it.
//!   - Files-list membership (`in_files_list`) = included ∪ exported — a couple that already left
//!     the app stays listed (with its ✓ time) even when it is no longer going out again.
//!
//! Inclusion is DERIVED shell state with the rule here: the shell stores only `explicit` and
//! `exportedAt` and re-derives on every verdict change, undo, restore or sync completion (no
//! recompute points). Repair (manual pick / drop) clears both stored optionals shell-side, so the
//! default rule applies again; this module does not need to know about repair.
//!
//! CONTRACT (the testable invariants):
//!   1. NON-GOOD IS NEVER INCLUDED — `Questionable`/`None` ⇒ `included == false` whatever
//!      `explicit`/`exported` say; `can_include` is the same predicate.
//!   2. EXPLICIT OVERRIDES THE DEFAULT WHILE GOOD — for a good row, `Some(x)` ⇒ `included == x`
//!      regardless of `exported`.
//!   3. DEFAULT = NOT YET EXPORTED — for a good row with `explicit == None`, `included == !exported`
//!      (so exported ∧ ¬explicit ⇒ excluded; a fresh good couple is included).
//!   4. FILES LIST = INCLUDED ∪ EXPORTED — `in_files_list` is exactly that union.
//!   5. COUNT = Σ included — `export_count` equals the number of rows for which `included` holds.

use serde::{Deserialize, Serialize};

use super::match_quality::MatchQuality;

/// One row's inclusion inputs (mirrors the Swift `ExportIncludePolicy.included` parameters).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncludeInput {
    /// The row's match quality (`CoupleRow.verdict`); only `Confident` is includable.
    pub verdict: MatchQuality,
    /// The user's checkbox, or `null`/absent when untouched (⇒ the default rule applies).
    #[serde(default)]
    pub explicit: Option<bool>,
    /// The couple has been exported already (`exportedAt != nil`, D8).
    pub exported: bool,
}

/// One row's inclusion outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IncludeDecision {
    /// The row goes out on the next export (drives the checkbox and the Ready-section count).
    pub included: bool,
    /// The row appears in the export rail's Files list (included ∪ exported).
    pub in_files_list: bool,
}

/// The envelope request: every row the shell wants decided, in display order.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Request {
    pub rows: Vec<IncludeInput>,
}

/// The envelope response: one decision per input row (same order) plus the export count.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Response {
    pub rows: Vec<IncludeDecision>,
    /// `export_count` of the request — the N in "Export N files".
    pub count: usize,
}

/// Whether a row with this verdict may be included at all (D1: good only). The checkbox is shown
/// for these rows only; a non-good row is never included whatever the user did.
pub fn can_include(verdict: MatchQuality) -> bool {
    verdict == MatchQuality::Confident
}

/// Whether the row is included for the next export (PLAN-COMPACT-MODE §3):
/// `verdict is good ∧ (explicit ?? ¬exported)`.
pub fn included(verdict: MatchQuality, explicit: Option<bool>, exported: bool) -> bool {
    can_include(verdict) && explicit.unwrap_or(!exported)
}

/// Whether the row appears in the Files list: `included ∪ exported` (an already-exported couple
/// stays listed with its ✓ time even when it is no longer going out again).
pub fn in_files_list(included: bool, exported: bool) -> bool {
    included || exported
}

/// The N in "Export N files": how many of `rows` are `included`.
pub fn export_count(rows: &[IncludeInput]) -> usize {
    rows.iter().filter(|r| included(r.verdict, r.explicit, r.exported)).count()
}

/// Decide one row.
pub fn decide_row(row: &IncludeInput) -> IncludeDecision {
    let inc = included(row.verdict, row.explicit, row.exported);
    IncludeDecision { included: inc, in_files_list: in_files_list(inc, row.exported) }
}

/// Decide every row of a request (the `cpl_policy_export_include` body): per-row decisions in
/// input order plus the export count.
pub fn decide(req: &Request) -> Response {
    Response { rows: req.rows.iter().map(decide_row).collect(), count: export_count(&req.rows) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use MatchQuality::*;

    fn row(verdict: MatchQuality, explicit: Option<bool>, exported: bool) -> IncludeInput {
        IncludeInput { verdict, explicit, exported }
    }

    // (1) Only a good verdict is includable.
    #[test]
    fn only_good_can_include() {
        assert!(can_include(Confident));
        assert!(!can_include(Questionable));
        assert!(!can_include(None));
    }

    // (3) A fresh good couple is included by default; an exported one drops out of the default.
    #[test]
    fn default_is_good_and_not_yet_exported() {
        assert!(included(Confident, Option::None, false));
        assert!(!included(Confident, Option::None, true));
    }

    // (2) The checkbox overrides the default both ways while the row is good.
    #[test]
    fn explicit_overrides_default_while_good() {
        assert!(!included(Confident, Some(false), false), "opted out of a fresh couple");
        assert!(included(Confident, Some(true), true), "re-included an exported couple");
        assert!(included(Confident, Some(true), false));
        assert!(!included(Confident, Some(false), true));
    }

    // (1) A non-good row is never included — not even explicitly.
    #[test]
    fn non_good_is_never_included() {
        for v in [Questionable, None] {
            for explicit in [Option::None, Some(true), Some(false)] {
                for exported in [false, true] {
                    assert!(!included(v, explicit, exported), "{v:?} {explicit:?} {exported}");
                }
            }
        }
    }

    // (4) Files list = included ∪ exported.
    #[test]
    fn files_list_is_included_or_exported() {
        assert!(!in_files_list(false, false));
        assert!(in_files_list(true, false));
        assert!(in_files_list(false, true));
        assert!(in_files_list(true, true));
    }

    // (5) The count is the number of included rows.
    #[test]
    fn export_count_counts_included_rows() {
        let rows = [
            row(Confident, Option::None, false), // included (default)
            row(Confident, Option::None, true),  // exported, untouched ⇒ out
            row(Confident, Some(true), true),    // re-included
            row(Confident, Some(false), false),  // opted out
            row(Questionable, Some(true), false), // never
            row(None, Option::None, false),       // never
        ];
        assert_eq!(export_count(&rows), 2);
        assert_eq!(export_count(&[]), 0);
    }

    // The envelope body: per-row decisions in input order + the count.
    #[test]
    fn decide_returns_rows_in_order_with_count() {
        let req = Request {
            rows: vec![
                row(Confident, Option::None, false),
                row(Confident, Option::None, true),
                row(Questionable, Option::None, true),
            ],
        };
        let r = decide(&req);
        assert_eq!(
            r,
            Response {
                rows: vec![
                    IncludeDecision { included: true, in_files_list: true },
                    IncludeDecision { included: false, in_files_list: true },
                    IncludeDecision { included: false, in_files_list: true },
                ],
                count: 1,
            }
        );
    }

    #[test]
    fn request_deserializes_camel_case_with_nullable_explicit() {
        let req: Request = serde_json::from_str(
            r#"{"rows":[{"verdict":"confident","explicit":null,"exported":false},
                        {"verdict":"confident","exported":true},
                        {"verdict":"none","explicit":true,"exported":false}]}"#,
        )
        .unwrap();
        assert_eq!(req.rows[0].explicit, Option::None);
        assert_eq!(req.rows[1].explicit, Option::None, "absent ⇒ untouched");
        let r = decide(&req);
        assert_eq!(r.count, 1);
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["rows"][0]["inFilesList"], serde_json::json!(true), "camelCase out: {json}");
    }

    fn any_verdict() -> impl Strategy<Value = MatchQuality> {
        prop_oneof![Just(Confident), Just(Questionable), Just(None)]
    }

    fn any_row() -> impl Strategy<Value = IncludeInput> {
        (any_verdict(), proptest::option::of(any::<bool>()), any::<bool>())
            .prop_map(|(verdict, explicit, exported)| IncludeInput { verdict, explicit, exported })
    }

    proptest! {
        #[test]
        fn contract_holds(r in any_row()) {
            let inc = included(r.verdict, r.explicit, r.exported);
            // 1. non-good is never included, whatever explicit/exported say
            if !can_include(r.verdict) { prop_assert!(!inc); }
            // 2. explicit overrides the default while good
            if can_include(r.verdict) {
                if let Some(x) = r.explicit { prop_assert_eq!(inc, x); }
                // 3. exported ∧ ¬explicit ⇒ excluded; fresh ∧ ¬explicit ⇒ included
                else { prop_assert_eq!(inc, !r.exported); }
            }
            // 4. files list = included ∪ exported
            let d = decide_row(&r);
            prop_assert_eq!(d.included, inc);
            prop_assert_eq!(d.in_files_list, inc || r.exported);
            if r.exported { prop_assert!(d.in_files_list); }
        }

        #[test]
        fn export_count_matches_included(rows in proptest::collection::vec(any_row(), 0..12)) {
            // 5. count == Σ included, and the envelope agrees row-for-row
            let expected = rows.iter().filter(|r| included(r.verdict, r.explicit, r.exported)).count();
            prop_assert_eq!(export_count(&rows), expected);
            let resp = decide(&Request { rows: rows.clone() });
            prop_assert_eq!(resp.count, expected);
            prop_assert_eq!(resp.rows.len(), rows.len());
            prop_assert_eq!(resp.rows.iter().filter(|d| d.included).count(), expected);
        }
    }
}
