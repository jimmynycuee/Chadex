import CoreGraphics
import XCTest
@testable import ChadexApp

final class ComputerOverlayModelTests: XCTestCase {
    private func decodeFrame(_ json: String) throws -> ComputerOverlayFrame {
        try JSONDecoder.chadex.decode(ComputerOverlayFrame.self, from: Data(json.utf8))
    }

    func testWillActFrameFromTheDesignDecodes() throws {
        let frame = try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"seq":42,"phase":"will_act","action_id":17,"action":"click","target":{"kind":"point","x":812.5,"y":433.0},"space":"macos_cg_global_pt","display":{"id":69733378,"bounds":{"x":0,"y":0,"width":1512,"height":982}},"ttl_ms":2000,"emitted_at_ms":1791500000123}}"#)
        XCTAssertEqual(frame.protocolVersion, 1)
        let data = frame.data
        XCTAssertEqual(data.phase, .willAct)
        XCTAssertEqual(data.seq, 42)
        XCTAssertEqual(data.actionId, 17)
        XCTAssertEqual(data.action, "click")
        XCTAssertEqual(data.target, .point(CGPoint(x: 812.5, y: 433)))
        XCTAssertEqual(data.space, "macos_cg_global_pt")
        XCTAssertEqual(data.display, ComputerOverlayDisplay(id: 69733378, bounds: CGRect(x: 0, y: 0, width: 1512, height: 982)))
        XCTAssertEqual(data.ttlMs, 2000)
        XCTAssertEqual(data.emittedAtMs, 1791500000123)
    }

    func testRectPathNoneAndKeyTargetsDecode() throws {
        let rect = try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"will_act","action_id":1,"action":"key","target":{"kind":"rect","x":10,"y":20,"width":300,"height":40},"space":"macos_cg_global_pt","display":null,"key":{"name":"enter","modifiers":["command"]},"ttl_ms":2000,"emitted_at_ms":1}}"#)
        XCTAssertEqual(rect.data.target, .rect(CGRect(x: 10, y: 20, width: 300, height: 40)))
        XCTAssertNil(rect.data.display)
        XCTAssertEqual(rect.data.key, ComputerOverlayKey(name: "enter", modifiers: ["command"]))

        let path = try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"will_act","action_id":1,"action":"drag","target":{"kind":"path","from":{"x":1,"y":2},"to":{"x":3,"y":4}},"space":"macos_cg_global_pt","ttl_ms":2000,"emitted_at_ms":1}}"#)
        XCTAssertEqual(path.data.target, .path(from: CGPoint(x: 1, y: 2), to: CGPoint(x: 3, y: 4)))

        let none = try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"will_act","action_id":1,"action":"press","target":{"kind":"none"},"space":"macos_cg_global_pt","ttl_ms":2000,"emitted_at_ms":1}}"#)
        XCTAssertEqual(none.data.target, ComputerOverlayTarget.none)
    }

    func testFinishedAndClearFramesDecode() throws {
        let finished = try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"seq":43,"phase":"finished","action_id":17,"outcome":"succeeded","emitted_at_ms":1791500000171}}"#)
        XCTAssertEqual(finished.data.phase, .finished)
        XCTAssertEqual(finished.data.outcome, "succeeded")
        XCTAssertNil(finished.data.target)

        let clear = try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"clear","reason":"stopped"}}"#)
        XCTAssertEqual(clear.data.phase, .clear)
        XCTAssertEqual(clear.data.reason, "stopped")
    }

    func testUnknownPhaseOrTargetKindFailsToDecode() {
        XCTAssertThrowsError(try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"during"}}"#))
        XCTAssertThrowsError(try decodeFrame(#"{"protocol_version":1,"event":"computer_overlay","data":{"v":1,"phase":"will_act","target":{"kind":"blob"}}}"#))
    }

    func testEnvelopeSeparatesEventFramesFromResponses() throws {
        func envelope(_ json: String) throws -> HelperFrameEnvelope {
            try JSONDecoder.chadex.decode(HelperFrameEnvelope.self, from: Data(json.utf8))
        }
        XCTAssertTrue(try envelope(#"{"protocol_version":1,"event":"computer_overlay","data":{}}"#).isEventFrame)
        XCTAssertTrue(try envelope(#"{"protocol_version":1,"event":"something_new","data":{"a":1}}"#).isEventFrame)
        XCTAssertFalse(try envelope(#"{"protocol_version":1,"request_id":"a","result":{}}"#).isEventFrame)
        XCTAssertFalse(try envelope(#"{"protocol_version":1,"request_id":"a","error":{"code":"x","message":"m"}}"#).isEventFrame)
        // A frame that has both is a response: the request id wins.
        XCTAssertFalse(try envelope(#"{"protocol_version":1,"request_id":"a","event":"x"}"#).isEventFrame)
    }

    func testSetEventsParamsUseTheSnakeCaseWireShape() throws {
        let data = try JSONEncoder.chadex.encode(SetComputerOverlayEventsParams(enabled: true))
        XCTAssertEqual(String(data: data, encoding: .utf8), #"{"enabled":true}"#)
    }
}
