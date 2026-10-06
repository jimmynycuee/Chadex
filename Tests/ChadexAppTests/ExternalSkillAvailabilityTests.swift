import Foundation
import XCTest
@testable import ChadexApp

final class ExternalSkillAvailabilityTests: XCTestCase {
    private func source(
        status: String = "available",
        canonical: String? = "/s",
        valid: Int = 1,
        providedBy: [String] = []
    ) -> ExternalSkillSource {
        ExternalSkillSource(
            kind: "agents", path: "/s", canonicalPath: canonical, status: status, rootIsLink: false,
            sameAs: nil, validCount: valid, symlinkCount: 0, invalidCount: 0, scriptCount: 0,
            truncated: false, providedBy: providedBy, packages: []
        )
    }

    private func discovery(_ sources: [ExternalSkillSource]) -> ExternalSkillSourceDiscovery {
        ExternalSkillSourceDiscovery(format: "f", sources: sources, recommendedRoots: [])
    }

    private func roots(_ list: [String]) -> ExternalSkillRootsState {
        ExternalSkillRootsState(format: "f", roots: list, scriptRoots: [], revision: "r", generation: nil)
    }

    func testLoadingOrFailedDiscoveryIsUnknownNotNone() {
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: nil, roots: nil, loading: true), .unknown)
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: discovery([]), roots: roots([]), loading: true), .unknown)
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: nil, roots: roots([]), loading: false), .unknown)
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: discovery([]), roots: nil, loading: false), .unknown)
    }

    func testNothingUsableAfterLoadIsNone() {
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: discovery([]), roots: roots([]), loading: false), .none)
        let unusable = [
            source(valid: 0),
            source(status: "duplicate_source"),
            source(canonical: nil),
            source(valid: 0, providedBy: ["/x"]),
            source(status: "not_found"),
        ]
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: discovery(unusable), roots: roots([]), loading: false), .none)
    }

    func testConnectableSourceWithSkillsOrConnectedRootIsAvailable() {
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: discovery([source()]), roots: roots([]), loading: false), .available)
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: discovery([]), roots: roots(["/a"]), loading: false), .available)
        XCTAssertEqual(ExternalSkillAvailability.evaluate(discovery: nil, roots: roots(["/a"]), loading: true), .available)
    }

    func testSkillManagementErrorCodesHaveMessages() {
        for code in [
            "skill_management_requires_local_runtime", "skill_management_credential_unavailable",
            "skill_management_credential_rejected", "runner_config_path_is_link",
        ] {
            let message = SkillManagementErrorMessage.message(forCode: code)
            XCTAssertNotNil(message, code)
            XCTAssertFalse(message?.hasPrefix("skills.error.") ?? true, "missing localization for \(code)")
        }
        XCTAssertNil(SkillManagementErrorMessage.message(forCode: "something_else"))
    }
}
