import CoreGraphics
import Foundation

// Wire models for the Computer Use cursor overlay.
// Protocol: docs/BRIDGE_PROTOCOL.md (Events) and docs/computer-use/cursor-overlay-design.md.

/// First-pass view of a helper stdout line. A line with `event` and no
/// `request_id` is an event frame; everything else is a response.
struct HelperFrameEnvelope: Decodable {
    var event: String?
    var requestId: String?

    var isEventFrame: Bool { event != nil && requestId == nil }
}

struct SetComputerOverlayEventsParams: Codable, Sendable, Equatable {
    var enabled: Bool
}

/// Reply of the read-only `getComputerOverlayStatus`. Counts and enum values only.
struct ComputerOverlayHelperStatus: Decodable, Sendable, Equatable {
    // Decoded with the helper client's snake_case key strategy.
    struct Counters: Decodable, Sendable, Equatable {
        var forwarded: Int
        var droppedInvalid: Int
        var droppedDisabled: Int
        var droppedBackpressure: Int
        var overflows: Int
    }

    var enabled: Bool
    var runnerChannel: String
    var counters: Counters
}

/// The "Computer cursor overlay" section of the exported diagnostics. Only
/// numbers and enum values: never coordinates, targets or any event content.
enum ComputerOverlayDiagnostics {
    static func lines(
        preferenceEnabled: Bool,
        helper: ComputerOverlayHelperStatus?,
        helperRunning: Bool,
        discardedEventFrames: Int,
        controllerDroppedEvents: Int
    ) -> [String] {
        var lines = [
            "Computer cursor overlay",
            "Preference enabled: \(preferenceEnabled)",
            "Helper running: \(helperRunning)"
        ]
        if let helper {
            lines.append("Helper events enabled: \(helper.enabled)")
            lines.append("Runner channel: \(sanitizedChannel(helper.runnerChannel))")
            lines.append("Helper forwarded: \(helper.counters.forwarded)")
            lines.append("Helper dropped (invalid): \(helper.counters.droppedInvalid)")
            lines.append("Helper dropped (disabled): \(helper.counters.droppedDisabled)")
            lines.append("Helper dropped (backpressure): \(helper.counters.droppedBackpressure)")
            lines.append("Helper overflows: \(helper.counters.overflows)")
        } else {
            lines.append("Helper status: unavailable")
        }
        lines.append("App discarded event frames: \(discardedEventFrames)")
        lines.append("App dropped events (stale or no matching screen): \(controllerDroppedEvents)")
        return lines
    }

    /// The channel is an enum on the wire; anything else is not echoed.
    private static func sanitizedChannel(_ value: String) -> String {
        ["attached", "detached", "unsupported"].contains(value) ? value : "unknown"
    }
}

/// Typed events the helper can push. Unknown event names never reach this type.
enum HelperEvent: Sendable, Equatable {
    case computerOverlay(ComputerOverlayEventData)
}

enum HelperLifecycleEvent: Sendable, Equatable {
    case started
    case terminated
}

enum ComputerOverlayPhase: String, Decodable, Sendable, Equatable {
    case willAct = "will_act"
    case finished
    case clear
}

struct ComputerOverlayFrame: Decodable, Sendable {
    var protocolVersion: Int
    var event: String
    var data: ComputerOverlayEventData
}

struct ComputerOverlayDisplay: Decodable, Sendable, Equatable {
    var id: UInt32
    var bounds: CGRect

    private enum CodingKeys: String, CodingKey { case id, bounds }
    private struct BoundsWire: Decodable {
        var x: Double
        var y: Double
        var width: Double
        var height: Double
    }

    init(id: UInt32, bounds: CGRect) {
        self.id = id
        self.bounds = bounds
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        id = try container.decode(UInt32.self, forKey: .id)
        let wire = try container.decode(BoundsWire.self, forKey: .bounds)
        bounds = CGRect(x: wire.x, y: wire.y, width: wire.width, height: wire.height)
    }
}

struct ComputerOverlayKey: Decodable, Sendable, Equatable {
    var name: String
    var modifiers: [String]
}

enum ComputerOverlayTarget: Decodable, Sendable, Equatable {
    case point(CGPoint)
    case rect(CGRect)
    case path(from: CGPoint, to: CGPoint)
    case none

    private enum CodingKeys: String, CodingKey { case kind, x, y, width, height, from, to }
    private struct XY: Decodable {
        var x: Double
        var y: Double
        var point: CGPoint { CGPoint(x: x, y: y) }
    }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        switch try container.decode(String.self, forKey: .kind) {
        case "point":
            self = .point(CGPoint(
                x: try container.decode(Double.self, forKey: .x),
                y: try container.decode(Double.self, forKey: .y)
            ))
        case "rect":
            self = .rect(CGRect(
                x: try container.decode(Double.self, forKey: .x),
                y: try container.decode(Double.self, forKey: .y),
                width: try container.decode(Double.self, forKey: .width),
                height: try container.decode(Double.self, forKey: .height)
            ))
        case "path":
            self = .path(
                from: try container.decode(XY.self, forKey: .from).point,
                to: try container.decode(XY.self, forKey: .to).point
            )
        case "none":
            self = .none
        default:
            throw DecodingError.dataCorruptedError(
                forKey: .kind,
                in: container,
                debugDescription: "Unknown overlay target kind"
            )
        }
    }
}

/// `data` object of a `computer_overlay` event frame (helper -> App).
struct ComputerOverlayEventData: Decodable, Sendable, Equatable {
    static let supportedVersion = 1
    static let supportedSpace = "macos_cg_global_pt"

    var v: Int
    var phase: ComputerOverlayPhase
    var seq: UInt64?
    var actionId: UInt64?
    var action: String?
    var target: ComputerOverlayTarget?
    var space: String?
    var display: ComputerOverlayDisplay?
    var ttlMs: Int?
    var outcome: String?
    var reason: String?
    var key: ComputerOverlayKey?
    var emittedAtMs: UInt64?

    init(
        v: Int = ComputerOverlayEventData.supportedVersion,
        phase: ComputerOverlayPhase,
        seq: UInt64? = nil,
        actionId: UInt64? = nil,
        action: String? = nil,
        target: ComputerOverlayTarget? = nil,
        space: String? = ComputerOverlayEventData.supportedSpace,
        display: ComputerOverlayDisplay? = nil,
        ttlMs: Int? = nil,
        outcome: String? = nil,
        reason: String? = nil,
        key: ComputerOverlayKey? = nil,
        emittedAtMs: UInt64? = nil
    ) {
        self.v = v
        self.phase = phase
        self.seq = seq
        self.actionId = actionId
        self.action = action
        self.target = target
        self.space = space
        self.display = display
        self.ttlMs = ttlMs
        self.outcome = outcome
        self.reason = reason
        self.key = key
        self.emittedAtMs = emittedAtMs
    }
}

/// What the overlay draws for. Unknown names (for example the reserved `drag`
/// and `wheel`) fall back to `.other`, which is drawn like a pointer.
enum OverlayActionKind: Equatable, Sendable {
    case move, click, press, focus, scroll, input, key, activate, other

    init(wire: String?) {
        switch wire {
        case "move": self = .move
        case "click": self = .click
        case "press": self = .press
        case "focus": self = .focus
        case "scroll": self = .scroll
        case "input": self = .input
        case "key": self = .key
        case "activate": self = .activate
        default: self = .other
        }
    }
}

enum OverlayOutcomeKind: Equatable, Sendable {
    case succeeded, failed, notStarted, unknown

    init(wire: String?) {
        switch wire {
        case "succeeded": self = .succeeded
        case "failed": self = .failed
        case "not_started": self = .notStarted
        default: self = .unknown
        }
    }

    var isSuccess: Bool { self == .succeeded }
}
