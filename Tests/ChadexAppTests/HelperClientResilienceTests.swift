import Foundation
import XCTest
@testable import ChadexApp

final class HelperClientResilienceTests: XCTestCase {
    private func makeSilentHelper() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-helper-test-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let executable = directory.appendingPathComponent("silent-helper", isDirectory: false)
        let script = """
        #!/bin/sh
        while IFS= read -r line; do
          :
        done
        """
        try script.write(to: executable, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes(
            [.posixPermissions: NSNumber(value: Int16(0o755))],
            ofItemAtPath: executable.path
        )
        addTeardownBlock {
            try? FileManager.default.removeItem(at: directory)
        }
        return executable
    }

    func testRequestDeadlineResumesExactlyOnceWhenHelperNeverReplies() async throws {
        let client = HelperClient(executableURL: try makeSilentHelper())
        defer { Task { await client.shutdown() } }

        do {
            let _: BackendSnapshot = try await client.request(
                method: "getStatus",
                params: EmptyParams(),
                timeout: 0.1
            )
            XCTFail("silent helper should time out")
        } catch HelperClientError.timedOut(let method) {
            XCTAssertEqual(method, "getStatus")
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testTaskCancellationResumesSilentHelperRequest() async throws {
        let client = HelperClient(executableURL: try makeSilentHelper())
        defer { Task { await client.shutdown() } }

        let request = Task {
            try await client.request(
                method: "getStatus",
                params: EmptyParams(),
                as: BackendSnapshot.self,
                timeout: 5
            )
        }
        try await Task.sleep(for: .milliseconds(50))
        request.cancel()

        do {
            _ = try await request.value
            XCTFail("cancelled request should not complete successfully")
        } catch HelperClientError.cancelled {
            // Expected.
        } catch {
            XCTFail("unexpected error: \(error)")
        }
    }

    func testShutdownDeadlineStartsBeforeUnresponsiveShutdownRPC() async throws {
        let client = HelperClient(executableURL: try makeSilentHelper())
        try client.startIfNeeded()
        let started = ProcessInfo.processInfo.systemUptime

        await client.shutdown()

        XCTAssertLessThan(
            ProcessInfo.processInfo.systemUptime - started,
            3.0,
            "shutdown should not wait for the helper RPC before starting its watchdog"
        )
    }
}
