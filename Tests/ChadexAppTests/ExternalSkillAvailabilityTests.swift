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

    func testSkillInstallFailuresExplainTheRuntimeReason() {
        // A ZIP that wraps the package in one folder (english-tv-coach/SKILL.md)
        // is refused by the runtime with skill_definition_missing.
        let generic = L10n.string("skills.error.installFailed", "x")
        for code in [
            "skill_definition_missing", "skill_frontmatter_missing", "skill_name_missing",
            "skill_definition_invalid_utf8", "skill_install_archive_malformed",
            "skill_install_archive_path_invalid", "skill_install_duplicate_path",
            "skill_install_total_too_large", "skill_install_artifact_changed", "skill_artifact_invalid",
            "skill_install_source_project_forbidden", "skill_key_invalid", "skill_archive_flatten_failed",
            "skill_store_skill_limit_exceeded", "skill_state_changed", "skill_store_capability_unavailable",
        ] {
            let message = SkillManagementErrorMessage.installMessage(forCode: code)
            XCTAssertFalse(message.hasPrefix("skills.error."), "missing localization for \(code)")
            XCTAssertNotEqual(message, generic, code)
        }
        XCTAssertNotEqual(
            SkillManagementErrorMessage.message(forCode: "skill_definition_missing"),
            SkillManagementErrorMessage.message(forCode: "skill_frontmatter_missing")
        )
        XCTAssertEqual(
            SkillManagementErrorMessage.installMessage(forCode: "skill_future_failure", helperMessage: "disk full"),
            "disk full"
        )
        XCTAssertEqual(
            SkillManagementErrorMessage.installMessage(
                forCode: "skill_future_failure",
                helperMessage: SkillManagementErrorMessage.genericInstallHelperMessage
            ),
            SkillManagementErrorMessage.installMessage(forCode: "skill_future_failure")
        )
        let unknown = SkillManagementErrorMessage.installMessage(forCode: "skill_future_failure")
        XCTAssertTrue(unknown.contains("skill_future_failure"), unknown)
        XCTAssertFalse(unknown.hasPrefix("skills.error."))
    }
}
