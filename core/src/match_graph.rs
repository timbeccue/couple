//! Cross-device graph consistency (PLAN-SPECTRAL-FINGERPRINT-MATCHING §13).
//!
//! After per-pair [`PairEvidence`] is available (boundary 6's `Ctx::match_candidates` over the
//! spectral matrix), this module builds a directed graph of accepted/ambiguous affine mappings and
//! uses CYCLE CONSISTENCY (compose A→B→C→A, measure the closure residual) to:
//!
//! - PROMOTE a direct `Ambiguous` edge to `Match` when ≥ `graph_required_independent_paths`
//!   distinct two-edge `Match`→`Match` detours agree with the direct edge AND each other at the
//!   start/mid/end of common coverage and in drift (all six §13.3 predicates);
//! - DEMOTE a direct `Match` to `Ambiguous` when the required number of independent composed
//!   `Match` paths agree with each other, CONFLICT with the direct edge, and each path's bottleneck
//!   score is ≥ the direct edge's score.
//!
//! ## Scope (boundary 11 graph pass; boundary 13 §13.4 wired it into resolution)
//!
//! [`adjust`] is a PURE core pass producing GRAPH-ADJUSTED `PairEvidence` (a NEW matrix; the input is
//! untouched). Boundary 13 promoted this from observational to the DEFAULT resolver's input: the CLI
//! and Swift resolvers consume `Ctx::match_candidates_graph` and key on its `classification`, and the
//! legacy audio-audio triangle's raw `z` was replaced by [`triangle_closes`] (affine composition +
//! composed-map residuals) — the core-owned predicate BOTH shells call (invariant 8). Coverage
//! clustering and user override stay in the shells. The `LegacyEnvelope` developer override has no
//! affine map, so the graph pass is a no-op there and the legacy path is byte-identical.
//!
//! ## The graph model (§13.1)
//!
//! - NODE = an imported track (a unified node index over `videos` then `audios`).
//! - DIRECTED EDGE = an accepted (`Match`) OR `Ambiguous` affine mapping `t_to = scale·t_from +
//!   intercept` from one node's clock to another's (§3.1 / §10.1). The forward edge is the matrix
//!   cell's mapping; its REVERSE is the exact affine [`AffineMap::inverse`] (the matcher only scores
//!   the video→audio direction, but a cycle needs both orientations).
//! - EDGE WEIGHT = the evidence `score` (uncertainty rides alongside as `support`); a composed
//!   path's strength is its BOTTLENECK (minimum) component score (§13.1 / §13.3 demotion).
//!
//! `NoMatch` / `Unavailable` / `InsufficientOverlap` cells contribute NO edge (§13.3.1): only an
//! accepted or ambiguous mapping is a graph edge, so a deliberate rejection can never be overturned.
//!
//! ## Exact composition + inverse (§13.2) — NO scalar-lag shortcut
//!
//! For `A→B = (scale_ab, intercept_ab)` and `B→C = (scale_bc, intercept_bc)`:
//!
//! ```text
//! scale_ac     = scale_bc · scale_ab
//! intercept_ac = scale_bc · intercept_ab + intercept_bc
//! ```
//!
//! The inverse of `A→B`:
//!
//! ```text
//! scale_ba     = 1 / scale_ab
//! intercept_ba = -intercept_ab / scale_ab
//! ```
//!
//! A non-positive or non-finite scale is REJECTED BEFORE graph construction (§13.2, §20): such an
//! edge is dropped, so no NaN/Infinity can enter a composition and no division by zero can occur.
//!
//! ## Cycle residual (§13.2)
//!
//! Two maps from a common source clock are compared NOT by one scalar lag but at the START,
//! MIDPOINT, and END of the common coverage in the source clock, plus a drift (scale) residual —
//! because when drift ≠ 0 the offset disagreement grows with time and a single-point check would
//! miss it. See [`CycleResidual`] / [`AffineMap::offset_residuals_ms`].
//!
//! ## Determinism (§3.2, §20)
//!
//! The graph build and traversal are ORDER-INDEPENDENT and contain NO hash-map iteration in the
//! output path: edges live in a dense `Vec`-of-`Vec` adjacency indexed by node, intermediate nodes
//! are visited in ascending index order, and the adjusted matrix preserves the input cell order.
//! Identical inputs yield byte-identical adjusted `PairEvidence`.

use crate::matching::{MatchClassification, PairEvidence};
use crate::pairing_params::PairingParams;

/// An affine clock map `t_to = scale · t_from + intercept_s` (§10.1, the unit the consensus speaks).
/// `scale` is dimensionless (≈ 1 for low drift); `intercept_s` is seconds. This is the graph's
/// internal edge geometry — distinct from `fingerprint::AffineModel` because the graph composes and
/// inverts maps across arbitrary node pairs and must own the exact §13.2 algebra (compose/inverse)
/// with its non-finite/non-positive-scale rejection.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AffineMap {
    /// Scale (to-seconds per from-second), dimensionless. Guaranteed finite and `> 0` for any map
    /// that reaches construction (a bad scale is rejected up front, §13.2).
    pub scale: f64,
    /// Intercept, seconds: the to-clock time when the from-clock time is 0.
    pub intercept_s: f64,
}

impl AffineMap {
    /// Build a map from the (lag_ms, drift_ppm) the evidence reports, validating §13.2 finiteness +
    /// positive scale. Returns `None` (the edge is dropped) when the resulting scale is non-finite
    /// or `<= 0`, or the intercept is non-finite — so NO NaN/Infinity and NO division-by-zero can
    /// ever enter a composition (§20). `scale = 1 + drift_ppm·1e-6`, `intercept_s = lag_ms/1000`.
    pub fn from_lag_drift(lag_ms: f64, drift_ppm: f64) -> Option<Self> {
        if !lag_ms.is_finite() || !drift_ppm.is_finite() {
            return None;
        }
        let scale = 1.0 + drift_ppm * 1e-6;
        let intercept_s = lag_ms / 1000.0;
        Self::new(scale, intercept_s)
    }

    /// Build a map directly from (scale, intercept), enforcing the §13.2 invariant. `None` when
    /// `scale` is non-finite or `<= 0`, or `intercept_s` is non-finite.
    pub fn new(scale: f64, intercept_s: f64) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 || !intercept_s.is_finite() {
            return None;
        }
        Some(Self { scale, intercept_s })
    }

    /// Predicted to-clock time for a from-clock time under this map.
    fn predict(&self, t_from_s: f64) -> f64 {
        self.scale * t_from_s + self.intercept_s
    }

    /// Drift in ppm (`(scale − 1) · 1e6`).
    fn drift_ppm(&self) -> f64 {
        (self.scale - 1.0) * 1_000_000.0
    }

    /// Compose `self` (`A→B`) with `next` (`B→C`) into the exact `A→C` map (§13.2):
    /// `scale_ac = scale_bc·scale_ab`, `intercept_ac = scale_bc·intercept_ab + intercept_bc`.
    /// `None` when the product is non-finite or non-positive (defensive; both inputs are already
    /// validated, but a finite·finite can overflow to ±∞, §20).
    pub fn compose(self, next: AffineMap) -> Option<AffineMap> {
        let scale = next.scale * self.scale;
        let intercept_s = next.scale * self.intercept_s + next.intercept_s;
        AffineMap::new(scale, intercept_s)
    }

    /// The exact inverse `B→A` of `self = A→B` (§13.2): `scale_ba = 1/scale_ab`,
    /// `intercept_ba = -intercept_ab/scale_ab`. `None` when the result is non-finite (defensive;
    /// `scale > 0` is an invariant so `1/scale` is finite, §20).
    pub fn inverse(self) -> Option<AffineMap> {
        let scale = 1.0 / self.scale;
        let intercept_s = -self.intercept_s / self.scale;
        AffineMap::new(scale, intercept_s)
    }

    /// Offset residuals (ms) between `self` and `other` — two maps from the SAME source clock — at
    /// the start/midpoint/end of `[t0_s, t1_s]` (the common coverage in the source clock, §13.2).
    /// Each residual is `1000·|self.predict(t) − other.predict(t)|`. NOT a single scalar lag: with
    /// non-zero relative drift the disagreement grows across the span, so three points capture it.
    fn offset_residuals_ms(&self, other: &AffineMap, t0_s: f64, t1_s: f64) -> [f64; 3] {
        let mid = 0.5 * (t0_s + t1_s);
        [t0_s, mid, t1_s]
            .map(|t| 1000.0 * (self.predict(t) - other.predict(t)).abs())
    }
}

/// The §13.2 cycle/closure residual between two maps from a common source clock: the start/mid/end
/// offset disagreement (ms) and the drift (ppm) disagreement. A residual is "within tolerance" when
/// ALL THREE offsets are `<= graph_offset_tol_ms` AND the drift difference is `<= graph_drift_tol_ppm`
/// (§13.3.4/§13.3.5/§13.3.6). Every field is finite (the maps are validated; `offset_residuals_ms`
/// and the drift difference of two finite scales are finite).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CycleResidual {
    /// Offset residual at the start of common coverage, ms (`>= 0`).
    pub offset_start_ms: f64,
    /// Offset residual at the midpoint of common coverage, ms (`>= 0`).
    pub offset_mid_ms: f64,
    /// Offset residual at the end of common coverage, ms (`>= 0`).
    pub offset_end_ms: f64,
    /// Drift (scale) residual, ppm (`>= 0`).
    pub drift_ppm: f64,
}

impl CycleResidual {
    /// The residual of two maps from a common source clock over `[t0_s, t1_s]`. Public so a shell
    /// resolver can compute the §3.4 triangle's composed-map residual for its calibration read-out
    /// (the affine analogue of the legacy scalar lag residual).
    pub fn between(a: &AffineMap, b: &AffineMap, t0_s: f64, t1_s: f64) -> Self {
        let [s, m, e] = a.offset_residuals_ms(b, t0_s, t1_s);
        Self {
            offset_start_ms: s,
            offset_mid_ms: m,
            offset_end_ms: e,
            drift_ppm: (a.drift_ppm() - b.drift_ppm()).abs(),
        }
    }

    /// The largest of the three offset residuals (ms) — the worst-case offset disagreement.
    pub fn max_offset_ms(&self) -> f64 {
        self.offset_start_ms.max(self.offset_mid_ms).max(self.offset_end_ms)
    }

    /// Whether this residual agrees within the §13.3 graph tolerances: every offset point within
    /// `offset_tol_ms` AND the drift within `drift_tol_ppm`.
    fn agrees(&self, offset_tol_ms: f64, drift_tol_ppm: f64) -> bool {
        self.max_offset_ms() <= offset_tol_ms && self.drift_ppm <= drift_tol_ppm
    }
}

/// A directed graph edge: an accepted/ambiguous affine mapping `from → to` with the evidence quality
/// the §13.3 rules consult. `coverage_s` is the source-node coverage span used as the cycle-residual
/// time window (the duration over which `from`'s clock has content to compare).
#[derive(Debug, Clone, Copy)]
struct Edge {
    to: usize,
    map: AffineMap,
    class: MatchClassification,
    /// Unweighted landmark support (§13.3.2 / the zero-support guard); `0` for a legacy/no-support
    /// edge, which therefore can never be promoted.
    support: u32,
    /// Evidence score (the §13.1 edge weight; the §13.3 demotion bottleneck lever).
    score: f64,
}

/// A two-edge detour `direct.from → m → direct.to` whose BOTH component edges are `Match`, already
/// composed into the end-to-end map and reduced to the closure residual against the direct edge.
/// Used by both the promotion gate (the composed path must agree with the direct edge) and the
/// mutual-agreement check (independent paths must agree with each other).
#[derive(Debug, Clone, Copy)]
struct ComposedPath {
    /// The distinct intermediate node `m` (the path identity for "independent paths" — §13.3.3).
    via: usize,
    /// The composed `from → to` map.
    map: AffineMap,
    /// The composed path's BOTTLENECK score = min(score of the two component `Match` edges)
    /// (§13.3 demotion: each path's bottleneck score must be ≥ the direct edge's score).
    bottleneck_score: f64,
}

/// The graph-consistency verdict for one directed edge (the result the matrix adjustment applies).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GraphAction {
    /// Leave the edge's classification unchanged.
    None,
    /// Promote `Ambiguous → Match` (§13.3): all six predicates held.
    Promote,
    /// Demote `Match → Ambiguous` (§13.3): independent composed `Match` paths conflict and dominate.
    Demote,
}

/// The directed multigraph over `n_nodes`, with a dense adjacency list (no hash map) so traversal is
/// deterministic and order-independent (§3.2 / §20). Edges are added with their validated [`AffineMap`].
struct Graph {
    /// `adj[from]` is the list of outgoing edges from node `from`, in insertion order; the traversal
    /// sorts candidate intermediates by node index, so insertion order does not affect the output.
    adj: Vec<Vec<Edge>>,
    /// Per-node coverage span (seconds) in that node's own clock — the cycle-residual time window.
    coverage_s: Vec<f64>,
}

impl Graph {
    fn new(n_nodes: usize, coverage_s: Vec<f64>) -> Self {
        Self { adj: vec![Vec::new(); n_nodes], coverage_s }
    }

    fn add_edge(&mut self, from: usize, edge: Edge) {
        self.adj[from].push(edge);
    }

    /// The directed edge `from → to`, if one exists. There is at most one per ordered node pair in
    /// the bipartite construction (a video↔audio cell and its inverse); ascending scan is deterministic.
    fn edge(&self, from: usize, to: usize) -> Option<&Edge> {
        self.adj[from].iter().find(|e| e.to == to)
    }

    /// All distinct two-edge `Match`→`Match` detours `from → m → to`, composed end-to-end, with
    /// `m != from` and `m != to`, in ASCENDING `m` order (deterministic). Each detour requires BOTH
    /// `from→m` and `m→to` to be `Match` edges (§13.3.3). At most one path per intermediate node, so
    /// distinct `via` ⇒ distinct independent paths.
    fn match_paths(&self, from: usize, to: usize) -> Vec<ComposedPath> {
        let mut paths = Vec::new();
        // Iterate intermediates in ascending node order for determinism — NOT adjacency/insertion
        // order. `m` ranges over all nodes; a missing or non-Match leg simply yields no path.
        for m in 0..self.adj.len() {
            if m == from || m == to {
                continue;
            }
            let (Some(e1), Some(e2)) = (self.edge(from, m), self.edge(m, to)) else {
                continue;
            };
            if e1.class != MatchClassification::Match || e2.class != MatchClassification::Match {
                continue;
            }
            let Some(composed) = e1.map.compose(e2.map) else {
                // A composition that overflowed to non-finite is dropped (§20) — never a path.
                continue;
            };
            paths.push(ComposedPath {
                via: m,
                map: composed,
                bottleneck_score: e1.score.min(e2.score),
            });
        }
        paths
    }

    /// The common-coverage window `[0, t1]` for cycle residuals on edge `from → to`: the source
    /// node's coverage span (seconds), floored to a strictly positive, finite value so the three
    /// sample points are well-defined. The source clock starts at 0; the end is the node's coverage.
    fn coverage_window(&self, from: usize) -> (f64, f64) {
        let c = self.coverage_s[from];
        let t1 = if c.is_finite() && c > 0.0 { c } else { 1.0 };
        (0.0, t1)
    }
}

/// Decide the §13.3 graph action for the direct edge `from → to`. PURE over the built graph + params.
///
/// PROMOTION (`Ambiguous → Match`) requires ALL SIX (§13.3):
/// 1. the direct edge is `Ambiguous` (not `NoMatch`/`Unavailable` — those are not edges at all here,
///    so this is enforced by the edge existing AND being `Ambiguous`);
/// 2. direct support ≥ `ceil(min_support · graph_min_direct_support_ratio)`;
/// 3. ≥ `graph_required_independent_paths` DISTINCT intermediate nodes give `Match`→`Match` detours;
/// 4. EVERY composed path agrees with the direct edge at start/mid/end within `graph_offset_tol_ms`;
/// 5. composed drift agrees within `graph_drift_tol_ppm`;
/// 6. the independent composed paths agree with EACH OTHER under the same limits.
///
/// Plus the absolute guard: an edge with ZERO direct landmark support is NEVER promoted (§13.3).
///
/// DEMOTION (`Match → Ambiguous`) requires: ≥ `graph_required_independent_paths` independent composed
/// `Match` paths that (a) agree with EACH OTHER, (b) CONFLICT with the direct edge (their composed map
/// disagrees beyond a tolerance), and (c) each path's bottleneck score ≥ the direct edge's score.
fn decide_action(graph: &Graph, from: usize, to: usize, params: &PairingParams) -> GraphAction {
    let Some(direct) = graph.edge(from, to) else {
        return GraphAction::None;
    };
    let (t0, t1) = graph.coverage_window(from);
    let offset_tol = params.graph_offset_tol_ms;
    let drift_tol = params.graph_drift_tol_ppm;
    let required = params.graph_required_independent_paths;

    match direct.class {
        MatchClassification::Ambiguous => {
            // §13.3.1: the direct edge IS Ambiguous (a NoMatch/Unavailable is not an edge → handled
            // by `edge()` returning None for those cells). Now the other five predicates.

            // §13.3 absolute guard + §13.3.2: zero support is never promotable; support must clear
            // ceil(min_support · ratio).
            let min_support = params.fingerprint_match.min_support;
            let needed = required_direct_support(min_support, params.graph_min_direct_support_ratio);
            if direct.support == 0 || direct.support < needed {
                return GraphAction::None;
            }

            // §13.3.3: ≥ `required` DISTINCT intermediate nodes give Match→Match paths. Count
            // distinct `via` explicitly (the path identity) rather than relying on path-list length.
            let paths = graph.match_paths(from, to);
            if distinct_intermediates(&paths) < required {
                return GraphAction::None;
            }

            // §13.3.4/§13.3.5: EVERY composed path agrees with the DIRECT edge (offset start/mid/end
            // + drift). One disagreeing detour vetoes the promotion (a wrong-take edge fails here).
            let all_agree_direct = paths.iter().all(|p| {
                CycleResidual::between(&direct.map, &p.map, t0, t1).agrees(offset_tol, drift_tol)
            });
            if !all_agree_direct {
                return GraphAction::None;
            }

            // §13.3.6: the independent composed paths agree with EACH OTHER under the same limits.
            if !paths_mutually_agree(&paths, t0, t1, offset_tol, drift_tol, required) {
                return GraphAction::None;
            }

            GraphAction::Promote
        }
        MatchClassification::Match => {
            // DEMOTION: independent composed Match paths that agree with each other, CONFLICT with
            // the direct edge, and each dominates the direct edge's score.
            let paths = graph.match_paths(from, to);
            if paths.len() < required {
                return GraphAction::None;
            }
            // Each path's bottleneck score must be ≥ the direct edge's score (the composed evidence
            // is at least as strong as the direct claim it overturns).
            let dominating: Vec<ComposedPath> =
                paths.iter().copied().filter(|p| p.bottleneck_score >= direct.score).collect();
            if dominating.len() < required {
                return GraphAction::None;
            }
            // The dominating paths must agree with EACH OTHER (a real alternate consensus, not noise).
            if !paths_mutually_agree(&dominating, t0, t1, offset_tol, drift_tol, required) {
                return GraphAction::None;
            }
            // And they must CONFLICT with the direct edge: at least `required` of them disagree with
            // the direct map beyond the tolerance (else they corroborate it — no demotion).
            let conflicting = dominating
                .iter()
                .filter(|p| {
                    !CycleResidual::between(&direct.map, &p.map, t0, t1).agrees(offset_tol, drift_tol)
                })
                .count();
            if conflicting >= required {
                GraphAction::Demote
            } else {
                GraphAction::None
            }
        }
        // Only Match/Ambiguous direct edges are actionable; the other classes are not edges here.
        _ => GraphAction::None,
    }
}

/// The number of DISTINCT intermediate nodes (`via`) among `paths` (§13.3.3). `match_paths` already
/// emits at most one path per intermediate in ascending order, so this equals `paths.len()`; counting
/// `via` explicitly makes the "distinct intermediate nodes" requirement self-evident and robust to any
/// future multi-path-per-node extension.
fn distinct_intermediates(paths: &[ComposedPath]) -> usize {
    let mut seen: Vec<usize> = paths.iter().map(|p| p.via).collect();
    seen.sort_unstable();
    seen.dedup();
    seen.len()
}

/// `ceil(min_support · ratio)` (§13.3.2) — the minimum direct support a promotion requires. `ratio`
/// is validated to `[0, 1]`; the result is at most `min_support`. Uses an exact integer ceil.
fn required_direct_support(min_support: u32, ratio: f64) -> u32 {
    let r = ratio.clamp(0.0, 1.0);
    let exact = (min_support as f64) * r;
    // ceil, guarded finite; exact is finite because both inputs are.
    exact.ceil() as u32
}

/// Whether ≥ `required` of `paths` mutually agree: there exists a subset of size ≥ `required` whose
/// every pair agrees (offset start/mid/end + drift) under the limits (§13.3.6). Implemented as the
/// largest mutually-agreeing clique by a deterministic greedy over ascending `via` order; for the
/// small path counts here (intermediates ≤ node count) this is exact enough and order-independent.
///
/// We require a CLIQUE (all-pairs agreement), not just pairwise-with-an-anchor, so a promotion needs
/// the independent paths to genuinely converge, not merely each match one reference.
fn paths_mutually_agree(
    paths: &[ComposedPath],
    t0: f64,
    t1: f64,
    offset_tol: f64,
    drift_tol: f64,
    required: usize,
) -> bool {
    if paths.len() < required {
        return false;
    }
    // Pairwise agreement matrix (dense, deterministic). paths are already in ascending `via` order.
    let n = paths.len();
    let mut agree = vec![vec![false; n]; n];
    for i in 0..n {
        agree[i][i] = true;
        for j in (i + 1)..n {
            let ok = CycleResidual::between(&paths[i].map, &paths[j].map, t0, t1)
                .agrees(offset_tol, drift_tol);
            agree[i][j] = ok;
            agree[j][i] = ok;
        }
    }
    // Greedy clique seeded from each start index in ascending order; take the first that reaches
    // `required`. Deterministic and independent of input permutation because `via` order is fixed.
    for (seed, seed_row) in agree.iter().enumerate() {
        let mut clique = vec![seed];
        for (cand, &seed_agrees) in seed_row.iter().enumerate() {
            // Skip the seed itself and any candidate the seed already disagrees with.
            if cand == seed || !seed_agrees {
                continue;
            }
            // The candidate joins only if it agrees with EVERY current clique member (all-pairs).
            if clique.iter().all(|&c| agree[c][cand]) {
                clique.push(cand);
                if clique.len() >= required {
                    return true;
                }
            }
        }
    }
    false
}

/// Build the directed [`Graph`] from a SQUARE track×track directed evidence matrix: `directed[i][j]`
/// is the edge from node `i` to node `j` (or `None` when no candidate). `coverage_s[i]` is node `i`'s
/// coverage span (seconds). Each `Match`/`Ambiguous` cell with a finite, positive-scale map becomes a
/// directed edge `i → j`; `NoMatch`/`Unavailable`/`InsufficientOverlap` and non-finite/non-positive
/// scales contribute NO edge (§13.1/§13.2/§13.3.1/§20). Both orientations are taken from the matrix as
/// authored — a caller wanting a symmetric graph populates `[i][j]` and `[j][i]` (or uses
/// [`adjust_bipartite`], which fills `[j][i]` with the exact inverse).
fn build_graph(directed: &[Vec<Option<PairEvidence>>], coverage_s: Vec<f64>) -> Graph {
    let n_nodes = coverage_s.len();
    let mut graph = Graph::new(n_nodes, coverage_s);
    for (i, row) in directed.iter().enumerate() {
        for (j, slot) in row.iter().enumerate() {
            let Some(cell) = slot else { continue };
            if !is_edge_class(cell.classification) {
                continue;
            }
            // Use the REFINED mapping when present, else the consensus mapping (§13.1).
            let (lag_ms, drift_ppm) = effective_map(cell);
            let Some(map) = AffineMap::from_lag_drift(lag_ms, drift_ppm) else {
                // A non-finite/non-positive-scale edge is dropped before construction (§13.2/§20).
                continue;
            };
            graph.add_edge(
                i,
                Edge { to: j, map, class: cell.classification, support: cell.support, score: cell.score },
            );
        }
    }
    graph
}

/// Graph-adjusted copy of a SQUARE track×track directed evidence matrix (§13). This is the GENERAL
/// §13.4 pass: node `i` is an imported track, `directed[i][j]` is the directed affine edge `i → j`
/// (`None` = no candidate). OBSERVATIONAL (§13.4): a NEW matrix is returned; the input is untouched
/// and NO resolver decision is changed by this call.
///
/// `coverage_s[i]` is node `i`'s coverage span (seconds) — the §13.2 cycle-residual time window in
/// node `i`'s own clock. Each present `Match`/`Ambiguous` cell is run through the §13.3
/// promotion/demotion rules over the graph of ALL edges; the returned matrix differs from the input
/// ONLY in the `classification` of cells the rules adjust (every other field — lag, support,
/// refined_*, legacy, the §11.2 refine reason — is carried through verbatim). The graph reflects the
/// ORIGINAL classifications throughout (one observational pass, not iterate-to-fixpoint).
pub fn adjust(
    directed: &[Vec<Option<PairEvidence>>],
    coverage_s: &[f64],
    params: &PairingParams,
) -> Vec<Vec<Option<PairEvidence>>> {
    let n_nodes = coverage_s.len();
    let graph = build_graph(directed, coverage_s.to_vec());

    let mut out: Vec<Vec<Option<PairEvidence>>> = Vec::with_capacity(directed.len());
    for (i, row) in directed.iter().enumerate() {
        let mut out_row: Vec<Option<PairEvidence>> = Vec::with_capacity(row.len());
        for (j, slot) in row.iter().enumerate() {
            let adjusted = slot.as_ref().map(|cell| {
                let mut a = cell.clone();
                // Only act on a real edge with both endpoints inside the node space.
                if is_edge_class(cell.classification) && i < n_nodes && j < n_nodes {
                    apply_action(&mut a, decide_action(&graph, i, j, params));
                }
                a
            });
            out_row.push(adjusted);
        }
        out.push(out_row);
    }
    out
}

/// Graph-adjusted copy of a BIPARTITE video×audio evidence matrix (the resolver shape). Folds the
/// matrix into the unified node space (videos `0..V`, then audios `V..V+A`): cell `(vi, ai)` becomes a
/// forward edge `vi → audio_node` AND its EXACT inverse `audio_node → vi` (§13.2), so a triangle that
/// traverses an audio→video leg has both orientations. Runs the general [`adjust`] over the square
/// node space and projects the video→audio cells back to the input bipartite shape.
///
/// OBSERVATIONAL (§13.4): a NEW matrix of the SAME bipartite shape is returned; only promoted/demoted
/// cells' `classification` differ. `video_coverage_s`/`audio_coverage_s` are the per-track coverage
/// spans (seconds). Note: a PURELY bipartite matrix has no two-edge video→audio detours (an
/// intermediate cannot be adjacent to both a video and an audio), so on a lone resolver matrix this is
/// effectively a pass-through; the richer track-vs-track graph (`Ctx::match_candidates_graph`, which
/// also cross-matches audio↔audio / video↔video) is where corroborating triangles actually close.
pub fn adjust_bipartite(
    evidence: &[Vec<PairEvidence>],
    video_coverage_s: &[f64],
    audio_coverage_s: &[f64],
    params: &PairingParams,
) -> Vec<Vec<PairEvidence>> {
    let n_videos = evidence.len();
    let n_audios = audio_coverage_s.len();
    let n_nodes = n_videos + n_audios;

    // Per-node coverage: videos first, then audios.
    let mut coverage = Vec::with_capacity(n_nodes);
    coverage.extend_from_slice(slice_or_zero(video_coverage_s, n_videos));
    coverage.extend_from_slice(slice_or_zero(audio_coverage_s, n_audios));

    // Square directed matrix over the node space, with forward + inverse bipartite edges.
    let mut directed: Vec<Vec<Option<PairEvidence>>> = vec![vec![None; n_nodes]; n_nodes];
    for (vi, row) in evidence.iter().enumerate() {
        for (ai, cell) in row.iter().enumerate() {
            let a_node = n_videos + ai;
            // Forward video→audio cell verbatim.
            directed[vi][a_node] = Some(cell.clone());
            // Exact inverse audio→video edge, synthesized so compositions can traverse audio→video.
            // Only an edge-class cell with a valid (positive-scale) map yields an inverse; otherwise
            // leave None (a non-edge is not an edge in either direction).
            if is_edge_class(cell.classification) {
                let (lag_ms, drift_ppm) = effective_map(cell);
                if let Some(inv) =
                    AffineMap::from_lag_drift(lag_ms, drift_ppm).and_then(AffineMap::inverse)
                {
                    directed[a_node][vi] = Some(inverse_evidence(cell, inv));
                }
            }
        }
    }

    let adjusted = adjust(&directed, &coverage, params);

    // Project the video→audio cells back to the bipartite shape (the inverse edges were scaffolding).
    let mut out: Vec<Vec<PairEvidence>> = Vec::with_capacity(n_videos);
    for (vi, row) in evidence.iter().enumerate() {
        let mut out_row: Vec<PairEvidence> = Vec::with_capacity(row.len());
        for (ai, input_cell) in row.iter().enumerate() {
            let a_node = n_videos + ai;
            // The adjusted forward cell is always Some (we set it above); fall back to the input.
            let cell = adjusted[vi][a_node].clone().unwrap_or_else(|| input_cell.clone());
            out_row.push(cell);
        }
        out.push(out_row);
    }
    out
}

/// The §13.4 audio-audio SIMULTANEITY check over affine evidence + composed-map residuals — the
/// core-owned replacement for the legacy resolver's raw-`z` triangle (`ab.z >= z_min && |lag_vb −
/// lag_va − lag_ab| <= triangle_tol`). Two audio recorders A and B of one video V are SIMULTANEOUS
/// when:
///
/// 1. the A↔B edge is a same-recording `Match` (the core identity authority — replaces `ab.z >=
///    z_min`; a resolver MUST NOT re-derive an acceptance threshold, invariant 8); AND
/// 2. composing the two legs `V→A` then `A→B` reproduces the direct `V→B` map within the graph
///    tolerances at the START, MIDPOINT, and END of the common coverage (replaces the single
///    scalar-lag residual — with non-zero drift the disagreement grows across the span, §13.2).
///
/// `va`, `vb`, `ab` are the affine maps the three `PairEvidence` cells report (built by the caller
/// from each cell's effective lag/drift via [`evidence_map`]); `ab_class` is the A↔B cell's core
/// `classification`. `coverage_s` is the video's coverage span (the residual time window in V's
/// clock). `offset_tol_ms` / `drift_tol_ppm` are the §13.3 graph tolerances (from `PairingParams`).
///
/// Returns `false` (NOT simultaneous) whenever the A↔B edge is not a `Match`, any map is missing
/// (a non-finite/non-positive scale was rejected upstream), or the composed residual exceeds a
/// tolerance. No NaN/Infinity can arise (all maps are validated; the residual of finite maps is
/// finite, §20).
pub fn triangle_closes(
    va: Option<AffineMap>,
    ab: Option<AffineMap>,
    vb: Option<AffineMap>,
    ab_class: MatchClassification,
    coverage_s: f64,
    offset_tol_ms: f64,
    drift_tol_ppm: f64,
) -> bool {
    // (1) The A↔B edge must be a same-recording Match — the identity authority (replaces z >= z_min).
    if ab_class != MatchClassification::Match {
        return false;
    }
    // (2) All three legs must have a valid affine map; compose V→A then A→B and compare to V→B.
    let (Some(va), Some(ab), Some(vb)) = (va, ab, vb) else {
        return false;
    };
    let Some(composed) = va.compose(ab) else {
        return false;
    };
    let t1 = if coverage_s.is_finite() && coverage_s > 0.0 { coverage_s } else { 1.0 };
    CycleResidual::between(&vb, &composed, 0.0, t1).agrees(offset_tol_ms, drift_tol_ppm)
}

/// The affine [`AffineMap`] a `PairEvidence` cell describes, using its REFINED mapping when present
/// else the consensus mapping (§13.1 — the same selection [`effective_map`] applies inside the
/// graph). `None` when the resulting scale is non-finite/non-positive (§13.2/§20), so a caller can
/// never feed a degenerate map into a composition. This is the shell-facing accessor the resolver
/// uses to turn the three triangle cells into maps for [`triangle_closes`].
pub fn evidence_map(cell: &PairEvidence) -> Option<AffineMap> {
    let (lag_ms, drift_ppm) = effective_map(cell);
    AffineMap::from_lag_drift(lag_ms, drift_ppm)
}

/// A synthetic `PairEvidence` for the INVERSE bipartite edge `audio → video`: same class / support /
/// score / quality as the forward cell, but its `lag_ms`/`drift_ppm` are the inverse map so the graph
/// composes the reverse leg correctly (§13.2). Its `refined_*` are cleared (the refined map was the
/// forward orientation; the edge map is taken from `lag_ms`/`drift_ppm` via `effective_map`, which now
/// equals the inverse). This value never leaves the graph — it is scaffolding for traversal.
fn inverse_evidence(forward: &PairEvidence, inv: AffineMap) -> PairEvidence {
    let mut e = forward.clone();
    e.lag_ms = inv.intercept_s * 1000.0;
    e.drift_ppm = inv.drift_ppm();
    e.refined_lag_ms = None;
    e.refined_drift_ppm = None;
    e
}

/// Apply a decided [`GraphAction`] to a cell's `classification` (§13.3). PROMOTE ⇒ `Match`, DEMOTE ⇒
/// `Ambiguous`, NONE ⇒ unchanged. Only the classification moves; all other evidence is preserved.
fn apply_action(cell: &mut PairEvidence, action: GraphAction) {
    match action {
        GraphAction::Promote => cell.classification = MatchClassification::Match,
        GraphAction::Demote => cell.classification = MatchClassification::Ambiguous,
        GraphAction::None => {}
    }
}

/// The (lag_ms, drift_ppm) to use for a cell's edge: the REFINED mapping when present, else the
/// consensus mapping (§13.1). A refined lag without a refined drift uses the consensus drift (and
/// vice-versa) so the map is always complete.
fn effective_map(cell: &PairEvidence) -> (f64, f64) {
    let lag = cell.refined_lag_ms.unwrap_or(cell.lag_ms);
    let drift = cell.refined_drift_ppm.unwrap_or(cell.drift_ppm);
    (lag, drift)
}

/// Whether a classification contributes a graph edge (§13.1 / §13.3.1): only an accepted (`Match`) or
/// `Ambiguous` mapping. `NoMatch`/`Unavailable`/`InsufficientOverlap` are NOT edges, so a deliberate
/// rejection can never be a path leg or be overturned.
fn is_edge_class(c: MatchClassification) -> bool {
    matches!(c, MatchClassification::Match | MatchClassification::Ambiguous)
}

/// `&slice[..n]` padded to length `n` with `0.0` if shorter (defensive; the caller passes matching
/// lengths, but a mismatch must not panic or read out of bounds).
fn slice_or_zero(s: &[f64], n: usize) -> &[f64] {
    if s.len() >= n {
        &s[..n]
    } else {
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::{MatchMethod, PairEvidence};

    // ---- evidence constructors (synthetic PairEvidence; no media needed, §13.4) ----

    /// A spectral cell with a given class, affine (lag_ms, drift_ppm), support, and score. The
    /// remaining quality fields are plausible finite values; refined_* are None (so `effective_map`
    /// uses the consensus lag/drift unless a test sets them).
    fn cell(
        class: MatchClassification,
        lag_ms: f64,
        drift_ppm: f64,
        support: u32,
        score: f64,
    ) -> PairEvidence {
        PairEvidence {
            method: MatchMethod::SpectralLandmarkV1,
            classification: class,
            lag_ms,
            drift_ppm,
            score,
            runner_up_score: 0.0,
            score_margin: 1.0,
            support,
            candidate_correspondences: support,
            inlier_fraction: 0.9,
            temporal_coverage: 0.8,
            residual_rms_ms: Some(0.5),
            residual_p95_ms: Some(1.0),
            overlap_s: 100.0,
            refined_lag_ms: None,
            refined_drift_ppm: None,
            refined_rms_ms: None,
            legacy: None,
            refine_reason: crate::match_refine::RefineReason::NotAttempted,
        }
    }

    fn no_match() -> PairEvidence {
        let mut c = cell(MatchClassification::NoMatch, 0.0, 0.0, 0, 0.0);
        c.residual_rms_ms = None;
        c.residual_p95_ms = None;
        c
    }

    /// Default params with a low min_support so synthetic supports clear the §13.3.2 floor, and the
    /// standard graph tolerances. `min_support = 12` ⇒ ceil(12·0.75) = 9 required direct support.
    fn params() -> PairingParams {
        let mut p = PairingParams::default();
        p.fingerprint_match.min_support = 12;
        p
    }

    // ============================ pure affine algebra (§13.2) ============================

    #[test]
    fn compose_matches_analytic() {
        // A→B: scale 1.0001 (100 ppm), +0.5 s; B→C: scale 1.0002 (200 ppm), -0.3 s.
        let ab = AffineMap::new(1.0001, 0.5).unwrap();
        let bc = AffineMap::new(1.0002, -0.3).unwrap();
        let ac = ab.compose(bc).unwrap();
        // scale_ac = scale_bc·scale_ab; intercept_ac = scale_bc·intercept_ab + intercept_bc.
        assert!((ac.scale - 1.0002 * 1.0001).abs() < 1e-15);
        assert!((ac.intercept_s - (1.0002 * 0.5 + -0.3)).abs() < 1e-15);
        // Composition equals applying B→C after A→B at any t.
        for &t in &[0.0, 12.34, 600.0] {
            let stepwise = bc.predict(ab.predict(t));
            assert!((ac.predict(t) - stepwise).abs() < 1e-9, "compose must equal sequential apply");
        }
    }

    #[test]
    fn inverse_is_exact_round_trip() {
        let ab = AffineMap::new(1.0003, 1.25).unwrap();
        let ba = ab.inverse().unwrap();
        // scale_ba = 1/scale_ab; intercept_ba = -intercept_ab/scale_ab.
        assert!((ba.scale - 1.0 / 1.0003).abs() < 1e-15);
        assert!((ba.intercept_s - (-1.25 / 1.0003)).abs() < 1e-15);
        // A→B then B→A is the identity (within fp epsilon).
        let id = ab.compose(ba).unwrap();
        assert!((id.scale - 1.0).abs() < 1e-12);
        assert!(id.intercept_s.abs() < 1e-12);
    }

    // §13.2 / §20: a non-positive or non-finite scale is rejected BEFORE construction.
    #[test]
    fn non_positive_or_non_finite_scale_rejected() {
        assert!(AffineMap::new(0.0, 1.0).is_none(), "zero scale rejected");
        assert!(AffineMap::new(-1.0, 1.0).is_none(), "negative scale rejected");
        assert!(AffineMap::new(f64::NAN, 1.0).is_none(), "NaN scale rejected");
        assert!(AffineMap::new(f64::INFINITY, 1.0).is_none(), "inf scale rejected");
        assert!(AffineMap::new(1.0, f64::NAN).is_none(), "NaN intercept rejected");
        // from_lag_drift: a drift that drives scale to 0 or below, or a non-finite lag/drift.
        assert!(AffineMap::from_lag_drift(f64::NAN, 0.0).is_none());
        assert!(AffineMap::from_lag_drift(0.0, f64::INFINITY).is_none());
        // -1_000_000 ppm ⇒ scale 1 + (-1.0) = 0.0 ⇒ rejected.
        assert!(AffineMap::from_lag_drift(0.0, -1_000_000.0).is_none(), "scale→0 rejected");
        // A drift below -1e6 ppm ⇒ negative scale ⇒ rejected.
        assert!(AffineMap::from_lag_drift(0.0, -2_000_000.0).is_none(), "scale<0 rejected");
    }

    // ============================ §13.4 test 5: drift composition over long durations ============================

    // Compose two NON-ZERO drifts and verify the start/mid/end residual and composed drift match the
    // ANALYTIC value over a long (600 s) coverage. The composed map A→C is compared against an
    // independently-constructed analytic A→C; the closure residual must be ~0 (they are the same map).
    #[test]
    fn drift_composition_correct_over_long_duration() {
        // A→B: +120 ppm, +0.2 s. B→C: -80 ppm, +0.4 s. (Non-zero, opposite-sign drifts.)
        let ab = AffineMap::from_lag_drift(200.0, 120.0).unwrap();
        let bc = AffineMap::from_lag_drift(400.0, -80.0).unwrap();
        let ac = ab.compose(bc).unwrap();

        // Analytic composed scale/intercept.
        let analytic_scale = (1.0 - 80e-6) * (1.0 + 120e-6);
        let analytic_intercept = (1.0 - 80e-6) * 0.2 + 0.4;
        assert!((ac.scale - analytic_scale).abs() < 1e-15);
        assert!((ac.intercept_s - analytic_intercept).abs() < 1e-12);

        // Composed drift ≈ +40 ppm to first order (120 - 80), exactly (analytic_scale-1)*1e6.
        let analytic_drift = (analytic_scale - 1.0) * 1e6;
        assert!((ac.drift_ppm() - analytic_drift).abs() < 1e-6);
        assert!((ac.drift_ppm() - 40.0).abs() < 0.02, "first-order drift ≈ 40 ppm");

        // Over a 600 s window the closure residual of the composed map vs the analytic map is ~0 at
        // start/mid/end and in drift (they are the SAME map) — proving drift was composed, not a
        // single-point scalar lag.
        let analytic = AffineMap::new(analytic_scale, analytic_intercept).unwrap();
        let res = CycleResidual::between(&ac, &analytic, 0.0, 600.0);
        assert!(res.max_offset_ms() < 1e-6, "start/mid/end residual ~0: {res:?}");
        assert!(res.drift_ppm < 1e-9);

        // And a single-scalar-lag check WOULD miss a drift-only disagreement: build a map with the
        // same intercept but a different drift; its start residual is ~0 yet mid/end blow up.
        let drift_only_wrong = AffineMap::new(analytic_scale + 100e-6, analytic_intercept).unwrap();
        let r2 = CycleResidual::between(&ac, &drift_only_wrong, 0.0, 600.0);
        assert!(r2.offset_start_ms < 1e-6, "start agrees (same intercept)");
        assert!(r2.offset_end_ms > 50.0, "but the END reveals the drift disagreement: {r2:?}");
        assert!(r2.drift_ppm > 99.0, "drift residual catches it: {r2:?}");
    }

    // ============================ directed-graph scene builders (general §13 topology) ============================
    //
    // Nodes are imported tracks. We place each node's clock at a global offset `off[i]` (seconds): a
    // directed edge i→j is the map `t_j = t_i + (off[i] - off[j])` (lag = (off[i]-off[j])·1000, scale
    // 1 unless a drift is given). With offset-based maps every triangle composes EXACTLY (A→B→C ==
    // A→C), so a "consistent" scene closes to zero residual by construction; a wrong edge is injected
    // by overriding one cell's lag/drift. This is the §13.4 "construct PairEvidence directly" path —
    // no media needed for the graph logic.

    /// An N×N empty directed matrix (all `None`).
    fn empty(n: usize) -> Vec<Vec<Option<PairEvidence>>> {
        vec![vec![None; n]; n]
    }

    /// Set the directed edge `from → to` to a `Match` derived from the node offsets (`off`), with the
    /// given support/score and zero drift.
    fn set_edge(
        m: &mut [Vec<Option<PairEvidence>>],
        off: &[f64],
        from: usize,
        to: usize,
        class: MatchClassification,
        support: u32,
        score: f64,
    ) {
        let lag = (off[from] - off[to]) * 1000.0;
        m[from][to] = Some(cell(class, lag, 0.0, support, score));
    }

    /// A SYMMETRIC consistent `Match` edge between `a` and `b` (both directions), offsets-derived.
    fn set_pair(m: &mut [Vec<Option<PairEvidence>>], off: &[f64], a: usize, b: usize, support: u32, score: f64) {
        set_edge(m, off, a, b, MatchClassification::Match, support, score);
        set_edge(m, off, b, a, MatchClassification::Match, support, score);
    }

    /// Uniform coverage (seconds) for `n` nodes.
    fn cov(n: usize, c: f64) -> Vec<f64> {
        vec![c; n]
    }

    // ============================ §13.4 test 1: three simultaneous recorders close ============================

    // Three simultaneous recorders of one event (nodes 0,1,2) are pairwise Matches with mutually
    // consistent maps. Every composed cycle closes (residual ~0), so the consistent cluster is NOT
    // demoted — each composed path corroborates (never conflicts with) the direct edge. This is the
    // "cluster identified / consistent" outcome: the mutually-composed maps close.
    #[test]
    fn three_simultaneous_recorders_consistent_triangle_closes() {
        // Three recorders of one event: nodes 0,1,2 at offsets 0.0/0.3/1.0 s, all pairwise symmetric
        // Matches with mutually-consistent offset-derived maps and zero drift.
        let off = [0.0, 0.3, 1.0];
        let mut m = empty(3);
        set_pair(&mut m, &off, 0, 1, 40, 5.0);
        set_pair(&mut m, &off, 0, 2, 40, 5.0);
        set_pair(&mut m, &off, 1, 2, 40, 5.0);
        let out = adjust(&m, &cov(3, 600.0), &params());
        // The consistent cluster stays all-Match: every composed path corroborates (never conflicts
        // with) the direct edge, so the demotion conflict predicate is never met (cluster identified).
        for (i, row) in out.iter().enumerate() {
            for (j, slot) in row.iter().enumerate() {
                if let Some(c) = slot {
                    assert_eq!(
                        c.classification,
                        MatchClassification::Match,
                        "consistent cluster edge {i}->{j} stays Match"
                    );
                }
            }
        }
        // And the composed cycle truly closes: direct 0→2 vs path 0→1→2 agree to ~0 residual.
        let m01 = AffineMap::from_lag_drift((off[0] - off[1]) * 1000.0, 0.0).unwrap();
        let m12 = AffineMap::from_lag_drift((off[1] - off[2]) * 1000.0, 0.0).unwrap();
        let m02 = AffineMap::from_lag_drift((off[0] - off[2]) * 1000.0, 0.0).unwrap();
        let composed = m01.compose(m12).unwrap();
        let res = CycleResidual::between(&m02, &composed, 0.0, 600.0);
        assert!(res.max_offset_ms() < 1e-6, "simultaneous-recorder cycle closes: {res:?}");
    }

    // ============================ §13.4 test 3: continuous recorder hub stays valid ============================

    // One continuous recorder (hub node 0) is used by several other recordings (nodes 1,2,3). Every
    // edge to the hub is a consistent Match. The hub's edges stay valid — a continuous recorder's
    // consistent relationships are NOT demoted, even though many cycles pass through it.
    #[test]
    fn continuous_recorder_hub_edges_stay_valid() {
        // Hub at 0.0 s; three users at 0.5/1.1/2.0 s, all pairwise consistent (so the hub and the
        // users form a fully consistent clique — every cycle through the hub closes).
        let off = [0.0, 0.5, 1.1, 2.0];
        let mut m = empty(4);
        for a in 0..4 {
            for b in (a + 1)..4 {
                set_pair(&mut m, &off, a, b, 40, 5.0);
            }
        }
        let out = adjust(&m, &cov(4, 600.0), &params());
        for (i, row) in out.iter().enumerate() {
            for (j, slot) in row.iter().enumerate() {
                if let Some(c) = slot {
                    assert_eq!(
                        c.classification,
                        MatchClassification::Match,
                        "hub edge {i}->{j} stays valid (no spurious demotion)"
                    );
                }
            }
        }
    }

    // ============================ §13.4 test 4: disjoint recorder restart is not a conflict ============================

    // A recorder restart splits coverage so a second take (node 3) does NOT overlap the others — every
    // attempted edge to/from it is NoMatch (not an edge). A genuine Match among the overlapping nodes
    // must survive: a non-overlapping (no-edge) relationship is NOT a cycle conflict ⇒ no false demotion.
    #[test]
    fn disjoint_recorder_restart_no_false_demotion() {
        // Nodes 0,1,2 are a consistent overlapping cluster (real Matches). Node 3 is the restart take:
        // NO edges connect it (disjoint coverage). The cluster's Matches survive; node 3 stays NoMatch.
        let off = [0.0, 0.5, 1.0, 9.9];
        let mut m = empty(4);
        set_pair(&mut m, &off, 0, 1, 40, 5.0);
        set_pair(&mut m, &off, 0, 2, 40, 5.0);
        set_pair(&mut m, &off, 1, 2, 40, 5.0);
        // Node 3 has only NoMatch relationships (disjoint) — explicitly NoMatch, not an edge.
        for k in [0usize, 1, 2] {
            m[k][3] = Some(no_match());
            m[3][k] = Some(no_match());
        }
        let out = adjust(&m, &cov(4, 600.0), &params());
        // The overlapping cluster's Matches are intact.
        assert_eq!(out[0][1].as_ref().unwrap().classification, MatchClassification::Match);
        assert_eq!(out[0][2].as_ref().unwrap().classification, MatchClassification::Match);
        assert_eq!(out[1][2].as_ref().unwrap().classification, MatchClassification::Match);
        // The disjoint restart's relationships stay NoMatch (no false promotion or demotion).
        for k in [0usize, 1, 2] {
            assert_eq!(out[k][3].as_ref().unwrap().classification, MatchClassification::NoMatch);
            assert_eq!(out[3][k].as_ref().unwrap().classification, MatchClassification::NoMatch);
        }
    }

    // ============================ promotable / demotable scene builders ============================
    //
    // A direct edge `from→to` (0→3) plus TWO distinct intermediates (1, 2), each giving a two-edge
    // Match→Match detour 0→m→3. Offsets-derived ⇒ every detour composes EXACTLY to the direct map, so
    // a wrong edge must be injected to break a cycle. node offsets: 0=0.0, 1=0.4, 2=0.9, 3=1.0 s.
    const SCENE_OFF: [f64; 4] = [0.0, 0.4, 0.9, 1.0];

    /// The promotable baseline: direct edge 0→3 is `Ambiguous` (support 40), and intermediates 1 and 2
    /// each give a consistent `Match`→`Match` detour 0→m→3. Returns (directed matrix, coverage).
    fn promotable_scene() -> (Vec<Vec<Option<PairEvidence>>>, Vec<f64>) {
        let mut m = empty(4);
        // Direct edge 0→3 Ambiguous (and its inverse 3→0 also Ambiguous, symmetric).
        set_edge(&mut m, &SCENE_OFF, 0, 3, MatchClassification::Ambiguous, 40, 5.0);
        set_edge(&mut m, &SCENE_OFF, 3, 0, MatchClassification::Ambiguous, 40, 5.0);
        // Two corroborating detours via intermediates 1 and 2 (consistent Match legs, both directions).
        set_pair(&mut m, &SCENE_OFF, 0, 1, 40, 5.0);
        set_pair(&mut m, &SCENE_OFF, 1, 3, 40, 5.0);
        set_pair(&mut m, &SCENE_OFF, 0, 2, 40, 5.0);
        set_pair(&mut m, &SCENE_OFF, 2, 3, 40, 5.0);
        (m, cov(4, 600.0))
    }

    /// The direct edge cell `0→3` of an adjusted scene matrix.
    fn direct(out: &[Vec<Option<PairEvidence>>]) -> MatchClassification {
        out[0][3].as_ref().unwrap().classification
    }

    // ============================ §13.4 test 2: one wrong-take edge fails the cycle ============================

    // Two consistent independent detours promote an Ambiguous direct edge; injecting ONE wrong-take
    // edge (a leg off by 300 ms) makes that detour's composed map disagree with the direct edge ⇒ the
    // §13.3.4 "every path agrees with the direct edge" predicate fails ⇒ NO promotion.
    #[test]
    fn one_wrong_take_edge_fails_cycle_no_promotion() {
        // Consistent ⇒ promote.
        let (m, c) = promotable_scene();
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Match,
            "two consistent independent detours promote the Ambiguous direct edge"
        );
        // Wrong-take: corrupt the 2→3 leg by +300 ms (still a Match edge, both directions). The
        // detour 0→2→3 now composes 300 ms off the direct edge ⇒ NOT every path agrees ⇒ no promotion.
        let (mut w, cw) = promotable_scene();
        let bad = (SCENE_OFF[2] - SCENE_OFF[3]) * 1000.0 + 300.0;
        w[2][3] = Some(cell(MatchClassification::Match, bad, 0.0, 40, 5.0));
        w[3][2] = Some(cell(MatchClassification::Match, -bad, 0.0, 40, 5.0));
        assert_eq!(
            direct(&adjust(&w, &cw, &params())),
            MatchClassification::Ambiguous,
            "a wrong-take detour fails the cycle residual ⇒ the Ambiguous edge is NOT promoted (§13.3.4)"
        );
    }

    // ============================ promotion requires ALL SIX §13.3 conditions ============================

    #[test]
    fn promotion_baseline_promotes() {
        let (m, c) = promotable_scene();
        assert_eq!(direct(&adjust(&m, &c, &params())), MatchClassification::Match, "baseline must promote");
    }

    #[test]
    fn promotion_needs_condition_1_direct_ambiguous() {
        // Condition 1: the direct edge must be Ambiguous. NoMatch is not even an edge ⇒ no promotion
        // (a deliberate rejection can never be resurrected by paths).
        let (mut m, c) = promotable_scene();
        m[0][3] = Some(no_match());
        m[3][0] = Some(no_match());
        assert_eq!(direct(&adjust(&m, &c, &params())), MatchClassification::NoMatch, "NoMatch never promoted");
    }

    #[test]
    fn promotion_needs_condition_2_direct_support() {
        // Condition 2: direct support ≥ ceil(min_support·ratio) = ceil(12·0.75) = 9. At 8 ⇒ blocked.
        let (mut m, c) = promotable_scene();
        m[0][3].as_mut().unwrap().support = 8;
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "support below ceil(min_support·ratio) blocks promotion"
        );
        // At exactly 9 it promotes (boundary).
        let (mut m2, c2) = promotable_scene();
        m2[0][3].as_mut().unwrap().support = 9;
        assert_eq!(direct(&adjust(&m2, &c2, &params())), MatchClassification::Match, "support == floor promotes");
    }

    #[test]
    fn promotion_needs_condition_3_enough_independent_paths() {
        // Condition 3: ≥ graph_required_independent_paths (2) distinct intermediates. Remove
        // intermediate 2's edges so only ONE detour (via 1) exists ⇒ no promotion.
        let (mut m, c) = promotable_scene();
        m[0][2] = None;
        m[2][0] = None;
        m[2][3] = None;
        m[3][2] = None;
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "only one independent path ⇒ no promotion (need 2)"
        );
    }

    #[test]
    fn promotion_needs_condition_4_paths_agree_with_direct_offset() {
        // Condition 4: every composed path agrees with the DIRECT edge at start/mid/end. Shift the
        // direct edge's OWN lag by 300 ms so BOTH detours disagree with it ⇒ no promotion.
        let (mut m, c) = promotable_scene();
        let lag = (SCENE_OFF[0] - SCENE_OFF[3]) * 1000.0 + 300.0;
        m[0][3] = Some(cell(MatchClassification::Ambiguous, lag, 0.0, 40, 5.0));
        m[3][0] = Some(cell(MatchClassification::Ambiguous, -lag, 0.0, 40, 5.0));
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "paths disagreeing with the direct offset block promotion"
        );
    }

    #[test]
    fn promotion_needs_condition_5_drift_agrees() {
        // Condition 5: composed drift agrees within graph_drift_tol_ppm (150). Give the direct edge a
        // large drift (+500 ppm) the (zero-drift) detours cannot match ⇒ no promotion.
        let (mut m, c) = promotable_scene();
        let lag = (SCENE_OFF[0] - SCENE_OFF[3]) * 1000.0;
        m[0][3] = Some(cell(MatchClassification::Ambiguous, lag, 500.0, 40, 5.0));
        m[3][0] = Some(cell(MatchClassification::Ambiguous, -lag, -500.0, 40, 5.0));
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "composed drift beyond tol blocks promotion"
        );
    }

    #[test]
    fn promotion_needs_condition_6_paths_agree_with_each_other() {
        // Condition 6: the independent paths agree with EACH OTHER. Split the two detours apart while
        // each stays within tol of the direct edge: nudge the 1→3 leg +60 ms and the 2→3 leg -60 ms.
        // Each detour is then 60 ms from the direct edge (< 100 tol, so cond 4 still holds) but 120 ms
        // apart from each other (> 100) ⇒ cond 6 fails ⇒ no promotion.
        let (mut m, c) = promotable_scene();
        nudge_leg(&mut m, 1, 3, 60.0);
        nudge_leg(&mut m, 2, 3, -60.0);
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "independent paths that disagree with each other block promotion (cond 6)"
        );
        // Isolate cond 6: at ±30 ms the detours are 60 apart (< 100) and STILL within tol of the
        // direct edge ⇒ promotion succeeds (only cond 6 was the blocker above).
        let (mut ok, c2) = promotable_scene();
        nudge_leg(&mut ok, 1, 3, 30.0);
        nudge_leg(&mut ok, 2, 3, -30.0);
        assert_eq!(
            direct(&adjust(&ok, &c2, &params())),
            MatchClassification::Match,
            "paths within mutual tol still promote (isolates cond 6)"
        );
    }

    /// Shift a single directed leg `a→b`'s lag by `delta_ms` (and its inverse by `-delta_ms`),
    /// keeping it a Match edge — used to perturb ONE detour's composed offset.
    fn nudge_leg(m: &mut [Vec<Option<PairEvidence>>], a: usize, b: usize, delta_ms: f64) {
        if let Some(c) = m[a][b].as_mut() {
            c.lag_ms += delta_ms;
        }
        if let Some(c) = m[b][a].as_mut() {
            c.lag_ms -= delta_ms;
        }
    }

    // ============================ zero direct support is NEVER promoted ============================

    #[test]
    fn zero_direct_support_never_promoted() {
        // Even with two perfectly-consistent independent Match detours, an Ambiguous direct edge with
        // ZERO landmark support is NOT promoted (§13.3 absolute guard).
        let (mut m, c) = promotable_scene();
        m[0][3].as_mut().unwrap().support = 0;
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "zero direct support is NEVER promoted, regardless of paths (§13.3)"
        );
    }

    // ============================ demotion requires the full rule ============================

    /// A demotable scene: a direct Match 0→3 with a WRONG lag (+400 ms) that CONFLICTS with two
    /// independent, mutually-agreeing, dominating (score 6 ≥ 5) composed Match detours via 1 and 2.
    fn demotable_scene() -> (Vec<Vec<Option<PairEvidence>>>, Vec<f64>) {
        let mut m = empty(4);
        // Consistent detour legs at the HIGHER score 6.0 (so each detour's bottleneck ≥ direct 5.0).
        set_pair(&mut m, &SCENE_OFF, 0, 1, 40, 6.0);
        set_pair(&mut m, &SCENE_OFF, 1, 3, 40, 6.0);
        set_pair(&mut m, &SCENE_OFF, 0, 2, 40, 6.0);
        set_pair(&mut m, &SCENE_OFF, 2, 3, 40, 6.0);
        // The DIRECT edge 0→3 is a Match but WRONG by +400 ms (score 5.0). Its inverse mirrors it.
        let wrong = (SCENE_OFF[0] - SCENE_OFF[3]) * 1000.0 + 400.0;
        m[0][3] = Some(cell(MatchClassification::Match, wrong, 0.0, 40, 5.0));
        m[3][0] = Some(cell(MatchClassification::Match, -wrong, 0.0, 40, 5.0));
        (m, cov(4, 600.0))
    }

    #[test]
    fn demotion_baseline_demotes() {
        let (m, c) = demotable_scene();
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Ambiguous,
            "a wrong Match conflicting with two dominating consistent paths is demoted"
        );
    }

    #[test]
    fn demotion_needs_bottleneck_score_ge_direct() {
        // If the direct edge's score EXCEEDS both detours' bottleneck, the composed paths do NOT
        // dominate ⇒ no demotion (a strong direct claim is not overturned by weaker paths).
        let (mut m, c) = demotable_scene();
        m[0][3].as_mut().unwrap().score = 9.0; // above the detours' 6.0
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Match,
            "paths weaker than the direct edge cannot demote it"
        );
    }

    #[test]
    fn demotion_needs_paths_to_conflict() {
        // If the detours AGREE with the direct edge (it is actually correct), there is no conflict ⇒
        // no demotion. Set the direct edge to the TRUE lag.
        let (mut m, c) = demotable_scene();
        let truth = (SCENE_OFF[0] - SCENE_OFF[3]) * 1000.0;
        m[0][3] = Some(cell(MatchClassification::Match, truth, 0.0, 40, 5.0));
        m[3][0] = Some(cell(MatchClassification::Match, -truth, 0.0, 40, 5.0));
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Match,
            "a correct Match corroborated by the paths is not demoted"
        );
    }

    #[test]
    fn demotion_needs_enough_independent_paths() {
        // With only ONE dominating path (remove intermediate 2) the required-paths floor (2) is unmet
        // ⇒ no demotion.
        let (mut m, c) = demotable_scene();
        m[0][2] = None;
        m[2][0] = None;
        m[2][3] = None;
        m[3][2] = None;
        assert_eq!(
            direct(&adjust(&m, &c, &params())),
            MatchClassification::Match,
            "one path cannot demote (need required independent paths)"
        );
    }

    // ============================ determinism: order-independent, byte-identical ============================

    #[test]
    fn adjust_is_deterministic_across_runs() {
        let (m, c) = promotable_scene();
        let a = adjust(&m, &c, &params());
        let b = adjust(&m, &c, &params());
        for (ra, rb) in a.iter().zip(&b) {
            for (ca, cb) in ra.iter().zip(rb) {
                assert_eq!(
                    serde_json::to_string(ca).unwrap(),
                    serde_json::to_string(cb).unwrap(),
                    "adjust must be byte-identical across runs"
                );
            }
        }
    }

    // Permuting the node ORDER must not change which cells promote/demote: the verdict for a given
    // logical (from, to) pair is independent of node ordering (traversal sorts by index). We apply a
    // fixed permutation, re-index, and compare the SAME logical edge.
    #[test]
    fn adjust_is_order_independent() {
        let (m, c) = promotable_scene();
        let base = adjust(&m, &c, &params());
        let base_direct = direct(&base); // logical edge (0,3)

        // Permutation: map old node k -> perm[k]. Reverse order: 0<->3, 1<->2.
        let perm = [3usize, 2, 1, 0];
        let n = 4;
        let mut pm = empty(n);
        let mut pc = vec![0.0; n];
        for (i, row) in m.iter().enumerate() {
            pc[perm[i]] = c[i];
            for (j, slot) in row.iter().enumerate() {
                pm[perm[i]][perm[j]] = slot.clone();
            }
        }
        let permuted = adjust(&pm, &pc, &params());
        // The old logical edge (0,3) is now (perm[0], perm[3]) = (3, 0).
        let perm_direct = permuted[perm[0]][perm[3]].as_ref().unwrap().classification;
        assert_eq!(
            base_direct, perm_direct,
            "the same logical (from, to) edge gets the same verdict regardless of node order"
        );
    }

    // The non-edge classes are carried through untouched (and never become edges/paths).
    #[test]
    fn non_edge_cells_unchanged() {
        let m = vec![
            vec![
                None,
                Some(no_match()),
                Some(PairEvidence::insufficient_overlap(MatchMethod::SpectralLandmarkV1, 2.0)),
                Some(PairEvidence::unavailable(MatchMethod::SpectralLandmarkV1, 30.0)),
            ],
            vec![Some(no_match()), None, None, None],
            vec![Some(no_match()), None, None, None],
            vec![Some(no_match()), None, None, None],
        ];
        let out = adjust(&m, &cov(4, 600.0), &params());
        assert_eq!(out[0][1].as_ref().unwrap().classification, MatchClassification::NoMatch);
        assert_eq!(out[0][2].as_ref().unwrap().classification, MatchClassification::InsufficientOverlap);
        assert_eq!(out[0][3].as_ref().unwrap().classification, MatchClassification::Unavailable);
    }

    // A non-finite/non-positive-scale edge is rejected before construction: a cell whose drift drives
    // scale ≤ 0 contributes NO edge, so it cannot be a path leg (and adjust does not panic / emit NaN).
    #[test]
    fn non_finite_scale_edge_excluded_from_graph() {
        // Break ONE detour leg (1→3) with a -1.2e6 ppm drift (scale = 1 - 1.2 = -0.2 < 0) ⇒ that edge
        // is dropped ⇒ only one valid detour (via 2) ⇒ fewer than 2 paths ⇒ no promotion, no NaN/panic.
        let (mut m, c) = promotable_scene();
        let lag = m[1][3].as_ref().unwrap().lag_ms;
        m[1][3] = Some(cell(MatchClassification::Match, lag, -1_200_000.0, 40, 5.0));
        let out = adjust(&m, &c, &params());
        assert_eq!(direct(&out), MatchClassification::Ambiguous, "broken leg ⇒ <2 paths ⇒ no promotion");
        let json = serde_json::to_string(&out[0][3]).unwrap();
        for bad in ["NaN", "Infinity", "inf", "nan"] {
            assert!(!json.contains(bad), "no non-finite token in adjusted evidence: {json}");
        }
    }

    // ============================ bipartite adapter + general adjust shape ============================

    // ============================ §13.4 triangle_closes (audio-audio simultaneity over affine evidence) ============================

    // Build the three triangle maps from node offsets: V=0, A=1, B=2 (seconds). V→A, V→B, A→B are
    // the offset-derived maps; with consistent offsets the composed V→A→B equals the direct V→B.
    fn triangle_maps(off_v: f64, off_a: f64, off_b: f64, ab_lag_extra_ms: f64) -> (AffineMap, AffineMap, AffineMap) {
        let va = AffineMap::from_lag_drift((off_v - off_a) * 1000.0, 0.0).unwrap();
        let vb = AffineMap::from_lag_drift((off_v - off_b) * 1000.0, 0.0).unwrap();
        let ab = AffineMap::from_lag_drift((off_a - off_b) * 1000.0 + ab_lag_extra_ms, 0.0).unwrap();
        (va, vb, ab)
    }

    #[test]
    fn triangle_closes_for_consistent_simultaneous_recorders() {
        // Two recorders A,B of video V with mutually-consistent offsets ⇒ the composed V→A→B equals
        // V→B AND the A↔B edge is a Match ⇒ simultaneous.
        let (va, vb, ab) = triangle_maps(0.0, 0.3, 1.0, 0.0);
        let p = params();
        assert!(
            triangle_closes(Some(va), Some(ab), Some(vb), MatchClassification::Match, 600.0,
                            p.graph_offset_tol_ms, p.graph_drift_tol_ppm),
            "consistent triangle + A↔B Match ⇒ simultaneous"
        );
    }

    #[test]
    fn triangle_does_not_close_when_ab_is_not_a_match() {
        // The A↔B identity authority is core's classification: even a perfectly-closing geometry is
        // NOT simultaneous if A↔B is NoMatch / Ambiguous (replaces the legacy `ab.z >= z_min`).
        let (va, vb, ab) = triangle_maps(0.0, 0.3, 1.0, 0.0);
        let p = params();
        for cls in [MatchClassification::NoMatch, MatchClassification::Ambiguous, MatchClassification::Unavailable] {
            assert!(
                !triangle_closes(Some(va), Some(ab), Some(vb), cls, 600.0,
                                 p.graph_offset_tol_ms, p.graph_drift_tol_ppm),
                "A↔B {cls:?} ⇒ NOT simultaneous (identity authority is core's classification)"
            );
        }
    }

    #[test]
    fn triangle_does_not_close_when_composed_residual_too_large() {
        // A↔B is a Match but its lag is 300 ms off the consistent geometry ⇒ the composed V→A→B map
        // disagrees with the direct V→B beyond the 100 ms offset tol ⇒ NOT simultaneous (a different
        // take that nonetheless registered a same-recording A↔B edge would fail the closure).
        let (va, vb, ab) = triangle_maps(0.0, 0.3, 1.0, 300.0);
        let p = params();
        assert!(
            !triangle_closes(Some(va), Some(ab), Some(vb), MatchClassification::Match, 600.0,
                             p.graph_offset_tol_ms, p.graph_drift_tol_ppm),
            "a 300 ms composed-residual triangle does NOT close (> 100 ms offset tol)"
        );
    }

    #[test]
    fn triangle_drift_disagreement_caught_at_span_end() {
        // The legacy scalar-lag check missed pure-drift disagreement; the composed-map residual at
        // start/mid/end catches it. V→B has the right intercept but a +200 ppm drift the composed
        // (zero-drift) path cannot match: start agrees, the END blows past the offset tol over 600 s.
        let va = AffineMap::from_lag_drift(-300.0, 0.0).unwrap(); // V→A
        let ab = AffineMap::from_lag_drift(-700.0, 0.0).unwrap(); // A→B (closes to V→B = -1000 ms)
        let vb = AffineMap::from_lag_drift(-1000.0, 200.0).unwrap(); // same intercept, wrong drift
        let p = params();
        assert!(
            !triangle_closes(Some(va), Some(ab), Some(vb), MatchClassification::Match, 600.0,
                             p.graph_offset_tol_ms, p.graph_drift_tol_ppm),
            "a drift-only disagreement is caught at the span end (NOT a single scalar lag)"
        );
    }

    #[test]
    fn triangle_returns_false_on_missing_map() {
        // A degenerate (non-finite/non-positive-scale) leg yields a None map ⇒ NOT simultaneous,
        // no panic/NaN. evidence_map() returns None for such a cell.
        let (va, vb, _ab) = triangle_maps(0.0, 0.3, 1.0, 0.0);
        let p = params();
        assert!(!triangle_closes(Some(va), None, Some(vb), MatchClassification::Match, 600.0,
                                 p.graph_offset_tol_ms, p.graph_drift_tol_ppm));
        // evidence_map: a NoMatch/edge cell with a sane lag still produces a map; a drift driving
        // scale ≤ 0 produces None.
        let good = cell(MatchClassification::Match, 1500.0, 0.0, 40, 5.0);
        assert!(evidence_map(&good).is_some(), "a finite map cell yields Some");
        let bad = cell(MatchClassification::Match, 1500.0, -2_000_000.0, 40, 5.0);
        assert!(evidence_map(&bad).is_none(), "a scale≤0 cell yields None (no degenerate map)");
        // evidence_map uses the REFINED mapping when present.
        let mut refined = cell(MatchClassification::Match, 1500.0, 0.0, 40, 5.0);
        refined.refined_lag_ms = Some(1490.0);
        let m = evidence_map(&refined).unwrap();
        assert!((m.intercept_s - 1.490).abs() < 1e-12, "evidence_map prefers the refined lag");
    }

    // The bipartite adapter preserves the input video×audio SHAPE and (on a lone bipartite matrix,
    // which has no two-edge video→audio detours) is a pass-through that never panics or emits NaN.
    #[test]
    fn adjust_bipartite_preserves_shape_and_is_safe() {
        let evidence = vec![
            vec![cell(MatchClassification::Match, 1500.0, 0.0, 40, 5.0), no_match()],
            vec![cell(MatchClassification::Ambiguous, 800.0, 0.0, 40, 5.0), cell(MatchClassification::Match, 1200.0, 0.0, 40, 5.0)],
        ];
        let vcov = vec![600.0, 600.0];
        let acov = vec![600.0, 600.0];
        let out = adjust_bipartite(&evidence, &vcov, &acov, &params());
        assert_eq!(out.len(), 2);
        assert!(out.iter().all(|r| r.len() == 2), "shape preserved");
        // A lone bipartite matrix has no corroborating two-edge detours ⇒ classifications unchanged.
        for (ri, row) in out.iter().enumerate() {
            for (ci, c) in row.iter().enumerate() {
                assert_eq!(
                    c.classification, evidence[ri][ci].classification,
                    "bipartite-only ⇒ pass-through (no two-edge video→audio detour exists)"
                );
                let json = serde_json::to_string(c).unwrap();
                assert!(!json.contains("NaN") && !json.contains("Infinity"));
            }
        }
    }
}
