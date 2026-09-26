import AppKit
import SwiftUI

@MainActor
final class ChadexAppDelegate: NSObject, NSApplicationDelegate {
    var shutdownHandler: (() async -> Void)?
    var reopenHandler: (() -> Void)?
    private var terminationInProgress = false

    func applicationShouldHandleReopen(
        _ sender: NSApplication,
        hasVisibleWindows flag: Bool
    ) -> Bool {
        if !flag {
            reopenHandler?()
            sender.activate(ignoringOtherApps: true)
        }
        return true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        if terminationInProgress { return .terminateNow }
        guard let shutdownHandler else { return .terminateNow }
        terminationInProgress = true
        Task { @MainActor in
            await shutdownHandler()
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }
}

@main
struct ChadexApp: App {
    @NSApplicationDelegateAdaptor(ChadexAppDelegate.self) private var appDelegate
    @StateObject private var model = AppModel(
        autostart: ProcessInfo.processInfo.environment["CHADEX_AUTOSTART_MODEL"] == "1"
    )
    @StateObject private var updateManager = UpdateManager()
    @AppStorage(ChadexPreferenceKey.language) private var languageRaw = ChadexLanguage.system.rawValue
    @AppStorage(ChadexPreferenceKey.interfaceSize) private var interfaceSizeRaw = ChadexInterfaceSize.comfortable.rawValue
    @AppStorage(ChadexPreferenceKey.appearance) private var appearanceRaw = ChadexAppearance.system.rawValue

    var body: some Scene {
        WindowGroup(id: "main") {
            MainWindowContent(appDelegate: appDelegate)
                .environmentObject(model)
                .environmentObject(updateManager)
                .chadexPresentation(
                    language: language,
                    interfaceSize: interfaceSize,
                    appearance: appearance
                )
                .chadexWindowZoom(interfaceSize)
                .frame(
                    minWidth: ChadexLayoutMetrics(interfaceSize: interfaceSize).control(820),
                    minHeight: ChadexLayoutMetrics(interfaceSize: interfaceSize).control(560)
                )
        }
        .defaultSize(width: 1000, height: 680)
        .commands {
            CommandGroup(after: .appInfo) {
                Button(L10n.string("updates.checkMenu")) {
                    Task { await updateManager.checkForUpdates(userInitiated: true) }
                }
                .disabled(updateManager.isBusy)
            }

            CommandGroup(after: .newItem) {
                Button(L10n.string("project.add")) {
                    model.addProjectFromPanel()
                }
                .keyboardShortcut("o", modifiers: .command)

                Button(L10n.string("settings.refresh")) {
                    Task { await model.refreshStatus(force: true) }
                }
                .keyboardShortcut("r", modifiers: .command)
            }

            CommandGroup(after: .toolbar) {
                Divider()

                Button(L10n.string("view.zoomIn")) {
                    interfaceSizeRaw = ChadexInterfaceSize.zoomedIn(from: interfaceSize).rawValue
                }
                .keyboardShortcut("+", modifiers: .command)
                .disabled(interfaceSize == ChadexInterfaceSize.allCases.last)

                Button(L10n.string("view.zoomOut")) {
                    interfaceSizeRaw = ChadexInterfaceSize.zoomedOut(from: interfaceSize).rawValue
                }
                .keyboardShortcut("-", modifiers: .command)
                .disabled(interfaceSize == ChadexInterfaceSize.allCases.first)

                Button(L10n.string("view.actualSize")) {
                    interfaceSizeRaw = ChadexInterfaceSize.standard.rawValue
                }
                .keyboardShortcut("0", modifiers: .command)
                .disabled(interfaceSize == .standard)
            }
        }

        Settings {
            SettingsView()
                .environmentObject(model)
                .environmentObject(updateManager)
                .chadexPresentation(
                    language: language,
                    interfaceSize: interfaceSize,
                    appearance: appearance
                )
                .chadexFixedWindowZoom(
                    interfaceSize,
                    baseSize: CGSize(width: 660, height: 420)
                )
        }

        MenuBarExtra {
            MenuBarContent()
                .environmentObject(model)
                .environmentObject(updateManager)
                .chadexPresentation(
                    language: language,
                    interfaceSize: interfaceSize,
                    appearance: appearance
                )
        } label: {
            Image(systemName: model.snapshot.phase == .verified ? "bolt.horizontal.circle.fill" : "bolt.horizontal.circle")
                .accessibilityLabel(L10n.string("app.name"))
        }
    }

    private var language: ChadexLanguage {
        ChadexLanguage(rawValue: languageRaw) ?? .system
    }

    private var interfaceSize: ChadexInterfaceSize {
        ChadexInterfaceSize.resolve(storedValue: interfaceSizeRaw)
    }

    private var appearance: ChadexAppearance {
        ChadexAppearance(rawValue: appearanceRaw) ?? .system
    }
}

private struct MainWindowContent: View {
    @Environment(\.openWindow) private var openWindow
    @EnvironmentObject private var model: AppModel
    @EnvironmentObject private var updateManager: UpdateManager
    let appDelegate: ChadexAppDelegate

    var body: some View {
        RootView()
            .background(WindowCloseMonitor {
                model.showBackgroundCloseHintIfNeeded()
            })
            .task {
                appDelegate.reopenHandler = {
                    openWindow(id: "main")
                }
                appDelegate.shutdownHandler = { [model] in
                    await model.shutdown()
                }
                model.start()
                await updateManager.markCurrentLaunchHealthy()
                updateManager.scheduleAutomaticCheckIfNeeded()
            }
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.didBecomeActiveNotification)) { _ in
                model.applicationBecameActive()
            }
            .onReceive(NotificationCenter.default.publisher(for: NSApplication.didResignActiveNotification)) { _ in
                model.applicationResignedActive()
            }
            .alert(item: $updateManager.notice) { notice in
                updateAlert(for: notice)
            }
    }

    private func updateAlert(for notice: ChadexUpdateNotice) -> Alert {
        switch notice.kind {
        case .available(let version):
            if model.hasUpdateBlockingWork {
                return Alert(
                    title: Text(L10n.string("updates.availableTitle", version)),
                    message: Text(L10n.string("updates.availableBusyMessage")),
                    dismissButton: .default(Text(L10n.string("updates.later")))
                )
            }
            return Alert(
                title: Text(L10n.string("updates.availableTitle", version)),
                message: Text(L10n.string("updates.availableMessage")),
                primaryButton: .default(Text(L10n.string("updates.installNow"))) {
                    Task {
                        await updateManager.installAvailableUpdate {
                            !model.hasUpdateBlockingWork
                        }
                    }
                },
                secondaryButton: .cancel(Text(L10n.string("updates.later")))
            )
        case .blocked(let version):
            return Alert(
                title: Text(L10n.string("updates.blockedTitle")),
                message: Text(L10n.string("updates.blockedMessage", version)),
                dismissButton: .default(Text(L10n.string("common.ok")))
            )
        case .upToDate(let version):
            return Alert(
                title: Text(L10n.string("updates.upToDateTitle")),
                message: Text(L10n.string("updates.upToDateMessage", version)),
                dismissButton: .default(Text(L10n.string("common.ok")))
            )
        case .error(let message):
            return Alert(
                title: Text(L10n.string("updates.errorTitle")),
                message: Text(message),
                dismissButton: .default(Text(L10n.string("common.ok")))
            )
        }
    }
}

private struct WindowCloseMonitor: NSViewRepresentable {
    var onClose: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator(onClose: onClose) }

    func makeNSView(context: Context) -> NSView {
        let view = NSView(frame: .zero)
        DispatchQueue.main.async { context.coordinator.attach(to: view.window) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {
        DispatchQueue.main.async { context.coordinator.attach(to: nsView.window) }
    }

    final class Coordinator {
        let onClose: () -> Void
        weak var observedWindow: NSWindow?
        var observer: NSObjectProtocol?

        init(onClose: @escaping () -> Void) { self.onClose = onClose }

        func attach(to window: NSWindow?) {
            guard let window, observedWindow !== window else { return }
            if let observer { NotificationCenter.default.removeObserver(observer) }
            observedWindow = window
            observer = NotificationCenter.default.addObserver(
                forName: NSWindow.willCloseNotification,
                object: window,
                queue: .main
            ) { [weak self] _ in self?.onClose() }
        }

        deinit {
            if let observer { NotificationCenter.default.removeObserver(observer) }
        }
    }
}
