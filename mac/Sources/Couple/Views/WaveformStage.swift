// Waveform stage (UX §1.6): overview strip (two lanes over the union extent,
// external lane drawn through the live mapping), anchor pins with snap, bezier
// connector layer (side-by-side only), and the loupe row.

import CoupleKit
import SwiftUI

extension PairSession {
    /// Union of both tracks' extents in video time (UX §1.6) — the external
    /// lane slides through the mapping, so this updates live during drags.
    var stageDomain: ClosedRange<Double> {
        guard let cam = camera.track, let ext = external.track else { return 0...1 }
        let e0 = mapping.tForU(0)
        let e1 = mapping.tForU(ext.durationS)
        let lo = min(0, e0)
        let hi = max(cam.durationS, e1)
        return lo < hi ? lo...hi : 0...1
    }
}

private let stagePad: CGFloat = 14

struct WaveformStage: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        // Bounded stage: loupes ~2.4× the overview height; extra window height
        // goes to the strip above (owner 2026-06-12, supersedes stage-greedy).
        let loupeHeight: CGFloat = pair.loupeLayout == .grid
            ? 230
            : CGFloat(pair.anchors.count) * 116 + CGFloat(max(pair.anchors.count - 1, 0)) * 8
        VStack(spacing: 0) {
            OverviewStrip()
                .frame(height: 96)
                .padding(.horizontal, stagePad)
            if pair.loupeLayout == .grid {
                ConnectorLayer()
                    .frame(height: 24)
                    .padding(.horizontal, stagePad)
            } else {
                Spacer().frame(height: 8)
            }
            LoupeArea()
                .frame(height: loupeHeight)
                .padding(.horizontal, stagePad)
                .padding(.bottom, 6)
        }
    }
}

// MARK: - Overview strip

struct OverviewStrip: View {
    @Environment(AppModel.self) private var model
    @State private var camBuf = BandBuffer(capacity: 4096)
    @State private var extBuf = BandBuffer(capacity: 4096)

    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .topLeading) {
                lanes(size: geo.size)
                coarseDragSurface(size: geo.size)
                pins(size: geo.size)
                chips
                laneLabels
            }
            .coordinateSpace(name: "overview")
        }
        .clipShape(RoundedRectangle(cornerRadius: 8))
    }

    private func lanes(size: CGSize) -> some View {
        let pair = model.pair
        return Canvas { ctx, sz in
            let t0 = CACurrentMediaTime()
            defer { FrameStats.shared.record("overview", seconds: CACurrentMediaTime() - t0) }

            ctx.fill(Path(CGRect(origin: .zero, size: sz)), with: .color(Theme.canvasBg))
            guard let cam = pair.camera.track, let ext = pair.external.track else { return }
            let domain = pair.stageDomain
            let d0 = domain.lowerBound
            let d1 = domain.upperBound
            let tickH: CGFloat = 14
            let waveH = sz.height - tickH
            let laneH = waveH / 2
            let cols = min(Int(sz.width), 2048)
            guard cols > 8 else { return }
            let colW = sz.width / CGFloat(cols)

            // Overlap highlight (subtle, behind the lanes).
            if let ov = pair.overlap {
                let x0 = CGFloat((ov.lowerBound - d0) / (d1 - d0)) * sz.width
                let x1 = CGFloat((ov.upperBound - d0) / (d1 - d0)) * sz.width
                ctx.fill(Path(CGRect(x: x0, y: 0, width: x1 - x0, height: waveH)),
                         with: .color(.white.opacity(0.045)))
            }

            // Camera lane (top): band over the full domain; core zero-fills
            // columns outside the track.
            if cam.band(t0: d0, t1: d1, nCols: cols, into: camBuf) {
                drawLane(ctx, buf: camBuf, cols: cols, colW: colW,
                         midY: laneH / 2, halfH: laneH / 2 - 3,
                         gain: 1.44 / max(cam.meta.stats.activeP959600, 1e-6),
                         normalization: cam.meta.normalization,
                         color: Theme.camera)
            }
            // External lane (bottom): drawn through the current mapping — the
            // u-range is affine in t, so one band query stays column-aligned.
            let mapping = pair.mapping
            if ext.band(t0: mapping.u(d0), t1: mapping.u(d1), nCols: cols, into: extBuf) {
                drawLane(ctx, buf: extBuf, cols: cols, colW: colW,
                         midY: laneH + laneH / 2, halfH: laneH / 2 - 3,
                         gain: 1.44 / max(ext.meta.stats.activeP959600, 1e-6),
                         normalization: ext.meta.normalization,
                         color: Theme.external)
            }

            drawTicks(ctx, size: sz, d0: d0, d1: d1, y: waveH, h: tickH)
        }
    }

    private func drawLane(_ ctx: GraphicsContext, buf: BandBuffer, cols: Int, colW: CGFloat,
                          midY: CGFloat, halfH: CGFloat, gain: Double, normalization: Double,
                          color: Color) {
        var path = Path()
        // The UX §4.1 gain applies to envelope *spans*; min/max amplitudes are
        // half a span, hence gain/2 (typical loud content ≈ 72% of lane height).
        let scale = 0.5 * gain * normalization / 127.0
        for i in 0..<min(cols, buf.count) {
            let vMin = max(-1, min(1, Double(buf.mins[i]) * scale))
            let vMax = max(-1, min(1, Double(buf.maxs[i]) * scale))
            guard vMax - vMin > 1e-4 else { continue }
            let yTop = midY - CGFloat(vMax) * halfH
            let yBot = midY - CGFloat(vMin) * halfH
            path.addRect(CGRect(x: CGFloat(i) * colW, y: yTop,
                                width: colW, height: max(yBot - yTop, 0.7)))
        }
        ctx.fill(path, with: .color(color.opacity(0.85)))
    }

    private func drawTicks(_ ctx: GraphicsContext, size: CGSize, d0: Double, d1: Double,
                           y: CGFloat, h: CGFloat) {
        let span = d1 - d0
        guard span > 0 else { return }
        let pxPerSec = size.width / CGFloat(span)
        let candidates: [Double] = [15, 30, 60, 120, 300, 600, 1200]
        let step = candidates.first { CGFloat($0) * pxPerSec >= 56 } ?? 1800
        var t = (d0 / step).rounded(.down) * step
        var ticks = Path()
        while t <= d1 {
            if t >= d0 {
                let x = CGFloat((t - d0) / span) * size.width
                ticks.move(to: CGPoint(x: x, y: y))
                ticks.addLine(to: CGPoint(x: x, y: y + 4))
                ctx.draw(Text(Fmt.hmmss(t))
                    .font(.system(size: 8, design: .monospaced))
                    .foregroundColor(Theme.textTertiary),
                         at: CGPoint(x: x + 3, y: y + 9), anchor: .leading)
            }
            t += step
        }
        ctx.stroke(ticks, with: .color(Theme.textTertiary.opacity(0.6)), lineWidth: 1)
    }

    /// Coarse offset drag on the external lane — lower half of the strip
    /// (UX §2.4, the ballpark/rescue gesture). Seconds-per-pixel scale.
    private func coarseDragSurface(size: CGSize) -> some View {
        let pair = model.pair
        let waveH = size.height - 14
        return Color.clear
            .contentShape(Rectangle())
            .frame(height: waveH / 2)
            .offset(y: waveH / 2)
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { g in
                        guard pair.inputsEnabled else { return }
                        let domain = pair.stageDomain
                        let msPerPx = (domain.upperBound - domain.lowerBound) / size.width * 1000
                        let dx = g.translation.width - lastCoarse
                        lastCoarse = g.translation.width
                        // Content follows the pointer.
                        pair.setOffsetLive(pair.offsetMs - dx * msPerPx)
                    }
                    .onEnded { _ in
                        lastCoarse = 0
                    }
            )
    }

    @State private var lastCoarse: CGFloat = 0

    private func pins(size: CGSize) -> some View {
        let pair = model.pair
        let domain = pair.stageDomain
        let span = domain.upperBound - domain.lowerBound
        let waveH = size.height - 14
        return ForEach(pair.anchors.indices, id: \.self) { i in
            let x = CGFloat((pair.anchors[i] - domain.lowerBound) / span) * size.width
            AnchorPin(index: i,
                      color: Theme.anchorColors[i % 4],
                      stemHeight: waveH,
                      secondsPerPoint: span / size.width)
                .position(x: x, y: waveH / 2)
        }
    }

    private var chips: some View {
        let pair = model.pair
        return HStack(spacing: 6) {
            overlapChip
            Spacer()
            Picker("", selection: Binding(
                get: { pair.loupeLayout },
                set: { pair.setLoupeLayout($0) }
            )) {
                Text("Side-by-side").tag(LoupeLayout.grid)
                Text("Stacked").tag(LoupeLayout.stack)
            }
            .pickerStyle(.segmented)
            .controlSize(.mini)
            .frame(width: 150)
            Button {
                pair.toggleLockZoom()
            } label: {
                Label("lock zoom", systemImage: pair.lockZoom ? "lock.fill" : "lock.open")
                    .font(.system(size: 9))
                    .foregroundStyle(pair.lockZoom ? Theme.accent : Theme.textSecondary)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 3)
                    .background(Theme.chipBg, in: Capsule())
            }
            .buttonStyle(.plain)
            Button {
                pair.addAnchor()
            } label: {
                Label("anchor", systemImage: "plus")
                    .font(.system(size: 9))
                    .foregroundStyle(pair.anchors.count < 4 ? Theme.textSecondary : Theme.textTertiary)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 3)
                    .background(Theme.chipBg, in: Capsule())
            }
            .buttonStyle(.plain)
            .disabled(pair.anchors.count >= 4 || !pair.inputsEnabled)
        }
        .padding(6)
    }

    @ViewBuilder
    private var overlapChip: some View {
        let chip = model.pair.overlapChip
        let (text, color): (String, Color) = {
            switch chip {
            case .noAlignment: return ("no alignment", Theme.warn)
            case .noOverlap: return ("no overlap", Theme.warn)
            case .overlap(let f, let s):
                return ("overlap \(Int((f * 100).rounded()))% · \(Fmt.hmmss(s))", Theme.good)
            }
        }()
        Text(text)
            .font(.system(size: 10, weight: .medium, design: .monospaced))
            .foregroundStyle(color)
            .padding(.horizontal, 7)
            .padding(.vertical, 2.5)
            .background(color.opacity(0.14), in: Capsule())
    }

    private var laneLabels: some View {
        let waveH: CGFloat = 96 - 14
        return VStack(alignment: .leading, spacing: 0) {
            Text("V · CAMERA")
                .font(.system(size: 8, weight: .bold, design: .monospaced))
                .foregroundStyle(Theme.camera.opacity(0.7))
                .padding(4)
            Spacer().frame(height: waveH / 2 - 22)
            Text("A · EXTERNAL")
                .font(.system(size: 8, weight: .bold, design: .monospaced))
                .foregroundStyle(Theme.external.opacity(0.7))
                .padding(4)
            Spacer()
        }
        .allowsHitTesting(false)
    }
}

private struct AnchorPin: View {
    @Environment(AppModel.self) private var model
    let index: Int
    let color: Color
    let stemHeight: CGFloat
    let secondsPerPoint: Double

    @State private var lastDragX: CGFloat = 0

    var body: some View {
        pinBody
            .gesture(
                DragGesture(minimumDistance: 1)
                    .onChanged { g in
                        let pair = model.pair
                        guard pair.inputsEnabled, pair.anchors.indices.contains(index) else { return }
                        let dx = g.translation.width - lastDragX
                        lastDragX = g.translation.width
                        // Precision drag: slow pointer movement gets a finer
                        // gain so the pin can land exactly where intended;
                        // fast movement passes through 1:1.
                        let gain = min(1.0, 0.25 + abs(dx) / 12.0)
                        pair.moveAnchor(index, to: pair.anchors[index] + Double(dx) * gain * secondsPerPoint)
                    }
                    .onEnded { _ in
                        lastDragX = 0
                        // Stays exactly where placed (no transient snap —
                        // owner 2026-06-12); ⌖ in the loupe header snaps.
                        model.pair.releaseAnchor(index)
                    }
            )
    }

    private var pinBody: some View {
        ZStack {
            Rectangle()
                .fill(color.opacity(0.8))
                .frame(width: 1.5, height: stemHeight)
            VStack {
                Text("\(index + 1)")
                    .font(.system(size: 9, weight: .bold))
                    .foregroundStyle(.black.opacity(0.8))
                    .frame(width: 14, height: 14)
                    .background(color, in: RoundedRectangle(cornerRadius: 3))
                Spacer()
            }
            .frame(height: stemHeight)
        }
        .frame(width: 16) // generous hit target
        .contentShape(Rectangle())
    }
}

// MARK: - Connector layer (side-by-side only, DECIDED)

struct ConnectorLayer: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        Canvas { ctx, size in
            guard pair.loupeLayout == .grid, !pair.anchors.isEmpty else { return }
            let domain = pair.stageDomain
            let span = domain.upperBound - domain.lowerBound
            let n = pair.anchors.count
            let spacing: CGFloat = 8
            let loupeW = (size.width - CGFloat(n - 1) * spacing) / CGFloat(n)
            for i in 0..<n {
                let xPin = CGFloat((pair.anchors[i] - domain.lowerBound) / span) * size.width
                let xLoupe = CGFloat(i) * (loupeW + spacing) + loupeW / 2
                var path = Path()
                path.move(to: CGPoint(x: xPin, y: 0))
                path.addCurve(to: CGPoint(x: xLoupe, y: size.height),
                              control1: CGPoint(x: xPin, y: size.height * 0.6),
                              control2: CGPoint(x: xLoupe, y: size.height * 0.3))
                ctx.stroke(path, with: .color(Theme.anchorColors[i % 4].opacity(0.55)), lineWidth: 1.2)
            }
        }
        .allowsHitTesting(false)
    }
}

// MARK: - Loupe area: equal columns (side-by-side) or stacked rows

struct LoupeArea: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        Group {
            if pair.loupeLayout == .grid {
                HStack(spacing: 8) {
                    ForEach(pair.anchors.indices, id: \.self) { i in
                        LoupeView(index: i)
                    }
                }
            } else {
                VStack(spacing: 8) {
                    ForEach(pair.anchors.indices, id: \.self) { i in
                        LoupeView(index: i)
                    }
                }
            }
        }
    }
}
