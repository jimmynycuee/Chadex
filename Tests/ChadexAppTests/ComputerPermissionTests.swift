@testable import ChadexApp
import XCTest

@MainActor
final class ComputerPermissionTests: XCTestCase {
    func testMissingListsOnlyUngrantedPermissionsInDisplayOrder() {
        XCTAssertEqual(
            ComputerPermissionState(accessibility: false, screenRecording: false).missing,
            [.accessibility, .screenRecording]
        )
        XCTAssertEqual(
            ComputerPermissionState(accessibility: true, screenRecording: false).missing,
            [.screenRecording]
        )
        XCTAssertTrue(ComputerPermissionState(accessibility: true, screenRecording: true).allGranted)
    }

    func testSettingsLinksOpenTheMatchingPrivacyPane() {
        XCTAssertEqual(
            ComputerPermission.accessibility.settingsURL.absoluteString,
            "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility"
        )
        XCTAssertEqual(
            ComputerPermission.screenRecording.settingsURL.absoluteString,
            "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture"
        )
    }

    func testRefreshPicksUpAGrantMadeInSystemSettings() {
        var granted = ComputerPermissionState(accessibility: false, screenRecording: false)
        let monitor = ComputerPermissionMonitor(read: { granted })
        XCTAssertFalse(monitor.state.allGranted)

        granted = ComputerPermissionState(accessibility: true, screenRecording: true)
        monitor.refresh()
        XCTAssertTrue(monitor.state.allGranted)
    }

    func testEveryPermissionStringIsLocalized() {
        for permission in ComputerPermission.allCases {
            for key in [permission.titleKey, permission.purposeKey] {
                XCTAssertNotEqual(L10n.string(key), key, "missing localization for \(key)")
            }
        }
        for key in ["computer.status.needsPermission", "computer.permission.title",
                    "computer.permission.help", "computer.permission.ready",
                    "computer.permission.open", "computer.permission.footnote"] {
            XCTAssertNotEqual(L10n.string(key), key, "missing localization for \(key)")
        }
    }
}
