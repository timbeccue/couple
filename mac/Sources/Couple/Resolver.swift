// Pairing resolution (SPEC-V2 §3.3), lifted from the Phase-7 CLI (cli/src/pair.rs)
// into the shell so the user can override it. Core supplies mechanical
// measurements (pairCandidates / pairCorrelate); THIS layer owns coverage
// intervals, disjoint/overlapping discrimination, the §3.4 simultaneity check
// (triangle closure), the §3.5 margin rule, and the §3.3 row states.
//
// Honesty posture (V2-F2): conflicting candidates that aren't separable resolve
// `ambiguous` with NO selection — never a coin-flip. Orderings are content/name
// deterministic (timestamps never order a pairing, SPEC §1.4), so the shell
// reproduces `couple pair` byte-for-byte: bach 169 partial→Gigue, 170 ambiguous,
// 171 confident→Prelude, 172 confident→ZOOM0033 on the owner's multifile-session.

import CoupleKit
import Foundation

/// SPEC-V2 §9 resolution constants. [TUNE] values are surfaced (raw z/lag/margin
/// ride in `ResolutionDetail`) so the owner can calibrate on real sessions —
/// v1's RMS thresholds do not transfer (real true-pairs run ~5 ms, V2-F2/§V2.9).
struct PairParams: Sendable, Equatable {
    var zMin = 8.5            // [VALIDATED] Session-C pairing floor
    var zMargin = 6.0         // [TUNE] decisive-best margin (V2-F2)
    var triangleTolMs = 250.0 // [SPEC, TUNE] §3.4 closure tolerance
    var coverageFull = 0.98   // [SPEC, TUNE] single-match `confident` floor
    var disjointTolS = 0.5    // [SPEC, TUNE] interval-overlap to be one cluster
}

/// Span of the video timeline an audio covers (§3.2; overlap_info with ppm 0).
struct Coverage: Sendable, Equatable {
    var t0: Double
    var t1: Double
    var seconds: Double
    var fraction: Double

    /// SPEC §5.6 overlap arithmetic at drift 0 (delta = 1).
    init(durVideo: Double, durAudio: Double, lagMs: Double) {
        let offS = lagMs / 1000
        t0 = max(0, -offS)
        t1 = min(durVideo, durAudio - offS)
        seconds = max(0, t1 - t0)
        fraction = durVideo > 0 ? seconds / durVideo : 0
    }
}

/// §3.3 row states (chip vocabulary mirrors the CLI/UX-DECISIONS-V2).
enum RowState: String, Sendable, Codable {
    case confident, partial, split, twoSources, ambiguous, noMatch

    var chip: String {
        switch self {
        case .confident: return "confident"
        case .partial: return "partial"
        case .split: return "split across files"
        case .twoSources: return "2 sources"
        case .ambiguous: return "ambiguous — check me"
        case .noMatch: return "no match"
        }
    }

    /// Verdict-model bucket (UX-DECISIONS-V2 §V2.3): good→TO APPROVE,
    /// questionable/none→TO FIX. Coverage is never a quality penalty, so a
    /// `partial` row with a real match is still good.
    var verdict: Verdict {
        switch self {
        case .confident, .partial, .split, .twoSources: return .good
        case .ambiguous: return .questionable
        case .noMatch: return .none
        }
    }
}

/// Three-category pairing quality (UX-DECISIONS-V2 §V2.3) — not a continuous score.
enum Verdict: Sendable { case good, questionable, none }

/// One candidate audio source for a video, with its measured fit and coverage.
/// `audioKey` is the pool content key (stable identity); `audioName` drives the
/// CLI-identical name tie-breaks (SPEC §1.4 — clock-free).
struct RowCandidate: Sendable, Identifiable {
    var id: String { audioKey }
    var audioKey: String
    var audioName: String
    var z: Double
    var lagMs: Double
    var coverage: Coverage
}

/// §3.4 record for one overlapping audio pair (raw numbers for V2-F2 calibration).
struct TriangleCheck: Sendable {
    var audioA: String
    var audioB: String
    var abZ: Double
    var abLagMs: Double
    var residualMs: Double
    var simultaneous: Bool
}

/// The honesty trail behind a row's state — surfaced raw so the owner can tune.
struct ResolutionDetail: Sendable {
    var margin: Double?
    var clusters: Int
    var triangles: [TriangleCheck] = []
    var note: String?
}

/// A resolved queue row = one video + its selected audio (nil for ambiguous /
/// no-match: never a coin-flip) + the alternates. Mirrors `couple pair`'s Row.
struct ResolvedRow: Sendable {
    var videoKey: String
    var videoName: String
    var state: RowState
    var chip: String
    var selected: RowCandidate?
    var alternates: [RowCandidate]
    var detail: ResolutionDetail
}

/// The video side of a pool entry as the resolver needs it.
struct ResolverVideo: Sendable {
    var key: String
    var name: String
    var durationS: Double
    var handle: TrackHandle?   // nil ⇒ stripped audio stream ⇒ no match
    var hasAudio: Bool
}

/// The audio side of a pool entry.
struct ResolverAudio: Sendable {
    var key: String
    var name: String
    var durationS: Double
    var handle: TrackHandle
}

enum Resolver {
    /// Full §3.3 resolution over the media pool. Pure measurement + arithmetic;
    /// safe to run off the main actor (Engine/TrackHandle are Sendable). The
    /// returned rows are in video input order; unused audio is anything no row
    /// selects (computed by the caller from the pool).
    static func resolve(videos: [ResolverVideo], audios: [ResolverAudio],
                        engine: Engine, params: PairParams = PairParams()) -> [ResolvedRow] {
        // Candidate matrix over the analyzable videos only (a stripped-audio
        // video has no handle and resolves `no match` without a matrix row).
        let importable = videos.filter { $0.handle != nil }
        let matrix = engine.pairCandidates(videos: importable.compactMap(\.handle),
                                           audios: audios.map(\.handle)) ?? []

        // Map matrix rows back onto the full video list, keeping per-video the
        // candidate cells (audio index, measurement) with z ≥ floor.
        var perVideo: [[(Int, PairCorrelation)]] = Array(repeating: [], count: videos.count)
        var matrixRow = 0
        for (vi, v) in videos.enumerated() where v.handle != nil {
            let row = matrixRow < matrix.count ? matrix[matrixRow] : []
            matrixRow += 1
            for (ai, m) in row.enumerated() where !m.insufficientOverlap && m.z >= params.zMin {
                perVideo[vi].append((ai, m))
            }
        }

        var abMemo: [PairKey: PairCorrelation] = [:]
        return videos.enumerated().map { vi, v in
            resolveRow(video: v, cands: perVideo[vi], audios: audios,
                       engine: engine, params: params, abMemo: &abMemo)
        }
    }

    /// Audio keys no resolved row selects — the §3.6 unused tray.
    static func unusedAudio(rows: [ResolvedRow], audios: [ResolverAudio]) -> [ResolverAudio] {
        let used = Set(rows.compactMap { $0.selected?.audioKey })
        return audios.filter { !used.contains($0.key) }
    }

    // MARK: - §3.3 single-row resolution

    private static func resolveRow(video: ResolverVideo, cands: [(Int, PairCorrelation)],
                                   audios: [ResolverAudio], engine: Engine, params: PairParams,
                                   abMemo: inout [PairKey: PairCorrelation]) -> ResolvedRow {
        func mk(_ ai: Int, _ m: PairCorrelation) -> RowCandidate {
            RowCandidate(audioKey: audios[ai].key, audioName: audios[ai].name, z: m.z, lagMs: m.lagMs,
                         coverage: Coverage(durVideo: video.durationS, durAudio: audios[ai].durationS, lagMs: m.lagMs))
        }
        func row(_ state: RowState, _ selected: RowCandidate?, _ alternates: [RowCandidate],
                 _ detail: ResolutionDetail) -> ResolvedRow {
            ResolvedRow(videoKey: video.key, videoName: video.name, state: state,
                        chip: chipText(state, selected, video.durationS),
                        selected: selected, alternates: alternates, detail: detail)
        }

        if !video.hasAudio {
            return row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0, note: "no audio stream"))
        }

        // Candidates sorted by z desc, ties by audio file name (deterministic,
        // clock-free — SPEC §1.4).
        var c = cands
        c.sort { a, b in a.1.z != b.1.z ? a.1.z > b.1.z : audios[a.0].name < audios[b.0].name }

        if c.isEmpty {
            return row(.noMatch, nil, [], ResolutionDetail(margin: nil, clusters: 0))
        }
        if c.count == 1 {
            let sel = mk(c[0].0, c[0].1)
            let state: RowState = sel.coverage.fraction >= params.coverageFull ? .confident : .partial
            return row(state, sel, [], ResolutionDetail(margin: nil, clusters: 1))
        }

        // Cluster coverage intervals into connected components by overlap
        // (§3.3 disjoint/overlapping discrimination, generalized past 2).
        let covs = c.map { Coverage(durVideo: video.durationS, durAudio: audios[$0.0].durationS, lagMs: $0.1.lagMs) }
        let clusters = clusterIntervals(covs, tol: params.disjointTolS)

        var triangles: [TriangleCheck] = []
        var marginOut: Double?
        var winners: [(idx: Int, simultaneous: Bool)] = []
        var ambiguous = false

        for cluster in clusters {
            if cluster.count == 1 {
                winners.append((cluster[0], false))
                continue
            }
            // §3.4 simultaneity over every pair in the cluster.
            var allSimultaneous = true
            for i in cluster.indices {
                for j in (i + 1)..<cluster.count {
                    let ci = cluster[i], cj = cluster[j]
                    let ai = c[ci].0, aj = c[cj].0
                    let ab = correlateAudioPair(ai, aj, audios: audios, engine: engine, memo: &abMemo)
                    let residual = abs(c[cj].1.lagMs - c[ci].1.lagMs - ab.lagMs)
                    let simultaneous = ab.z >= params.zMin && residual <= params.triangleTolMs
                    allSimultaneous = allSimultaneous && simultaneous
                    triangles.append(TriangleCheck(audioA: audios[ai].name, audioB: audios[aj].name,
                                                   abZ: ab.z, abLagMs: ab.lagMs,
                                                   residualMs: residual, simultaneous: simultaneous))
                }
            }
            if allSimultaneous {
                winners.append((cluster[0], true)) // genuine alternates → best z, `2 sources`
            } else {
                // Conflict (different takes?): §3.5 margin rule, biased to
                // `ambiguous` over a coin-flip (V2-F2).
                let zBest = c[cluster[0]].1.z
                let zSecond = c[cluster[1]].1.z
                let margin = zBest - zSecond
                marginOut = margin
                if zSecond < params.zMin || margin >= params.zMargin {
                    winners.append((cluster[0], false))
                } else {
                    ambiguous = true
                }
            }
        }

        if ambiguous {
            let alternates = c.map { mk($0.0, $0.1) }
            return row(.ambiguous, nil, alternates,
                       ResolutionDetail(margin: marginOut, clusters: clusters.count, triangles: triangles))
        }

        // Row selection among cluster winners: disjoint clusters ⇒ recorder
        // restart ⇒ max coverage (§3.3); ties by z then file name (smaller wins).
        let selPos = winners.indices.max { a, b in
            let wa = winners[a], wb = winners[b]
            let ca = covs[wa.idx], cb = covs[wb.idx]
            if ca.seconds != cb.seconds { return ca.seconds < cb.seconds }
            if c[wa.idx].1.z != c[wb.idx].1.z { return c[wa.idx].1.z < c[wb.idx].1.z }
            return audios[c[wa.idx].0].name > audios[c[wb.idx].0].name // smaller name = greater
        }!
        let selIdx = winners[selPos].idx
        let selSimul = winners[selPos].simultaneous
        let sel = mk(c[selIdx].0, c[selIdx].1)
        let alternates = c.enumerated().filter { $0.offset != selIdx }.map { mk($0.element.0, $0.element.1) }
        let state: RowState
        if clusters.count > 1 {
            state = .split
        } else if selSimul {
            state = .twoSources
        } else {
            state = sel.coverage.fraction >= params.coverageFull ? .confident : .partial
        }
        return row(state, sel, alternates,
                   ResolutionDetail(margin: marginOut, clusters: clusters.count, triangles: triangles))
    }

    // MARK: - helpers (ports of pair.rs)

    private struct PairKey: Hashable { var lo: Int; var hi: Int }

    /// A×B stage-1, memoized with antisymmetric lag (L(B,A) = −L(A,B)).
    private static func correlateAudioPair(_ a: Int, _ b: Int, audios: [ResolverAudio],
                                           engine: Engine, memo: inout [PairKey: PairCorrelation]) -> PairCorrelation {
        let key = PairKey(lo: min(a, b), hi: max(a, b))
        let base: PairCorrelation
        if let hit = memo[key] {
            base = hit
        } else {
            base = engine.pairCorrelate(audios[key.lo].handle, audios[key.hi].handle)
                ?? PairCorrelation(z: 0, lagMs: 0, ncc: 0, fineNcc: 0, overlapBins: 0,
                                   runnerUpZ: 0, refined: false, insufficientOverlap: true)
            memo[key] = base
        }
        var m = base
        if a > b { m.lagMs = -m.lagMs }
        return m
    }

    /// Connected components of intervals overlapping by more than `tol` seconds,
    /// each component preserving input (z-descending) order; groups by first index.
    private static func clusterIntervals(_ covs: [Coverage], tol: Double) -> [[Int]] {
        let n = covs.count
        var parent = Array(0..<n)
        func find(_ i: Int) -> Int {
            var r = i
            while parent[r] != r { r = parent[r] }
            var x = i
            while parent[x] != r { let next = parent[x]; parent[x] = r; x = next }
            return r
        }
        for i in 0..<n {
            for j in (i + 1)..<n {
                let inter = min(covs[i].t1, covs[j].t1) - max(covs[i].t0, covs[j].t0)
                if inter > tol {
                    let ri = find(i), rj = find(j)
                    parent[max(ri, rj)] = min(ri, rj)
                }
            }
        }
        var groups: [Int: [Int]] = [:]
        for i in 0..<n { groups[find(i), default: []].append(i) }
        return groups.sorted { ($0.value.first ?? 0) < ($1.value.first ?? 0) }.map(\.value)
    }

    private static func chipText(_ state: RowState, _ selected: RowCandidate?, _ durVideo: Double) -> String {
        switch state {
        case .partial, .split:
            if let s = selected {
                return "\(state.chip) — audio covers \(mmss(s.coverage.t0))–\(mmss(s.coverage.t1)) of \(mmss(durVideo))"
            }
            return state.chip
        default:
            return state.chip
        }
    }

    private static func mmss(_ s: Double) -> String {
        let total = Int(s.rounded())
        return String(format: "%d:%02d", total / 60, total % 60)
    }
}
