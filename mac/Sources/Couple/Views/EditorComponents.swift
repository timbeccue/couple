// Reusable editor components shared by the v2 shell — the v1 editor's video hero,
// transport HUD, sync-tool readouts, and the AVPlayerLayer host / fullscreen
// window, lifted out of the old WorkflowStrip so the three-column shell can place
// them. Behavior is v1-unchanged (UX §1.4/§2.7/§11.4); only the layout moved.

import AVFoundation
import CoupleKit
import SwiftUI

func sectionLabel(_ s: String) -> some View {
    Text(s)
        .font(.system(size: 9, weight: .bold))
        .kerning(1.2)
        .foregroundStyle(Theme.textTertiary)
}

// MARK: - Video hero (center column top, greedy; aspect inviolable)

struct VideoHero: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        let audition = model.audition
        VStack(spacing: 6) {
            // AVPlayerLayer letterboxes internally (.resizeAspect): the video
            // never distorts under any layout change (owner, inviolable).
            PlayerLayerView(player: audition.player)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Color.black)
                .clipShape(RoundedRectangle(cornerRadius: 8))

            HStack(spacing: 8) {
                Button {
                    let start = max(0, (pair.anchors.first ?? 0.8) - 0.8)
                    audition.toggle(defaultStart: start)
                } label: {
                    Image(systemName: audition.isPlaying ? "pause.fill" : "play.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.textPrimary)
                        .frame(width: 22, height: 18)
                        .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 4))
                }
                .buttonStyle(.plain)
                .disabled(pair.session == nil)
                Text(timecode(audition.playheadT))
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(audition.isPlaying ? Theme.textPrimary : Theme.textTertiary)
                if audition.audioUnavailable {
                    Text("audio output unavailable")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.warn)
                }
                Spacer()
                ForEach(pair.anchors.indices, id: \.self) { i in
                    Button {
                        audition.jump(toAnchorAt: pair.anchors[i], index: i)
                    } label: {
                        Circle()
                            .fill(Theme.anchorColors[i % 4]
                                .opacity(audition.focusedAnchor == i ? 0.9 : 0.45))
                            .frame(width: 15, height: 15)
                            .overlay(Text("\(i + 1)").font(.system(size: 8, weight: .bold))
                                .foregroundStyle(.black.opacity(0.7)))
                    }
                    .buttonStyle(.plain)
                    .help("Play from anchor \(i + 1) (−0.8 s)")
                }
                // Monitor modes live in the HUD beside the timecode (§V2.6).
                Picker("", selection: Binding(
                    get: { audition.monitorMode },
                    set: { audition.monitorMode = $0 }
                )) {
                    ForEach(MonitorMode.allCases, id: \.self) { m in
                        Text(m.label).tag(m)
                    }
                }
                .pickerStyle(.segmented)
                .controlSize(.mini)
                .frame(width: 140)
                .disabled(pair.session == nil)
                .help("Pan L/R: camera left, external right · Overlay: both mixed")
                Button {
                    VideoFullscreen.shared.toggle(player: audition.player)
                } label: {
                    Image(systemName: "arrow.up.left.and.arrow.down.right")
                        .font(.system(size: 10))
                        .foregroundStyle(Theme.textSecondary)
                        .frame(width: 22, height: 18)
                        .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 4))
                }
                .buttonStyle(.plain)
                .disabled(pair.session == nil)
                .help("Fullscreen video (esc or double-click exits)")
            }
        }
    }

    private func timecode(_ t: Double) -> String {
        let s = max(0, t)
        return String(format: "%d:%05.2f", Int(s) / 60, s.truncatingRemainder(dividingBy: 60))
    }
}

/// AVPlayerLayer host (video rendering is shell-side, SPEC §4.7 boundary).
struct PlayerLayerView: NSViewRepresentable {
    let player: AVPlayer

    func makeNSView(context: Context) -> PlayerNSView {
        let v = PlayerNSView()
        v.playerLayer.player = player
        v.playerLayer.videoGravity = .resizeAspect
        return v
    }

    func updateNSView(_ nsView: PlayerNSView, context: Context) {
        if nsView.playerLayer.player !== player {
            nsView.playerLayer.player = player
        }
    }

    final class PlayerNSView: NSView {
        let playerLayer = AVPlayerLayer()

        init() {
            super.init(frame: .zero)
            wantsLayer = true
            layer = playerLayer
        }

        @available(*, unavailable)
        required init?(coder: NSCoder) {
            fatalError()
        }
    }
}

// MARK: - Fullscreen video (one window across N couples; shares the AVPlayer)

@MainActor
final class VideoFullscreen {
    static let shared = VideoFullscreen()
    private var window: FullscreenVideoWindow?

    func toggle(player: AVPlayer) {
        if window != nil { close() } else { open(player) }
    }

    private func open(_ player: AVPlayer) {
        guard let screen = NSApp.keyWindow?.screen ?? NSScreen.main else { return }
        let w = FullscreenVideoWindow(contentRect: screen.frame, styleMask: .borderless,
                                      backing: .buffered, defer: false)
        w.isReleasedWhenClosed = false
        w.backgroundColor = .black
        w.level = NSWindow.Level(rawValue: NSWindow.Level.mainMenu.rawValue + 1)
        w.collectionBehavior = [.fullScreenAuxiliary, .transient]
        w.onDismiss = { [weak self] in self?.close() }
        w.contentView = NSHostingView(rootView: FullscreenVideoSurface(player: player))
        NSApp.presentationOptions = [.autoHideMenuBar, .autoHideDock]
        w.makeKeyAndOrderFront(nil)
        window = w
    }

    private func close() {
        NSApp.presentationOptions = []
        window?.orderOut(nil)
        window = nil
    }
}

private final class FullscreenVideoWindow: NSWindow {
    var onDismiss: (() -> Void)?
    override var canBecomeKey: Bool { true }

    override func keyDown(with event: NSEvent) {
        if event.keyCode == 53 { onDismiss?() } else { super.keyDown(with: event) }
    }

    override func mouseDown(with event: NSEvent) {
        if event.clickCount == 2 { onDismiss?() } else { super.mouseDown(with: event) }
    }
}

private struct FullscreenVideoSurface: View {
    let player: AVPlayer

    var body: some View {
        PlayerLayerView(player: player)
            .ignoresSafeArea()
            .overlay(alignment: .bottomTrailing) {
                Text("esc exits")
                    .font(.system(size: 10, design: .monospaced))
                    .foregroundStyle(.white.opacity(0.25))
                    .padding(12)
            }
    }
}

// MARK: - Sync-tool readouts (§11.1 = on-demand disclosure, option A)

/// Offset/drift numeric readouts + nudge buttons — the manual-rescue surface that
/// appears when the bottom bar's sync tools are expanded (never deleted, never a
/// default surface). Used inline in the bottom bar.
struct SyncReadouts: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        let pair = model.pair
        HStack(alignment: .top, spacing: 18) {
            offsetGroup(pair)
            driftGroup(pair)
        }
        .disabled(!pair.inputsEnabled)
    }

    private func offsetGroup(_ pair: PairSession) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                sectionLabel("OFFSET")
                NumericField(value: pair.offsetMs, decimals: 2, width: 86) {
                    pair.commitTypedOffset($0)
                }
                Text("ms").font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
                Text(abs(pair.offsetMs) >= 1000 ? Fmt.humanOffset(ms: pair.offsetMs) : "")
                    .font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            nudgeRow(steps: [-10, -1, 1, 10], unit: "ms",
                     format: { $0 > 0 ? "+\(Int($0))" : "\(Int($0))" }) { pair.nudgeOffset($0) }
        }
    }

    private func driftGroup(_ pair: PairSession) -> some View {
        let disabled = pair.driftMode == .none
        return VStack(alignment: .leading, spacing: 4) {
            HStack(alignment: .firstTextBaseline, spacing: 5) {
                sectionLabel("DRIFT")
                NumericField(value: pair.driftPpm, decimals: 2, width: 64) {
                    pair.commitTypedDrift($0)
                }
                Text("ppm").font(.system(size: 10)).foregroundStyle(Theme.textSecondary)
                Text(driftSubline(pair)).font(.system(size: 9, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            nudgeRow(steps: [-1, -0.1, 0.1, 1], unit: "ppm",
                     format: { $0 > 0 ? "+\($0)" : "\($0)" }) { pair.nudgeDrift($0) }
        }
        .opacity(disabled ? 0.45 : 1)
    }

    private func driftSubline(_ pair: PairSession) -> String {
        if pair.driftMode == .none {
            return pair.savedDriftPpm != 0
                ? String(format: "off — measured %.2f ppm", pair.savedDriftPpm) : "off"
        }
        return String(format: "≈ %.1f ms over overlap", pair.endToEndDriftMs)
    }

    private func nudgeRow(steps: [Double], unit: String, format: @escaping (Double) -> String,
                          action: @escaping (Double) -> Void) -> some View {
        HStack(spacing: 4) {
            ForEach(steps, id: \.self) { step in
                Button(format(step)) { action(step) }
                    .buttonStyle(.bordered).controlSize(.small)
                    .font(.system(size: 10, design: .monospaced))
            }
            Text(unit).font(.system(size: 9)).foregroundStyle(Theme.textTertiary)
        }
    }
}

// MARK: - Numeric field (commit on Enter/blur; focus suppresses global keys)

struct NumericField: View {
    let value: Double
    let decimals: Int
    let width: CGFloat
    let commit: (Double) -> Void

    @State private var text = ""
    @FocusState private var focused: Bool

    var body: some View {
        TextField("", text: $text)
            .textFieldStyle(.plain)
            .font(.system(size: 13, weight: .semibold, design: .monospaced))
            .foregroundStyle(Theme.textPrimary)
            .multilineTextAlignment(.trailing)
            .frame(width: width)
            .padding(.horizontal, 5)
            .padding(.vertical, 2)
            .background(Theme.canvasBg, in: RoundedRectangle(cornerRadius: 5))
            .focused($focused)
            // Enter commits AND releases focus so the global shortcuts (space =
            // play/pause, etc.) resume instead of typing into the field. Blur
            // just commits (focus moved on its own — don't fight tab navigation).
            .onSubmit { commitText(); releaseFocus() }
            .onChange(of: focused) { _, isFocused in if !isFocused { commitText() } }
            .onChange(of: value) { _, v in if !focused { text = format(v) } }
            .onAppear { text = format(value) }
    }

    private func releaseFocus() {
        focused = false
        DispatchQueue.main.async { NSApp.keyWindow?.makeFirstResponder(nil) }
    }

    private func format(_ v: Double) -> String { String(format: "%+.\(decimals)f", v) }

    private func commitText() {
        let cleaned = text.replacingOccurrences(of: "−", with: "-")
            .replacingOccurrences(of: "+", with: "")
            .trimmingCharacters(in: .whitespaces)
        if let v = Double(cleaned) { commit(v) }
        text = format(value)
    }
}
