import CoreGraphics
import XCTest
@testable import ChadexApp

final class ComputerOverlayGeometryTests: XCTestCase {
    private typealias Geometry = ComputerOverlayGeometry

    // MARK: - CG <-> AppKit

    func testSingleScreenConversionFlipsOnlyY() {
        let rect = CGRect(x: 100, y: 50, width: 200, height: 40)
        let converted = Geometry.appKitRect(fromCG: rect, primaryHeight: 982)
        XCTAssertEqual(converted, CGRect(x: 100, y: 892, width: 200, height: 40))
        XCTAssertEqual(
            Geometry.appKitPoint(fromCG: CGPoint(x: 812.5, y: 433), primaryHeight: 982),
            CGPoint(x: 812.5, y: 549)
        )
    }

    func testConversionRoundTripsOnEveryScreenArrangement() {
        // Primary 1512x982 at the origin; secondary placements in CG space.
        let primary = CGRect(x: 0, y: 0, width: 1512, height: 982)
        let arrangements: [(String, CGRect)] = [
            ("right", CGRect(x: 1512, y: 0, width: 1920, height: 1080)),
            ("left (negative x)", CGRect(x: -1920, y: 0, width: 1920, height: 1080)),
            ("above (negative y)", CGRect(x: 0, y: -1080, width: 1920, height: 1080)),
            ("below", CGRect(x: 0, y: 982, width: 1920, height: 1080)),
            ("left, taller than primary", CGRect(x: -2560, y: -458, width: 2560, height: 1440)),
        ]
        for (name, secondary) in arrangements {
            for screen in [primary, secondary] {
                let point = CGPoint(x: screen.midX, y: screen.midY)
                let appKit = Geometry.appKitPoint(fromCG: point, primaryHeight: primary.height)
                let back = Geometry.appKitPoint(fromCG: appKit, primaryHeight: primary.height)
                XCTAssertEqual(back, point, name)
                let rect = screen.insetBy(dx: 10, dy: 10)
                let appKitRect = Geometry.appKitRect(fromCG: rect, primaryHeight: primary.height)
                XCTAssertEqual(
                    Geometry.appKitRect(fromCG: appKitRect, primaryHeight: primary.height),
                    rect,
                    name
                )
                XCTAssertEqual(appKitRect.size, rect.size, name)
            }
        }
    }

    func testSecondaryScreenPositionsMatchAppKitGlobalSpace() {
        // The AppKit frame of a screen is its CG bounds flipped around the primary height.
        let primaryHeight: CGFloat = 982
        // Secondary above the primary: CG y is negative, AppKit y is above primaryHeight.
        let above = CGRect(x: 0, y: -1080, width: 1920, height: 1080)
        XCTAssertEqual(
            Geometry.appKitRect(fromCG: above, primaryHeight: primaryHeight),
            CGRect(x: 0, y: 982, width: 1920, height: 1080)
        )
        // Secondary below the primary: AppKit y goes negative.
        let below = CGRect(x: 0, y: 982, width: 1920, height: 1080)
        XCTAssertEqual(
            Geometry.appKitRect(fromCG: below, primaryHeight: primaryHeight),
            CGRect(x: 0, y: -1080, width: 1920, height: 1080)
        )
        // Secondary to the left, a different height: bottoms differ, tops share CG y = 0.
        let left = CGRect(x: -1440, y: 0, width: 1440, height: 900)
        XCTAssertEqual(
            Geometry.appKitRect(fromCG: left, primaryHeight: primaryHeight),
            CGRect(x: -1440, y: 82, width: 1440, height: 900)
        )
    }

    func testPrimaryHeightRequiresAppKitAndCoreGraphicsToAgree() {
        XCTAssertEqual(Geometry.primaryHeight(appKitPrimaryFrameHeight: 982, cgMainDisplayHeight: 982), 982)
        XCTAssertEqual(Geometry.primaryHeight(appKitPrimaryFrameHeight: 982.3, cgMainDisplayHeight: 982), 982.3)
        XCTAssertNil(Geometry.primaryHeight(appKitPrimaryFrameHeight: 900, cgMainDisplayHeight: 982))
        XCTAssertNil(Geometry.primaryHeight(appKitPrimaryFrameHeight: nil, cgMainDisplayHeight: 982))
        XCTAssertNil(Geometry.primaryHeight(appKitPrimaryFrameHeight: 0, cgMainDisplayHeight: 0))
    }

    // MARK: - Screen resolution

    private let main = OverlayScreen(displayID: 1, cgBounds: CGRect(x: 0, y: 0, width: 1512, height: 982))
    private let left = OverlayScreen(displayID: 2, cgBounds: CGRect(x: -1920, y: -100, width: 1920, height: 1080))

    func testResolveScreenByIdentifierAndMatchingBounds() {
        let display = ComputerOverlayDisplay(id: 2, bounds: CGRect(x: -1920, y: -100, width: 1920, height: 1080))
        XCTAssertEqual(
            Geometry.resolveScreen(display: display, anchor: CGPoint(x: -500, y: 200), screens: [main, left]),
            left
        )
        // Half a point of drift is tolerated.
        let drifting = ComputerOverlayDisplay(id: 2, bounds: CGRect(x: -1919.6, y: -100.4, width: 1920, height: 1080))
        XCTAssertEqual(Geometry.resolveScreen(display: drifting, anchor: .zero, screens: [main, left]), left)
    }

    func testResolveScreenDropsUnknownIdentifierAndChangedBounds() {
        let unknown = ComputerOverlayDisplay(id: 99, bounds: main.cgBounds)
        XCTAssertNil(Geometry.resolveScreen(display: unknown, anchor: .zero, screens: [main, left]))
        let moved = ComputerOverlayDisplay(id: 2, bounds: CGRect(x: -1920, y: 0, width: 1920, height: 1080))
        XCTAssertNil(Geometry.resolveScreen(display: moved, anchor: .zero, screens: [main, left]))
        let resized = ComputerOverlayDisplay(id: 1, bounds: CGRect(x: 0, y: 0, width: 1470, height: 956))
        XCTAssertNil(Geometry.resolveScreen(display: resized, anchor: .zero, screens: [main, left]))
        XCTAssertNil(Geometry.resolveScreen(display: unknown, anchor: .zero, screens: []))
    }

    func testResolveScreenWithoutDisplayUsesTheAnchorPoint() {
        XCTAssertEqual(
            Geometry.resolveScreen(display: nil, anchor: CGPoint(x: -10, y: 5), screens: [main, left]),
            left
        )
        XCTAssertEqual(
            Geometry.resolveScreen(display: nil, anchor: CGPoint(x: 10, y: 5), screens: [main, left]),
            main
        )
        XCTAssertNil(Geometry.resolveScreen(display: nil, anchor: CGPoint(x: 5000, y: 5), screens: [main, left]))
    }

    // MARK: - Layout

    func testPointLayoutCentersA64PointPanelAndPlacesTheTip() throws {
        let layout = try XCTUnwrap(Geometry.layout(
            target: .point(CGPoint(x: 812, y: 433)),
            screen: main,
            primaryHeight: 982
        ))
        XCTAssertEqual(layout.style, .pointer)
        XCTAssertEqual(layout.panelFrame.size, CGSize(width: 64, height: 64))
        // CG center (812, 433) -> AppKit (812, 549).
        XCTAssertEqual(layout.panelFrame.midX, 812)
        XCTAssertEqual(layout.panelFrame.midY, 549)
        XCTAssertEqual(layout.localPoint, CGPoint(x: 32, y: 32))
        XCTAssertEqual(layout.screenID, 1)
    }

    func testPointNearEdgeClampsPanelButKeepsTipExact() throws {
        let layout = try XCTUnwrap(Geometry.layout(
            target: .point(CGPoint(x: 5, y: 5)),
            screen: main,
            primaryHeight: 982
        ))
        // The panel is cut to the screen: 37x37 (5 + 32 each way).
        XCTAssertEqual(layout.panelFrame.size, CGSize(width: 37, height: 37))
        XCTAssertEqual(layout.panelFrame.origin, CGPoint(x: 0, y: 982 - 37))
        // The tip is 5 points from the left and from the top of the screen.
        XCTAssertEqual(layout.localPoint.x, 5)
        XCTAssertEqual(layout.panelFrame.maxY - (layout.panelFrame.minY + layout.localPoint.y), 5)
    }

    func testRectLayoutOutsetsByCGSixPointsAndKeepsTheElementFrame() throws {
        let rect = CGRect(x: 100, y: 200, width: 300, height: 40)
        let layout = try XCTUnwrap(Geometry.layout(target: .rect(rect), screen: main, primaryHeight: 982))
        XCTAssertEqual(layout.style, .frame)
        XCTAssertEqual(layout.panelFrame.size, CGSize(width: 312, height: 52))
        XCTAssertEqual(layout.localRect.size, rect.size)
        XCTAssertEqual(layout.localRect.origin, CGPoint(x: 6, y: 6))
        // AppKit y of the panel: 982 - (200 - 6 + 52) = 736
        XCTAssertEqual(layout.panelFrame.origin, CGPoint(x: 94, y: 736))
    }

    func testRectOnTheSecondaryScreenWithNegativeOrigin() throws {
        let rect = CGRect(x: -1000, y: 100, width: 200, height: 100)
        let layout = try XCTUnwrap(Geometry.layout(target: .rect(rect), screen: left, primaryHeight: 982))
        XCTAssertEqual(layout.style, .frame)
        XCTAssertEqual(layout.panelFrame.origin, CGPoint(x: -1006, y: 982 - 100 - 100 - 6))
        XCTAssertEqual(layout.screenID, 2)
    }

    func testRectCrossingTheScreenEdgeIsClampedToTheScreen() throws {
        let rect = CGRect(x: 1400, y: 900, width: 300, height: 200)
        let layout = try XCTUnwrap(Geometry.layout(target: .rect(rect), screen: main, primaryHeight: 982))
        XCTAssertEqual(layout.style, .frame)
        XCTAssertLessThanOrEqual(layout.panelFrame.maxX, 1512)
        XCTAssertGreaterThanOrEqual(layout.panelFrame.minY, 0)
        // The element frame keeps its true size even though the panel is cut.
        XCTAssertEqual(layout.localRect.size, rect.size)
    }

    func testHugeRectIsDrawnAsAPointerAtItsCenter() throws {
        // 1500x900 of a 1512x982 screen is far above 60 percent.
        let rect = CGRect(x: 6, y: 40, width: 1500, height: 900)
        let layout = try XCTUnwrap(Geometry.layout(target: .rect(rect), screen: main, primaryHeight: 982))
        XCTAssertEqual(layout.style, .pointer)
        XCTAssertEqual(layout.panelFrame.size, CGSize(width: 64, height: 64))
        XCTAssertEqual(layout.panelFrame.midX, rect.midX)
        // Just under the limit still draws a frame.
        let ok = CGRect(x: 100, y: 100, width: 1100, height: 800) // ~59.3 percent
        XCTAssertEqual(
            Geometry.layout(target: .rect(ok), screen: main, primaryHeight: 982)?.style,
            .frame
        )
    }

    func testPathTargetsAnchorAtTheEnd() throws {
        let layout = try XCTUnwrap(Geometry.layout(
            target: .path(from: CGPoint(x: 1, y: 1), to: CGPoint(x: 300, y: 400)),
            screen: main,
            primaryHeight: 982
        ))
        XCTAssertEqual(layout.style, .pointer)
        XCTAssertEqual(layout.panelFrame.midX, 300)
    }

    func testNoGeometryOffScreenAndNonFiniteTargetsProduceNoLayout() {
        XCTAssertNil(Geometry.layout(target: .none, screen: main, primaryHeight: 982))
        XCTAssertNil(Geometry.layout(target: .point(CGPoint(x: 5000, y: 5000)), screen: main, primaryHeight: 982))
        XCTAssertNil(Geometry.layout(target: .point(CGPoint(x: CGFloat.nan, y: 1)), screen: main, primaryHeight: 982))
        XCTAssertNil(Geometry.layout(target: .point(CGPoint(x: 1, y: 1)), screen: main, primaryHeight: 0))
        XCTAssertNil(Geometry.layout(target: .point(CGPoint(x: 1, y: 1)), screen: main, primaryHeight: CGFloat.nan))
    }

    // MARK: - Animation plan

    func testReduceMotionNeverGlidesRipplesOrScales() {
        for action in [OverlayActionKind.click, .move, .press, .key] {
            for previous in [OverlayVisibility.hidden, .shown(screenID: 1)] {
                for sinceLast in [0.0, 0.5, 10] {
                    let plan = OverlayAnimationPlan.plan(
                        action: action,
                        reduceMotion: true,
                        previous: previous,
                        sameScreen: true,
                        sinceLast: sinceLast
                    )
                    XCTAssertNil(plan.glideDuration)
                    XCTAssertEqual(plan.appearDuration, 0)
                    XCTAssertFalse(plan.ripple)
                    XCTAssertEqual(plan.fadeOutDuration, 0.15)
                }
            }
        }
    }

    func testMotionPlanGlidesOnlyOnTheSameScreenSoonAfterAVisibleMarker() {
        let glide = OverlayAnimationPlan.plan(
            action: .click, reduceMotion: false, previous: .shown(screenID: 1), sameScreen: true, sinceLast: 0.4
        )
        XCTAssertEqual(glide.glideDuration, 0.16)
        XCTAssertEqual(glide.appearDuration, 0)
        XCTAssertTrue(glide.ripple)
        XCTAssertEqual(glide.fadeOutDuration, 0.2)

        let crossScreen = OverlayAnimationPlan.plan(
            action: .click, reduceMotion: false, previous: .shown(screenID: 1), sameScreen: false, sinceLast: 0.4
        )
        XCTAssertNil(crossScreen.glideDuration)
        XCTAssertGreaterThan(crossScreen.appearDuration, 0)

        let stale = OverlayAnimationPlan.plan(
            action: .press, reduceMotion: false, previous: .shown(screenID: 1), sameScreen: true, sinceLast: 1.5
        )
        XCTAssertNil(stale.glideDuration)
        XCTAssertFalse(stale.ripple, "only clicks ripple")

        let first = OverlayAnimationPlan.plan(
            action: .move, reduceMotion: false, previous: .hidden, sameScreen: true, sinceLast: 0.1
        )
        XCTAssertNil(first.glideDuration)
    }

    // MARK: - Timing and badges

    func testTTLIsClampedToTheDocumentedRange() {
        XCTAssertEqual(ComputerOverlayTiming.ttl(milliseconds: nil), 2.0)
        XCTAssertEqual(ComputerOverlayTiming.ttl(milliseconds: 0), 0.5)
        XCTAssertEqual(ComputerOverlayTiming.ttl(milliseconds: 100), 0.5)
        XCTAssertEqual(ComputerOverlayTiming.ttl(milliseconds: 2000), 2.0)
        XCTAssertEqual(ComputerOverlayTiming.ttl(milliseconds: 9000), 5.0)
        XCTAssertEqual(ComputerOverlayTiming.ttl(milliseconds: -50), 0.5)
    }

    func testRetentionDependsOnOutcome() {
        XCTAssertEqual(ComputerOverlayTiming.retention(after: .succeeded), 0.8)
        for outcome in [OverlayOutcomeKind.failed, .notStarted, .unknown] {
            XCTAssertEqual(ComputerOverlayTiming.retention(after: outcome), 1.2)
        }
    }

    func testBadgeShowsActionTypeOnlyByDefault() {
        let key = ComputerOverlayKey(name: "enter", modifiers: ["command"])
        XCTAssertFalse(ComputerOverlayStyle.showsKeyNames, "decision 8: no key names by default")
        XCTAssertEqual(OverlayBadge.badge(for: .key, key: key), .symbol("keyboard"))
        XCTAssertEqual(OverlayBadge.badge(for: .input, key: nil), .symbol("character.cursor.ibeam"))
        for action in [OverlayActionKind.click, .move, .press, .focus, .scroll, .activate, .other] {
            XCTAssertNil(OverlayBadge.badge(for: action, key: key))
        }
    }

    func testKeyNamesAreAvailableBehindTheStyleSwitch() {
        let key = ComputerOverlayKey(name: "enter", modifiers: ["shift", "command"])
        XCTAssertEqual(OverlayBadge.badge(for: .key, key: key, showsKeyNames: true), .text("⇧ ⌘ ↩"))
        XCTAssertEqual(
            OverlayBadge.badge(for: .key, key: ComputerOverlayKey(name: "arrow_left", modifiers: []), showsKeyNames: true),
            .text("←")
        )
        // Unknown key names never invent text.
        XCTAssertEqual(
            OverlayBadge.badge(for: .key, key: ComputerOverlayKey(name: "a", modifiers: []), showsKeyNames: true),
            .symbol("keyboard")
        )
    }

    func testActionAndOutcomeWireMapping() {
        XCTAssertEqual(OverlayActionKind(wire: "click"), .click)
        XCTAssertEqual(OverlayActionKind(wire: "activate"), .activate)
        XCTAssertEqual(OverlayActionKind(wire: "drag"), .other)
        XCTAssertEqual(OverlayActionKind(wire: "wheel"), .other)
        XCTAssertEqual(OverlayActionKind(wire: nil), .other)
        XCTAssertEqual(OverlayOutcomeKind(wire: "succeeded"), .succeeded)
        XCTAssertEqual(OverlayOutcomeKind(wire: "not_started"), .notStarted)
        XCTAssertEqual(OverlayOutcomeKind(wire: "nonsense"), .unknown)
    }
}
