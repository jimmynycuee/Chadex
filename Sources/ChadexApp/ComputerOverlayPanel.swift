import AppKit
import QuartzCore

/// The transparent, click-through window that carries the agent cursor.
///
/// Rules (design section 7.2): never key, never main, never activates Chadex,
/// shown only with `orderFrontRegardless()`, hidden with `orderOut(nil)` so it
/// is absent from the window list while idle, and excluded from screen
/// sharing / capture with `sharingType = .none`.
final class ComputerOverlayPanel: NSPanel {
    /// The runtime hides the window with exactly this title from `list_windows`.
    /// Keep in sync with `OVERLAY_WINDOW_TITLE` in chadex-runtime-computer.
    static let windowTitle = "Chadex Agent Cursor"

    init() {
        super.init(
            contentRect: .zero,
            styleMask: [.borderless, .nonactivatingPanel],
            backing: .buffered,
            defer: true
        )
        title = Self.windowTitle
        isOpaque = false
        backgroundColor = .clear
        hasShadow = false
        ignoresMouseEvents = true
        // TODO(release gate, design 12.5 / 10.2 item 2): `.none` is not guaranteed to
        // keep this window out of every capture path (ScreenCaptureKit on macOS 15+,
        // system screenshots, meeting apps). There is deliberately no per-OS-version
        // auto-disable yet. Verify on macOS 14, 15 and 26 before release; if a
        // supported version still captures it, add an OverlayCapturePolicy here (or
        // in ComputerOverlayController.isEnabled) that turns the overlay off there.
        sharingType = .none
        isFloatingPanel = true
        hidesOnDeactivate = false
        becomesKeyOnlyIfNeeded = true
        isReleasedWhenClosed = false
        isExcludedFromWindowsMenu = true
        animationBehavior = .none
        level = Self.windowLevel
        collectionBehavior = Self.collectionBehaviorFlags
        setAccessibilityElement(false)
    }

    /// Above menus and pop-ups so an AX-pressed menu item is not hidden by its
    /// own menu. If this ever covers system UI, fall back to `.statusBar`.
    static var windowLevel: NSWindow.Level {
        NSWindow.Level(rawValue: Int(CGWindowLevelForKey(.assistiveTechHighWindow)))
    }

    /// `.canJoinAllSpaces` and `.moveToActiveSpace` are mutually exclusive: only the former.
    static let collectionBehaviorFlags: NSWindow.CollectionBehavior = [
        .canJoinAllSpaces, .fullScreenAuxiliary, .stationary, .ignoresCycle
    ]

    override var canBecomeKey: Bool { false }
    override var canBecomeMain: Bool { false }
}

/// Layer-backed drawing of the marker. Pure drawing: all decisions come from
/// the `ComputerOverlayPresentation` and `OverlayAnimationPlan`.
final class ComputerOverlayContentView: NSView {
    private let frameLayer = CAShapeLayer()
    private let pointerLayer = CAShapeLayer()
    private let ringLayer = CAShapeLayer()
    private let rippleLayer = CAShapeLayer()
    private let badgeView = NSView()
    private let badgeImageView = NSImageView()
    private let badgeLabel = NSTextField(labelWithString: "")

    override init(frame frameRect: NSRect) {
        super.init(frame: frameRect)
        wantsLayer = true
        layer?.masksToBounds = false
        for shape in [frameLayer, ringLayer, rippleLayer, pointerLayer] {
            shape.fillColor = nil
            shape.isHidden = true
            layer?.addSublayer(shape)
        }
        badgeView.wantsLayer = true
        badgeView.isHidden = true
        badgeView.addSubview(badgeImageView)
        badgeView.addSubview(badgeLabel)
        badgeLabel.isHidden = true
        badgeImageView.isHidden = true
        addSubview(badgeView)
        setAccessibilityElement(false)
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError("init(coder:) is not supported") }

    override var isFlipped: Bool { false }
    override func hitTest(_ point: NSPoint) -> NSView? { nil }

    // MARK: - Drawing

    func apply(_ presentation: ComputerOverlayPresentation, plan: OverlayAnimationPlan) {
        let accent = NSColor.controlAccentColor
        let lineWidth: CGFloat = NSWorkspace.shared.accessibilityDisplayShouldIncreaseContrast ? 3 : 2
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }

        removeAnimations()
        resetTint(accent: accent, lineWidth: lineWidth)

        switch presentation.layout.style {
        case .pointer:
            frameLayer.isHidden = true
            let tip = presentation.layout.localPoint
            pointerLayer.path = Self.arrowPath(tip: tip)
            pointerLayer.isHidden = false
            ringLayer.path = Self.circlePath(center: tip, radius: 14)
            ringLayer.isHidden = false
            rippleLayer.path = Self.circlePath(center: tip, radius: 14)
            rippleLayer.isHidden = true
            if plan.ripple { runRipple(center: tip) }
        case .frame:
            pointerLayer.isHidden = true
            ringLayer.isHidden = true
            rippleLayer.isHidden = true
            let rect = presentation.layout.localRect.insetBy(dx: -2, dy: -2)
            frameLayer.path = CGPath(
                roundedRect: rect,
                cornerWidth: 6,
                cornerHeight: 6,
                transform: nil
            )
            frameLayer.isHidden = false
        }
        layoutBadge(presentation)
    }

    /// Failed / not-started actions turn the ring orange; no text is ever shown.
    func applyOutcome(_ outcome: OverlayOutcomeKind) {
        guard outcome == .failed || outcome == .notStarted else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        ringLayer.strokeColor = NSColor.systemOrange.cgColor
        frameLayer.strokeColor = NSColor.systemOrange.cgColor
        CATransaction.commit()
    }

    func clear() {
        removeAnimations()
        for shape in [frameLayer, pointerLayer, ringLayer, rippleLayer] { shape.isHidden = true }
        badgeView.isHidden = true
    }

    private func resetTint(accent: NSColor, lineWidth: CGFloat) {
        frameLayer.strokeColor = accent.cgColor
        frameLayer.lineWidth = lineWidth
        pointerLayer.fillColor = accent.cgColor
        pointerLayer.strokeColor = NSColor.white.cgColor
        pointerLayer.lineWidth = 1.5
        pointerLayer.lineJoin = .round
        ringLayer.strokeColor = accent.withAlphaComponent(0.6).cgColor
        ringLayer.lineWidth = lineWidth
        rippleLayer.strokeColor = accent.cgColor
        rippleLayer.lineWidth = lineWidth
    }

    private func removeAnimations() {
        for shape in [frameLayer, pointerLayer, ringLayer, rippleLayer] {
            shape.removeAllAnimations()
        }
    }

    private func runRipple(center: CGPoint) {
        rippleLayer.isHidden = false
        let duration = OverlayAnimationPlan.rippleDuration
        let scale = CABasicAnimation(keyPath: "transform")
        // Scale around the ripple center, not the layer origin.
        let from = Self.scaleTransform(0.6, around: center)
        let to = Self.scaleTransform(1.4, around: center)
        scale.fromValue = NSValue(caTransform3D: from)
        scale.toValue = NSValue(caTransform3D: to)
        let fade = CABasicAnimation(keyPath: "opacity")
        fade.fromValue = 0.8
        fade.toValue = 0.0
        let group = CAAnimationGroup()
        group.animations = [scale, fade]
        group.duration = duration
        group.timingFunction = CAMediaTimingFunction(name: .easeOut)
        group.fillMode = .forwards
        group.isRemovedOnCompletion = false
        rippleLayer.add(group, forKey: "ripple")
    }

    private static func scaleTransform(_ scale: CGFloat, around center: CGPoint) -> CATransform3D {
        var transform = CATransform3DMakeTranslation(center.x, center.y, 0)
        transform = CATransform3DScale(transform, scale, scale, 1)
        return CATransform3DTranslate(transform, -center.x, -center.y, 0)
    }

    // MARK: - Badge

    private func layoutBadge(_ presentation: ComputerOverlayPresentation) {
        guard let badge = presentation.badge else {
            badgeView.isHidden = true
            return
        }
        let accent = NSColor.controlAccentColor
        let size: CGSize
        switch badge {
        case .symbol(let name):
            let image = NSImage(systemSymbolName: name, accessibilityDescription: nil)
            badgeImageView.image = image
            badgeImageView.contentTintColor = .white
            badgeImageView.isHidden = false
            badgeLabel.isHidden = true
            size = CGSize(width: 26, height: 20)
            badgeImageView.frame = CGRect(x: 4, y: 2, width: size.width - 8, height: size.height - 4)
        case .text(let text):
            badgeLabel.stringValue = text
            badgeLabel.font = .systemFont(ofSize: 11, weight: .semibold)
            badgeLabel.textColor = .white
            badgeLabel.sizeToFit()
            badgeLabel.isHidden = false
            badgeImageView.isHidden = true
            size = CGSize(width: badgeLabel.frame.width + 12, height: 20)
            badgeLabel.frame.origin = CGPoint(x: 6, y: (size.height - badgeLabel.frame.height) / 2)
        }
        badgeView.layer?.backgroundColor = accent.withAlphaComponent(0.9).cgColor
        badgeView.layer?.cornerRadius = 6
        badgeView.frame = badgeFrame(for: presentation, size: size)
        badgeView.isHidden = false
    }

    private func badgeFrame(for presentation: ComputerOverlayPresentation, size: CGSize) -> CGRect {
        var origin: CGPoint
        switch presentation.layout.style {
        case .frame:
            let rect = presentation.layout.localRect
            // key: bottom center; input: left side (text-cursor icon).
            if presentation.action == .input {
                origin = CGPoint(x: rect.minX + 4, y: rect.midY - size.height / 2)
            } else {
                origin = CGPoint(x: rect.midX - size.width / 2, y: rect.minY + 4)
            }
        case .pointer:
            let tip = presentation.layout.localPoint
            origin = CGPoint(x: tip.x + 16, y: tip.y - 30)
        }
        // Keep inside the panel.
        origin.x = min(max(origin.x, 0), max(bounds.width - size.width, 0))
        origin.y = min(max(origin.y, 0), max(bounds.height - size.height, 0))
        return CGRect(origin: origin, size: size)
    }

    // MARK: - Paths

    /// Arrow cursor with its tip at `tip`; AppKit orientation (y up), so the body
    /// extends down and to the right.
    static func arrowPath(tip: CGPoint) -> CGPath {
        let path = CGMutablePath()
        let points: [CGPoint] = [
            CGPoint(x: 0, y: 0), CGPoint(x: 0, y: -17), CGPoint(x: 4.2, y: -13),
            CGPoint(x: 6.8, y: -19), CGPoint(x: 9.4, y: -17.9), CGPoint(x: 6.9, y: -12),
            CGPoint(x: 12, y: -12)
        ]
        path.addLines(between: points.map { CGPoint(x: tip.x + $0.x, y: tip.y + $0.y) })
        path.closeSubpath()
        return path
    }

    static func circlePath(center: CGPoint, radius: CGFloat) -> CGPath {
        CGPath(
            ellipseIn: CGRect(
                x: center.x - radius,
                y: center.y - radius,
                width: radius * 2,
                height: radius * 2
            ),
            transform: nil
        )
    }
}

/// Production presenter: owns the (lazily created) panel and animates it.
@MainActor
final class ComputerOverlayPanelPresenter: ComputerOverlayPresenting {
    private var panel: ComputerOverlayPanel?
    private var contentView: ComputerOverlayContentView?
    private var lastLayout: OverlayLayout?
    /// Bumped on every present/hide so a stale fade-out completion cannot hide a newer marker.
    private var generation = 0

    func present(_ presentation: ComputerOverlayPresentation, plan: OverlayAnimationPlan) {
        generation += 1
        let panel = ensurePanel()
        let layout = presentation.layout
        let wasVisible = panel.isVisible && panel.alphaValue > 0
        let canGlide = plan.glideDuration != nil
            && wasVisible
            && lastLayout?.style == .pointer
            && layout.style == .pointer

        if canGlide, let duration = plan.glideDuration {
            // Same-size pointer panels: move the window, redraw inside it at once.
            contentView?.frame = CGRect(origin: .zero, size: panel.frame.size)
            contentView?.apply(presentation, plan: plan)
            NSAnimationContext.runAnimationGroup { context in
                context.duration = duration
                context.timingFunction = CAMediaTimingFunction(name: .easeOut)
                panel.animator().setFrame(layout.panelFrame, display: true)
            }
        } else {
            panel.setFrame(layout.panelFrame, display: false)
            contentView?.frame = CGRect(origin: .zero, size: layout.panelFrame.size)
            contentView?.apply(presentation, plan: plan)
        }
        lastLayout = layout

        if plan.appearDuration > 0 && !wasVisible {
            panel.alphaValue = 0
            panel.orderFrontRegardless()
            NSAnimationContext.runAnimationGroup { context in
                context.duration = plan.appearDuration
                panel.animator().alphaValue = 1
            }
        } else {
            panel.alphaValue = 1
            panel.orderFrontRegardless()
        }
    }

    func showOutcome(_ outcome: OverlayOutcomeKind) {
        contentView?.applyOutcome(outcome)
    }

    func hide(fadeDuration: TimeInterval) {
        generation += 1
        guard let panel, panel.isVisible else { return }
        let token = generation
        NSAnimationContext.runAnimationGroup({ context in
            context.duration = fadeDuration
            panel.animator().alphaValue = 0
        }, completionHandler: { [weak self] in
            MainActor.assumeIsolated {
                guard let self, self.generation == token else { return }
                self.finishHiding()
            }
        })
    }

    func hideImmediately() {
        generation += 1
        finishHiding()
    }

    private func finishHiding() {
        contentView?.clear()
        panel?.orderOut(nil)
        panel?.alphaValue = 1
        lastLayout = nil
    }

    private func ensurePanel() -> ComputerOverlayPanel {
        if let panel { return panel }
        let panel = ComputerOverlayPanel()
        let content = ComputerOverlayContentView(frame: .zero)
        panel.contentView = content
        self.panel = panel
        self.contentView = content
        return panel
    }
}

extension ComputerOverlayEnvironment {
    /// Real screens, accessibility settings, clocks and timers.
    @MainActor
    static func live() -> ComputerOverlayEnvironment {
        ComputerOverlayEnvironment(
            screens: { currentScreens() },
            primaryHeight: {
                let primary = NSScreen.screens.first { $0.frame.origin == .zero }
                return ComputerOverlayGeometry.primaryHeight(
                    appKitPrimaryFrameHeight: primary?.frame.height,
                    cgMainDisplayHeight: CGDisplayBounds(CGMainDisplayID()).height
                )
            },
            reduceMotion: { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion },
            nowMs: { UInt64(max(0, Date().timeIntervalSince1970 * 1000)) },
            uptime: { ProcessInfo.processInfo.systemUptime },
            schedule: { delay, action in
                let item = DispatchWorkItem {
                    MainActor.assumeIsolated { action() }
                }
                DispatchQueue.main.asyncAfter(deadline: .now() + delay, execute: item)
                return { item.cancel() }
            }
        )
    }

    @MainActor
    private static func currentScreens() -> [OverlayScreen] {
        let key = NSDeviceDescriptionKey("NSScreenNumber")
        return NSScreen.screens.compactMap { screen in
            guard let number = screen.deviceDescription[key] as? NSNumber else { return nil }
            let displayID = CGDirectDisplayID(number.uint32Value)
            return OverlayScreen(displayID: displayID, cgBounds: CGDisplayBounds(displayID))
        }
    }
}

/// Hides the overlay on system events where a stale marker would be wrong.
/// Reduce Motion needs no observer: it is read at every event.
@MainActor
final class ComputerOverlaySystemObservers {
    private var tokens: [(center: NotificationCenter, token: NSObjectProtocol)] = []

    init(onHide: @escaping @MainActor () -> Void) {
        let workspace = NSWorkspace.shared.notificationCenter
        let defaultCenter = NotificationCenter.default
        let sources: [(NotificationCenter, Notification.Name)] = [
            (defaultCenter, NSApplication.didChangeScreenParametersNotification),
            (workspace, NSWorkspace.willSleepNotification),
            (workspace, NSWorkspace.sessionDidResignActiveNotification)
        ]
        for (center, name) in sources {
            let token = center.addObserver(forName: name, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { onHide() }
            }
            tokens.append((center, token))
        }
    }

    deinit {
        for entry in tokens { entry.center.removeObserver(entry.token) }
    }
}
