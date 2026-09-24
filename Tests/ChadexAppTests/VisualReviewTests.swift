import AppKit
import SwiftUI
import XCTest
@testable import ChadexApp

final class VisualReviewTests: XCTestCase {
    @MainActor
    func testRenderReviewScreens() throws {
        let projectRoot = URL(fileURLWithPath: FileManager.default.currentDirectoryPath, isDirectory: true)
        let temporaryRoot = FileManager.default.temporaryDirectory
            .appendingPathComponent("chadex-ui-review-\(UUID().uuidString)", isDirectory: true)
        defer { try? FileManager.default.removeItem(at: temporaryRoot) }
        try FileManager.default.createDirectory(at: temporaryRoot, withIntermediateDirectories: true)

        let updateCommittedReview = ProcessInfo.processInfo.environment["CHADEX_UPDATE_UI_REVIEW"] == "1"
        let output = updateCommittedReview
            ? projectRoot.appendingPathComponent("ui-review", isDirectory: true)
            : temporaryRoot.appendingPathComponent("ui-review", isDirectory: true)
        if updateCommittedReview {
            try? FileManager.default.removeItem(at: output)
        }
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

        let fileManager = ReviewFileManager(applicationSupportRoot: temporaryRoot)
        let store = ProjectStore(fileManager: fileManager)
        let project = ProjectRecord(name: "Chadex", path: projectRoot.path)
        var preferences = ChadexPreferences()
        preferences.projects = [project]
        preferences.selectedProjectID = project.id
        preferences.tunnelID = "tunnel_chadex_dev"
        preferences.backgroundCloseHintShown = true
        try store.save(preferences)

        let keychain = KeychainStore(
            service: "app.chadex.tests.\(UUID().uuidString)",
            account: "visual-review"
        )
        let model = AppModel(keychain: keychain, store: store)

        try render(
            presented(RootView().environmentObject(model), interfaceSize: .standard),
            size: CGSize(width: 1100, height: 720),
            scheme: .light,
            to: output.appendingPathComponent("main-wide-100-light.png")
        )
        try render(
            presented(RootView().environmentObject(model), interfaceSize: .extraLarge),
            size: CGSize(width: 1400, height: 900),
            scheme: .light,
            to: output.appendingPathComponent("main-wide-140-light.png")
        )
        try render(
            presented(RootView().environmentObject(model), interfaceSize: .extraLarge),
            size: CGSize(width: 940, height: 640),
            scheme: .dark,
            to: output.appendingPathComponent("main-narrow-140-dark.png")
        )
        try render(
            presented(SettingsView(initialTab: .general).environmentObject(model), interfaceSize: .standard),
            size: CGSize(width: 660, height: 460),
            scheme: .light,
            to: output.appendingPathComponent("settings-general-100-light.png")
        )
        try render(
            presented(SettingsView(initialTab: .connection).environmentObject(model), interfaceSize: .extraLarge),
            size: CGSize(width: 752, height: 524),
            scheme: .dark,
            to: output.appendingPathComponent("settings-connection-140-dark.png")
        )
        try render(
            presented(SettingsView(initialTab: .advanced).environmentObject(model), interfaceSize: .extraLarge),
            size: CGSize(width: 752, height: 524),
            scheme: .light,
            to: output.appendingPathComponent("settings-advanced-140-light.png")
        )
        try render(
            presented(GuideView().environmentObject(model), interfaceSize: .extraLarge),
            size: CGSize(width: 1320, height: 900),
            scheme: .light,
            to: output.appendingPathComponent("guide-wide-140-light.png")
        )
        try render(
            presented(GuideView().environmentObject(model), interfaceSize: .standard),
            size: CGSize(width: 900, height: 760),
            scheme: .dark,
            to: output.appendingPathComponent("guide-narrow-100-dark.png")
        )

        let reviewImages = [
            "main-wide-100-light.png",
            "main-wide-140-light.png",
            "main-narrow-140-dark.png",
            "settings-general-100-light.png",
            "settings-connection-140-dark.png",
            "settings-advanced-140-light.png",
            "guide-wide-140-light.png",
            "guide-narrow-100-dark.png"
        ].map { output.appendingPathComponent($0) }
        try makeContactSheet(from: reviewImages, to: output.appendingPathComponent("contact-sheet.jpg"))
    }

    private func makeContactSheet(from urls: [URL], to outputURL: URL) throws {
        let cellSize = CGSize(width: 600, height: 400)
        let rows = Int(ceil(Double(urls.count) / 2.0))
        let canvasSize = CGSize(width: cellSize.width * 2, height: cellSize.height * CGFloat(rows))
        let canvas = NSImage(size: canvasSize)
        canvas.lockFocus()
        NSColor(calibratedWhite: 0.18, alpha: 1).setFill()
        NSBezierPath(rect: CGRect(origin: .zero, size: canvasSize)).fill()

        for (index, url) in urls.enumerated() {
            guard let image = NSImage(contentsOf: url) else { continue }
            let column = index % 2
            let row = index / 2
            let cell = CGRect(
                x: CGFloat(column) * cellSize.width,
                y: CGFloat(rows - 1 - row) * cellSize.height,
                width: cellSize.width,
                height: cellSize.height
            ).insetBy(dx: 8, dy: 8)
            image.draw(in: cell, from: .zero, operation: .sourceOver, fraction: 1)
        }

        canvas.unlockFocus()
        guard let tiff = canvas.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let jpeg = bitmap.representation(using: .jpeg, properties: [.compressionFactor: 0.68])
        else {
            XCTFail("Could not build visual review contact sheet")
            return
        }
        try jpeg.write(to: outputURL, options: .atomic)
    }

    private func presented<V: View>(
        _ root: V,
        interfaceSize: ChadexInterfaceSize
    ) -> some View {
        root.chadexPresentation(
            language: .system,
            interfaceSize: interfaceSize,
            appearance: .system
        )
    }

    @MainActor
    private func render<V: View>(
        _ root: V,
        size: CGSize,
        scheme: ColorScheme,
        to url: URL
    ) throws {
        let view = ZStack {
            Color(nsColor: .windowBackgroundColor)
                .ignoresSafeArea()
            root
        }
        .environment(\.colorScheme, scheme)
        let hosting = NSHostingView(rootView: view)
        hosting.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
        hosting.frame = CGRect(origin: .zero, size: size)
        hosting.layoutSubtreeIfNeeded()

        guard let bitmap = hosting.bitmapImageRepForCachingDisplay(in: hosting.bounds) else {
            XCTFail("Could not create bitmap for \(url.lastPathComponent)")
            return
        }
        hosting.cacheDisplay(in: hosting.bounds, to: bitmap)
        guard let data = bitmap.representation(using: .png, properties: [:]) else {
            XCTFail("Could not encode \(url.lastPathComponent)")
            return
        }
        try data.write(to: url, options: .atomic)
    }
}

private final class ReviewFileManager: FileManager {
    private let applicationSupportRoot: URL

    init(applicationSupportRoot: URL) {
        self.applicationSupportRoot = applicationSupportRoot
        super.init()
    }

    override func urls(for directory: SearchPathDirectory, in domainMask: SearchPathDomainMask) -> [URL] {
        if directory == .applicationSupportDirectory {
            return [applicationSupportRoot]
        }
        return super.urls(for: directory, in: domainMask)
    }
}
