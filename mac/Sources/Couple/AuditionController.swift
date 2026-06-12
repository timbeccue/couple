// Audition playback (SPEC §11.4, UX §2.7/§4.7): AVAudioEngine source node pulls
// cpl_audition_fill directly on the render thread (no Swift allocation there);
// a muted AVPlayer carries the video, slaved to the audio clock — 1 Hz skew
// monitor, rate nudge past 20 ms, reseek past 200 ms (budget: ≤ 1 frame skew).

import AVFoundation
import CCouple
import CoupleKit
import Observation
import QuartzCore

/// Render-thread → main-thread mailbox. Aligned 64-bit loads/stores are
/// single-copy atomic on arm64; only the render thread writes.
final class PlayheadMailbox: @unchecked Sendable {
    private let raw: UnsafeMutablePointer<UInt64>

    init() {
        raw = UnsafeMutablePointer<UInt64>.allocate(capacity: 2)
        raw[0] = Double(0).bitPattern
        raw[1] = 0 // ended flag
    }

    deinit {
        raw.deallocate()
    }

    func store(_ t: Double) {
        raw[0] = t.bitPattern
    }

    var value: Double {
        Double(bitPattern: raw[0])
    }

    func markEnded(_ e: Bool) {
        raw[1] = e ? 1 : 0
    }

    var ended: Bool {
        raw[1] != 0
    }
}

@MainActor
@Observable
final class AuditionController {
    let player = AVPlayer() // muted video, slaved
    private(set) var isPlaying = false
    private(set) var playheadT: Double = 0
    private(set) var lastSkewMs: Double = 0
    /// Set when the audio device can't be brought up (engine start timeout) —
    /// e.g. a locked/headless session. Shown inline in the transport.
    private(set) var audioUnavailable = false
    var focusedAnchor: Int?
    @ObservationIgnored private var isStarting = false

    var monitorMode: MonitorMode = .overlay {
        didSet { stream?.setMode(monitorMode) }
    }

    @ObservationIgnored private var session: SessionHandle?
    @ObservationIgnored private var stream: AuditionHandle?
    @ObservationIgnored private var audioEngine: AVAudioEngine?
    @ObservationIgnored private var sourceNode: AVAudioSourceNode?
    @ObservationIgnored private let mailbox = PlayheadMailbox()
    @ObservationIgnored private var uiTimer: Timer?
    @ObservationIgnored private var ticksSinceSkewCheck = 0
    @ObservationIgnored private var videoDuration: Double = 0

    var isAttached: Bool { session != nil }

    func attach(session: SessionHandle, videoURL: URL, videoDuration: Double) {
        teardown()
        self.session = session
        self.videoDuration = videoDuration
        let item = AVPlayerItem(url: videoURL)
        player.replaceCurrentItem(with: item)
        player.isMuted = true
        // Poster frame.
        player.seek(to: CMTime(seconds: 1.0, preferredTimescale: 48000),
                    toleranceBefore: .zero, toleranceAfter: .positiveInfinity)
    }

    func teardown() {
        pause()
        uiTimer?.invalidate()
        uiTimer = nil
        if let engine = audioEngine {
            engine.stop()
        }
        audioEngine = nil
        sourceNode = nil
        stream?.close()
        stream = nil
        session = nil
        player.replaceCurrentItem(with: nil)
        isPlaying = false
        focusedAnchor = nil
    }

    // MARK: - Transport

    func toggle(defaultStart: Double) {
        if isPlaying {
            pause()
        } else {
            play(from: stream == nil ? defaultStart : nil)
        }
    }

    /// Start playback; `from` nil resumes at the stream cursor. The engine
    /// bring-up is asynchronous with a timeout: a wedged audio HAL (locked
    /// session, dead device) must never hang the main thread.
    func play(from t: Double?) {
        guard session != nil, !isStarting, !isPlaying else { return }
        isStarting = true
        Task { [weak self] in
            await self?.bringUpAndStart(from: t)
            self?.isStarting = false
        }
    }

    private func bringUpAndStart(from t: Double?) async {
        guard let session else { return }
        if stream == nil {
            // Opening computes loudness gains over the overlap (two large PCM
            // scans) — off the main actor.
            let mode = monitorMode
            let tStart = t ?? 0
            // Explicit QoS: an unspecified detached priority inherits low QoS
            // in a napped app and its page-fault I/O gets throttled to a crawl.
            let opened = await Task.detached(priority: .userInitiated) {
                try? session.auditionOpen(mode: mode, tStart: tStart)
            }.value
            guard let opened else {
                audioUnavailable = true
                return
            }
            stream = opened
            playheadT = tStart
        } else if let t {
            stream?.seek(to: t)
            playheadT = t
        }
        mailbox.markEnded(false)
        mailbox.store(playheadT)
        if audioEngine == nil {
            // Build + start entirely off-main under a timeout: AVAudioSourceNode
            // init / engine.connect / engine.start all talk to the audio HAL,
            // and a wedged HAL (locked or headless session) blocks them
            // indefinitely — that must never reach the main thread.
            guard let stream else { return }
            let built = await Self.buildAndStartEngine(
                ctx: stream.rawCtx, streamID: stream.id,
                mailbox: mailbox, timeoutSeconds: 5)
            guard let built else {
                audioUnavailable = true
                return
            }
            audioEngine = built.engine
            sourceNode = built.node
        } else if (try? audioEngine?.start()) == nil {
            audioUnavailable = true
            return
        }
        audioUnavailable = false
        // Slave the video: align to the audio position now; the skew loop converges the rest.
        isPlaying = true
        let start = CMTime(seconds: playheadT, preferredTimescale: 48000)
        player.seek(to: start, toleranceBefore: .zero, toleranceAfter: .zero) { [weak self] _ in
            Task { @MainActor in
                guard let self, self.isPlaying else { return }
                self.player.play()
            }
        }
        startUITimer()
    }

    struct BuiltEngine: @unchecked Sendable {
        let engine: AVAudioEngine
        let node: AVAudioSourceNode
    }

    /// Build the engine graph and start it, off-main, racing a timeout. The
    /// render block touches only raw handles + the mailbox (RT-safe). A timed
    /// out build is abandoned (its thread may stay wedged in the HAL; the app
    /// stays responsive and a later retry builds fresh).
    private nonisolated static func buildAndStartEngine(
        ctx: UInt64, streamID: UInt64, mailbox: PlayheadMailbox, timeoutSeconds: Double
    ) async -> BuiltEngine? {
        await withTaskGroup(of: BuiltEngine??.self) { group in
            group.addTask {
                let engine = AVAudioEngine()
                // The mixer rejects interleaved connections (kAudioUnitErr
                // −10868): render deinterleaved, de-interleaving the core's
                // stereo fill through a preallocated scratch (RT-safe).
                let format = AVAudioFormat(standardFormatWithSampleRate: 48000, channels: 2)!
                final class Scratch: @unchecked Sendable {
                    let ptr: UnsafeMutablePointer<Float>
                    let capacityFrames: Int

                    init(capacityFrames: Int) {
                        self.capacityFrames = capacityFrames
                        ptr = .allocate(capacity: capacityFrames * 2)
                    }

                    deinit {
                        ptr.deallocate()
                    }
                }
                let scratch = Scratch(capacityFrames: 8192)
                let node = AVAudioSourceNode(format: format) { isSilence, _, frameCount, abl -> OSStatus in
                    let buffers = UnsafeMutableAudioBufferListPointer(abl)
                    guard buffers.count >= 2,
                          let lRaw = buffers[0].mData, let rRaw = buffers[1].mData else {
                        return noErr
                    }
                    let l = lRaw.assumingMemoryBound(to: Float.self)
                    let r = rRaw.assumingMemoryBound(to: Float.self)
                    var rendered: UInt32 = 0
                    var streamT: Double = 0
                    var done = 0
                    while done < Int(frameCount) {
                        let chunk = min(Int(frameCount) - done, scratch.capacityFrames)
                        var frames: UInt32 = 0
                        _ = cpl_audition_fill(ctx, streamID, scratch.ptr, UInt32(chunk),
                                              &frames, &streamT)
                        for i in 0..<chunk {
                            l[done + i] = scratch.ptr[2 * i]
                            r[done + i] = scratch.ptr[2 * i + 1]
                        }
                        rendered += frames
                        done += chunk
                    }
                    mailbox.store(streamT)
                    if rendered == 0 {
                        isSilence.pointee = true
                        mailbox.markEnded(true)
                    }
                    return noErr
                }
                engine.attach(node)
                engine.connect(node, to: engine.mainMixerNode, format: format)
                guard (try? engine.start()) != nil else { return BuiltEngine??.some(nil) }
                return BuiltEngine??.some(BuiltEngine(engine: engine, node: node))
            }
            group.addTask {
                try? await Task.sleep(nanoseconds: UInt64(timeoutSeconds * 1e9))
                return BuiltEngine??.none
            }
            defer { group.cancelAll() }
            if let first = await group.next(), let result = first {
                return result // build finished: engine or explicit failure
            }
            return nil // timeout
        }
    }

    func pause() {
        audioEngine?.pause()
        player.pause()
        player.rate = 0
        isPlaying = false
        uiTimer?.invalidate()
        uiTimer = nil
    }

    func seek(to t: Double, focusAnchor: Int? = nil) {
        if let focusAnchor {
            focusedAnchor = focusAnchor
        }
        let clamped = max(0, min(t, videoDuration))
        playheadT = clamped
        if stream == nil {
            if isPlaying {
                return
            }
            // Not opened yet: just move the video poster.
            player.seek(to: CMTime(seconds: clamped, preferredTimescale: 48000),
                        toleranceBefore: .zero, toleranceAfter: .zero)
            return
        }
        stream?.seek(to: clamped)
        mailbox.store(clamped)
        player.seek(to: CMTime(seconds: clamped, preferredTimescale: 48000),
                    toleranceBefore: .zero, toleranceAfter: .zero)
    }

    /// Jump button: anchor − 0.8 s and play (UX §1.4).
    func jump(toAnchorAt t: Double, index: Int) {
        focusedAnchor = index
        let target = max(0, t - 0.8)
        if isPlaying {
            seek(to: target)
        } else {
            if stream == nil {
                play(from: target)
            } else {
                seek(to: target)
                play(from: nil)
            }
        }
    }

    // MARK: - UI tick + A/V skew loop (§11.4)

    private func startUITimer() {
        uiTimer?.invalidate()
        let timer = Timer(timeInterval: 1.0 / 15.0, repeats: true) { [weak self] _ in
            Task { @MainActor in
                self?.tick()
            }
        }
        RunLoop.main.add(timer, forMode: .common)
        uiTimer = timer
    }

    private func tick() {
        guard isPlaying else { return }
        playheadT = mailbox.value
        if mailbox.ended {
            pause()
            return
        }
        ticksSinceSkewCheck += 1
        if ticksSinceSkewCheck >= 15 { // ~1 Hz
            ticksSinceSkewCheck = 0
            let videoT = player.currentTime().seconds
            guard videoT.isFinite else { return }
            // What's audible now lags the render cursor by the output latency.
            let audioT = playheadT - (audioEngine?.outputNode.presentationLatency ?? 0)
            let skew = videoT - audioT // + = video ahead
            lastSkewMs = skew * 1000
            // Audio is master: gain-scheduled rate nudge with a deadband, so
            // steady state settles instead of oscillating around the budget.
            if abs(skew) > 0.2 {
                player.seek(to: CMTime(seconds: audioT, preferredTimescale: 48000),
                            toleranceBefore: .zero, toleranceAfter: .zero)
                player.rate = 1.0
            } else if abs(skew) > 0.06 {
                player.rate = skew > 0 ? 0.94 : 1.06
            } else if abs(skew) > 0.015 {
                player.rate = skew > 0 ? 0.99 : 1.01
            } else {
                player.rate = 1.0
            }
        }
    }
}
