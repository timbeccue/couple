import Foundation
import Testing
@testable import CoupleKit

@Suite(.serialized) struct JobStartTests {
    private func engine() throws -> Engine {
        try Engine(cacheDir: NSTemporaryDirectory() + "couple-job-tests-" + UUID().uuidString)
    }

    @Test func malformedStartsThrowAndConsumeTheirCoreError() throws {
        let engine = try engine()
        let starts: [() throws -> Void] = [
            { _ = try engine.resolveStart(videos: [], audios: [], paramsJSON: "{") },
            { _ = try engine.matchCandidatesGraphStart(videos: [], audios: [], paramsJSON: "{") },
            { _ = try SessionHandle(engine: engine, id: 0).exportStart(optionsJSON: "{") },
        ]
        for start in starts {
            do {
                try start()
                Issue.record("Malformed JSON should fail before a job starts")
            } catch let error as CoreError {
                #expect(error.message.contains("invalid"))
            }
            #expect(engine.takeError().message == "unknown core error")
        }
    }

    @Test func onlyTerminalCallbacksReleaseTheRetainedBox() {
        for terminal in ["done", "error", "cancelled"] {
            var box: JobBox? = JobBox()
            weak var observed = box
            let user = Unmanaged.passRetained(box!).toOpaque()
            box = nil
            #expect(observed != nil)

            jobTrampoline(1, "SyncCoarse", 0.5, nil, user)
            #expect(observed != nil)

            jobTrampoline(1, terminal, 1, "{}", user)
            #expect(observed == nil)
        }
    }

    @Test func terminalBeforeAwaitPreservesProgressAndResult() async throws {
        let engine = try engine()
        let box = JobBox()
        let job = Job<Int>(engine: engine, box: box, id: 1)
        let user = Unmanaged.passRetained(box).toOpaque()
        jobTrampoline(1, "SyncCoarse", 0.25, nil, user)
        jobTrampoline(1, "done", 1, "42", user)

        var updates: [ProgressUpdate] = []
        for await update in job.progress { updates.append(update) }
        #expect(updates.count == 1)
        #expect(updates.first?.phase == "SyncCoarse")
        #expect(updates.first?.fraction == 0.25)
        #expect(try await job.value() == 42)
    }

    @Test func cancellationTerminalThrowsFromValue() async throws {
        let engine = try engine()
        let box = JobBox()
        let job = Job<Int>(engine: engine, box: box, id: 1)
        let user = Unmanaged.passRetained(box).toOpaque()
        jobTrampoline(1, "cancelled", 0, nil, user)
        do {
            _ = try await job.value()
            Issue.record("Cancellation should not produce a result")
        } catch JobError.cancelled {
            // The C terminal event is surfaced without being converted to a core error.
        }
    }

    @Test func failedStartReleasesTheBoxWithoutACallback() throws {
        let engine = try engine()
        weak var observed: JobBox?
        do {
            let _: Job<Int> = try engine.startJob { user in
                observed = Unmanaged<JobBox>.fromOpaque(user).takeUnretainedValue()
                return 0
            }
            Issue.record("A zero job ID should fail synchronously")
        } catch let error as CoreError {
            #expect(error.message == "unknown core error")
        }
        #expect(observed == nil)
    }

    @Test func synchronousTerminalLeavesTheResultOwnedByTheJob() async throws {
        let engine = try engine()
        weak var observed: JobBox?
        var job: Job<Int>? = try engine.startJob { user in
            observed = Unmanaged<JobBox>.fromOpaque(user).takeUnretainedValue()
            jobTrampoline(42, "done", 1, "7", user)
            return 42
        }
        #expect(job?.id == 42)
        #expect(observed != nil)
        #expect(try await job?.value() == 7)
        job = nil
        #expect(observed == nil)
    }

    @Test func droppingTheJobKeepsTheBoxAliveUntilCancellationCallback() throws {
        let engine = try engine()
        weak var observed: JobBox?
        var callbackUser: UnsafeMutableRawPointer?
        var job: Job<Int>? = try engine.startJob { user in
            observed = Unmanaged<JobBox>.fromOpaque(user).takeUnretainedValue()
            callbackUser = user
            return 42
        }
        #expect(job?.id == 42)
        job = nil
        #expect(observed != nil)
        jobTrampoline(42, "SyncCoarse", 0.5, nil, callbackUser)
        #expect(observed != nil)
        jobTrampoline(42, "cancelled", 0.5, nil, callbackUser)
        #expect(observed == nil)
    }
}
