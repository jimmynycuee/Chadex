import AppKit
import XCTest
@testable import ChadexApp

@MainActor
final class ComputerOverlayPanelTests: XCTestCase {
    func testPanelNeverTakesFocusAndPassesClicksThrough() {
        _ = NSApplication.shared
        let panel = ComputerOverlayPanel()
        XCTAssertEqual(panel.title, ComputerOverlayPanel.windowTitle)
        XCTAssertEqual(ComputerOverlayPanel.windowTitle, "Chadex Agent Cursor")
        XCTAssertTrue(panel.ignoresMouseEvents)
        XCTAssertFalse(panel.canBecomeKey)
        XCTAssertFalse(panel.canBecomeMain)
        XCTAssertFalse(panel.hidesOnDeactivate)
        XCTAssertFalse(panel.hasShadow)
        XCTAssertFalse(panel.isOpaque)
        XCTAssertEqual(panel.backgroundColor, .clear)
        XCTAssertTrue(panel.isFloatingPanel)
        XCTAssertTrue(panel.becomesKeyOnlyIfNeeded)
        XCTAssertFalse(panel.isReleasedWhenClosed)
        XCTAssertTrue(panel.isExcludedFromWindowsMenu)
        XCTAssertTrue(panel.styleMask.contains(.nonactivatingPanel))
        XCTAssertTrue(panel.styleMask.contains(.borderless))
        XCTAssertFalse(panel.isAccessibilityElement())
    }

    func testPanelIsExcludedFromCaptureAndFollowsTheUserAcrossSpaces() {
        _ = NSApplication.shared
        let panel = ComputerOverlayPanel()
        XCTAssertEqual(panel.sharingType, .none)
        XCTAssertTrue(panel.collectionBehavior.contains(.canJoinAllSpaces))
        XCTAssertTrue(panel.collectionBehavior.contains(.fullScreenAuxiliary))
        XCTAssertTrue(panel.collectionBehavior.contains(.stationary))
        XCTAssertTrue(panel.collectionBehavior.contains(.ignoresCycle))
        XCTAssertFalse(
            panel.collectionBehavior.contains(.moveToActiveSpace),
            "canJoinAllSpaces and moveToActiveSpace are mutually exclusive"
        )
        XCTAssertEqual(panel.level, ComputerOverlayPanel.windowLevel)
        XCTAssertGreaterThan(panel.level.rawValue, NSWindow.Level.popUpMenu.rawValue)
    }

    func testPanelStaysOutOfTheWindowListUntilShown() {
        _ = NSApplication.shared
        let presenter = ComputerOverlayPanelPresenter()
        // Nothing is created until the first event.
        presenter.hideImmediately()
        XCTAssertFalse(NSApp.windows.contains { $0.title == ComputerOverlayPanel.windowTitle })
    }

    func testContentViewNeverCapturesTheMouse() {
        let view = ComputerOverlayContentView(frame: CGRect(x: 0, y: 0, width: 64, height: 64))
        XCTAssertNil(view.hitTest(CGPoint(x: 10, y: 10)))
        XCTAssertFalse(view.isAccessibilityElement())
    }
}
