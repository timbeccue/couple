// The C+O brand mark: the two track colors as an interlocked chain link.
// Geometry mirrors design/logo/couple-wordmark.svg (mark bbox x 21..205,
// y 41..159 in that file's coordinates); the over/under gaps are inverse
// clips, so the mark renders on any background. In the wordmark lockup the
// rings are the C and O of "COUPLE" — size `height` to ~1.04x the cap height
// of the adjacent text.

import SwiftUI

struct CoupleMark: View {
    var height: CGFloat

    var body: some View {
        Canvas { ctx, size in
            let s = size.height / 118.0
            func pt(_ x: CGFloat, _ y: CGFloat) -> CGPoint {
                CGPoint(x: (x - 21.0) * s, y: (y - 41.0) * s)
            }
            let cC = pt(80, 100)
            let cO = pt(146, 100)
            let r = 45.0 * s
            let main = StrokeStyle(lineWidth: 28.0 * s, lineCap: .round)

            // Teal C: open ring, mouth facing the O (arc runs the long way around).
            var cArc = Path()
            cArc.addArc(center: cC, radius: r, startAngle: .degrees(-33), endAngle: .degrees(33), clockwise: true)

            // Cut in the C where the amber band passes over its upper arm.
            var cutTeal = Path()
            cutTeal.addArc(center: cO, radius: r, startAngle: .degrees(-100), endAngle: .degrees(-170), clockwise: true)
            let cutTealArea = cutTeal.strokedPath(StrokeStyle(lineWidth: 44.0 * s, lineCap: .butt))

            // Cut in the O where the C's lower arm (tip included) passes over it.
            var cutAmber = Path()
            cutAmber.addArc(center: cC, radius: r, startAngle: .degrees(33), endAngle: .degrees(95), clockwise: false)
            let cutAmberArea = cutAmber.strokedPath(StrokeStyle(lineWidth: 44.0 * s, lineCap: .round))

            let oCircle = Path(ellipseIn: CGRect(x: cO.x - r, y: cO.y - r, width: 2 * r, height: 2 * r))

            ctx.drawLayer { layer in
                layer.clip(to: cutTealArea, options: .inverse)
                layer.stroke(cArc, with: .color(Theme.camera), style: main)
            }
            ctx.drawLayer { layer in
                layer.clip(to: cutAmberArea, options: .inverse)
                layer.stroke(oCircle, with: .color(Theme.external), style: main)
            }
        }
        .frame(width: height * 184.0 / 118.0, height: height)
        .accessibilityHidden(true)
    }
}
