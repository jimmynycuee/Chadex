import Foundation
import XCTest
@testable import ChadexApp

@MainActor
final class GoalNotifierTests: XCTestCase {
    private func run(_ id: String, _ state: String) -> GoalRun {
        GoalRun(goalRunId: id, state: state)
    }

    private func request(_ id: String, state: String = "drafting", origin: String = "goal") -> PlanSummary {
        PlanSummary(planId: id, title: "Fix add", state: state, latestRevision: 1, origin: origin)
    }

    func testFirstObservationOnlySeeds() {
        let overview = GoalOverviewOutput(plans: [request("p1")], goalRuns: [run("r1", "succeeded")])
        let (events, states, requests) = GoalNotifier.events(overview: overview, previousRuns: nil, previousRequests: nil)
        XCTAssertTrue(events.isEmpty)
        XCTAssertEqual(states["r1"], "succeeded")
        XCTAssertEqual(requests, ["p1"])
    }

    func testRunSettlingOnceAndNewRequestNotify() {
        let overview = GoalOverviewOutput(
            plans: [request("p1"), request("p2"), request("p3", origin: "plan"), request("p4", state: "approved")],
            goalRuns: [run("r1", "succeeded"), run("r2", "running"), run("r3", "failed"), run("r4", "needs_attention")]
        )
        let (events, states, _) = GoalNotifier.events(
            overview: overview,
            previousRuns: ["r1": "running", "r2": "queued", "r4": "running"],
            previousRequests: ["p1"]
        )
        // r3 was never seen running: old news. p3 is a Plan Mode plan, p4 not waiting.
        XCTAssertEqual(events, [
            .runSettled(run("r1", "succeeded")),
            .runSettled(run("r4", "needs_attention")),
            .requestWaiting(request("p2")),
        ])
        let (again, _, _) = GoalNotifier.events(overview: overview, previousRuns: states, previousRequests: ["p1", "p2"])
        XCTAssertTrue(again.isEmpty)
    }

    func testOriginDecodes() throws {
        let json = #"{"plan_id":"p","title":"t","state":"drafting","latest_revision":1,"origin":"goal"}"#
        let plan = try JSONDecoder.chadex.decode(PlanSummary.self, from: Data(json.utf8))
        XCTAssertTrue(plan.isGoalRequest)
    }

    func testGitReadinessDecodesAndBlocks() throws {
        let json = #"""
        {"plan":{"plan_id":"p","title":"t","state":"drafting","latest_revision":1},
         "git_readiness":{"is_git_repo":true,"has_commit":false,"head_commit":null,"uncommitted_files":3,
           "blocks_goal_start":true,"project_root":"/tmp/x","message":"m",
           "how_to_commit":["git -C \"/tmp/x\" add -A"],"tips":["check .gitignore"]}}
        """#
        let output = try JSONDecoder.chadex.decode(PlanInspectOutput.self, from: Data(json.utf8))
        let readiness = try XCTUnwrap(output.gitReadiness)
        XCTAssertTrue(readiness.blocks)
        XCTAssertFalse(readiness.hasUncommittedWork)
        XCTAssertEqual(readiness.howToCommit?.count, 1)

        let unknown = #"{"plan":{"plan_id":"p","title":"t","state":"drafting","latest_revision":1},"git_readiness":null}"#
        XCTAssertNil(try JSONDecoder.chadex.decode(PlanInspectOutput.self, from: Data(unknown.utf8)).gitReadiness)

        let dirty = GitReadiness(isGitRepo: true, hasCommit: true, uncommittedFiles: 2, blocksGoalStart: false)
        XCTAssertTrue(dirty.hasUncommittedWork)
        XCTAssertFalse(dirty.blocks)
    }
}
