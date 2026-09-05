import Foundation
import XCTest
@testable import VibraRemoteProtocol

final class EnvelopeTests: XCTestCase {
    struct Screen: Codable, Sendable {
        let kind: String
        let pane_id: String
        let revision: UInt64
        let size: Size
        let ansi: String
    }
    struct Size: Codable, Sendable { let columns: UInt16; let rows: UInt16 }
    struct Empty: Codable, Sendable { let kind: String }

    func testRustScreenFixture() throws {
        let path = try XCTUnwrap(ProcessInfo.processInfo.environment["VIBRA_PROTOCOL_FIXTURES"]
            ?? Bundle.module.url(forResource: "frames", withExtension: "jsonl", subdirectory: "Fixtures")?.path)
        let records = try String(contentsOfFile: path, encoding: .utf8).split(separator: "\n")
        let frame = try XCTUnwrap(records.first { $0.contains("\"kind\":\"screen\"") })
        let envelope = try Envelope<Screen>.decode(Data(frame.utf8))
        XCTAssertEqual(envelope.requestID, 2)
        XCTAssertEqual(envelope.message.size.columns, 80)
        XCTAssertEqual(envelope.message.ansi, "\u{1b}[31mEspañol 日本語 🦀\u{1b}[0m")
        let encoded = try envelope.encode()
        let original = try JSONSerialization.jsonObject(with: Data(frame.utf8)) as! NSDictionary
        XCTAssertEqual(try JSONSerialization.jsonObject(with: encoded) as! NSDictionary, original)
    }
    func testVersionLimitAndUInt64() throws {
        let message = Envelope(requestID: UInt64.max, message: Empty(kind: "list_panes"))
        XCTAssertEqual(try Envelope<Empty>.decode(message.encode()).requestID, UInt64.max)
        XCTAssertThrowsError(try Envelope<Empty>.decode(Data(repeating: 32, count: 1_048_577)))
        let unsupported = Data(#"{"version":2,"request_id":0,"message":{"kind":"list_panes"}}"#.utf8)
        XCTAssertThrowsError(try Envelope<Empty>.decode(unsupported))
    }
}
