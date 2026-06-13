// Value types mirroring couple-core's wire JSON (SPEC §4; field names are the
// wire format verbatim, so no key-decoding strategy is needed).

import Foundation

public enum TrackRole: String, Codable, Sendable {
    case camera, external
}

public struct VideoStreamInfo: Codable, Sendable {
    public var codec: String
    public var width: UInt32
    public var height: UInt32
    public var nominalFps: Double
    public var avgFps: Double
    public var startTimeS: Double
    public var bitRate: UInt64?
}

public struct AudioStreamInfo: Codable, Sendable {
    public var codec: String
    public var sampleRate: UInt32
    public var channels: UInt32
    public var bitsPerSample: UInt32?
    public var startTimeS: Double
    public var durationS: Double?
}

public struct BwfInfo: Codable, Sendable {
    public var date: String?
    public var time: String?
    public var timeReferenceSamples: UInt64?
    public var encodedBy: String?
    public var codingHistory: String?
}

public struct MediaInfo: Codable, Sendable {
    public var path: String
    public var container: String
    public var durationS: Double
    public var video: VideoStreamInfo?
    public var audio: [AudioStreamInfo]
    public var creationTimeUtc: String?
    public var bwf: BwfInfo?
    public var tags: [String: String]
}

public struct TrackStats: Codable, Sendable {
    public var absPeak: Double
    public var activeP959600: Double
    public var activeRmsDbfs: Double
}

public struct TrackMeta: Codable, Sendable {
    public var id: UInt64
    public var role: TrackRole
    public var info: MediaInfo
    public var epochS: Double
    public var durationS: Double
    public var sampleRate: UInt32
    public var normalization: Double
    public var stats: TrackStats
    public var contentKey: String
}

// ---------- pairing (SPEC-V2 §3.2 / §4) ----------

/// Role-agnostic stage-1 measurement between any two imported tracks
/// (`cpl_pair_correlate` wire form). Lag convention: content at time t in
/// track_a occurs at t + lagMs/1000 in track_b — so for (video, audio) this is
/// the audio offset. `z` is the stage-1 peak prominence; a cell is a *candidate*
/// when `!insufficientOverlap && z >= Z_MIN` (resolution is shell-side).
public struct PairCorrelation: Codable, Sendable {
    public var z: Double
    public var lagMs: Double
    public var ncc: Double
    /// null (core None) when the peak was below floor and refinement was skipped.
    public var fineNcc: Double?
    public var overlapBins: UInt64
    public var runnerUpZ: Double
    public var refined: Bool
    public var insufficientOverlap: Bool

    public init(z: Double, lagMs: Double, ncc: Double, fineNcc: Double?, overlapBins: UInt64,
                runnerUpZ: Double, refined: Bool, insufficientOverlap: Bool) {
        self.z = z
        self.lagMs = lagMs
        self.ncc = ncc
        self.fineNcc = fineNcc
        self.overlapBins = overlapBins
        self.runnerUpZ = runnerUpZ
        self.refined = refined
        self.insufficientOverlap = insufficientOverlap
    }
}

// ---------- sync (§4.4) ----------

public struct StagePeak: Codable, Sendable {
    public var lagMs: Double
    public var ncc: Double
    public var z: Double
    public var overlapBins: UInt64
}

public struct DriftWindow: Codable, Sendable {
    public var t: Double
    public var lagMs: Double
    public var ncc: Double
    public var accepted: Bool
}

public struct OverlapInfo: Codable, Sendable {
    public var t0: Double
    public var t1: Double
    public var seconds: Double
    public var fractionOfVideo: Double
}

public enum SyncStatus: Sendable, Equatable {
    case synced
    case lowConfidence
    case failed(reason: String, detail: Double)
}

public enum DriftMode: String, Codable, Sendable {
    case resample, none
}

public struct SyncResult: Decodable, Sendable {
    public var offsetMs: Double
    public var driftPpm: Double
    public var driftMeasurable: Bool
    public var coarse: StagePeak
    public var runnerUpZ: Double
    public var fineNcc: Double
    public var windows: [DriftWindow]
    public var fitRmsMs: Double?
    public var endToEndDriftMs: Double
    public var overlap: OverlapInfo
    public var status: SyncStatus
    public var suggestedDriftMode: DriftMode
    public var computeMs: Double

    enum CodingKeys: String, CodingKey {
        case offsetMs, driftPpm, driftMeasurable, coarse, runnerUpZ, fineNcc
        case windows, fitRmsMs, endToEndDriftMs, overlap, status, fail
        case suggestedDriftMode, computeMs
    }

    struct Fail: Decodable {
        var reason: String
        var bestZ: Double?
        var overlapS: Double?
    }

    public init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: CodingKeys.self)
        offsetMs = try c.decode(Double.self, forKey: .offsetMs)
        driftPpm = try c.decode(Double.self, forKey: .driftPpm)
        driftMeasurable = try c.decode(Bool.self, forKey: .driftMeasurable)
        coarse = try c.decode(StagePeak.self, forKey: .coarse)
        runnerUpZ = try c.decode(Double.self, forKey: .runnerUpZ)
        fineNcc = try c.decode(Double.self, forKey: .fineNcc)
        windows = try c.decode([DriftWindow].self, forKey: .windows)
        fitRmsMs = try c.decodeIfPresent(Double.self, forKey: .fitRmsMs)
        endToEndDriftMs = try c.decode(Double.self, forKey: .endToEndDriftMs)
        overlap = try c.decode(OverlapInfo.self, forKey: .overlap)
        suggestedDriftMode = try c.decode(DriftMode.self, forKey: .suggestedDriftMode)
        computeMs = try c.decode(Double.self, forKey: .computeMs)
        switch try c.decode(String.self, forKey: .status) {
        case "synced": status = .synced
        case "lowConfidence": status = .lowConfidence
        default:
            let f = try c.decodeIfPresent(Fail.self, forKey: .fail)
            status = .failed(reason: f?.reason ?? "unknown",
                             detail: f?.bestZ ?? f?.overlapS ?? 0)
        }
    }
}

// ---------- alignment state (§4.5) ----------

public struct Frame: Codable, Sendable, Equatable {
    public var winMs: Double
    public var auto: Bool
    public init(winMs: Double, auto: Bool) {
        self.winMs = winMs
        self.auto = auto
    }
}

public struct LocalMeasurement: Codable, Sendable, Equatable {
    public var lagMs: Double
    public var ncc: Double
    public init(lagMs: Double, ncc: Double) {
        self.lagMs = lagMs
        self.ncc = ncc
    }
}

public struct DriftWindowState: Codable, Sendable, Equatable {
    public var t: Double
    public var lagMs: Double
    public var ncc: Double
    public init(t: Double, lagMs: Double, ncc: Double) {
        self.t = t
        self.lagMs = lagMs
        self.ncc = ncc
    }
}

public enum LoupeLayout: String, Codable, Sendable {
    case grid, stack
}

public struct AlignmentState: Codable, Sendable {
    public var version: UInt32
    public var offsetMs: Double
    public var driftPpm: Double
    public var driftMode: DriftMode
    public var savedDriftPpm: Double
    public var anchors: [Double]
    public var frames: [Frame]
    public var lockZoom: Bool
    public var perAnchorMeasurements: [LocalMeasurement?]
    public var driftWindows: [DriftWindowState]
    public var verified: Bool
    public var loupeLayout: LoupeLayout
}

public struct Onset: Codable, Sendable {
    public var t: Double
    public var score: Double
}

public struct Fit: Codable, Sendable {
    public var offMs: Double
    public var ppm: Double
    public var rmsMs: Double
}
