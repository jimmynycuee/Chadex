import CoreGraphics
import Foundation

/// One display as the overlay sees it. `cgBounds` is `CGDisplayBounds`: global
/// Quartz points, origin at the top-left of the main display, y down.
struct OverlayScreen: Equatable, Sendable {
    var displayID: UInt32
    var cgBounds: CGRect
}

/// How a target is drawn.
enum OverlayLayoutStyle: Equatable, Sendable {
    /// Cursor arrow at a point.
    case pointer
    /// Rounded frame around an element.
    case frame
}

/// Resolved placement. Everything in `localPoint` / `localRect` is in the
/// panel's own coordinate space (AppKit orientation: origin bottom-left, y up).
struct OverlayLayout: Equatable, Sendable {
    var style: OverlayLayoutStyle
    /// Panel frame in AppKit global coordinates.
    var panelFrame: CGRect
    var screenID: UInt32
    /// Pointer tip (style `.pointer`), panel coordinates.
    var localPoint: CGPoint
    /// Element frame (style `.frame`), panel coordinates. Equal to a zero rect
    /// for `.pointer`.
    var localRect: CGRect
}

/// Pure coordinate math for the overlay. No AppKit, so it is unit-testable.
enum ComputerOverlayGeometry {
    static let pointerPanelSize: CGFloat = 64
    static let frameOutset: CGFloat = 6
    /// A frame covering more of the screen than this is drawn as a pointer at its center.
    static let maxFrameScreenFraction: CGFloat = 0.6
    /// Allowed drift between the event's display bounds and the current ones.
    static let boundsTolerance: CGFloat = 0.5

    /// CG global (top-left origin, y down) to AppKit global (primary-screen
    /// bottom-left origin, y up). `primaryHeight` is the height of the display
    /// whose AppKit frame origin is zero, never `NSScreen.main`.
    static func appKitRect(fromCG rect: CGRect, primaryHeight: CGFloat) -> CGRect {
        CGRect(x: rect.minX, y: primaryHeight - rect.maxY, width: rect.width, height: rect.height)
    }

    static func appKitPoint(fromCG point: CGPoint, primaryHeight: CGFloat) -> CGPoint {
        CGPoint(x: point.x, y: primaryHeight - point.y)
    }

    /// Primary height from AppKit, accepted only when it agrees with
    /// `CGDisplayBounds(CGMainDisplayID())`. A disagreement means the screen
    /// configuration is changing, so the caller drops the event.
    static func primaryHeight(
        appKitPrimaryFrameHeight: CGFloat?,
        cgMainDisplayHeight: CGFloat
    ) -> CGFloat? {
        guard let appKitPrimaryFrameHeight,
              appKitPrimaryFrameHeight > 0,
              abs(appKitPrimaryFrameHeight - cgMainDisplayHeight) <= boundsTolerance
        else { return nil }
        return appKitPrimaryFrameHeight
    }

    /// The screen an event refers to, or nil when the layout changed on the way
    /// (drawing at a wrong position is worse than not drawing).
    static func resolveScreen(
        display: ComputerOverlayDisplay?,
        anchor: CGPoint,
        screens: [OverlayScreen]
    ) -> OverlayScreen? {
        if let display {
            guard let screen = screens.first(where: { $0.displayID == display.id }),
                  boundsMatch(display.bounds, screen.cgBounds)
            else { return nil }
            return screen
        }
        return screens.first { $0.cgBounds.contains(anchor) }
    }

    static func boundsMatch(_ lhs: CGRect, _ rhs: CGRect) -> Bool {
        abs(lhs.minX - rhs.minX) <= boundsTolerance
            && abs(lhs.minY - rhs.minY) <= boundsTolerance
            && abs(lhs.width - rhs.width) <= boundsTolerance
            && abs(lhs.height - rhs.height) <= boundsTolerance
    }

    /// Point the overlay anchors to: the point itself, the center of a rect,
    /// or the end of a path. `nil` for `.none`.
    static func anchor(of target: ComputerOverlayTarget) -> CGPoint? {
        switch target {
        case .point(let point): return point
        case .rect(let rect): return CGPoint(x: rect.midX, y: rect.midY)
        case .path(_, let to): return to
        case .none: return nil
        }
    }

    /// Placement for a target on a resolved screen, or nil when there is
    /// nothing to draw (no geometry, non-finite values, or fully off screen).
    static func layout(
        target: ComputerOverlayTarget,
        screen: OverlayScreen,
        primaryHeight: CGFloat
    ) -> OverlayLayout? {
        guard primaryHeight.isFinite, primaryHeight > 0, let anchor = anchor(of: target),
              anchor.x.isFinite, anchor.y.isFinite
        else { return nil }

        let style: OverlayLayoutStyle
        let desiredCG: CGRect
        switch target {
        case .rect(let rect) where isDrawableFrame(rect, on: screen):
            style = .frame
            desiredCG = rect.insetBy(dx: -frameOutset, dy: -frameOutset)
        default:
            style = .pointer
            desiredCG = CGRect(
                x: anchor.x - pointerPanelSize / 2,
                y: anchor.y - pointerPanelSize / 2,
                width: pointerPanelSize,
                height: pointerPanelSize
            )
        }

        // The panel never leaves its screen; the marker is drawn relative to it.
        let visibleCG = desiredCG.intersection(screen.cgBounds)
        guard !visibleCG.isNull, visibleCG.width > 0, visibleCG.height > 0 else { return nil }
        let panelFrame = appKitRect(fromCG: visibleCG, primaryHeight: primaryHeight)

        switch style {
        case .pointer:
            let tip = appKitPoint(fromCG: anchor, primaryHeight: primaryHeight)
            return OverlayLayout(
                style: .pointer,
                panelFrame: panelFrame,
                screenID: screen.displayID,
                localPoint: CGPoint(x: tip.x - panelFrame.minX, y: tip.y - panelFrame.minY),
                localRect: .zero
            )
        case .frame:
            guard case .rect(let rect) = target else { return nil }
            let elementFrame = appKitRect(fromCG: rect, primaryHeight: primaryHeight)
            return OverlayLayout(
                style: .frame,
                panelFrame: panelFrame,
                screenID: screen.displayID,
                localPoint: .zero,
                localRect: elementFrame.offsetBy(dx: -panelFrame.minX, dy: -panelFrame.minY)
            )
        }
    }

    static func isDrawableFrame(_ rect: CGRect, on screen: OverlayScreen) -> Bool {
        guard rect.width > 0, rect.height > 0 else { return false }
        let screenArea = screen.cgBounds.width * screen.cgBounds.height
        guard screenArea > 0 else { return false }
        return rect.width * rect.height <= screenArea * maxFrameScreenFraction
    }
}

/// Where the overlay currently is. (Not named `State`: that clashes with the
/// SwiftUI property wrapper type in this module.)
enum OverlayVisibility: Equatable, Sendable {
    case hidden
    case shown(screenID: UInt32)
}

/// Motion decisions for one `will_act`, as a pure function of the inputs.
struct OverlayAnimationPlan: Equatable, Sendable {
    /// `nil` places the marker directly; otherwise glide from the previous one.
    var glideDuration: TimeInterval?
    /// `0` appears immediately.
    var appearDuration: TimeInterval
    var ripple: Bool
    var fadeOutDuration: TimeInterval

    static let glide: TimeInterval = 0.16
    static let appear: TimeInterval = 0.12
    static let fadeOut: TimeInterval = 0.2
    static let reducedMotionFadeOut: TimeInterval = 0.15
    static let rippleDuration: TimeInterval = 0.35
    /// A previous marker only counts for gliding when the last event is this recent.
    static let glideWindow: TimeInterval = 1.5

    static func plan(
        action: OverlayActionKind,
        reduceMotion: Bool,
        previous: OverlayVisibility,
        sameScreen: Bool,
        sinceLast: TimeInterval
    ) -> OverlayAnimationPlan {
        if reduceMotion {
            // No movement, no scaling, no ripple: the marker just appears and fades.
            return OverlayAnimationPlan(
                glideDuration: nil,
                appearDuration: 0,
                ripple: false,
                fadeOutDuration: reducedMotionFadeOut
            )
        }
        var glideDuration: TimeInterval?
        if case .shown = previous, sameScreen, sinceLast >= 0, sinceLast < glideWindow {
            glideDuration = glide
        }
        return OverlayAnimationPlan(
            glideDuration: glideDuration,
            appearDuration: glideDuration == nil ? appear : 0,
            ripple: action == .click,
            fadeOutDuration: fadeOut
        )
    }
}

/// Timing constants of the controller (design section 7.4).
enum ComputerOverlayTiming {
    static let retainAfterSuccess: TimeInterval = 0.8
    static let retainAfterFailure: TimeInterval = 1.2
    static let minTTL: TimeInterval = 0.5
    static let maxTTL: TimeInterval = 5.0
    static let defaultTTL: TimeInterval = 2.0
    /// Events older than this when they arrive are dropped.
    static let staleEventLimitMs: UInt64 = 1000

    static func ttl(milliseconds: Int?) -> TimeInterval {
        guard let milliseconds else { return defaultTTL }
        return min(max(TimeInterval(milliseconds) / 1000, minTTL), maxTTL)
    }

    static func retention(after outcome: OverlayOutcomeKind) -> TimeInterval {
        outcome.isSuccess ? retainAfterSuccess : retainAfterFailure
    }
}

/// Style switches that are decisions rather than code paths. Flip a value here
/// to change behavior; there is no other place to touch.
enum ComputerOverlayStyle {
    /// Decision 8 (pending the user's confirmation): the key badge shows the
    /// kind of action only, not which key. Set to true to show names such as
    /// "⌘ ↩". The runtime already sends the closed-vocabulary key name.
    static let showsKeyNames = false
}

/// Small label drawn next to the marker. Pure so the decision is testable.
enum OverlayBadge: Equatable, Sendable {
    /// SF Symbol only: tells what kind of action it is.
    case symbol(String)
    /// Text such as "⌘ ↩".
    case text(String)

    static func badge(
        for action: OverlayActionKind,
        key: ComputerOverlayKey?,
        showsKeyNames: Bool = ComputerOverlayStyle.showsKeyNames
    ) -> OverlayBadge? {
        switch action {
        case .key:
            if showsKeyNames, let key, let text = keyText(name: key.name, modifiers: key.modifiers) {
                return .text(text)
            }
            return .symbol("keyboard")
        case .input:
            // The text cursor icon only; the typed text never reaches the App.
            return .symbol("character.cursor.ibeam")
        default:
            return nil
        }
    }

    static func keyText(name: String, modifiers: [String]) -> String? {
        let glyphs: [String: String] = [
            "enter": "↩", "escape": "⎋", "tab": "⇥",
            "arrow_up": "↑", "arrow_down": "↓", "arrow_left": "←", "arrow_right": "→",
            "page_up": "⇞", "page_down": "⇟", "home": "↖", "end": "↘"
        ]
        let modifierGlyphs: [(String, String)] = [
            ("control", "⌃"), ("option", "⌥"), ("shift", "⇧"), ("command", "⌘")
        ]
        guard let glyph = glyphs[name] else { return nil }
        let prefix = modifierGlyphs.compactMap { modifiers.contains($0.0) ? $0.1 : nil }
        return (prefix + [glyph]).joined(separator: " ")
    }
}
