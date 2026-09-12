import CCouple
import Foundation
import Testing
@testable import CoupleKit

@Suite(.serialized) struct JobStartTests {
    private func engine() throws -> Engine {
        try Engine(cacheDir: NSTemporaryDirectory() + "couple-job-tests-" + UUID().uuidString)
    }

    @Test func rejectedStartReleasesTheBoxAndConsumesTheCoreError() throws {
        let engine = try engine()
        weak var observed: JobBox?
        do {
            let _: Job<Int> = try engine.startJob { user in
                observed = Unmanaged<JobBox>.fromOpaque(user).takeUnretainedValue()
                return cpl_resolve_start(engine.ctx, nil, 0, nil, 0, "{", jobTrampoline, user)
            }
            Issue.record("Malformed JSON should fail before a job starts")
        } catch let error as CoreError {
            #expect(error.message.contains("invalid"))
        }
        #expect(observed == nil)
        #expect(engine.takeError().message == "unknown core error")
    }

    @Test func cancellationTerminalThrowsFromValue() async throws {
        let engine = try engine()
        let job: Job<Int> = try engine.startJob { user in
            jobTrampoline(1, "cancelled", 0, nil, user)
            return 1
        }
        do {
            _ = try await job.value()
            Issue.record("Cancellation should not produce a result")
        } catch JobError.cancelled {
            // The C terminal event is surfaced without being converted to a core error.
        }
    }

    @Test func synchronousTerminalLeavesProgressAndResultOwnedByTheJob() async throws {
        let engine = try engine()
        weak var observed: JobBox?
        var job: Job<Int>? = try engine.startJob { user in
            observed = Unmanaged<JobBox>.fromOpaque(user).takeUnretainedValue()
            jobTrampoline(42, "SyncCoarse", 0.25, nil, user)
            jobTrampoline(42, "done", 1, "7", user)
            return 42
        }
        #expect(job?.id == 42)
        #expect(observed != nil)
        let progress = try #require(job?.progress)
        var updates: [ProgressUpdate] = []
        for await update in progress { updates.append(update) }
        #expect(updates.count == 1)
        #expect(updates.first?.phase == "SyncCoarse")
        #expect(updates.first?.fraction == 0.25)
        #expect(try await job?.value() == 7)
        job = nil
        #expect(observed == nil)
    }

    @Test(arguments: ["done", "error", "cancelled"])
    func droppingTheJobKeepsTheBoxAliveUntilTerminalCallback(terminal: String) throws {
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
        jobTrampoline(42, terminal, 0.5, "{}", callbackUser)
        #expect(observed == nil)
    }
}
