// PairStrip (PLAN-COMPACT-MODE Phase 3 step 1; UX §V2.11): the compact two-lane
// waveform SEEK strip under the video, for the current couple, in video time.
// Camera lane (teal) above over the whole clip; external lane (amber) below,
// placed through the pair's mapping and drawn ONLY inside the coverage window,
// with a faint baseline elsewhere. Playhead bound to the shared audition; click
// or drag anywhere seeks (direct manipulation: scrub, never snap — the same
// absolute-x mapping the overview's playhead drag uses).
//
// Both lanes come from `TrackHandle.band` (cpl_band) at ~2 px/column — the same
// read path as `OverviewStrip`, no extra decode. While a candidate is being
// auditioned (`AppModel.candidateAudition`) the amber lane is that candidate's
// audio at its best-fit lag with its coverage, so a late-starting recorder shows
// as a gap. Used in the full arrangement's hero; Phase 4 reuses it in compact.

import CoupleKit
import SwiftUI

/// An external lane to draw: which audio, how it maps into video time, and the
/// video-time window it covers (nil = draw nothing but the baseline).
struct StripExternal {
    let track: TrackHandle
    let mapping: Mapping
    let coverage: ClosedRange<Double>?
}

struct PairStrip: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow?
    /// Override for the amber lane (a candidate audition). nil = the couple's own pairing.
    var candidate: AppModel.CandidateAudition?

    @State private var camBuf = BandBuffer(capacity: 2048)
    @State private var extBuf = BandBuffer(capacity: 2048)

    private var cameraTrack: TrackHandle? {
        guard let couple else { return nil }
        return couple.video.handle ?? couple.pair.camera.track
    }

    /// The lane to draw: the audition candidate, else the open session's live
    /// mapping + overlap, else the selected candidate's lag + coverage.
    private var external: StripExternal? {
        if let c = candidate {
            let cov = c.candidate.coverage
            return StripExternal(track: c.track,
                                 mapping: Mapping(offsetMs: c.candidate.lagMs, driftPpm: 0),
                                 coverage: cov.t1 > cov.t0 ? cov.t0...cov.t1 : nil)
        }
        guard let couple else { return nil }
        let pair = couple.pair
        if pair.session != nil, let ext = pair.external.track {
            return StripExternal(track: ext, mapping: pair.mapping, coverage: pair.overlap)
        }
        if let sel = couple.selectedCandidate, let ext = model.pool.entry(key: sel.audioKey)?.handle {
            let cov = sel.coverage
            return StripExternal(track: ext, mapping: Mapping(offsetMs: sel.lagMs, driftPpm: 0),
                                 coverage: cov.t1 > cov.t0 ? cov.t0...cov.t1 : nil)
        }
        return nil
    }

    private var duration: Double { cameraTrack?.durationS ?? 0 }

    var body: some View {
        GeometryReader { geo in
            ZStack {
                lanes
                playhead
            }
            .contentShape(Rectangle())
            .gesture(scrub(width: geo.size.width))
        }
        .background(Theme.canvasBg)
        .clipShape(RoundedRectangle(cornerRadius: 4))
        .accessibilityLabel("Waveform seek strip")
    }

    private var lanes: some View {
        let cam = cameraTrack
        let ext = external
        let dur = duration
        return Canvas { ctx, sz in
            let laneH = sz.height / 2
            let cols = min(max(Int(sz.width / 2), 1), 2048)
            let colW = sz.width / CGFloat(cols)
            // Faint baselines: visible wherever a lane has nothing to draw.
            for mid in [laneH / 2, laneH + laneH / 2] {
                ctx.fill(Path(CGRect(x: 0, y: mid - 0.5, width: sz.width, height: 1)),
                         with: .color(.white.opacity(0.10)))
            }
            guard let cam, dur > 0, cols > 4 else { return }
            if cam.band(t0: 0, t1: dur, nCols: cols, into: camBuf) {
                drawLane(ctx, buf: camBuf, cols: cols, colW: colW, midY: laneH / 2,
                         halfH: laneH / 2 - 1, meta: cam.meta, color: Theme.camera,
                         include: { _ in true })
            }
            guard let ext, let cov = ext.coverage else { return }
            // The u-range is affine in t, so one band query stays column-aligned.
            if ext.track.band(t0: ext.mapping.u(0), t1: ext.mapping.u(dur), nCols: cols, into: extBuf) {
                drawLane(ctx, buf: extBuf, cols: cols, colW: colW, midY: laneH + laneH / 2,
                         halfH: laneH / 2 - 1, meta: ext.track.meta, color: Theme.external,
                         include: { i in
                             let t = (Double(i) + 0.5) / Double(cols) * dur
                             return cov.contains(t)
                         })
            }
        }
    }

    private var playhead: some View {
        let audition = model.audition
        let dur = duration
        return Canvas { ctx, sz in
            guard dur > 0 else { return }
            let x = CGFloat(audition.playheadT / dur) * sz.width
            let active = audition.isPlaying || audition.isScrubbing
            ctx.fill(Path(CGRect(x: x - 0.75, y: 0, width: 1.5, height: sz.height)),
                     with: .color(.white.opacity(active ? 0.9 : 0.6)))
        }
        .allowsHitTesting(false)
    }

    /// Same idiom as the overview playhead: absolute pointer x → time through the
    /// displayed range (OverviewTimeline), every change seeks; no snapping.
    private func scrub(width: CGFloat) -> some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { g in
                let dur = duration
                guard dur > 0, width > 0 else { return }
                model.audition.isScrubbing = true
                model.audition.seek(to: OverviewTimeline.time(atX: g.location.x, width: width,
                                                              visible: 0...dur))
            }
            .onEnded { _ in model.audition.isScrubbing = false }
    }

    /// Min/max columns as bars; the UX §4.1 lane gain (half a span), same as the overview.
    private func drawLane(_ ctx: GraphicsContext, buf: BandBuffer, cols: Int, colW: CGFloat,
                          midY: CGFloat, halfH: CGFloat, meta: TrackMeta, color: Color,
                          include: (Int) -> Bool) {
        let gain = 1.44 / max(meta.stats.activeP959600, 1e-6)
        let scale = 0.5 * gain * meta.normalization / 127.0
        var path = Path()
        for i in 0..<min(cols, buf.count) where include(i) {
            let vMin = max(-1, min(1, Double(buf.mins[i]) * scale))
            let vMax = max(-1, min(1, Double(buf.maxs[i]) * scale))
            let yTop = midY - CGFloat(max(vMax, 0.02)) * halfH
            let yBot = midY - CGFloat(min(vMin, -0.02)) * halfH
            path.addRect(CGRect(x: CGFloat(i) * colW, y: yTop,
                                width: max(colW - 0.6, 0.8), height: max(yBot - yTop, 0.8)))
        }
        ctx.fill(path, with: .color(color.opacity(0.85)))
    }
}
