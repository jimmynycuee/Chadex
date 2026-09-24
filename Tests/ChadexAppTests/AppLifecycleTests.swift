import AppKit
import XCTest
@testable import ChadexApp

@MainActor
final class AppLifecycleTests: XCTestCase {
    func testDockReopenInvokesMainWindowHandlerWhenNoWindowIsVisible() {
        let delegate = ChadexAppDelegate()
        var reopenCount = 0
        delegate.reopenHandler = { reopenCount += 1 }

        let handled = delegate.applicationShouldHandleReopen(
            NSApplication.shared,
            hasVisibleWindows: false
        )

        XCTAssertTrue(handled)
        XCTAssertEqual(reopenCount, 1)
    }

    func testDockReopenDoesNotCreateDuplicateWhenAWindowIsVisible() {
        let delegate = ChadexAppDelegate()
        var reopenCount = 0
        delegate.reopenHandler = { reopenCount += 1 }

        let handled = delegate.applicationShouldHandleReopen(
            NSApplication.shared,
            hasVisibleWindows: true
        )

        XCTAssertTrue(handled)
        XCTAssertEqual(reopenCount, 0)
    }
}
