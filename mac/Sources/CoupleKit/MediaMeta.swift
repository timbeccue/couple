// Media metadata projection (TODO-003) — the PURE, testable core behind the
// media-pool info popover. The app probes a file once (`Engine.probe` →
// `MediaInfo`) and captures the on-disk size via filesystem APIs; this type turns
// that already-acquired evidence into ordered, displayable rows. It performs NO
// I/O and NO probing, so the popover never re-probes when it opens — it just
// renders the projection retained on the pool entry.
//
// CONTRACT (the testable invariants):
//   1. STRUCTURED ONLY — every technical value comes from `MediaInfo` (the probe)
//      or a filesystem scalar. Nothing is inferred from the filename.
//   2. CLEAN OMISSION — an unavailable field produces NO row (not a row with a
//      blank/`unknown`/`—` value), so there are never dangling separators or
//      fabricated values. A row exists iff its value is genuinely known.
//   3. STABLE FORMATTING — `ByteCountFormatter` for size and a fixed-locale,
//      readable formatter for the creation date (so output is deterministic and
//      not locale-dependent in tests).

import Foundation

/// The role of a pooled file, mirrored here so the projection (in CoupleKit) does
/// not depend on the app target's `MediaPool.Role`.
public enum MediaRole: String, Sendable {
    case video, audio
}

/// One label/value line in the metadata popover. `value` is always non-empty —
/// the projector only emits a row when the underlying field is available.
public struct MediaMetaRow: Sendable, Equatable, Identifiable {
    public var label: String
    public var value: String
    public var id: String { label }
    public init(label: String, value: String) {
        self.label = label
        self.value = value
    }
}

/// A grouped, ordered projection of one pooled file's metadata for display.
/// `common` always leads (role, duration, creation, size, path); `streamA`
/// carries the audio- or video-stream technical block; `streamB` carries the
/// embedded-audio summary for a video (empty otherwise). Each group omits any
/// field that is genuinely unavailable.
public struct MediaMetaCard: Sendable, Equatable {
    public var common: [MediaMetaRow]
    public var stream: [MediaMetaRow]
    /// Embedded-audio rows for a video (empty for audio files or silent video).
    public var embeddedAudio: [MediaMetaRow]

    public init(common: [MediaMetaRow], stream: [MediaMetaRow], embeddedAudio: [MediaMetaRow]) {
        self.common = common
        self.stream = stream
        self.embeddedAudio = embeddedAudio
    }

    /// Build the projection from already-acquired evidence. `durationS` is the
    /// authoritative track duration (the pool's value); `info` is the probe;
    /// `fileSizeBytes`/`creationTimeUtc` are filesystem/probe scalars (`nil` when
    /// the filesystem or container did not supply them).
    public static func make(
        role: MediaRole,
        durationS: Double,
        info: MediaInfo?,
        fileSizeBytes: UInt64?,
        creationTimeUtc: String?
    ) -> MediaMetaCard {
        var common: [MediaMetaRow] = []
        common.append(MediaMetaRow(label: "Role", value: role == .video ? "Video" : "Audio"))
        if durationS > 0 {
            common.append(MediaMetaRow(label: "Duration", value: formatDuration(durationS)))
        }
        if let created = formatCreationDate(creationTimeUtc) {
            common.append(MediaMetaRow(label: "Created", value: created))
        }
        if let size = fileSizeBytes {
            common.append(MediaMetaRow(label: "Size", value: formatBytes(size)))
        }
        // Full path always last in the common block; it is always known.
        if let path = info?.path, !path.isEmpty {
            common.append(MediaMetaRow(label: "Path", value: path))
        }

        var stream: [MediaMetaRow] = []
        var embeddedAudio: [MediaMetaRow] = []

        if role == .video {
            if let v = info?.video {
                stream.append(contentsOf: videoStreamRows(container: info?.container, video: v))
            } else if let container = nonEmpty(info?.container) {
                stream.append(MediaMetaRow(label: "Container", value: container))
            }
            // Embedded audio: a compact per-stream summary (cameras record audio,
            // so this is the common case). Multiple streams stay readable by
            // numbering them rather than exploding every field into its own row.
            embeddedAudio = embeddedAudioRows(info?.audio ?? [])
        } else {
            // Audio file: its first (primary) stream carries the technical block.
            if let a = (info?.audio ?? []).first {
                stream.append(contentsOf: audioStreamRows(container: info?.container, audio: a))
            } else if let container = nonEmpty(info?.container) {
                stream.append(MediaMetaRow(label: "Container", value: container))
            }
        }

        return MediaMetaCard(common: common, stream: stream, embeddedAudio: embeddedAudio)
    }

    // MARK: - stream blocks

    private static func videoStreamRows(container: String?, video v: VideoStreamInfo) -> [MediaMetaRow] {
        var rows: [MediaMetaRow] = []
        if let cc = formatContainerCodec(container: container, codec: v.codec) {
            rows.append(MediaMetaRow(label: "Format", value: cc))
        }
        if v.width > 0 && v.height > 0 {
            rows.append(MediaMetaRow(label: "Dimensions", value: "\(v.width)×\(v.height)"))
        }
        if let fps = formatFps(avg: v.avgFps, nominal: v.nominalFps) {
            rows.append(MediaMetaRow(label: "Frame rate", value: fps))
        }
        return rows
    }

    private static func audioStreamRows(container: String?, audio a: AudioStreamInfo) -> [MediaMetaRow] {
        var rows: [MediaMetaRow] = []
        if let cc = formatContainerCodec(container: container, codec: a.codec) {
            rows.append(MediaMetaRow(label: "Format", value: cc))
        }
        if a.sampleRate > 0 {
            rows.append(MediaMetaRow(label: "Sample rate", value: formatSampleRate(a.sampleRate)))
        }
        if a.channels > 0 {
            rows.append(MediaMetaRow(label: "Channels", value: formatChannels(a.channels)))
        }
        if let bits = a.bitsPerSample, bits > 0 {
            rows.append(MediaMetaRow(label: "Bit depth", value: "\(bits)-bit"))
        }
        return rows
    }

    /// Compact embedded-audio summary for a video. One row per stream, each a
    /// single readable line (codec · sample rate · channels · bit depth), with
    /// only the known parts joined — so a stream missing bit depth has no trailing
    /// separator. Streams are numbered only when there is more than one.
    private static func embeddedAudioRows(_ audio: [AudioStreamInfo]) -> [MediaMetaRow] {
        guard !audio.isEmpty else { return [] }
        let multiple = audio.count > 1
        var rows: [MediaMetaRow] = []
        for (i, a) in audio.enumerated() {
            var parts: [String] = []
            if let codec = nonEmpty(a.codec) { parts.append(codec) }
            if a.sampleRate > 0 { parts.append(formatSampleRate(a.sampleRate)) }
            if a.channels > 0 { parts.append(formatChannels(a.channels)) }
            if let bits = a.bitsPerSample, bits > 0 { parts.append("\(bits)-bit") }
            let value = parts.isEmpty ? "present" : parts.joined(separator: " · ")
            let label = multiple ? "Audio \(i + 1)" : "Audio"
            rows.append(MediaMetaRow(label: label, value: value))
        }
        return rows
    }

    // MARK: - formatters (deterministic; no locale leakage)

    /// `h:mm:ss` / `m:ss`, matching the pool's own `Fmt.hmmss`.
    public static func formatDuration(_ seconds: Double) -> String {
        let s = max(0, Int(seconds.rounded()))
        if s >= 3600 {
            return String(format: "%d:%02d:%02d", s / 3600, (s % 3600) / 60, s % 60)
        }
        return String(format: "%d:%02d", s / 60, s % 60)
    }

    public static func formatBytes(_ bytes: UInt64) -> String {
        let f = ByteCountFormatter()
        f.countStyle = .file
        f.allowsNonnumericFormatting = false
        return f.string(fromByteCount: Int64(min(bytes, UInt64(Int64.max))))
    }

    /// kHz when it divides cleanly (44.1 kHz, 48 kHz, 96 kHz), Hz otherwise.
    public static func formatSampleRate(_ hz: UInt32) -> String {
        let khz = Double(hz) / 1000.0
        if khz == khz.rounded() {
            return "\(Int(khz)) kHz"
        }
        // One decimal covers the standard 44.1 / 88.2 / 176.4 family.
        return String(format: "%.1f kHz", khz)
    }

    public static func formatChannels(_ ch: UInt32) -> String {
        switch ch {
        case 1: return "mono"
        case 2: return "stereo"
        default: return "\(ch) ch"
        }
    }

    /// Average fps when it is a usable positive value; otherwise the nominal fps.
    /// Returns `nil` when neither is usable. Integer rates render without a
    /// decimal; fractional rates (e.g. 23.976) keep up to three places.
    public static func formatFps(avg: Double, nominal: Double) -> String? {
        let chosen = (avg.isFinite && avg > 0) ? avg : nominal
        guard chosen.isFinite, chosen > 0 else { return nil }
        if chosen == chosen.rounded() {
            return "\(Int(chosen)) fps"
        }
        var s = String(format: "%.3f", chosen)
        while s.hasSuffix("0") { s.removeLast() }
        if s.hasSuffix(".") { s.removeLast() }
        return "\(s) fps"
    }

    /// `CODEC (container)` when both are known, just one when the other is blank,
    /// and `nil` when both are blank. Avoids `codec ()` / dangling parens.
    public static func formatContainerCodec(container: String?, codec: String?) -> String? {
        let cont = nonEmpty(container)
        let cod = nonEmpty(codec)
        switch (cod, cont) {
        case let (c?, k?): return "\(c) (\(k))"
        case let (c?, nil): return c
        case let (nil, k?): return k
        default: return nil
        }
    }

    /// Parse the probe's UTC creation timestamp (commonly ISO-8601, optionally
    /// with fractional seconds) and render it in a stable, readable local form.
    /// Returns `nil` when absent or unparseable — never a fabricated value.
    public static func formatCreationDate(_ utc: String?) -> String? {
        guard let raw = nonEmpty(utc) else { return nil }
        guard let date = parseTimestamp(raw) else { return nil }
        return displayDateFormatter.string(from: date)
    }

    // MARK: - private helpers

    private static func nonEmpty(_ s: String?) -> String? {
        guard let s else { return nil }
        let trimmed = s.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    private static func parseTimestamp(_ raw: String) -> Date? {
        if let d = isoFractional.date(from: raw) { return d }
        if let d = isoPlain.date(from: raw) { return d }
        return nil
    }

    private static let isoFractional: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return f
    }()

    private static let isoPlain: ISO8601DateFormatter = {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime]
        return f
    }()

    /// Fixed `en_US_POSIX` locale so the rendered date is deterministic (tests do
    /// not depend on the host locale) while still reading naturally.
    private static let displayDateFormatter: DateFormatter = {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "MMM d, yyyy 'at' h:mm a"
        return f
    }()
}
