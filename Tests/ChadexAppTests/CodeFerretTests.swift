import AppKit
import SwiftUI
import XCTest
@testable import ChadexApp

final class CodeFerretTests: XCTestCase {
    private let now = Date(timeIntervalSince1970: 2_000_000)
    private func trace(_ sequence: UInt64 = 1, tool: String = "search_project_texts", start: UInt64 = 2_000_000_000, finished: UInt64? = nil, completion: String = "running") -> McpPerformanceTraceEntry {
        McpPerformanceTraceEntry(sequence: sequence, startedAtMs: start, finishedAtMs: finished,
            methods: ["tools/call"], toolNames: [tool], requestBytes: 1, responseBytes: 0,
            ingressPreBackendUs: 0, backendHeadersUs: 0, responseStreamUs: 0, totalUs: 0, completion: completion)
    }
    private func task(_ status: String = "running", kind: String = "search", validation: String = "not_run") -> TaskProgressSnapshot {
        TaskProgressSnapshot(taskId: "t1", project: "Chadex", goal: "Test", status: status,
            currentStep: 0, totalSteps: 2, completedSteps: 1, plan: [kind, "review"],
            cancelRequested: false, startedAtMs: 2_000_000_000,
            validation: TaskValidationSummary(status: validation, checksPassed: 0, checksFailed: 0),
            review: TaskReviewSummary(status: "pending"), steps: [])
    }

    func testLiveRequestReceivesThenRetainsSearchWithoutClaimingTaskSuccess() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        c.update(snapshot: .initial, traces: [trace()], activities: [], now: now)
        XCTAssertEqual(c.presentation.state, .listening)
        c.update(snapshot: .initial, traces: [trace()], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .searching)
        c.update(snapshot: .initial, traces: [trace(finished: 2_000_001_000, completion: "completed")], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .searching)
        XCTAssertTrue(c.presentation.isRecentActivity)
        XCTAssertEqual(c.presentation.title, L10n.string("ferret.recent.searching"))
        c.update(snapshot: .initial, traces: [trace(finished: 2_000_001_000, completion: "completed")], activities: [], now: now.addingTimeInterval(5))
        XCTAssertEqual(c.presentation.state, .idle)
        XCTAssertFalse(c.presentation.isRecentActivity)
    }

    func testFastToolsBetweenPollsShowRecentActivityWithBoundedExpiry() {
        for (tool, state) in [("read_file", FerretState.searching), ("apply_patch", .coding),
                              ("run_validation", .testing), ("run_shell", .thinking)] {
            var c = FerretController()
            c.update(snapshot: .initial, traces: [], activities: [], now: now)
            let result = trace(tool: tool, finished: 2_000_000_100, completion: "completed")
            c.update(snapshot: .initial, traces: [result], activities: [], now: now.addingTimeInterval(1))
            XCTAssertEqual(c.presentation.state, state)
            XCTAssertTrue(c.presentation.isRecentActivity)
            XCTAssertNil(c.presentation.progress)
            c.update(snapshot: .initial, traces: [result], activities: [], now: now.addingTimeInterval(2.9))
            XCTAssertEqual(c.presentation.state, state)
            c.update(snapshot: .initial, traces: [result], activities: [], now: now.addingTimeInterval(3))
            XCTAssertEqual(c.presentation.state, .idle, "Repeated polls must not extend the hold")
            XCTAssertFalse(c.presentation.isRecentActivity)
        }
    }

    func testConnectionVerificationAloneAllowsIdleAndSleep() {
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        snapshot.phase = .waitingForChatGPTVerification
        snapshot.tunnelReady = true
        snapshot.chatGPTConnected = true
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        XCTAssertEqual(c.presentation.state, .idle)
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(181))
        XCTAssertEqual(c.presentation.state, .sleep)
        XCTAssertEqual(snapshot.phase, .waitingForChatGPTVerification)
    }

    func testStatusPollsNeitherWakeNorConsumeRecentActivityOrSuccess() {
        let tools = ["task_status", "job_status", "poll_job", "observe_jobs", "observe_task", "list_jobs"]
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        let polls = tools.enumerated().map { trace(UInt64($0.offset + 1), tool: $0.element) }
        c.update(snapshot: snapshot, traces: polls, activities: [], now: now.addingTimeInterval(181))
        XCTAssertEqual(c.presentation.state, .sleep)
        let finishedPolls = tools.enumerated().map {
            trace(UInt64($0.offset + 1), tool: $0.element, finished: 2_000_181_000, completion: "completed")
        }
        let edit = trace(7, tool: "apply_patch", finished: 2_000_182_000, completion: "completed")
        c.update(snapshot: snapshot, traces: finishedPolls + [edit], activities: [], now: now.addingTimeInterval(182))
        XCTAssertEqual(c.presentation.state, .coding)
        XCTAssertTrue(c.presentation.isRecentActivity)
        c.update(snapshot: snapshot, traces: polls + [edit], activities: [], now: now.addingTimeInterval(183))
        XCTAssertEqual(c.presentation.state, .coding)
        snapshot.taskProgress = task("completed", validation: "passed")
        c.update(snapshot: snapshot, traces: polls + [edit], activities: [], now: now.addingTimeInterval(184))
        XCTAssertEqual(c.presentation.state, .success)
        XCTAssertFalse(c.presentation.isRecentActivity)
    }

    func testNewLiveWorkImmediatelySupersedesRecentActivity() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let search = trace(finished: 2_000_000_100, completion: "completed")
        c.update(snapshot: .initial, traces: [search], activities: [], now: now.addingTimeInterval(1))
        let edit = trace(2, tool: "apply_patch", start: 2_000_001_000)
        c.update(snapshot: .initial, traces: [search, edit], activities: [], now: now.addingTimeInterval(1.1))
        XCTAssertEqual(c.presentation.state, .listening)
        XCTAssertFalse(c.presentation.isRecentActivity)
        c.update(snapshot: .initial, traces: [search, edit], activities: [], now: now.addingTimeInterval(2.2))
        XCTAssertEqual(c.presentation.state, .coding)
        XCTAssertFalse(c.presentation.isRecentActivity)
    }

    func testRecentActivityDoesNotCoverActualTaskWaits() {
        for status in ["blocked", "interrupted", "ready_to_apply", "unknown", "queued", "cancelling"] {
            var c = FerretController()
            var snapshot = BackendSnapshot.initial
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            snapshot.taskProgress = task(status)
            let result = trace(finished: 2_000_001_000, completion: "completed")
            c.update(snapshot: snapshot, traces: [result], activities: [], now: now.addingTimeInterval(1))
            c.update(snapshot: snapshot, traces: [result], activities: [], now: now.addingTimeInterval(2))
            XCTAssertEqual(c.presentation.state, .waiting, status)
            XCTAssertFalse(c.presentation.isRecentActivity)
        }
    }

    func testCompletedActivityCoalescesByCompletionTimeAndPreservesFailures() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let edit = trace(1, tool: "apply_patch", finished: 2_000_001_000, completion: "completed")
        let search = trace(2, finished: 2_000_000_100, completion: "completed")
        c.update(snapshot: .initial, traces: [edit, search], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .coding, "Highest sequence need not finish last")
        let failure = trace(3, finished: 2_000_001_500, completion: "backend_error")
        let success = trace(4, finished: 2_000_001_900, completion: "completed")
        c.update(snapshot: .initial, traces: [edit, search, failure, success], activities: [], now: now.addingTimeInterval(2))
        XCTAssertEqual(c.presentation.state, .error, "A batch success must not erase an observed failure")
        XCTAssertFalse(c.presentation.isRecentActivity)
    }

    func testCompletedWaitDoesNotHideFastWorkInTheSamePoll() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let search = trace(1, finished: 2_000_000_100, completion: "completed")
        let wait = trace(2, tool: "await_job", finished: 2_000_000_200, completion: "completed")
        c.update(snapshot: .initial, traces: [search, wait], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .searching)
        XCTAssertTrue(c.presentation.isRecentActivity)
    }

    func testRecentActivityDoesNotReplayAcrossProjectsOrFromStaleHistory() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let result = trace(finished: 2_000_000_100, completion: "completed")
        c.update(snapshot: .initial, traces: [result], activities: [], now: now.addingTimeInterval(1))
        var snapshot = BackendSnapshot.initial
        snapshot.selectedProject = ProjectInspection(path: "/other", allowedRoot: "/other", isGitRepository: true, readable: true, writable: true)
        c.update(snapshot: snapshot, traces: [result], activities: [], now: now.addingTimeInterval(1.5))
        XCTAssertEqual(c.presentation.state, .idle)
        XCTAssertFalse(c.presentation.isRecentActivity)
        let stale = trace(2, finished: 2_000_000_200, completion: "completed")
        c.update(snapshot: snapshot, traces: [result, stale], activities: [], now: now.addingTimeInterval(10))
        XCTAssertEqual(c.presentation.state, .idle)
    }

    func testConcurrentRequestsCanFinishOutOfOrder() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let older = trace(1, tool: "apply_text_edits")
        let newer = trace(2, tool: "read_files", finished: 2_000_000_000, completion: "completed")
        c.update(snapshot: .initial, traces: [older, newer], activities: [], now: now)
        c.update(snapshot: .initial, traces: [older, newer], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .coding)
        c.update(snapshot: .initial, traces: [trace(1, tool: "apply_text_edits", finished: 2_000_002_000, completion: "backend_error"), newer], activities: [], now: now.addingTimeInterval(2))
        XCTAssertEqual(c.presentation.state, .error)
    }

    func testTaskValidationOutcomeReactionAndExpiry() {
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        snapshot.taskProgress = task(kind: "validate")
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .testing)
        XCTAssertEqual(c.presentation.progress, 0.5)
        snapshot.taskProgress = task("completed", validation: "passed")
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(2))
        XCTAssertEqual(c.presentation.state, .success)
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(6))
        XCTAssertEqual(c.presentation.state, .idle)
        snapshot.taskProgress = task("failed_validation", validation: "failed")
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(7))
        XCTAssertEqual(c.presentation.state, .error)
    }

    func testLongTaskNeverSleepsWhenBackgrounded() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let active = trace(tool: "cargo_test", start: 1_999_900_000)
        c.update(snapshot: .initial, traces: [active], activities: [], foreground: false, now: now)
        c.update(snapshot: .initial, traces: [active], activities: [], foreground: false, now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .longTask)
        XCTAssertEqual(c.presentation.activity, .testing)
        XCTAssertNil(c.presentation.progress)
    }

    func testIdleSleepAndWakeOnRealWork() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        c.update(snapshot: .initial, traces: [], activities: [], now: now.addingTimeInterval(181))
        XCTAssertEqual(c.presentation.state, .sleep)
        c.update(snapshot: .initial, traces: [trace(start: 2_000_182_000)], activities: [], now: now.addingTimeInterval(182))
        XCTAssertEqual(c.presentation.state, .listening)
    }

    func testHistoryAndProjectSwitchDoNotReplayOldResults() {
        var c = FerretController()
        let old = trace(finished: 1_999_999_000, completion: "backend_error")
        c.update(snapshot: .initial, traces: [old], activities: [], now: now)
        XCTAssertEqual(c.presentation.state, .idle)
        var snapshot = BackendSnapshot.initial
        snapshot.selectedProject = ProjectInspection(path: "/other", allowedRoot: "/other", isGitRepository: true, readable: true, writable: true)
        c.update(snapshot: snapshot, traces: [old], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .idle)
    }

    func testHttpSuccessWithToolFailureShowsError() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        var result = trace(tool: "cargo_test", finished: 2_000_000_000, completion: "completed")
        result.toolFailed = true
        c.update(snapshot: .initial, traces: [result], activities: [], now: now)
        XCTAssertEqual(c.presentation.state, .error)
    }

    func testToolSemanticsDoNotTreatEveryProcessAsTest() {
        XCTAssertEqual(FerretController.toolState("cargo_test"), .testing)
        XCTAssertEqual(FerretController.toolState("apply_text_edits"), .coding)
        XCTAssertEqual(FerretController.toolState("run_shell"), .thinking)
        XCTAssertEqual(FerretController.taskState(task(kind: "run_process")), .thinking)
        XCTAssertTrue(task("validating").isActive)
    }

    func testDurableJobSurvivesHttpCompletionAndBackgrounding() {
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        snapshot.mascotJobs = []
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        snapshot.mascotJobs = [FerretJobSnapshot(jobId: "j1", status: "running", startedAtMs: 1_999_900_000)]
        let finished = trace(tool: "run_shell", finished: 2_000_000_000, completion: "completed")
        c.update(snapshot: snapshot, traces: [finished], activities: [], foreground: false, now: now)
        c.update(snapshot: snapshot, traces: [finished], activities: [], foreground: false, now: now.addingTimeInterval(5))
        XCTAssertEqual(c.presentation.state, .longTask)
        XCTAssertNil(c.presentation.progress)
        snapshot.mascotJobs = nil
        c.update(snapshot: snapshot, traces: [finished], activities: [], foreground: false, now: now.addingTimeInterval(6))
        XCTAssertEqual(c.presentation.state, .waiting, "Missing observation is not job completion")
        snapshot.mascotJobs = [FerretJobSnapshot(jobId: "j1", status: "completed", startedAtMs: 1_999_900_000, finishedAtMs: 2_000_007_000, exitCode: 0)]
        c.update(snapshot: snapshot, traces: [finished], activities: [], now: now.addingTimeInterval(7))
        XCTAssertEqual(c.presentation.state, .success)
        c.update(snapshot: snapshot, traces: [finished], activities: [], now: now.addingTimeInterval(11))
        XCTAssertEqual(c.presentation.state, .idle)
    }

    func testDurableFailureHistoryAndRecoveryAreDistinct() {
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        snapshot.mascotJobs = [FerretJobSnapshot(jobId: "old", status: "failed", finishedAtMs: 2_000_000_000, exitCode: 1)]
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        XCTAssertEqual(c.presentation.state, .idle)
        snapshot.mascotJobs = [FerretJobSnapshot(jobId: "new", status: "recovering", startedAtMs: 1_999_900_000)]
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .waiting)
        snapshot.mascotJobs = [FerretJobSnapshot(jobId: "new", status: "failed", finishedAtMs: 2_000_002_000, exitCode: 1)]
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(2))
        XCTAssertEqual(c.presentation.state, .error)
        snapshot.mascotJobs = []
        snapshot.taskProgress = task("unknown")
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(8))
        XCTAssertEqual(c.presentation.state, .waiting)
    }

    func testWaitingNeverReceivesOrBecomesLongTask() {
        for status in ["queued", "cancelling"] {
            var c = FerretController()
            var snapshot = BackendSnapshot.initial
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            snapshot.taskProgress = task(status)
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            XCTAssertEqual(c.presentation.state, .waiting, status)
            c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(61))
            XCTAssertEqual(c.presentation.state, .waiting, status)
        }
        for status in ["queued", "recovering", "stop_requested"] {
            var c = FerretController()
            var snapshot = BackendSnapshot.initial
            snapshot.mascotJobs = []
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            snapshot.mascotJobs = [FerretJobSnapshot(jobId: "j1", status: status, startedAtMs: 2_000_000_000)]
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            XCTAssertEqual(c.presentation.state, .waiting, status)
        }
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let wait = trace(tool: "await_job", start: 1_999_900_000)
        c.update(snapshot: .initial, traces: [wait], activities: [], now: now)
        XCTAssertEqual(c.presentation.state, .waiting)
        c.update(snapshot: .initial, traces: [wait], activities: [], now: now.addingTimeInterval(61))
        XCTAssertEqual(c.presentation.state, .waiting)
        var snapshot = BackendSnapshot.initial
        snapshot.currentOperation = OperationSnapshot(id: "op", kind: "task", phase: "cancelling", startedAtMs: 1_999_900_000, cancellable: true)
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(62))
        XCTAssertEqual(c.presentation.state, .waiting)
    }

    func testReceivingLastsOneSecondAndDoesNotReplayOldWork() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        c.update(snapshot: .initial, traces: [trace()], activities: [], now: now)
        c.update(snapshot: .initial, traces: [trace()], activities: [], now: now.addingTimeInterval(0.8))
        XCTAssertEqual(c.presentation.state, .listening)
        c.update(snapshot: .initial, traces: [trace()], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .searching)
        var restored = FerretController()
        restored.update(snapshot: .initial, traces: [trace(start: 1_999_990_000)], activities: [], now: now)
        XCTAssertEqual(restored.presentation.state, .searching, "Already running work is not a new prompt")
    }

    func testConcurrentToolsProvideActivityWithoutInheritingOldJobOrWaitDuration() {
        for (tool, state) in [("read_files", FerretState.searching), ("write_project_file", .coding), ("go_test", .testing)] {
            var c = FerretController()
            var snapshot = BackendSnapshot.initial
            snapshot.mascotJobs = []
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            snapshot.mascotJobs = [FerretJobSnapshot(jobId: "j1", status: "running", startedAtMs: 1_999_900_000)]
            let wait = trace(1, tool: "await_job", start: 1_999_900_000)
            c.update(snapshot: snapshot, traces: [wait], activities: [], now: now)
            XCTAssertEqual(c.presentation.state, .longTask)
            let work = trace(2, tool: tool, start: 2_000_001_000)
            c.update(snapshot: snapshot, traces: [wait, work], activities: [], now: now.addingTimeInterval(1))
            XCTAssertEqual(c.presentation.state, state)
            XCTAssertEqual(c.presentation.activity, state)
            c.update(snapshot: snapshot, traces: [wait, work], activities: [], now: now.addingTimeInterval(61))
            XCTAssertEqual(c.presentation.state, .longTask)
            XCTAssertEqual(c.presentation.activity, state)
            c.update(snapshot: snapshot, traces: [wait], activities: [], now: now.addingTimeInterval(62))
            XCTAssertEqual(c.presentation.state, .longTask, "Job remains active after tool observation ends")
        }
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        let wait = trace(1, tool: "await_job", start: 1_999_900_000)
        let search = trace(2)
        c.update(snapshot: .initial, traces: [wait, search], activities: [], now: now)
        c.update(snapshot: .initial, traces: [wait, search], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .searching, "Waiting duration is not working duration")
    }

    func testNewFastWorkSupersedesOldSuccessAndDoesNotReplayItAfterExpiry() {
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        snapshot.taskProgress = task("completed", validation: "passed")
        c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .success)
        let edit = trace(tool: "apply_patch", start: 2_000_001_100, finished: 2_000_001_200, completion: "completed")
        c.update(snapshot: snapshot, traces: [edit], activities: [], now: now.addingTimeInterval(1.5))
        XCTAssertEqual(c.presentation.state, .coding)
        XCTAssertTrue(c.presentation.isRecentActivity)
        c.update(snapshot: snapshot, traces: [edit], activities: [], now: now.addingTimeInterval(3.5))
        XCTAssertEqual(c.presentation.state, .idle)
    }

    func testFastWorkRetainsRecentPoseDuringGenericDurableWorkButNotActualWaits() {
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        snapshot.mascotJobs = []
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        snapshot.mascotJobs = [FerretJobSnapshot(jobId: "j1", status: "running", startedAtMs: 1_999_900_000)]
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        let edit = trace(tool: "apply_patch", start: 2_000_001_000, finished: 2_000_001_100, completion: "completed")
        c.update(snapshot: snapshot, traces: [edit], activities: [], now: now.addingTimeInterval(1.5))
        XCTAssertEqual(c.presentation.state, .coding)
        XCTAssertTrue(c.presentation.isRecentActivity)
        XCTAssertNil(c.presentation.progress)
        c.update(snapshot: snapshot, traces: [edit], activities: [], foreground: false, now: now.addingTimeInterval(2))
        XCTAssertEqual(c.presentation.state, .longTask)
        XCTAssertFalse(c.presentation.isRecentActivity)
        snapshot.mascotJobs?[0].status = "recovering"
        c.update(snapshot: snapshot, traces: [edit], activities: [], now: now.addingTimeInterval(2.5))
        XCTAssertEqual(c.presentation.state, .waiting)
        snapshot.mascotJobs?[0].status = "running"
        c.update(snapshot: snapshot, traces: [edit], activities: [], now: now.addingTimeInterval(3.5))
        XCTAssertEqual(c.presentation.state, .longTask)
        XCTAssertFalse(c.presentation.isRecentActivity)
    }

    func testFailureWinsAcrossToolTaskAndConcurrentJobResults() {
        for jobsReversed in [false, true] {
            var c = FerretController()
            var snapshot = BackendSnapshot.initial
            snapshot.mascotJobs = []
            c.update(snapshot: snapshot, traces: [], activities: [], now: now)
            snapshot.taskProgress = task("completed", validation: "passed")
            let failed = FerretJobSnapshot(jobId: "failed", status: "failed", finishedAtMs: 2_000_001_000, exitCode: 1)
            let succeeded = FerretJobSnapshot(jobId: "ok", status: "completed", finishedAtMs: 2_000_001_000, exitCode: 0)
            snapshot.mascotJobs = jobsReversed ? [succeeded, failed] : [failed, succeeded]
            c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(1))
            XCTAssertEqual(c.presentation.state, .error)
            c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(5.9))
            XCTAssertEqual(c.presentation.state, .error)
            c.update(snapshot: snapshot, traces: [], activities: [], now: now.addingTimeInterval(6))
            XCTAssertEqual(c.presentation.state, .idle)
        }
        var c = FerretController()
        var snapshot = BackendSnapshot.initial
        c.update(snapshot: snapshot, traces: [], activities: [], now: now)
        snapshot.taskProgress = task("completed", validation: "passed")
        c.update(snapshot: snapshot, traces: [trace(finished: 2_000_001_000, completion: "backend_error")], activities: [], now: now.addingTimeInterval(1))
        XCTAssertEqual(c.presentation.state, .error)
        let next = trace(2, tool: "write_project_file", start: 2_000_001_100, finished: 2_000_001_200, completion: "completed")
        c.update(snapshot: snapshot, traces: [next], activities: [], now: now.addingTimeInterval(1.5))
        XCTAssertEqual(c.presentation.state, .coding, "New work replaces a previous failure reaction")
    }

    func testActualRuntimeToolNamesMapToSpecificStates() {
        for tool in ["project_overview", "list_project_tracked_files", "document_symbols", "workspace_symbols", "goto_definition", "find_references", "call_hierarchy"] {
            XCTAssertEqual(FerretController.toolState(tool), .searching, tool)
        }
        for tool in ["write_project_file", "apply_unified_diff"] {
            XCTAssertEqual(FerretController.toolState(tool), .coding, tool)
        }
        XCTAssertEqual(FerretController.toolState("go_test"), .testing)
        XCTAssertEqual(FerretController.toolState("cargo_fmt"), .thinking, "Trace lacks check-mode parameters")
    }

    func testDiagnosticsDoNotWakeIdleOrConsumeReaction() {
        var c = FerretController()
        c.update(snapshot: .initial, traces: [], activities: [], now: now)
        c.update(snapshot: .initial, traces: [trace(tool: "runtime_status")], activities: [], now: now.addingTimeInterval(181))
        XCTAssertEqual(c.presentation.state, .sleep)
        c.update(snapshot: .initial, traces: [trace(tool: "runtime_status", finished: 2_000_182_000, completion: "completed")], activities: [], now: now.addingTimeInterval(182))
        XCTAssertEqual(c.presentation.state, .sleep)
    }

    @MainActor
    func testBlinkChangesOnlyEyesAndKeepsHeadAndBodyStationary() throws {
        for state in [FerretState.idle, .coding] {
            func pixels(_ blink: Bool) throws -> NSBitmapImageRep {
                let renderer = ImageRenderer(content: CodeFerretStage(
                    presentation: FerretPresentation(state: state), sampleTime: 1, blinkOverride: blink, sampleElapsed: 0.45)
                    .frame(width: 360, height: 296))
                renderer.scale = 1
                let image = try XCTUnwrap(renderer.nsImage)
                let rep = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(image.tiffRepresentation)))
                if let path = ProcessInfo.processInfo.environment["CHADEX_FERRET_REVIEW_OUTPUT"] {
                    let url = URL(fileURLWithPath: path, isDirectory: true)
                    try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
                    try XCTUnwrap(rep.representation(using: .png, properties: [:]))
                        .write(to: url.appendingPathComponent("blink-\(state.rawValue)-\(blink ? "closed" : "open").png"))
                }
                return rep
            }
            let open = try pixels(false), closed = try pixels(true)
            var changed = 0
            for y in 0..<open.pixelsHigh {
                for x in 0..<open.pixelsWide {
                    let a = try XCTUnwrap(open.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB))
                    let b = try XCTUnwrap(closed.colorAt(x: x, y: y)?.usingColorSpace(.deviceRGB))
                    if abs(a.redComponent - b.redComponent) + abs(a.greenComponent - b.greenComponent)
                        + abs(a.blueComponent - b.blueComponent) + abs(a.alphaComponent - b.alphaComponent) > 0.01 {
                        changed += 1
                        // Pixel bounds for the new neutral and laptop poses at 2x stage scale.
                        // Keep the two eyes separate so moving the muzzle or head still fails.
                        let eyeRegions = state == .idle
                            ? [CGRect(x: 108, y: 76, width: 43, height: 36), CGRect(x: 171, y: 75, width: 35, height: 36)]
                            : [CGRect(x: 175, y: 110, width: 53, height: 44), CGRect(x: 132, y: 118, width: 38, height: 43)]
                        XCTAssertTrue(eyeRegions.contains { $0.contains(CGPoint(x: x, y: y)) },
                                      "Blink moved silhouette at \(x),\(y) in \(state)")
                    }
                }
            }
            XCTAssertGreaterThan(changed, 10, "Blink must visibly close eyes")
            XCTAssertLessThan(changed, 2000, "Blink must remain localized")
        }
    }

    @MainActor
    func testAllAssetsAndRenderStatesInBothAppearances() throws {
        let manifestURL = try XCTUnwrap(L10n.resourceBundle()?.url(forResource: "ferret-motion-poses", withExtension: "json"))
        let manifest = try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: manifestURL)) as? [String: [String: Any]])
        XCTAssertEqual(Set(manifest.keys), Set(FerretState.allCases.map(\.rawValue) + ["tail", "tail-left"]))
        var poseData = Set<Data>()
        for (name, spec) in manifest {
            let eyes = try XCTUnwrap(spec["eyes"] as? [[String: Any]])
            let names = [name] + eyes.compactMap { $0["name"] as? String }
            for asset in names {
                let url = try XCTUnwrap(L10n.resourceBundle()?.url(forResource: "ferret-motion-\(asset)", withExtension: "png"))
                let data = try Data(contentsOf: url)
                let image = try XCTUnwrap(NSBitmapImageRep(data: data))
                XCTAssertTrue(image.hasAlpha, asset)
                XCTAssertGreaterThan(image.pixelsWide, 20, asset)
                if asset == name && !name.hasPrefix("tail") { poseData.insert(data) }
            }
            for eye in eyes {
                let x = try XCTUnwrap(eye["x"] as? Double), y = try XCTUnwrap(eye["y"] as? Double)
                XCTAssertGreaterThanOrEqual(x, 0)
                XCTAssertGreaterThanOrEqual(y, 0)
                XCTAssertLessThanOrEqual(x + (eye["width"] as? Double ?? 0), spec["width"] as? Double ?? 0)
                XCTAssertLessThanOrEqual(y + (eye["height"] as? Double ?? 0), spec["height"] as? Double ?? 0)
            }
        }
        XCTAssertEqual(poseData.count, 11, "Every semantic state needs its own character pose")
        let path = ProcessInfo.processInfo.environment["CHADEX_FERRET_REVIEW_OUTPUT"] ?? NSTemporaryDirectory() + "chadex-ferret-test-render"
        let output = URL(fileURLWithPath: path, isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        for scheme in [ColorScheme.light, .dark] {
            for state in FerretState.allCases {
                let p = FerretPresentation(state: state, activity: state == .longTask ? .testing : nil,
                    progress: state == .testing ? 0.6 : nil, since: now)
                let root = VStack(spacing: 6) {
                    CodeFerretStage(presentation: p, sampleTime: 1, sampleElapsed: 0.45).frame(width: 360, height: 296)
                    Text(state.rawValue).font(.system(size: 14))
                }.padding(12).frame(width: 400, height: 340)
                    .background(Color(nsColor: .windowBackgroundColor))
                    .environment(\.colorScheme, scheme)
                let renderer = ImageRenderer(content: root)
                renderer.scale = 1
                let image = try XCTUnwrap(renderer.nsImage)
                let rep = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(image.tiffRepresentation)))
                let data = try XCTUnwrap(rep.representation(using: .png, properties: [:]))
                try data.write(to: output.appendingPathComponent("\(state.rawValue)-\(scheme == .dark ? "dark" : "light").png"))
                if scheme == .light {
                    for blink in [false, true] {
                        let renderer = ImageRenderer(content: CodeFerretStage(presentation: p, sampleTime: 1,
                            blinkOverride: blink, sampleElapsed: 0.45).frame(width: 360, height: 296))
                        let rep = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(renderer.nsImage?.tiffRepresentation)))
                        try XCTUnwrap(rep.representation(using: .png, properties: [:])).write(to:
                            output.appendingPathComponent("blink-\(state.rawValue)-\(blink ? "closed" : "open").png"))
                    }
                }
            }
        }
    }

    func testReactionsPlayOnceAndPauseWithoutLosingWorkingPose() {
        for state in [FerretState.listening, .success, .error] {
            let early = FerretMotion(state: state, time: 1, elapsed: 0.3, moving: true)
            XCTAssertNotEqual(state == .error ? early.bodyAngle : early.lift, 0)
            for elapsed in [2.0, 5.0, 20.0] {
                let settled = FerretMotion(state: state, time: elapsed, elapsed: elapsed, moving: true)
                XCTAssertEqual(settled.lift, 0, accuracy: 0.00001)
                XCTAssertEqual(settled.bodyAngle, 0, accuracy: 0.00001)
                XCTAssertEqual(settled.reaction, 0)
            }
            let paused = FerretMotion(state: state, time: 1, elapsed: 0.3, moving: false)
            XCTAssertEqual(paused.lift, 0)
            XCTAssertEqual(paused.tailAngle, 0)
            XCTAssertEqual(paused.breath, 0)
        }
    }

    @MainActor
    func testDisabledMotionIsPixelStableAcrossClockAndReactionChanges() throws {
        for state in [FerretState.success, .thinking, .longTask, .sleep] {
            func pixels(_ time: Double) throws -> Data {
                let content = CodeFerretStage(presentation: FerretPresentation(state: state), animated: false,
                    sampleTime: time, sampleElapsed: time).frame(width: 180, height: 148)
                let renderer = ImageRenderer(content: content)
                let rep = try XCTUnwrap(NSBitmapImageRep(data: XCTUnwrap(renderer.nsImage?.tiffRepresentation)))
                let pixels = try XCTUnwrap(rep.bitmapData)
                if let path = ProcessInfo.processInfo.environment["CHADEX_FERRET_REVIEW_OUTPUT"] {
                    let url = URL(fileURLWithPath: path).appendingPathComponent("paused-\(state.rawValue)-\(time).png")
                    try XCTUnwrap(rep.representation(using: .png, properties: [:])).write(to: url)
                }
                return Data(bytes: pixels, count: rep.bytesPerRow * rep.pixelsHigh)
            }
            XCTAssertEqual(try pixels(0.3), try pixels(8), state.rawValue)
        }
    }
}
