// Batch export coordinator (SPEC-V2 §6 / §V2.8). Serial export jobs over the
// approved couples, disk preflight, per-row status, continue-past-failure. Each
// per-couple job is the unchanged Phase-4 pipeline, so the output is
// byte-identical to the CLI export for the same state (the Phase-6 hash keystone,
// now asserted across a batch).

import AppKit
import CoupleKit
import Foundation

@MainActor
@Observable
final class BatchExport {
    unowned let model: AppModel

    enum Phase: Equatable {
        case idle
        case preflight
        case running(done: Int, total: Int)
        case finished(exported: Int, failed: Int)
    }

    var phase: Phase = .idle
    var lastError: String?

    init(model: AppModel) {
        self.model = model
    }

    var isRunning: Bool {
        if case .running = phase { return true }
        if case .preflight = phase { return true }
        return false
    }

    /// Export every approved couple, serially, into the destination folder.
    /// Disk preflight sums the queue first; per-couple status updates live; a
    /// failed couple records its error and the batch proceeds (continue-past-
    /// failure); a closing toast reports "n exported, m failed".
    func run() {
        guard !isRunning else { return }
        guard let dest = model.exportDestination else { return }
        let couples = model.queued
        guard !couples.isEmpty else { return }
        phase = .preflight
        lastError = nil

        Task {
            // Disk preflight: stream-copy mux ≈ the source video size; WAV is
            // small. Compare the sum against the destination volume's free space.
            if let shortfall = diskShortfall(couples: couples, dest: dest) {
                phase = .idle
                model.toasts.show(Toast(style: .error, title: "Not enough disk space",
                                        detail: shortfall))
                return
            }

            var usedNames = Set<String>()
            var exported = 0
            var failed = 0
            var firstError: String?
            for couple in couples { couple.exportStatus = .queued }
            phase = .running(done: 0, total: couples.count)

            for (i, couple) in couples.enumerated() {
                let url = uniqueURL(dest: dest, couple: couple, used: &usedNames)
                couple.exportStatus = .running(0)
                do {
                    try await couple.pair.exportAwait(
                        to: url,
                        format: model.globalFormat,
                        keepCameraAudio: model.globalKeepCameraAudio,
                        onProgress: { couple.exportStatus = .running($0) })
                    couple.exportStatus = .done(url)
                    exported += 1
                } catch {
                    let msg = (error as? LocalizedError)?.errorDescription ?? "\(error)"
                    couple.exportStatus = .failed(msg)
                    if firstError == nil { firstError = msg }
                    failed += 1
                }
                phase = .running(done: i + 1, total: couples.count)
            }

            model.saveQueue() // persist per-couple status/approval before announcing done
            phase = .finished(exported: exported, failed: failed)
            lastError = firstError
            let title = failed == 0
                ? "Exported \(exported) file\(exported == 1 ? "" : "s")"
                : "\(exported) exported, \(failed) failed"
            model.toasts.show(Toast(
                style: failed == 0 ? .success : .warning, title: title,
                detail: failed == 0 ? dest.lastPathComponent : (firstError ?? ""),
                actionLabel: "Reveal in Finder",
                action: { NSWorkspace.shared.activateFileViewerSelecting([dest]) }), seconds: 12)
        }
    }

    /// Output URL for a couple, avoiding collisions with this batch's earlier
    /// outputs and existing files (append _2, _3, …).
    private func uniqueURL(dest: URL, couple: CoupleRow, used: inout Set<String>) -> URL {
        let ext = couple.ext(format: model.globalFormat)
        let stem = couple.outputStem
        var name = "\(stem).\(ext)"
        var n = 2
        while used.contains(name) || FileManager.default.fileExists(atPath: dest.appendingPathComponent(name).path) {
            name = "\(stem)_\(n).\(ext)"
            n += 1
        }
        used.insert(name)
        return dest.appendingPathComponent(name)
    }

    /// nil if there's room; otherwise a human description of the shortfall.
    private func diskShortfall(couples: [CoupleRow], dest: URL) -> String? {
        var needed: Int64 = 0
        for couple in couples {
            let fmt = model.globalFormat
            if fmt == .wav {
                needed += Int64(couple.video.durationS * 48000 * 2 * 2) // 48k stereo s16 est.
            } else if let size = try? FileManager.default
                .attributesOfItem(atPath: couple.video.url.path)[.size] as? Int64 {
                needed += size // stream copy ≈ source video size
            }
        }
        guard let free = try? dest.resourceValues(forKeys: [.volumeAvailableCapacityForImportantUsageKey])
            .volumeAvailableCapacityForImportantUsage else { return nil }
        // Keep a 5% headroom margin.
        if Int64(Double(needed) * 1.05) > free {
            let gb = { (b: Int64) in String(format: "%.1f GB", Double(b) / 1e9) }
            return "need ≈ \(gb(needed)), \(gb(free)) free on \(dest.lastPathComponent)"
        }
        return nil
    }
}
