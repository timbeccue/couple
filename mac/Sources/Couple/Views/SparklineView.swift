// Drift sparkline (UX §1.4 row 2, deliberately peripheral): measured per-window
// local offsets as dots (opacity = confidence) vs camera time, with the current
// mapping line L(t) drawn through them.

import SwiftUI

struct SparklineView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        Canvas { ctx, size in
            let t0 = CACurrentMediaTime()
            defer { FrameStats.shared.record("sparkline", seconds: CACurrentMediaTime() - t0) }

            ctx.fill(Path(roundedRect: CGRect(origin: .zero, size: size), cornerRadius: 5),
                     with: .color(Theme.canvasBg))
            let windows = pair.driftWindows
            guard let cam = pair.camera.track, !windows.isEmpty else {
                return
            }
            let mapping = pair.mapping
            let tMin = 0.0
            let tMax = cam.durationS
            // y domain: dots ∪ mapping line endpoints, padded.
            var lags = windows.map(\.lagMs)
            lags.append(mapping.localLagMs(tMin))
            lags.append(mapping.localLagMs(tMax))
            let lo = lags.min()!
            let hi = lags.max()!
            let pad = max((hi - lo) * 0.25, 0.5)
            let yLo = lo - pad
            let yHi = hi + pad
            func pt(_ t: Double, _ lag: Double) -> CGPoint {
                CGPoint(x: (t - tMin) / (tMax - tMin) * size.width,
                        y: size.height - (lag - yLo) / (yHi - yLo) * size.height)
            }
            // Mapping line.
            var line = Path()
            line.move(to: pt(tMin, mapping.localLagMs(tMin)))
            line.addLine(to: pt(tMax, mapping.localLagMs(tMax)))
            ctx.stroke(line, with: .color(Theme.accent.opacity(0.8)), lineWidth: 1)
            // Measurement dots.
            for w in windows {
                let p = pt(w.t, w.lagMs)
                let r: CGFloat = 2.2
                let opacity = min(max(w.ncc, 0.15), 1)
                ctx.fill(Path(ellipseIn: CGRect(x: p.x - r, y: p.y - r, width: 2 * r, height: 2 * r)),
                         with: .color(Theme.external.opacity(opacity)))
            }
        }
        .frame(maxWidth: .infinity)
        .clipShape(RoundedRectangle(cornerRadius: 5))
    }
}
