// Drift sparkline (UX §1.4 row 2, deliberately peripheral): measured per-window
// local offsets as dots (opacity = confidence) vs camera time, with the current
// mapping line L(t) drawn through them.
//
// 2026-07-03 (UX-DECISIONS): the x-domain is the MEASURED span, not the full video —
// on a partial-coverage pair (21 s of 4:15) the full-timeline domain crammed every
// dot into a few pixels and the graph read as two lonely points. Corner time labels
// keep the where-in-the-video context. Windows that were attempted but NOT accepted
// (from the last sync's telemetry) are drawn as baseline ticks at their time — an
// honest "measured here, no lock" — never as dots, because a failed measurement
// carries no meaningful lag to plot.

import SwiftUI

struct SparklineView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        Canvas { ctx, size in
            let t0c = CACurrentMediaTime()
            defer { FrameStats.shared.record("sparkline", seconds: CACurrentMediaTime() - t0c) }

            ctx.fill(Path(roundedRect: CGRect(origin: .zero, size: size), cornerRadius: 5),
                     with: .color(Theme.canvasBg))
            let accepted = pair.driftWindows
            guard pair.camera.track != nil, !accepted.isEmpty else {
                return
            }
            // Attempted-but-rejected windows exist only in the last sync's telemetry
            // (session state persists accepted ones). A restored session shows
            // accepted dots only — never invented rejections.
            let rejected = (pair.lastSync?.windows ?? []).filter { !$0.accepted }
            let mapping = pair.mapping

            // x domain: the measured span (accepted ∪ rejected), padded; floor a
            // degenerate span so a single cluster still renders mid-canvas.
            let times = accepted.map(\.t) + rejected.map(\.t)
            var tMin = times.min()!
            var tMax = times.max()!
            let tPad = max((tMax - tMin) * 0.08, 1.5)
            tMin -= tPad
            tMax += tPad
            // y domain: accepted lags ∪ the mapping line over the DOMAIN (not the whole
            // video — a long take's drift endpoints would flatten the local scatter).
            var lags = accepted.map(\.lagMs)
            lags.append(mapping.localLagMs(tMin))
            lags.append(mapping.localLagMs(tMax))
            let lo = lags.min()!
            let hi = lags.max()!
            let pad = max((hi - lo) * 0.25, 0.5)
            let yLo = lo - pad
            let yHi = hi + pad
            let labelH: CGFloat = 9 // corner time labels strip at the bottom
            let plotH = size.height - labelH
            func pt(_ t: Double, _ lag: Double) -> CGPoint {
                CGPoint(x: (t - tMin) / (tMax - tMin) * size.width,
                        y: plotH - (lag - yLo) / (yHi - yLo) * plotH)
            }
            // Mapping line.
            var line = Path()
            line.move(to: pt(tMin, mapping.localLagMs(tMin)))
            line.addLine(to: pt(tMax, mapping.localLagMs(tMax)))
            ctx.stroke(line, with: .color(Theme.accent.opacity(0.8)), lineWidth: 1)
            // Rejected attempts: short baseline ticks at their time (no lag to plot).
            for w in rejected {
                let x = (w.t - tMin) / (tMax - tMin) * size.width
                var tick = Path()
                tick.move(to: CGPoint(x: x, y: plotH - 4))
                tick.addLine(to: CGPoint(x: x, y: plotH))
                ctx.stroke(tick, with: .color(Theme.textTertiary.opacity(0.6)), lineWidth: 1)
            }
            // Accepted measurement dots.
            for w in accepted {
                let p = pt(w.t, w.lagMs)
                let r: CGFloat = 2.2
                let opacity = min(max(w.ncc, 0.15), 1)
                ctx.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)),
                         with: .color(Theme.external.opacity(opacity)))
            }
            // Corner time labels: where this measured span sits in the video.
            func mmss(_ t: Double) -> String {
                let s = Int(t.rounded())
                return String(format: "%d:%02d", s / 60, s % 60)
            }
            let labelFont = Font.system(size: 7, design: .monospaced)
            ctx.draw(Text(mmss(tMin)).font(labelFont).foregroundStyle(Theme.textTertiary),
                     at: CGPoint(x: 3, y: size.height - 1), anchor: .bottomLeading)
            ctx.draw(Text(mmss(tMax)).font(labelFont).foregroundStyle(Theme.textTertiary),
                     at: CGPoint(x: size.width - 3, y: size.height - 1), anchor: .bottomTrailing)
        }
        .frame(maxWidth: .infinity)
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }
}
