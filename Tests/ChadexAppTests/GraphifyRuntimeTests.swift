import Foundation
import XCTest
@testable import ChadexApp

final class GraphifyRuntimeTests: XCTestCase {
    func testHelperEnvironmentResolvesExplicitGraphifyOverride() {
        let path = "/Users/test/Library/Python/3.13/bin/graphify"
        let environment = HelperClient.environmentForHelper(
            baseEnvironment: [
                "CHADEX_GRAPHIFY_BIN": path,
                "PATH": "/usr/bin"
            ],
            homeDirectory: URL(fileURLWithPath: "/Users/test"),
            isExecutable: { $0 == path }
        )

        XCTAssertEqual(environment["CHADEX_GRAPHIFY_BIN"], path)
        XCTAssertEqual(environment["PATH"], "/Users/test/Library/Python/3.13/bin:/usr/bin")
    }

    func testHelperEnvironmentDoesNotDuplicateGraphifyDirectory() {
        XCTAssertEqual(
            HelperClient.prependPath("/usr/local/bin", to: "/usr/local/bin:/usr/bin"),
            "/usr/local/bin:/usr/bin"
        )
    }

    func testBackendSnapshotCarriesGraphifyStatus() throws {
        let json = #"""
        {
          "phase": "stopped",
          "graphify": {
            "available": true,
            "path": "/Users/test/Library/Python/3.13/bin/graphify",
            "source": "CHADEX_GRAPHIFY_BIN"
          },
          "selected_project": null,
          "tunnel_ready": false,
          "chat_gpt_connected": false,
          "chat_gpt_verified_for_selected_project": false,
          "last_verified_at_ms": null,
          "current_operation": null,
          "error": null,
          "activity_sequence": 0,
          "state_revision": 1
        }
        """#.data(using: .utf8)!

        let snapshot = try JSONDecoder.chadex.decode(BackendSnapshot.self, from: json)
        XCTAssertEqual(snapshot.graphify?.available, true)
        XCTAssertEqual(snapshot.graphify?.source, "CHADEX_GRAPHIFY_BIN")
        XCTAssertEqual(snapshot.graphify?.path, "/Users/test/Library/Python/3.13/bin/graphify")
    }
}
