import AppKit
import SwiftUI

/// The companion occupies the sidebar's quiet space, with native status text.
/// Rendering and runtime projection are independent; preview never drives AppModel.
struct CodeFerretCompanion: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.chadexLayout) private var layout
    @AppStorage("ferret.visible") private var visible = true
    @AppStorage("ferret.motion") private var motion = true
    @State private var controller = FerretController()
    @State private var showingDetails = false
    @State private var preview: FerretState?
    @State private var previewSince = Date()

    var body: some View {
        VStack(spacing: 0) {
            if visible {
                CodeFerretStage(presentation: displayed, animated: motion && model.isAppActive)
                    .frame(width: layout.control(176), height: layout.control(148))
                    .allowsHitTesting(false)
                    .accessibilityHidden(true)
            }
            Button { showingDetails.toggle() } label: {
                HStack(spacing: 6) {
                    Image(systemName: displayed.state.symbol)
                        .foregroundStyle(displayed.state == .error ? Color.orange : .secondary)
                    Text(visible ? displayed.title : "Code Ferret")
                        .lineLimit(1)
                    if preview != nil { Image(systemName: "play.rectangle").foregroundStyle(.secondary) }
                    Spacer(minLength: 0)
                    Image(systemName: "ellipsis").foregroundStyle(.tertiary)
                }
                .chadexFont(.callout, weight: .medium)
                .padding(.horizontal, layout.spacing(12))
                .frame(height: layout.control(28))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
            .help(L10n.string("ferret.details"))
            .accessibilityLabel("Code Ferret, \(displayed.title)")
            .popover(isPresented: $showingDetails, arrowEdge: .trailing) {
                VStack(alignment: .leading, spacing: 12) {
                    Text("Code Ferret").font(.headline)
                    Label(controller.presentation.title, systemImage: controller.presentation.state.symbol)
                    Text(L10n.string("ferret.explanation"))
                        .font(.callout).foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                    Divider()
                    Toggle(L10n.string("ferret.visible"), isOn: $visible)
                    Toggle(L10n.string("ferret.motion"), isOn: $motion)
                    if FerretReviewMode.enabled {
                        Divider()
                        Picker(L10n.string("ferret.preview"), selection: $preview) {
                            Text(L10n.string("ferret.live")).tag(FerretState?.none)
                            ForEach(FerretState.allCases) { Text($0.title).tag(Optional($0)) }
                        }
                        Text(L10n.string("ferret.previewHint")).font(.caption).foregroundStyle(.secondary)
                    }
                }
                .padding(16).frame(width: 270)
            }
        }
        .padding(.bottom, layout.spacing(6))
        .task {
            while !Task.isCancelled {
                update()
                try? await Task.sleep(for: .milliseconds(500))
            }
        }
        .onChange(of: preview) { _, _ in previewSince = Date() }
        .onChange(of: model.snapshot) { _, _ in update() }
        .onChange(of: model.mascotTraces) { _, _ in update() }
    }

    private var displayed: FerretPresentation {
        preview.map { FerretPresentation(state: $0, progress: $0 == .testing ? 0.6 : nil, since: previewSince) }
            ?? controller.presentation
    }

    private func update() {
        controller.update(snapshot: model.snapshot, traces: model.mascotTraces, activities: model.activities,
                          preparing: model.isBootstrapping || model.isSwitchingProject || model.connectionActionInFlight,
                          foreground: model.isAppActive)
    }
}

/// Each pose has a continuous silhouette. A separate tail, feathered eyelids,
/// native work UI and a shared clock supply motion without swapping whole heads.
struct CodeFerretStage: View {
    let presentation: FerretPresentation
    var animated = true
    var sampleTime: TimeInterval? = nil
    var blinkOverride: Bool? = nil
    var sampleElapsed: TimeInterval? = nil
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var scheme
    private var state: FerretState { presentation.state }
    private var accent: Color { state == .error ? Color(red: 1, green: 0.70, blue: 0.28) : Color(red: 0.30, green: 0.86, blue: 1) }

    var body: some View {
        GeometryReader { geometry in
            TimelineView(.animation(minimumInterval: 1.0 / 30, paused: !animated || reduceMotion || sampleTime != nil)) { context in
                let time = sampleTime ?? context.date.timeIntervalSinceReferenceDate
                let moving = animated && !reduceMotion
                let elapsed = sampleElapsed ?? max(0, context.date.timeIntervalSince(presentation.since))
                let motion = FerretMotion(state: state, time: time, elapsed: elapsed, moving: moving)
                let phase = time.truncatingRemainder(dividingBy: state == .longTask ? 8.1 : 5.7)
                let blinking = blinkOverride ?? (moving && phase < 0.16)
                scene(motion: motion, blinking: blinking, time: time, moving: moving)
                    .frame(width: 180, height: 148)
                    .scaleEffect(max(0, min(geometry.size.width / 180, geometry.size.height / 148)), anchor: .topLeading)
            }
        }
        .animation(reduceMotion || !animated ? nil : .easeInOut(duration: 0.32), value: state)
    }

    private func scene(motion: FerretMotion, blinking: Bool, time: Double, moving: Bool) -> some View {
        let layout = FerretPoseLayout(state: state)
        return ZStack {
            Ellipse().fill(Color.primary.opacity(scheme == .dark ? 0.10 : 0.055))
                .frame(width: layout.shadowWidth, height: 7).position(x: 87, y: 139)
            if state != .sleep {
                sprite(layout.tailMirrored ? "tail-left" : "tail")
                    .frame(width: layout.tailWidth, height: layout.tailHeight)
                    // The thick root pivots inside the hip, behind the continuous body.
                    .rotationEffect(.degrees(layout.tailAngle + motion.tailAngle),
                                    anchor: UnitPoint(x: layout.tailMirrored ? 0.82 : 0.18, y: 0.86))
                    .position(x: layout.tailX, y: layout.tailY + motion.lift)
            }
            if let spec = FerretAssets.poses[state.rawValue] {
                let scale = min(layout.width / spec.width, layout.height / spec.height)
                let width = spec.width * scale
                let height = spec.height * scale
                ZStack(alignment: .topLeading) {
                    sprite(state.rawValue).frame(width: width, height: height)
                    if let light = spec.light, light.count == 2 {
                        Circle().fill(accent.opacity(0.18 + (motion.breath + 1) * 0.1))
                            .frame(width: 3, height: 3).blur(radius: 1.8)
                            .position(x: light[0] * scale, y: light[1] * scale)
                    }
                    if blinking {
                        ForEach(spec.eyes, id: \.name) { eye in
                            sprite(eye.name)
                                .frame(width: eye.width * scale, height: eye.height * scale)
                                .mask(Ellipse().fill(.white).blur(radius: 0.5))
                                .position(x: (eye.x + eye.width / 2) * scale, y: (eye.y + eye.height / 2) * scale)
                        }
                    }
                }
                .frame(width: width, height: height)
                .scaleEffect(x: 1 + motion.breath * 0.002, y: 1 + motion.breath * 0.006, anchor: .bottom)
                .rotationEffect(.degrees(motion.bodyAngle), anchor: .bottom)
                .offset(y: motion.lift)
                .position(x: layout.x, y: 137 - height / 2)
                .id(state).transition(.opacity)
            }
            stateUI(time: time, moving: moving, reaction: motion.reaction)
        }
    }

    @ViewBuilder
    private func stateUI(time: Double, moving: Bool, reaction: Double) -> some View {
        switch state {
        case .thinking:
            thoughtBubble(time: time, moving: moving).position(x: 143, y: 33)
        case .waiting:
            HStack(spacing: 5) {
                Image(systemName: "clock").font(.system(size: 13, weight: .light))
                dots(time: time, moving: moving)
            }.foregroundStyle(accent.opacity(0.65)).position(x: 137, y: 49)
        case .testing:
            testPanel(time: time, moving: moving).position(x: 144, y: 43)
        case .longTask:
            progressPanel(time: time, moving: moving).position(x: 104, y: 56)
        case .coding:
            // The bitmap includes connected paws and keyboard; code activity stays native.
            Text("</>").font(.system(size: 10, weight: .medium, design: .monospaced))
                .foregroundStyle(accent.opacity(moving ? 0.55 + 0.25 * sin(time * 2.4) : 0.75))
                .position(x: 149, y: 53)
        case .searching:
            ForEach(0..<3) { index in
                Capsule().fill(accent.opacity(0.65)).frame(width: 6, height: 1.3)
                    .rotationEffect(.degrees(Double(index - 1) * 24))
                    .offset(x: moving ? sin(time * 2.6) * 1.1 : 0)
                    .position(x: 17, y: 122 + Double(index) * 5)
            }
        case .listening:
            ForEach(0..<3) { index in
                Capsule().fill(accent.opacity(moving ? reaction * 0.8 : 0.65))
                    .frame(width: 2, height: 6 + Double(index) * 2)
                    .rotationEffect(.degrees(20 + Double(index) * 18))
                    .position(x: 127 + Double(index) * 6, y: 23 - Double(index) * 2)
            }
        case .success:
            ForEach(0..<4) { index in
                Image(systemName: "sparkle").font(.system(size: index == 1 ? 10 : 7))
                    .foregroundStyle(accent.opacity(moving ? reaction : 0.65))
                    .scaleEffect(0.75 + reaction * 0.3)
                    .position(x: [28.0, 142, 140, 30][index], y: [65.0, 37, 106, 108][index] - reaction * 3)
            }
        case .error:
            Image(systemName: "exclamationmark").font(.system(size: 12, weight: .semibold))
                .foregroundStyle(accent).frame(width: 23, height: 23)
                .background(Circle().fill(accent.opacity(scheme == .dark ? 0.13 : 0.10)))
                .position(x: 142, y: 52)
        case .sleep:
            ForEach(0..<3) { index in
                Text("z").font(.system(size: 9 + Double(index) * 2, weight: .medium, design: .rounded))
                    .foregroundStyle(accent.opacity(0.3 + Double(index) * 0.12))
                    .offset(y: moving ? sin(time + Double(index)) * 1.5 : 0)
                    .position(x: 118 + Double(index) * 11, y: 50 - Double(index) * 10)
            }
        default: EmptyView()
        }
    }

    private func dots(time: Double, moving: Bool) -> some View {
        HStack(spacing: 3) {
            ForEach(0..<3) { i in
                Circle().fill(accent.opacity(moving ? 0.3 + 0.45 * (sin(time * 2 - Double(i)) + 1) / 2 : 0.6))
                    .frame(width: 3, height: 3)
            }
        }
    }

    private func thoughtBubble(time: Double, moving: Bool) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            dots(time: time, moving: moving).padding(.horizontal, 8).padding(.vertical, 7)
                .background(Capsule().fill(accent.opacity(scheme == .dark ? 0.12 : 0.1)))
            Circle().fill(accent.opacity(0.15)).frame(width: 3, height: 3).padding(.leading, 3)
        }
    }

    private func testPanel(time: Double, moving: Bool) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(0..<3) { i in
                HStack(spacing: 4) {
                    // Only known task progress marks a completed step. Unknown work has no fake checks.
                    Image(systemName: presentation.progress.map { Double(i + 1) / 3 <= $0 } == true
                          ? "checkmark.circle.fill" : "circle")
                        .font(.system(size: 7)).foregroundStyle(accent)
                    Capsule().fill(accent.opacity(0.3)).frame(width: [21.0, 25, 18][i], height: 2)
                }
            }
            progressBar(time: time, moving: moving).frame(height: 3)
        }.padding(7).frame(width: 51, height: 53)
            .background(RoundedRectangle(cornerRadius: 5).fill(accent.opacity(scheme == .dark ? 0.09 : 0.055)))
            .overlay(RoundedRectangle(cornerRadius: 5).stroke(accent.opacity(0.3), lineWidth: 0.7))
    }

    private func progressPanel(time: Double, moving: Bool) -> some View {
        HStack(spacing: 5) {
            Image(systemName: presentation.activity?.symbol ?? "hourglass")
                .font(.system(size: 8)).foregroundStyle(accent.opacity(0.7))
            progressBar(time: time, moving: moving).frame(height: 3)
        }.padding(5).frame(width: 78, height: 17)
            .background(RoundedRectangle(cornerRadius: 4).fill(accent.opacity(0.06)))
            .overlay(RoundedRectangle(cornerRadius: 4).stroke(accent.opacity(0.28), lineWidth: 0.7))
    }

    private func progressBar(time: Double, moving: Bool) -> some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(accent.opacity(0.13))
                if let progress = presentation.progress {
                    Capsule().fill(accent.opacity(0.85)).frame(width: max(0, geometry.size.width) * max(0, min(1, progress)))
                } else {
                    Capsule().fill(accent.opacity(0.75)).frame(width: 12)
                        .offset(x: moving ? (sin(time * 1.7) + 1) * max(0, geometry.size.width - 12) / 2 : 8)
                }
            }
        }
    }

    private func sprite(_ name: String) -> some View {
        Group {
            if let image = FerretAssets.images[name] {
                Image(nsImage: image).resizable().interpolation(.high)
            }
        }
    }
}

/// Reactions depend on time since a real state transition, not on the looping
/// breathing phase, so completion celebrates once and never repeats the hop.
struct FerretMotion {
    let breath: Double
    let tailAngle: Double
    let bodyAngle: Double
    let lift: Double
    let reaction: Double

    init(state: FerretState, time: Double, elapsed: Double, moving: Bool) {
        guard moving else { breath = 0; tailAngle = 0; bodyAngle = 0; lift = 0; reaction = 0; return }
        let slow = state == .sleep || state == .longTask
        breath = sin(time * (slow ? 0.85 : 1.6))
        let oneShot = max(0, min(1, elapsed / (state == .listening ? 0.65 : 0.85)))
        let pulse = oneShot >= 1 ? 0 : sin(.pi * oneShot)
        reaction = max(0, 1 - elapsed / (state == .listening ? 0.8 : 1.8))
        tailAngle = sin(time * (slow ? 0.65 : 1.3)) * (slow ? 1.5 : state == .thinking ? 5 : 3.5)
            + (state == .success ? pulse * 7 : 0)
        bodyAngle = state == .thinking ? sin(time * 0.65) * 1.2
            : state == .searching ? sin(time * 2.6) * 0.7
            : state == .listening ? -pulse * 1.3 : state == .error ? pulse * 1.6 : 0
        lift = state == .success ? -pulse * 6 : state == .listening ? -pulse * 2.5
            : state == .searching ? sin(time * 2.6) * 0.7 : 0
    }
}

private struct FerretPoseLayout {
    var width = 100.0, height = 119.0, x = 76.0, shadowWidth = 108.0
    var tailWidth = 49.0, tailHeight = 63.0, tailX = 120.0, tailY = 106.0
    var tailAngle = 0.0, tailMirrored = false
    init(state: FerretState) {
        switch state {
        case .thinking: width = 99; height = 122
        case .listening: height = 122
        case .testing: width = 92; height = 117; x = 72
        case .coding: width = 144; height = 111; x = 84; tailX = 145; tailY = 105; tailWidth = 39; tailHeight = 50
        case .searching:
            width = 144; height = 103; x = 83; tailX = 143; tailY = 71; tailWidth = 45; tailHeight = 58; tailAngle = 14
        case .waiting, .longTask:
            width = 145; height = 85; x = 91; shadowWidth = 135
            tailX = 28; tailY = 103; tailWidth = 44; tailHeight = 56; tailMirrored = true; tailAngle = -12
        case .success: width = 112; height = 123
        case .error: width = 98; height = 117
        case .sleep: width = 143; height = 97; x = 87; shadowWidth = 126
        default: break
        }
    }
}

private struct FerretEyeSpec: Decodable {
    let name: String
    let x, y, width, height: Double
}
private struct FerretSpriteSpec: Decodable {
    let name: String
    let width, height: Double
    let eyes: [FerretEyeSpec]
    let light: [Double]?
}

/// Use the same relocation-safe resource lookup as the rest of Chadex.
private enum FerretAssets {
    static let poses: [String: FerretSpriteSpec] = {
        guard let url = L10n.resourceBundle()?.url(forResource: "ferret-motion-poses", withExtension: "json"),
              let data = try? Data(contentsOf: url),
              let poses = try? JSONDecoder().decode([String: FerretSpriteSpec].self, from: data) else { return [:] }
        return poses
    }()
    static let images: [String: NSImage] = {
        var result: [String: NSImage] = [:]
        let names = poses.values.flatMap { [$0.name] + $0.eyes.map(\.name) }
        for name in names {
            if let url = L10n.resourceBundle()?.url(forResource: "ferret-motion-\(name)", withExtension: "png"),
               let image = NSImage(contentsOf: url) { result[name] = image }
        }
        return result
    }()
}
