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
