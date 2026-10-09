import AppKit
import XCTest
@testable import ChadexApp
#if canImport(ScreenCaptureKit)
import ScreenCaptureKit
#endif

/// Live checks that need a real screen and Screen Recording permission for the
/// test runner. Off by default; run with CHADEX_LIVE_CAPTURE_TESTS=1.
///
/// This covers the ScreenCaptureKit path and the window server's sharing state.
/// The runtime's own full-screen capture (`CGDisplayCreateImage`) is obsoleted in
/// the macOS 15 SDK and cannot be called from Swift; it is part of the manual
/// release checklist instead (design section 10.2 item 2).
@MainActor
final class ComputerOverlayLiveCaptureTests: XCTestCase {
    override func setUpWithError() throws {
        try XCTSkipUnless(
            ProcessInfo.processInfo.environment["CHADEX_LIVE_CAPTURE_TESTS"] == "1",
            "set CHADEX_LIVE_CAPTURE_TESTS=1 (needs a display and Screen Recording permission)"
        )
    }

    func testOverlayWindowIsExcludedFromScreenCaptureKitAndReportsNoSharing() async throws {
        _ = NSApplication.shared
        guard let screen = NSScreen.main else { throw XCTSkip("no screen") }
        let panel = ComputerOverlayPanel()
        let side: CGFloat = 80
        let frame = CGRect(
            x: screen.frame.midX - side / 2,
            y: screen.frame.midY - side / 2,
            width: side,
            height: side
        )
        let content = NSView(frame: CGRect(origin: .zero, size: frame.size))
        content.wantsLayer = true
        content.layer?.backgroundColor = NSColor.magenta.cgColor
        panel.contentView = content
        panel.setFrame(frame, display: true)
        panel.orderFrontRegardless()
        defer { panel.orderOut(nil) }
        try await Task.sleep(for: .milliseconds(400))

        // The window server must report the window as not shareable.
        let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]] ?? []
        let ours = windows.first { ($0[kCGWindowName as String] as? String) == ComputerOverlayPanel.windowTitle }
        let info = try XCTUnwrap(ours, "overlay window not found in the window list while shown")
        XCTAssertEqual(info[kCGWindowSharingState as String] as? Int, 0, "kCGWindowSharingState must be none")

        #if canImport(ScreenCaptureKit)
        let content2 = try await SCShareableContent.current
        guard let display = content2.displays.first else { throw XCTSkip("no shareable display") }
        let filter = SCContentFilter(display: display, excludingWindows: [])
        let configuration = SCStreamConfiguration()
        configuration.width = display.width
        configuration.height = display.height
        let image = try await SCScreenshotManager.captureImage(contentFilter: filter, configuration: configuration)
        let rep = NSBitmapImageRep(cgImage: image)
        let scaleX = CGFloat(image.width) / CGFloat(display.width)
        let scaleY = CGFloat(image.height) / CGFloat(display.height)
        let centerX = Int((display.frame.width / 2) * scaleX)
        let centerY = Int((display.frame.height / 2) * scaleY)
        let color = try XCTUnwrap(rep.colorAt(x: centerX, y: centerY)?.usingColorSpace(.sRGB))
        let isMagenta = color.redComponent > 0.9 && color.greenComponent < 0.1 && color.blueComponent > 0.9
        XCTAssertFalse(isMagenta, "the overlay window leaked into a ScreenCaptureKit screenshot")
        #endif
    }
}
