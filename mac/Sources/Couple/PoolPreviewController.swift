// Media-pool audio preview (SPEC-V2 §V2.7, TODO-002): one shared, observable
// preview controller is the single source of truth for the pool's inline audio
// preview. Replaces the old per-row `playing` flag + global stop-only singleton:
// exactly one entry is active at a time, and BOTH the active and the previously
// active row read their icon/expansion straight off this controller, so starting
// a second file can never leave the first row's icon stale.
//
// The transport is split in two so the state machine is testable without an audio
// device: `PoolPreviewState` is a PURE value type carrying the whole observable
// surface (active key, playing/paused, currentTime, duration, scrubbing) and the
// transitions are pure functions returning a `PoolPreviewAction` describing what
// the AVPlayer must then do (play / pause / seek). `PoolPreviewController` is the
// thin @MainActor shell that drives a single AVPlayer from those actions and
// feeds back its periodic time-observer ticks. The pure state machine is covered
// deterministically by the COUPLE_PREVIEW_TEST DebugDrive hook (no media needed).

import AVFoundation
import Observation

/// What the AVPlayer should do after a state transition. The state machine never
/// touches AVFoundation; it only describes the side effect.
enum PoolPreviewAction: Equatable {
    case none
    /// Load a different file and position at `at` seconds (the previous item, if
    /// any, must be released first — the controller owns that swap). `play` is the
    /// auto-start flag: a `toggle`-driven load plays immediately (true), while the
    /// SELECT path (load-paused, owner decision 1) loads the item ready at `at`
    /// WITHOUT calling play (false) — the user presses play to hear it.
    case load(key: String, url: URL, at: Double, play: Bool)
    case play
    case pause
    case seek(Double)
}

/// The complete observable preview surface as a pure value. All transitions are
/// deterministic functions of the current value + the event, so the state machine
/// is unit-testable with no AVPlayer / audio output.
struct PoolPreviewState: Equatable {
    /// Content key of the active pool entry, or nil when nothing is loaded.
    private(set) var activeKey: String?
    private(set) var isPlaying = false
    /// Playback position in seconds; preserved across pause, defines the next
    /// start, and is the slider value. Never overwritten by ticks while scrubbing.
    private(set) var currentTime: Double = 0
    /// Item duration in seconds (0 until the asset reports it).
    private(set) var duration: Double = 0
    /// True only between scrubBegin/scrubEnd; suppresses tick overwrites so the
    /// periodic observer can't fight the user's drag.
    private(set) var isScrubbing = false

    /// True when `key` is the active entry (drives that row's expansion + icon).
    func isActive(_ key: String) -> Bool { activeKey == key }

    /// Treat positions within this margin of the (known) duration as "at the end"
    /// so a natural finish and a scrub-to-the-very-end both reset to 0 next play.
    private static let endEpsilon = 0.05

    private var atEnd: Bool {
        duration > 0 && currentTime >= duration - Self.endEpsilon
    }

    // MARK: - Transitions (pure; each returns the player action to perform)

    /// Toggle the play/pause button for `key`.
    /// - A different (or no) active entry ⇒ switch to `key`, start at 0.
    /// - The active entry while playing ⇒ pause, preserving position.
    /// - The active entry while paused ⇒ resume (from 0 if it had ended).
    mutating func toggle(key: String, url: URL) -> PoolPreviewAction {
        guard activeKey == key else {
            return start(key: key, url: url)
        }
        if isPlaying {
            isPlaying = false
            return .pause
        }
        if atEnd { currentTime = 0 }
        isPlaying = true
        return .play
    }

    /// Switch the active entry to `key` and begin playing it from the start. Any
    /// previously active entry is dropped from the state here; the controller does
    /// the matching player swap from the `.load` action.
    private mutating func start(key: String, url: URL) -> PoolPreviewAction {
        activeKey = key
        currentTime = 0
        duration = 0
        isScrubbing = false
        isPlaying = true
        return .load(key: key, url: url, at: 0, play: true)
    }

    /// SELECT = load-paused (owner decision 1): make `key` the active entry, loaded
    /// and positioned at 0 but NOT playing, so the sticky bar shows its duration and
    /// the user presses play to start. Switching the primary to a different (or new)
    /// key loads-paused and drops the old key here; the controller does the matching
    /// player swap from the `.load(play: false)` action. Re-selecting the already
    /// active key is a no-op (it stays as-is — a play in progress is not interrupted).
    mutating func loadPaused(key: String, url: URL) -> PoolPreviewAction {
        guard activeKey != key else { return .none }
        activeKey = key
        currentTime = 0
        duration = 0
        isScrubbing = false
        isPlaying = false
        return .load(key: key, url: url, at: 0, play: false)
    }

    /// The asset reported its duration (or it changed). Clamp the position in case
    /// a scrub ran ahead of a not-yet-known duration.
    mutating func setDuration(_ d: Double) {
        guard d.isFinite, d > 0 else { return }
        duration = d
        if currentTime > d { currentTime = d }
    }

    /// Periodic time-observer tick. Ignored while scrubbing (the drag owns the
    /// position) or when paused/idle, so it never fights the user.
    mutating func tick(_ t: Double) {
        guard isPlaying, !isScrubbing, activeKey != nil, t.isFinite else { return }
        currentTime = max(0, t)
    }

    /// User grabbed the slider: freeze tick updates until release.
    mutating func scrubBegin() {
        guard activeKey != nil else { return }
        isScrubbing = true
    }

    /// User dragged the slider to `t`. Works while playing or paused; seeks the
    /// player immediately so audio tracks the thumb.
    mutating func scrubTo(_ t: Double) -> PoolPreviewAction {
        guard activeKey != nil else { return .none }
        let clamped = clampToTrack(t)
        currentTime = clamped
        return .seek(clamped)
    }

    /// User released the slider. Position is already set; just re-enable ticks.
    mutating func scrubEnd() {
        isScrubbing = false
    }

    /// The item played to its natural end: stop and rewind so the next play starts
    /// from the beginning (AC: "returns the item to a non-playing state and seeks
    /// to the beginning for the next play").
    mutating func reachedEnd() -> PoolPreviewAction {
        guard activeKey != nil else { return .none }
        isPlaying = false
        isScrubbing = false
        currentTime = 0
        return .pause
    }

    /// Explicit stop/cleanup (teardown, or — future TODO-001 — removing the active
    /// file). Clears the active entry entirely.
    mutating func stop() -> PoolPreviewAction {
        let wasActive = activeKey != nil
        activeKey = nil
        isPlaying = false
        isScrubbing = false
        currentTime = 0
        duration = 0
        return wasActive ? .pause : .none
    }

    private func clampToTrack(_ t: Double) -> Double {
        guard t.isFinite else { return 0 }
        if duration > 0 { return min(max(0, t), duration) }
        return max(0, t)
    }
}

/// The thin @MainActor shell: owns one AVPlayer, applies `PoolPreviewAction`s, and
/// feeds periodic ticks + end notifications back into the pure state. Views read
/// `state` (observed) and call the transition wrappers; nothing else mutates it.
@MainActor
@Observable
final class PoolPreviewController {
    private(set) var state = PoolPreviewState()

    @ObservationIgnored private var player: AVPlayer?
    @ObservationIgnored private var timeObserver: Any?
    @ObservationIgnored private var endObserver: NSObjectProtocol?
    /// Retained so a periodic observer / notification from a replaced item can be
    /// removed against the exact player/item it was added to.
    @ObservationIgnored private var observedPlayer: AVPlayer?

    init() {}

    // MARK: - View-facing transport

    func toggle(key: String, url: URL) { apply(state.toggle(key: key, url: url)) }
    /// SELECT = load-paused (owner decision 1): load the item into the sticky bar
    /// without auto-playing. Drives the primary follow in AppModel.selectPool.
    func select(key: String, url: URL) { apply(state.loadPaused(key: key, url: url)) }
    func scrubBegin() { state.scrubBegin() }
    func scrubTo(_ t: Double) { apply(state.scrubTo(t)) }
    func scrubEnd() { state.scrubEnd() }

    /// Explicit cleanup/stop path. Invoked from the app/pool teardown lifecycle;
    /// also the seam a future per-file removal (TODO-001) calls before releasing a
    /// pool entry's handle. Idempotent.
    func stop() {
        apply(state.stop())
        releasePlayer()
    }

    // MARK: - Action application (the only place the AVPlayer is touched)

    private func apply(_ action: PoolPreviewAction) {
        switch action {
        case .none:
            break
        case let .load(_, url, at, play):
            loadPlayer(url: url, at: at, play: play)
        case .play:
            player?.play()
        case .pause:
            player?.pause()
        case let .seek(t):
            seekPlayer(to: t)
        }
    }

    private func loadPlayer(url: URL, at start: Double, play: Bool) {
        releasePlayer()
        let item = AVPlayerItem(url: url)
        let p = AVPlayer(playerItem: item)
        player = p
        observedPlayer = p
        // End-of-item ⇒ pure reachedEnd() (non-playing, rewound to 0).
        endObserver = NotificationCenter.default.addObserver(
            forName: .AVPlayerItemDidPlayToEndTime, object: item, queue: .main
        ) { [weak self] _ in
            MainActor.assumeIsolated { self?.handleEnd() }
        }
        // Periodic position feed (~10 Hz) → state.tick (ignored while scrubbing).
        let interval = CMTime(seconds: 0.1, preferredTimescale: 600)
        timeObserver = p.addPeriodicTimeObserver(forInterval: interval, queue: .main) { [weak self] time in
            MainActor.assumeIsolated {
                guard let self else { return }
                self.refreshDuration()
                self.state.tick(time.seconds)
            }
        }
        if start > 0 { seekPlayer(to: start) }
        // SELECT loads the item ready but paused (owner decision 1); only the
        // toggle/play path auto-starts. AVPlayer holds at the position until play().
        if play { p.play() }
    }

    private func seekPlayer(to t: Double) {
        player?.seek(to: CMTime(seconds: t, preferredTimescale: 600),
                     toleranceBefore: .zero, toleranceAfter: .zero)
    }

    private func handleEnd() { apply(state.reachedEnd()) }

    /// Surface the item's duration into state once AVFoundation knows it.
    private func refreshDuration() {
        guard let d = player?.currentItem?.duration.seconds, d.isFinite, d > 0 else { return }
        state.setDuration(d)
    }

    private func releasePlayer() {
        if let o = timeObserver { observedPlayer?.removeTimeObserver(o); timeObserver = nil }
        if let o = endObserver { NotificationCenter.default.removeObserver(o); endObserver = nil }
        player?.pause()
        player = nil
        observedPlayer = nil
    }
}
