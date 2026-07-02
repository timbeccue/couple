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

/// The ONE x↔time mapping for the overview strip. Both the playhead scrub and the
/// anchor drag map the pointer's CURRENT x DIRECTLY through the displayed visible
/// range (absolute position, never accumulated deltas), so a given pointer x always
/// yields the same time regardless of how fast or by what path it got there. Pin and
/// connector x positions invert the same mapping, so the pin stays under the pointer.
/// `visible` is the displayed range (today `PairSession.stageDomain`); routing every
/// caller through here — instead of a hardcoded full-duration assumption — keeps the
/// strip compatible with TODO-010's future zoomed/panned overview range for free.
enum OverviewTimeline {
    /// Pointer x (in lane coordinates) → time, clamped to the visible range. Direct
    /// and monotonic in x; degenerate width/range pins to the lower bound.
    static func time(atX x: CGFloat, width: CGFloat, visible: ClosedRange<Double>) -> Double {
        let span = visible.upperBound - visible.lowerBound
        guard width > 0, span > 0 else { return visible.lowerBound }
        let frac = max(0, min(1, Double(x / width)))
        return visible.lowerBound + frac * span
    }

    /// Inverse: time → lane x. Used to draw pins/connectors so they track the pointer.
    static func x(forTime t: Double, width: CGFloat, visible: ClosedRange<Double>) -> CGFloat {
        let span = visible.upperBound - visible.lowerBound
        guard span > 0 else { return 0 }
        return CGFloat((t - visible.lowerBound) / span) * width
    }
}

private let stagePad: CGFloat = 14

// Overview strip vertical metrics: a badge gutter above the lanes keeps the
// anchor pin heads out of the waveforms, and the tick row sits below the
// lanes. Pin stems span the full strip so they meet the connector layer.
private let badgeGutterH: CGFloat = 16
private let tickRowH: CGFloat = 14
private let overviewH: CGFloat = 112
// Volume/mute column to the right of the lanes (own space, never overlapping
// the waveforms). The connector layer subtracts the same reserve so pin x
// positions stay aligned between the strip and the loupes.
private let laneControlsW: CGFloat = 100
private let laneControlsGap: CGFloat = 8

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
                .frame(height: overviewH)
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
            let waveSize = CGSize(width: geo.size.width - laneControlsW - laneControlsGap,
                                  height: geo.size.height)
            HStack(spacing: laneControlsGap) {
                ZStack(alignment: .topLeading) {
                    lanes(size: waveSize)
                    interactionSurface(size: waveSize)
                    pins(size: waveSize)
                    chips
                    laneLabels
                }
                .coordinateSpace(name: "overview")
                .frame(width: waveSize.width)
                laneControlsColumn(height: geo.size.height)
                    .frame(width: laneControlsW)
            }
            .background(Theme.canvasBg)
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
            let waveH = sz.height - tickRowH - badgeGutterH
            let laneH = waveH / 2
            let cols = min(Int(sz.width), 2048)
            guard cols > 8 else { return }
            let colW = sz.width / CGFloat(cols)

            // Overlap highlight (subtle, behind the lanes).
            if let ov = pair.overlap {
                let x0 = CGFloat((ov.lowerBound - d0) / (d1 - d0)) * sz.width
                let x1 = CGFloat((ov.upperBound - d0) / (d1 - d0)) * sz.width
                ctx.fill(Path(CGRect(x: x0, y: badgeGutterH, width: x1 - x0, height: waveH)),
                         with: .color(.white.opacity(0.045)))
            }

            // Camera lane (top): band over the full domain; core zero-fills
            // columns outside the track. The monitor gain scales the lane so
            // what you see tracks what you'd hear; mute dims it instead.
            let audition = pair.audition
            if cam.band(t0: d0, t1: d1, nCols: cols, into: camBuf) {
                drawLane(ctx, buf: camBuf, cols: cols, colW: colW,
                         midY: badgeGutterH + laneH / 2, halfH: laneH / 2 - 3,
                         gain: 1.44 / max(cam.meta.stats.activeP959600, 1e-6) * audition.camGain,
                         normalization: cam.meta.normalization,
                         color: Theme.camera, dimmed: audition.camMuted)
            }
            // External lane (bottom): drawn through the current mapping — the
            // u-range is affine in t, so one band query stays column-aligned.
            let mapping = pair.mapping
            if ext.band(t0: mapping.u(d0), t1: mapping.u(d1), nCols: cols, into: extBuf) {
                drawLane(ctx, buf: extBuf, cols: cols, colW: colW,
                         midY: badgeGutterH + laneH + laneH / 2, halfH: laneH / 2 - 3,
                         gain: 1.44 / max(ext.meta.stats.activeP959600, 1e-6) * audition.extGain,
                         normalization: ext.meta.normalization,
                         color: Theme.external, dimmed: audition.extMuted)
            }

            drawTicks(ctx, size: sz, d0: d0, d1: d1, y: badgeGutterH + waveH, h: tickRowH)

            // Playhead (UX §1.6): always visible once a session exists —
            // bright while playing/scrubbing, grey when parked. Dragging is
            // handled by playheadDragLayer.
            if pair.session != nil {
                let x = CGFloat((audition.playheadT - d0) / (d1 - d0)) * sz.width
                let active = audition.isPlaying || audition.isScrubbing
                var line = Path()
                line.move(to: CGPoint(x: x, y: badgeGutterH))
                line.addLine(to: CGPoint(x: x, y: badgeGutterH + waveH))
                ctx.stroke(line, with: .color(.white.opacity(active ? 0.85 : 0.5)),
                           lineWidth: active ? 1 : 1.5)
                var knob = Path() // grab handle at the gutter edge
                knob.move(to: CGPoint(x: x - 4, y: badgeGutterH - 6))
                knob.addLine(to: CGPoint(x: x + 4, y: badgeGutterH - 6))
                knob.addLine(to: CGPoint(x: x, y: badgeGutterH + 2))
                knob.closeSubpath()
                ctx.fill(knob, with: .color(.white.opacity(active ? 0.85 : 0.55)))
            }
        }
    }

    private func drawLane(_ ctx: GraphicsContext, buf: BandBuffer, cols: Int, colW: CGFloat,
                          midY: CGFloat, halfH: CGFloat, gain: Double, normalization: Double,
                          color: Color, dimmed: Bool = false) {
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
        ctx.fill(path, with: .color(color.opacity(dimmed ? 0.25 : 0.85)))
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

    private enum OverviewDrag {
        case scrub, coarseOffset
    }

    @State private var dragMode: OverviewDrag?
    @State private var lastCoarse: CGFloat = 0

    /// One surface for the whole strip: click or drag anywhere moves the
    /// playhead there (audio + video follow, even while paused). ⌥-drag on
    /// the external lane keeps the coarse offset gesture (UX §2.4, the
    /// ballpark/rescue move). Anchor pins sit above this surface, so dragging
    /// a pin never scrubs. The mode latches at drag start.
    private func interactionSurface(size: CGSize) -> some View {
        let pair = model.pair
        return Color.clear
            .contentShape(Rectangle())
            .frame(width: size.width, height: size.height)
            .gesture(
                DragGesture(minimumDistance: 0, coordinateSpace: .named("overview"))
                    .onChanged { g in
                        let domain = pair.stageDomain
                        let span = domain.upperBound - domain.lowerBound
                        if dragMode == nil {
                            let waveH = size.height - tickRowH - badgeGutterH
                            let inExternalLane = g.startLocation.y > badgeGutterH + waveH / 2
                                && g.startLocation.y < badgeGutterH + waveH
                            let option = NSEvent.modifierFlags.contains(.option)
                            dragMode = option && inExternalLane && pair.inputsEnabled
                                ? .coarseOffset : .scrub
                        }
                        switch dragMode {
                        case .coarseOffset:
                            let msPerPx = span / size.width * 1000
                            let dx = g.translation.width - lastCoarse
                            lastCoarse = g.translation.width
                            // Content follows the pointer.
                            pair.setOffsetLive(pair.offsetMs - dx * msPerPx)
                        case .scrub:
                            guard pair.session != nil else { return }
                            pair.audition.isScrubbing = true
                            // Absolute pointer x → time through the displayed range
                            // (the same mapping the anchor drag uses); seek clamps.
                            pair.audition.seek(to: OverviewTimeline.time(
                                atX: g.location.x, width: size.width, visible: domain))
                        case nil:
                            break
                        }
                    }
                    .onEnded { _ in
                        dragMode = nil
                        lastCoarse = 0
                        model.pair.audition.isScrubbing = false
                    }
            )
    }

    /// Per-lane monitor volume + mute in their own column beside the lanes.
    /// The slider scales both the audio (core gains) and the drawn waveform.
    private func laneControlsColumn(height: CGFloat) -> some View {
        let waveH = height - tickRowH - badgeGutterH
        return VStack(spacing: 0) {
            Spacer().frame(height: badgeGutterH)
            LaneAudioControls(lane: .camera)
                .frame(height: waveH / 2)
            LaneAudioControls(lane: .external)
                .frame(height: waveH / 2)
            Spacer(minLength: 0)
        }
        .opacity(model.pair.session == nil ? 0 : 1)
    }

    private func pins(size: CGSize) -> some View {
        let pair = model.pair
        let domain = pair.stageDomain
        return ForEach(pair.anchors.indices, id: \.self) { i in
            let x = OverviewTimeline.x(forTime: pair.anchors[i].t, width: size.width, visible: domain)
            AnchorPin(index: i,
                      color: Theme.anchorColors[i % 4],
                      height: size.height,
                      laneWidth: size.width,
                      visible: domain)
                .position(x: x, y: size.height / 2)
        }
    }

    private var chips: some View {
        let pair = model.pair
        return HStack(spacing: 6) {
            overlapChip
            if pair.anchorsUnavailable {
                // TODO-011 manual-rescue state: automatic proposal placed no anchor
                // because there is no usable overlap (or no two non-degenerate
                // candidates). Explain why, instead of inserting misleading midpoints.
                Text("place anchors manually")
                    .font(.system(size: 10, weight: .medium))
                    .foregroundStyle(Theme.warn)
                    .padding(.horizontal, 7)
                    .padding(.vertical, 2.5)
                    .background(Theme.warn.opacity(0.14), in: Capsule())
                    .help("Anchors need shared content in both tracks. There isn't enough overlap to propose them automatically — drag the external lane to align, or add anchors by hand.")
            }
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
        let waveH: CGFloat = overviewH - tickRowH - badgeGutterH
        return VStack(alignment: .leading, spacing: 0) {
            Spacer().frame(height: badgeGutterH)
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

private struct LaneAudioControls: View {
    @Environment(AppModel.self) private var model
    let lane: SlotKind

    var body: some View {
        let audition = model.pair.audition
        @Bindable var a = audition
        let muted = lane == .camera ? audition.camMuted : audition.extMuted
        let tint = lane == .camera ? Theme.camera : Theme.external
        HStack(spacing: 6) {
            Button {
                if lane == .camera {
                    audition.camMuted.toggle()
                } else {
                    audition.extMuted.toggle()
                }
            } label: {
                // Reads as a button: same chip treatment as the transport
                // controls, with a filled warn state while muted.
                Image(systemName: muted ? "speaker.slash.fill" : "speaker.wave.2.fill")
                    .font(.system(size: 9))
                    .foregroundStyle(muted ? Theme.warn : tint)
                    .frame(width: 24, height: 18)
                    .background(muted ? Theme.warn.opacity(0.18) : Theme.chipBg,
                                in: RoundedRectangle(cornerRadius: 4))
                    .overlay(RoundedRectangle(cornerRadius: 4)
                        .strokeBorder(muted ? Theme.warn.opacity(0.5) : Theme.panelStroke))
            }
            .buttonStyle(.plain)
            .help(muted ? "Unmute" : "Mute")
            Slider(value: lane == .camera ? $a.camGain : $a.extGain, in: 0...2)
                .controlSize(.mini)
                .frame(width: 64)
                .tint(tint.opacity(0.8))
                .disabled(muted)
                .help("Monitor volume (waveform scales to match)")
        }
    }
}

private struct AnchorPin: View {
    @Environment(AppModel.self) private var model
    let index: Int
    let color: Color
    let height: CGFloat
    // The displayed overview timeline this pin lives on (lane width + visible
    // range), so the drag maps the pointer's absolute x to a time exactly like
    // the playhead scrub — TODO-014 Part 1.
    let laneWidth: CGFloat
    let visible: ClosedRange<Double>

    var body: some View {
        pinBody
            // Clicking a pin summons the loupes and focuses this anchor (§V2.4).
            .onTapGesture {
                model.loupesShown = true
                model.audition.focusedAnchor = index
            }
            .gesture(
                // Direct 1:1 drag (TODO-014 Part 1): map the pointer's CURRENT x
                // in the shared "overview" space straight to a time through the
                // displayed range — the same absolute mapping the playhead uses,
                // not accumulated deltas. No velocity gain, so sensitivity never
                // changes mid-drag and the same x always yields the same time. The
                // pin tracks the pointer; `moveAnchor` applies the existing
                // playable-range clamp. Unrounded value; stays where released.
                DragGesture(minimumDistance: 1, coordinateSpace: .named("overview"))
                    .onChanged { g in
                        let pair = model.pair
                        guard pair.inputsEnabled, pair.anchors.indices.contains(index) else { return }
                        pair.moveAnchor(index, to: OverviewTimeline.time(
                            atX: g.location.x, width: laneWidth, visible: visible))
                    }
                    .onEnded { _ in
                        // Stays exactly where placed (no transient snap —
                        // owner 2026-06-12); ⌖ in the loupe header snaps.
                        model.pair.releaseAnchor(index)
                    }
            )
    }

    private var pinBody: some View {
        // Badge in the gutter, stem running through the lanes and tick row to
        // the strip's bottom edge, where the connector layer picks it up.
        VStack(spacing: 0) {
            Text("\(index + 1)")
                .font(.system(size: 9, weight: .bold))
                .foregroundStyle(.black.opacity(0.8))
                .frame(width: 14, height: 14)
                .background(color, in: RoundedRectangle(cornerRadius: 3))
            Rectangle()
                .fill(color.opacity(0.8))
                .frame(width: 1.5)
                .frame(maxHeight: .infinity)
        }
        .frame(width: 16, height: height) // generous hit target
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
            let n = pair.anchors.count
            let spacing: CGFloat = 8
            let loupeW = (size.width - CGFloat(n - 1) * spacing) / CGFloat(n)
            // Pin x lives in the lane area, which stops short of the volume column.
            let laneW = size.width - laneControlsW - laneControlsGap
            for i in 0..<n {
                let xPin = OverviewTimeline.x(forTime: pair.anchors[i].t, width: laneW, visible: domain)
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
