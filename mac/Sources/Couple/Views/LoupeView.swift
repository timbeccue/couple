// Loupe (UX §1.6): both tracks overlaid at one center line, additive "screen"
// blending, stepped per-bin rendering (DECIDED), per-loupe local normalization
// to 86% half-height (DECIDED), residual guidance arrow, header with local Δ.
// Below 120 ms windows the canvas switches to raw-sample rendering (SPEC §11.2).

import CoupleKit
import SwiftUI

private final class LoupeBuffers {
    let cam = BandBuffer(capacity: 2048)
    let ext = BandBuffer(capacity: 2048)
    var camRaw = [Float](repeating: 0, count: 8192)
    var extRaw = [Float](repeating: 0, count: 8192)
}

struct LoupeView: View {
    @Environment(AppModel.self) private var model
    let index: Int

    @State private var buffers = LoupeBuffers()
    @State private var lastDragX: CGFloat = 0

    private var accent: Color { Theme.anchorColors[index % 4] }

    var body: some View {
        let pair = model.pair
        VStack(spacing: 4) {
            header
            canvas
        }
        .padding(6)
        .background(Theme.panelBg, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(accent.opacity(0.45)))
        .opacity(pair.inputsEnabled ? 1 : 0.6)
    }

    // MARK: Header

    private var header: some View {
        let pair = model.pair
        let winMs = pair.frames.indices.contains(index) ? pair.frames[index].winMs : 240
        let delta = pair.displayedDelta(index)
        return HStack(spacing: 6) {
            Text("\(index + 1)")
                .font(.system(size: 8, weight: .bold))
                .foregroundStyle(.black.opacity(0.8))
                .frame(width: 12, height: 12)
                .background(accent, in: RoundedRectangle(cornerRadius: 3))
            Text(Fmt.hmmss(pair.anchors.indices.contains(index) ? pair.anchors[index] : 0))
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textSecondary)
            if let delta {
                Text(String(format: "local Δ %+.2f ms", delta))
                    .font(.system(size: 10, weight: .medium, design: .monospaced))
                    .foregroundStyle(Theme.deltaColor(absMs: abs(delta)))
            } else {
                Text("local Δ —")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            Text(winMs >= 2 ? String(format: "±%.0fms", winMs / 2) : String(format: "±%.1fms", winMs / 2))
                .font(.system(size: 9, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
            Button {
                pair.armAuto(loupe: index)
            } label: {
                Text("auto")
                    .font(.system(size: 8, weight: .semibold))
                    .foregroundStyle(isAuto ? Theme.accent : Theme.textTertiary)
                    .padding(.horizontal, 4)
                    .padding(.vertical, 1.5)
                    .background(isAuto ? Theme.accent.opacity(0.15) : Theme.chipBg, in: Capsule())
            }
            .buttonStyle(.plain)

            Spacer(minLength: 2)

            headerButton("⌖", help: "Re-snap to nearest transient") {
                pair.snapAnchor(index)
            }
            headerButton("⇥", help: "Align: zero this anchor's Δ") {
                pair.alignAnchor(index)
            }
            .disabled(delta == nil)
            headerButton("×", help: "Remove anchor") {
                model.loupeRects[index] = nil
                pair.removeAnchor(index)
            }
            .disabled(pair.anchors.count <= 2)
        }
        .lineLimit(1)
    }

    private var isAuto: Bool {
        let pair = model.pair
        return pair.frames.indices.contains(index) && pair.frames[index].auto
    }

    private func headerButton(_ label: String, help: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Text(label)
                .font(.system(size: 10))
                .foregroundStyle(Theme.textSecondary)
                .frame(width: 16, height: 14)
                .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 3))
        }
        .buttonStyle(.plain)
        .help(help)
    }

    // MARK: Canvas

    private var canvas: some View {
        GeometryReader { geo in
            Canvas { ctx, size in
                let t0 = CACurrentMediaTime()
                defer { FrameStats.shared.record("loupe\(index)", seconds: CACurrentMediaTime() - t0) }
                draw(ctx: ctx, size: size)
            }
            .onAppear {
                model.loupeRects[index] = geo.frame(in: .global)
            }
            .onChange(of: geo.frame(in: .global)) { _, frame in
                model.loupeRects[index] = frame
            }
        }
        .background(Theme.canvasBg, in: RoundedRectangle(cornerRadius: 5))
        .clipShape(RoundedRectangle(cornerRadius: 5))
        .contentShape(Rectangle())
        .gesture(dragGesture)
    }

    /// Plain drag slides offset (ms/px follows zoom); ⇧-drag is the pivot drift
    /// drag (UX §2.4). Content follows the pointer in both.
    private var dragGesture: some Gesture {
        let pair = model.pair
        return DragGesture(minimumDistance: 1)
            .onChanged { g in
                guard pair.inputsEnabled,
                      pair.frames.indices.contains(index),
                      let rect = model.loupeRects[index], rect.width > 0 else { return }
                let dx = g.translation.width - lastDragX
                lastDragX = g.translation.width
                let msPerPx = pair.frames[index].winMs / rect.width
                let deltaL = -dx * msPerPx // content follows pointer (§2.1 sign)
                if NSEvent.modifierFlags.contains(.shift) {
                    pair.pivotDrag(anchorIndex: index, deltaLMs: deltaL)
                } else {
                    pair.setOffsetLive(pair.offsetMs + deltaL)
                }
            }
            .onEnded { _ in
                lastDragX = 0
                pair.commitDrift()
            }
    }

    private func draw(ctx: GraphicsContext, size: CGSize) {
        let pair = model.pair
        guard let cam = pair.camera.track, let ext = pair.external.track,
              pair.anchors.indices.contains(index),
              pair.frames.indices.contains(index) else { return }
        let anchor = pair.anchors[index]
        let winS = pair.frames[index].winMs / 1000
        let t0 = anchor - winS / 2
        let t1 = anchor + winS / 2
        let mapping = pair.mapping
        let cols = min(Int(size.width), 2048)
        guard cols > 8 else { return }
        let midY = size.height / 2
        let halfH = size.height / 2 - 2

        var layer = ctx
        layer.blendMode = .screen // additive: overlap literally brightens (DECIDED)

        if winS < 0.12 {
            // Deep zoom: raw-sample fetch (SPEC §14 item 6.5).
            drawRaw(&layer, track: cam, t0: t0, t1: t1, buf: &buffers.camRaw,
                    cols: cols, size: size, midY: midY, halfH: halfH, color: Theme.camera)
            drawRaw(&layer, track: ext, t0: mapping.u(t0), t1: mapping.u(t1), buf: &buffers.extRaw,
                    cols: cols, size: size, midY: midY, halfH: halfH, color: Theme.external)
        } else {
            if cam.band(t0: t0, t1: t1, nCols: cols, into: buffers.cam) {
                drawBand(&layer, buf: buffers.cam, cols: cols, size: size,
                         midY: midY, halfH: halfH, color: Theme.camera)
            }
            if ext.band(t0: mapping.u(t0), t1: mapping.u(t1), nCols: cols, into: buffers.ext) {
                drawBand(&layer, buf: buffers.ext, cols: cols, size: size,
                         midY: midY, halfH: halfH, color: Theme.external)
            }
        }

        // Anchor center line.
        var center = Path()
        center.move(to: CGPoint(x: size.width / 2, y: 0))
        center.addLine(to: CGPoint(x: size.width / 2, y: size.height))
        ctx.stroke(center, with: .color(accent.opacity(0.65)), lineWidth: 1)

        drawGuidanceArrow(ctx, size: size)
    }

    /// Stepped per-bin columns, locally normalized to 86% of half-height (DECIDED).
    private func drawBand(_ ctx: inout GraphicsContext, buf: BandBuffer, cols: Int, size: CGSize,
                          midY: CGFloat, halfH: CGFloat, color: Color) {
        let n = min(cols, buf.count)
        guard n > 0 else { return }
        var peak: Int32 = 0
        for i in 0..<n {
            peak = max(peak, max(Int32(abs(Int(buf.mins[i]))), Int32(abs(Int(buf.maxs[i])))))
        }
        guard peak > 0 else { return }
        let scale = 0.86 * halfH / CGFloat(peak)
        let colW = size.width / CGFloat(n)
        var path = Path()
        for i in 0..<n {
            let yTop = midY - CGFloat(buf.maxs[i]) * scale
            let yBot = midY - CGFloat(buf.mins[i]) * scale
            path.addRect(CGRect(x: CGFloat(i) * colW, y: yTop,
                                width: colW, height: max(yBot - yTop, 0.7)))
        }
        ctx.fill(path, with: .color(color.opacity(0.8)))
    }

    private func drawRaw(_ ctx: inout GraphicsContext, track: TrackHandle, t0: Double, t1: Double,
                         buf: inout [Float], cols: Int, size: CGSize,
                         midY: CGFloat, halfH: CGFloat, color: Color) {
        let frames = track.rawWindow(t0: t0, t1: t1, into: &buf)
        guard frames > 8 else { return }
        var peak: Float = 0
        for i in 0..<frames {
            peak = max(peak, abs(buf[i]))
        }
        guard peak > 0 else { return }
        let scale = 0.86 * halfH / CGFloat(peak)
        let colW = size.width / CGFloat(cols)
        var path = Path()
        for c in 0..<cols {
            let a = c * frames / cols
            let b = max((c + 1) * frames / cols, a + 1)
            var lo: Float = .greatestFiniteMagnitude
            var hi: Float = -.greatestFiniteMagnitude
            for i in a..<min(b, frames) {
                lo = min(lo, buf[i])
                hi = max(hi, buf[i])
            }
            guard hi >= lo else { continue }
            let yTop = midY - CGFloat(hi) * scale
            let yBot = midY - CGFloat(lo) * scale
            path.addRect(CGRect(x: CGFloat(c) * colW, y: yTop,
                                width: colW, height: max(yBot - yTop, 0.7)))
        }
        ctx.fill(path, with: .color(color.opacity(0.8)))
    }

    /// Residual guidance arrow (UX §1.6): when |Δ| exceeds ~12% of the window,
    /// point from center toward where the matching external content sits.
    private func drawGuidanceArrow(_ ctx: GraphicsContext, size: CGSize) {
        let pair = model.pair
        guard let delta = pair.displayedDelta(index),
              pair.frames.indices.contains(index) else { return }
        let winMs = pair.frames[index].winMs
        guard abs(delta) > 0.12 * winMs else { return }
        let y: CGFloat = 16
        let cx = size.width / 2
        let dir: CGFloat = delta > 0 ? 1 : -1
        let len: CGFloat = min(size.width * 0.28, 90)
        var path = Path()
        path.move(to: CGPoint(x: cx, y: y))
        path.addLine(to: CGPoint(x: cx + dir * len, y: y))
        path.move(to: CGPoint(x: cx + dir * len, y: y))
        path.addLine(to: CGPoint(x: cx + dir * (len - 5), y: y - 4))
        path.move(to: CGPoint(x: cx + dir * len, y: y))
        path.addLine(to: CGPoint(x: cx + dir * (len - 5), y: y + 4))
        ctx.stroke(path, with: .color(Theme.warn), lineWidth: 1.5)
        let label = delta > 0
            ? String(format: "ext content → %.1f ms", abs(delta))
            : String(format: "← ext content %.1f ms", abs(delta))
        ctx.draw(Text(label)
            .font(.system(size: 9, design: .monospaced))
            .foregroundColor(Theme.warn),
                 at: CGPoint(x: cx + dir * (len / 2), y: y - 10), anchor: .center)
    }
}
