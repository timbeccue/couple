// Queue rail (left column, §V2.1/§V2.2/§V2.7): the session header, the couple
// list in three live workflow sections (QUEUED FOR EXPORT / TO APPROVE / TO FIX,
// not a sort control — couples migrate between them as they're approved/repaired),
// and the media pool generalizing the v1 unused tray (every imported file with a
// usage indicator + audio preview + drag-to-re-pair). Nothing the user dropped
// ever silently disappears.

import AVFoundation
import CoupleKit
import SwiftUI

struct QueueRail: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    QueueSection(title: "QUEUED FOR EXPORT", couples: model.queued)
                    QueueSection(title: "TO APPROVE", couples: model.toApprove)
                    QueueSection(title: "TO FIX", couples: model.toFix)
                    MediaPoolSection()
                }
                .padding(.horizontal, 12)
                .padding(.bottom, 14)
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .background(Theme.panelBg)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("COUPLE")
                .font(.system(size: 13, weight: .bold, design: .rounded))
                .kerning(3)
                .foregroundStyle(Theme.textPrimary)
            HStack(spacing: 6) {
                Text("\(model.pool.videos.count) video · \(model.pool.audios.count) audio")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textSecondary)
                if model.resolving {
                    ProgressView().controlSize(.mini).scaleEffect(0.7)
                }
            }
            Text("Approve to queue · \(model.approvedCount)/\(model.couples.count) approved")
                .font(.system(size: 10, design: .monospaced))
                .foregroundStyle(Theme.textTertiary)
        }
        .padding(.horizontal, 12)
        .padding(.top, 26) // traffic-light clearance
        .padding(.bottom, 12)
    }
}

// MARK: - Workflow section

private struct QueueSection: View {
    @Environment(AppModel.self) private var model
    let title: String
    let couples: [CoupleRow]

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                sectionLabel(title)
                Text("\(couples.count)")
                    .font(.system(size: 9, weight: .bold, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
            }
            if couples.isEmpty {
                Text("—")
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textTertiary)
                    .padding(.vertical, 2)
            } else {
                ForEach(couples) { couple in
                    CoupleRowView(couple: couple)
                }
            }
        }
        .opacity(couples.isEmpty ? 0.45 : 1) // empty sections stay visible, dimmed
    }
}

// MARK: - One couple row

struct CoupleRowView: View {
    @Environment(AppModel.self) private var model
    let couple: CoupleRow

    private var selected: Bool { model.selectedID == couple.id }

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            VStack(spacing: 3) {
                VideoThumbnail(url: couple.video.url)
                    .frame(width: 54, height: 32)
                    .clipShape(RoundedRectangle(cornerRadius: 4))
                coverageStrip
                    .frame(width: 54)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(couple.video.name)
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
                Text(audioLine)
                    .font(.system(size: 10))
                    .foregroundStyle(audioLineColor)
                    .lineLimit(1).truncationMode(.middle)
            }
            Spacer(minLength: 4)
            VStack(alignment: .trailing, spacing: 6) {
                VerdictDot(verdict: couple.verdict)
                if couple.pair.session != nil {
                    approveBox
                } else if couple.pinned {
                    Image(systemName: "pin.fill")
                        .font(.system(size: 9)).foregroundStyle(Theme.textTertiary)
                }
            }
        }
        .padding(8)
        .background(selected ? Theme.accent.opacity(0.14) : Theme.chipBg.opacity(0.5),
                    in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7)
            .strokeBorder(selected ? Theme.accent.opacity(0.6) : .clear))
        .contentShape(Rectangle())
        .onTapGesture { model.select(couple.id) }
        // Drag an audio file from the pool onto a couple to re-pair (§V2.6).
        .dropDestination(for: String.self) { items, _ in
            guard let key = items.first else { return false }
            model.repair(couple, audioKey: key)
            return true
        }
    }

    private var audioLine: String {
        if let word = couple.actionWord { return word }
        return couple.selectedCandidate?.audioName ?? "—"
    }

    private var audioLineColor: Color {
        couple.actionWord != nil ? Theme.verdictColor(couple.verdict) : Theme.textSecondary
    }

    /// Thin amber bar under the thumbnail (§V2.2): the span the audio covers.
    /// Amber = the external-audio identity; full coverage = full bar; coverage is
    /// never a quality penalty.
    @ViewBuilder
    private var coverageStrip: some View {
        if let cov = couple.selectedCandidate?.coverage, couple.video.durationS > 0 {
            GeometryReader { geo in
                let dur = couple.video.durationS
                let x0 = CGFloat(cov.t0 / dur) * geo.size.width
                let w = CGFloat(cov.seconds / dur) * geo.size.width
                ZStack(alignment: .leading) {
                    Capsule().fill(Theme.external.opacity(0.12))
                    Capsule().fill(Theme.external.opacity(0.75))
                        .frame(width: max(w, 2)).offset(x: x0)
                }
            }
            .frame(height: 2.5)
        } else {
            Color.clear.frame(height: 2.5)
        }
    }

    private var approveBox: some View {
        Button {
            model.toggleApprove(couple)
        } label: {
            Image(systemName: couple.approved ? "checkmark.square.fill" : "square")
                .font(.system(size: 13))
                .foregroundStyle(couple.approved ? Theme.good : Theme.textSecondary)
        }
        .buttonStyle(.plain)
        .help("Approve — queue this couple for export (v)")
    }
}

struct VerdictDot: View {
    let verdict: Verdict
    var body: some View {
        Circle()
            .fill(verdict == .none ? Color.clear : Theme.verdictColor(verdict))
            .frame(width: 9, height: 9)
            .overlay(Circle().strokeBorder(Theme.verdictColor(verdict),
                                           lineWidth: verdict == .none ? 1.2 : 0))
    }
}

// MARK: - Media pool (§V2.7)

private struct MediaPoolSection: View {
    @Environment(AppModel.self) private var model
    @State private var collapsed = false
    @State private var importerShown = false

    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            HStack(spacing: 6) {
                Button {
                    collapsed.toggle()
                } label: {
                    HStack(spacing: 4) {
                        Image(systemName: collapsed ? "chevron.right" : "chevron.down")
                            .font(.system(size: 8))
                        sectionLabel("MEDIA POOL · \(model.pool.entries.count)")
                    }
                }
                .buttonStyle(.plain)
                Spacer()
                Button {
                    importerShown = true
                } label: {
                    Label("import", systemImage: "plus")
                        .font(.system(size: 9))
                        .foregroundStyle(Theme.textSecondary)
                }
                .buttonStyle(.plain)
                .fileImporter(isPresented: $importerShown,
                              allowedContentTypes: [.movie, .audio, .audiovisualContent, .folder],
                              allowsMultipleSelection: true) { result in
                    if case .success(let urls) = result { model.importDropped(urls) }
                }
            }
            if !collapsed {
                ForEach(model.pool.entries) { entry in
                    PoolEntryRow(entry: entry)
                }
            }
        }
    }
}

private struct PoolEntryRow: View {
    @Environment(AppModel.self) private var model
    let entry: MediaPool.Entry

    var body: some View {
        HStack(spacing: 7) {
            Circle().fill(entry.role == .video ? Theme.camera : Theme.external)
                .frame(width: 6, height: 6)
            VStack(alignment: .leading, spacing: 1) {
                Text(entry.name)
                    .font(.system(size: 10))
                    .foregroundStyle(Theme.textPrimary)
                    .lineLimit(1).truncationMode(.middle)
                Text(usage)
                    .font(.system(size: 8.5, design: .monospaced))
                    .foregroundStyle(Theme.textTertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 2)
            if entry.role == .audio {
                PoolPreviewButton(url: entry.url)
            }
        }
        .padding(.vertical, 3)
        .padding(.horizontal, 4)
        .contentShape(Rectangle())
        // Drag an audio entry onto a couple to re-pair (§V2.6); payload = key.
        .draggable(entry.role == .audio ? entry.id : "")
    }

    /// Live usage indicator (§V2.7): "take N" / "alt" / "unused".
    private var usage: String {
        if entry.role == .video {
            if let c = model.couples.first(where: { $0.id == entry.id }) {
                return "video · \(Fmt.hmmss(entry.durationS)) · \(c.state.chip)"
            }
            return "video · \(Fmt.hmmss(entry.durationS))"
        }
        let selectedBy = model.couples.filter { $0.selectedAudioKey == entry.id }
        if !selectedBy.isEmpty {
            let names = selectedBy.map(\.video.name).joined(separator: ", ")
            return "in \(selectedBy.count) couple\(selectedBy.count == 1 ? "" : "s") · \(names)"
        }
        let altOf = model.couples.filter { c in c.alternates.contains { $0.audioKey == entry.id } }
        if !altOf.isEmpty { return "alt · \(altOf.count) couple\(altOf.count == 1 ? "" : "s")" }
        return "unused"
    }
}

/// Low-key inline audio preview (§V2.7): a tiny play/stop toggle on a shared
/// AVPlayer (the real app may use Quick Look). One preview at a time.
private struct PoolPreviewButton: View {
    let url: URL
    @State private var playing = false

    var body: some View {
        Button {
            if playing {
                PoolPreview.shared.stop()
                playing = false
            } else {
                PoolPreview.shared.play(url) { playing = false }
                playing = true
            }
        } label: {
            Image(systemName: playing ? "stop.fill" : "play.fill")
                .font(.system(size: 8))
                .foregroundStyle(Theme.textSecondary)
                .frame(width: 18, height: 14)
                .background(Theme.chipBg, in: RoundedRectangle(cornerRadius: 3))
        }
        .buttonStyle(.plain)
        .help("Preview audio")
    }
}

@MainActor
final class PoolPreview {
    static let shared = PoolPreview()
    private var player: AVPlayer?
    private var onStop: (() -> Void)?
    private var endObserver: NSObjectProtocol?

    func play(_ url: URL, onStop: @escaping () -> Void) {
        stop()
        let p = AVPlayer(url: url)
        player = p
        self.onStop = onStop
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: p.currentItem, queue: .main
        ) { [weak self] _ in MainActor.assumeIsolated { self?.stop() } }
        p.play()
    }

    func stop() {
        player?.pause()
        player = nil
        if let o = endObserver { NotificationCenter.default.removeObserver(o); endObserver = nil }
        onStop?()
        onStop = nil
    }
}

// MARK: - Video thumbnail (async frame extraction)

struct VideoThumbnail: View {
    let url: URL
    @State private var image: NSImage?

    var body: some View {
        ZStack {
            Rectangle().fill(Theme.canvasBg)
            if let image {
                Image(nsImage: image).resizable().scaledToFill()
            } else {
                Image(systemName: "film")
                    .font(.system(size: 12))
                    .foregroundStyle(Theme.textTertiary)
            }
        }
        .task(id: url) { await loadThumbnail() }
    }

    private func loadThumbnail() async {
        let img = await Task.detached(priority: .utility) { () -> NSImage? in
            let asset = AVURLAsset(url: url)
            let gen = AVAssetImageGenerator(asset: asset)
            gen.appliesPreferredTrackTransform = true
            gen.maximumSize = CGSize(width: 160, height: 160)
            let time = CMTime(seconds: 1.0, preferredTimescale: 600)
            guard let cg = try? gen.copyCGImage(at: time, actualTime: nil) else { return nil }
            return NSImage(cgImage: cg, size: .zero)
        }.value
        image = img
    }
}
