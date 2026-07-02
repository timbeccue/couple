// Pool list height policy (SPEC-CHANGE 2026-07-01, revising the §V2.7 fixed
// bounded-list cap): the pool's inner scroll may GROW into rail height the
// couples list is not using, instead of always stopping at the fixed cap while
// blank space sits above it. Pure so the clamp is unit-testable; QueueRail
// feeds it live measurements.

public enum PoolRailLayout {
    /// The v2.7 fixed cap, now the FLOOR: the pool list never shrinks below the
    /// original "a few rows" height, so a long couples list sees exactly the
    /// pre-change layout.
    public static let minListHeight: Double = 168

    /// The pool list's height budget.
    /// - `available`: the rail height shared by the couples viewport and the pool
    ///   list (their measured heights summed — invariant under redistribution).
    /// - `couplesContent`: the couples list's natural (unclipped) content height.
    ///   The couples keep first claim on the rail: the pool only takes what they
    ///   leave (`spare`), so growth can never squeeze them into scrolling.
    /// - `poolContent`: the pool list's natural content height. Growth is also
    ///   bounded by it, so the box never inflates past its rows into blank space.
    public static func listHeight(available: Double,
                                  couplesContent: Double,
                                  poolContent: Double) -> Double {
        let spare = available - couplesContent
        return max(minListHeight, min(poolContent, spare))
    }
}
