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
        let customOutput = ProcessInfo.processInfo.environment["CHADEX_UI_REVIEW_OUTPUT"]
            .flatMap { $0.isEmpty ? nil : URL(fileURLWithPath: $0, isDirectory: true) }
        let output = customOutput ?? (updateCommittedReview
            ? projectRoot.appendingPathComponent("ui-review", isDirectory: true)
            : temporaryRoot.appendingPathComponent("ui-review", isDirectory: true))
        if customOutput != nil {
            // Only clear images this test writes; never delete a caller's folder.
            let existing = (try? FileManager.default.contentsOfDirectory(at: output, includingPropertiesForKeys: nil)) ?? []
            for file in existing where ["png", "jpg"].contains(file.pathExtension) {
                try? FileManager.default.removeItem(at: file)
            }
        } else if updateCommittedReview {
            try? FileManager.default.removeItem(at: output)
        }
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)

        // Never the real ~/Library/Application Support/Chadex: ProjectStore
        // derives its folder from the home directory, so only the explicit
        // override keeps a test run from replacing the user's preferences.
        let store = ProjectStore(environment: ["CHADEX_PREFERENCES_DIR": temporaryRoot.path])
        let project = ProjectRecord(name: "Chadex", path: projectRoot.path)
        var preferences = ChadexPreferences()
        preferences.projects = [project]
        preferences.selectedProjectID = project.id
        preferences.tunnelID = "tunnel_chadex_dev"
        preferences.backgroundCloseHintShown = true
        try store.save(preferences)
        XCTAssertTrue(
            FileManager.default.fileExists(atPath: temporaryRoot.appendingPathComponent("preferences.json").path),
            "review preferences must stay in the temporary folder"
        )

        let keychain = KeychainStore(
            service: "app.chadex.tests.\(UUID().uuidString)",
            account: "visual-review"
        )
        let model = AppModel(keychain: keychain, store: store)
        let updateManager = UpdateManager()

        try render(
            presented(
                RootView()
                    .environmentObject(model)
                    .environmentObject(updateManager),
                interfaceSize: .standard
            ),
            size: CGSize(width: 1100, height: 720),
            scheme: .light,
            to: output.appendingPathComponent("main-wide-100-light.png")
        )
        try render(
            presented(
                RootView()
                    .environmentObject(model)
                    .environmentObject(updateManager),
                interfaceSize: .extraLarge
            ),
            size: CGSize(width: 1400, height: 900),
            scheme: .light,
            to: output.appendingPathComponent("main-wide-140-light.png")
        )
        try render(
            presented(
                RootView()
                    .environmentObject(model)
                    .environmentObject(updateManager),
                interfaceSize: .extraLarge
            ),
            size: CGSize(width: 940, height: 640),
            scheme: .dark,
            to: output.appendingPathComponent("main-narrow-140-dark.png")
        )
        try render(
            presented(
                SettingsView(initialTab: .general)
                    .environmentObject(model)
                    .environmentObject(updateManager),
                interfaceSize: .standard
            ),
            size: CGSize(width: 660, height: 460),
            scheme: .light,
            to: output.appendingPathComponent("settings-general-100-light.png")
        )
        try render(
            presented(
                SettingsView(initialTab: .connection)
                    .environmentObject(model)
                    .environmentObject(updateManager),
                interfaceSize: .extraLarge
            ),
            size: CGSize(width: 752, height: 524),
            scheme: .dark,
            to: output.appendingPathComponent("settings-connection-140-dark.png")
        )
        try render(
            presented(
                SettingsView(initialTab: .advanced)
                    .environmentObject(model)
                    .environmentObject(updateManager),
                interfaceSize: .extraLarge
            ),
            size: CGSize(width: 752, height: 524),
            scheme: .light,
            to: output.appendingPathComponent("settings-advanced-140-light.png")
        )
        try render(
            presented(
                GlobalInstructionsStandaloneView()
                    .environmentObject(model),
                interfaceSize: .standard
            ),
            size: CGSize(width: 900, height: 680),
            scheme: .light,
            to: output.appendingPathComponent("global-instructions-100-light.png")
        )
        try render(
            presented(
                ProjectDetailView(project: project, destination: .computerUse) {}
                    .environmentObject(model),
                interfaceSize: .standard
            ),
            size: CGSize(width: 900, height: 720),
            scheme: .light,
            to: output.appendingPathComponent("computer-use-100-light.png")
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

        try render(
            presented(
                ProjectDetailView(project: project, destination: .agentSettings) {}
                    .environmentObject(model),
                interfaceSize: .comfortable
            ),
            size: CGSize(width: 980, height: 820),
            scheme: .dark,
            to: output.appendingPathComponent("agent-settings-120-dark.png")
        )
        try render(
            presented(ActivityView().environmentObject(model), interfaceSize: .comfortable),
            size: CGSize(width: 900, height: 520),
            scheme: .light,
            to: output.appendingPathComponent("activity-empty-120-light.png")
        )
        try render(
            presented(ConnectionSettingsSheet().environmentObject(model), interfaceSize: .comfortable),
            size: CGSize(width: 640, height: 460),
            scheme: .light,
            to: output.appendingPathComponent("connection-sheet-120-light.png")
        )
        try render(
            presented(
                ProjectDetailView(project: project, destination: .computerUse) {}
                    .environmentObject(model),
                interfaceSize: .extraLarge
            ),
            size: CGSize(width: 900, height: 720),
            scheme: .dark,
            to: output.appendingPathComponent("computer-use-160-dark.png")
        )

        let runningTask = TaskProgressSnapshot(
            taskId: "review-running", project: "Chadex",
            goal: "Fix the flaky connection retry test and validate the runner",
            status: "running", currentStep: 2, totalSteps: 4, completedSteps: 2,
            plan: ["search", "read", "edit", "validate"], cancelRequested: false,
            startedAtMs: UInt64(Date().addingTimeInterval(-95).timeIntervalSince1970 * 1_000),
            validation: TaskValidationSummary(status: "not_run", checksPassed: 0, checksFailed: 0),
            review: TaskReviewSummary(status: "pending"),
            steps: [
                TaskProgressStep(index: 0, kind: "search", status: "completed", durationMs: 2_100, attempts: 1, retries: 0),
                TaskProgressStep(index: 1, kind: "read", status: "completed", durationMs: 4_800, attempts: 1, retries: 0)
            ]
        )
        var failedTask = runningTask
        failedTask.status = "failed_validation"
        failedTask.currentStep = 3
        failedTask.completedSteps = 3
        failedTask.steps.append(TaskProgressStep(index: 2, kind: "edit", status: "completed", durationMs: 61_000, attempts: 1, retries: 0))
        failedTask.steps.append(TaskProgressStep(index: 3, kind: "validate", status: "failed", durationMs: 244_500, attempts: 1, retries: 0))
        failedTask.durationMs = 312_400
        failedTask.validation = TaskValidationSummary(status: "failed", checksPassed: 41, checksFailed: 2)
        try render(
            presented(
                VStack(alignment: .leading, spacing: 28) {
                    TaskProgressView(task: runningTask) {}
                    Divider()
                    TaskProgressView(task: failedTask) {}
                }
                .padding(28)
                .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading),
                interfaceSize: .comfortable
            ),
            size: CGSize(width: 900, height: 420),
            scheme: .light,
            to: output.appendingPathComponent("task-progress-120-light.png")
        )

        let reviewImages = [
            "main-wide-100-light.png",
            "main-wide-140-light.png",
            "main-narrow-140-dark.png",
            "settings-general-100-light.png",
            "settings-connection-140-dark.png",
            "settings-advanced-140-light.png",
            "global-instructions-100-light.png",
            "computer-use-100-light.png",
            "guide-wide-140-light.png",
            "guide-narrow-100-dark.png",
            "agent-settings-120-dark.png",
            "activity-empty-120-light.png",
            "connection-sheet-120-light.png",
            "computer-use-160-dark.png",
            "task-progress-120-light.png"
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
        // Host inside a real (never ordered-front) window so AppKit-backed
        // containers such as sidebar Lists and split views lay out their rows.
        let window = NSWindow(
            contentRect: CGRect(origin: .zero, size: size),
            styleMask: [.borderless],
            backing: .buffered,
            defer: false
        )
        window.appearance = hosting.appearance
        NSApplication.shared.appearance = hosting.appearance
        defer { NSApplication.shared.appearance = nil }
        window.contentView = hosting
        hosting.layoutSubtreeIfNeeded()
        RunLoop.main.run(until: Date().addingTimeInterval(0.35))
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
