import Foundation
import XCTest
@testable import ChadexApp

/// Event frames on helper stdout: routed to `onEvent`, never mistaken for a
/// protocol error. Uses shell stand-ins for the helper, like
/// `HelperClientResilienceTests`.
final class ComputerOverlayHelperEventTests: XCTestCase {
    private final class EventSink: @unchecked Sendable {
        private let lock = NSLock()
        private var stored: [HelperEvent] = []
        func append(_ event: HelperEvent) {
            lock.lock()
            stored.append(event)
            lock.unlock()
        }
        var events: [HelperEvent] {
            lock.lock()
            defer { lock.unlock() }
            return stored
        }
    }

    private func makeHelper(
        replyLines: [String],
        logMethods: URL? = nil
    ) throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-overlay-helper-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let executable = directory.appendingPathComponent("event-helper", isDirectory: false)
        // Lines are emitted before the response, in order. `%ID%` is the request id.
        let printLines = replyLines
            .map { "printf '%s\\n' '\($0.replacingOccurrences(of: "'", with: "'\\''"))'" }
            .joined(separator: "\n  ")
        let log = logMethods.map { "printf '%s\\n' \"$line\" >> '\($0.path)'" } ?? ":"
        let script = """
        #!/bin/sh
        while IFS= read -r line; do
          id=$(printf '%s' "$line" | sed -n 's/.*"request_id":"\\([^"]*\\)".*/\\1/p')
          \(log)
          \(printLines)
          printf '{"protocol_version":1,"request_id":"%s","result":{"ok":true}}\\n' "$id"
        done
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o755))],
            ofItemAtPath: executable.path
        )
        addTeardownBlock { try? FileManager.default.removeItem(at: directory) }
        return executable
    }

    private func willActLine(id: Int) -> String {
        #"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"seq":\#(id),"phase":"will_act","action_id":\#(id),"action":"click","target":{"kind":"point","x":10.0,"y":20.0},"space":"macos_cg_global_pt","ttl_ms":2000,"emitted_at_ms":1}}"#
    }

    func testEventFramesAreDeliveredAndNeverBreakTheRequest() async throws {
        let executable = try makeHelper(replyLines: [
            willActLine(id: 1),
            // Malformed event frame: unknown phase.
            #"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"bogus"}}"#,
            // Unknown event name.
            #"{"protocol_version":1,"event":"brand_new_event","data":{"anything":[1,2,3]}}"#,
            // Event frame that is not even a valid overlay payload.
            #"{"protocol_version":1,"event":"computer_overlay","data":"nope"}"#,
            // Wrong protocol version on an event.
            #"{"protocol_version":2,"event":"computer_overlay","data":{"v":1,"phase":"clear","reason":"stopped"}}"#,
            willActLine(id: 2),
        ])
        let client = HelperClient(executableURL: executable)
        let sink = EventSink()
        client.onEvent = { sink.append($0) }

        let result: JSONValue = try await client.request(
            method: "getStatus",
            params: EmptyParams(),
            timeout: 5
        )
        XCTAssertEqual(result, .object(["ok": .bool(true)]))
        // Read before shutdown: the shutdown request makes this stand-in helper
        // replay the same lines.
        let events = sink.events
        let discarded = client.discardedEventFrameCount
        await client.shutdown()

        XCTAssertEqual(events.count, 2, "only the two valid overlay events are delivered")
        guard case .computerOverlay(let first) = events[0], case .computerOverlay(let second) = events[1] else {
            return XCTFail("unexpected event types")
        }
        XCTAssertEqual(first.actionId, 1)
        XCTAssertEqual(second.actionId, 2, "events keep the order the helper wrote them")
        XCTAssertEqual(discarded, 4, "bad phase, unknown event, bad payload and wrong protocol version")
    }

    func testEventFramesWithoutAHandlerAreHarmless() async throws {
        let executable = try makeHelper(replyLines: [willActLine(id: 1), willActLine(id: 2)])
        let client = HelperClient(executableURL: executable)
        for _ in 0..<3 {
            let result: JSONValue = try await client.request(
                method: "getStatus",
                params: EmptyParams(),
                timeout: 5
            )
            XCTAssertEqual(result, .object(["ok": .bool(true)]))
        }
        await client.shutdown()
    }

    func testAnUnparseableLineStillFailsPendingRequestsAsBefore() async throws {
        let executable = try makeHelper(replyLines: ["this is not json"])
        let client = HelperClient(executableURL: executable)
        do {
            let _: JSONValue = try await client.request(
                method: "getStatus",
                params: EmptyParams(),
                timeout: 5
            )
            XCTFail("a garbage line is still a protocol error")
        } catch HelperClientError.malformedResponse {
            // Expected: only event frames are tolerated.
        } catch HelperClientError.timedOut {
            XCTFail("pending request should fail fast, not time out")
        } catch {
            XCTFail("unexpected error: \(error)")
        }
        await client.shutdown()
    }

    func testLifecycleCallbacksReportStartAndTermination() async throws {
        let executable = try makeHelper(replyLines: [])
        let client = HelperClient(executableURL: executable)
        let started = expectation(description: "started")
        let terminated = expectation(description: "terminated")
        client.onLifecycle = { event in
            switch event {
            case .started: started.fulfill()
            case .terminated: terminated.fulfill()
            }
        }
        let _: JSONValue = try await client.request(method: "getStatus", params: EmptyParams(), timeout: 5)
        await fulfillment(of: [started], timeout: 5)
        await client.shutdown()
        await fulfillment(of: [terminated], timeout: 5)
    }
}
