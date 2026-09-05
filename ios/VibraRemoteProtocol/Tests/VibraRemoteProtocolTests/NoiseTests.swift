import Foundation
import XCTest

@testable import VibraRemoteProtocol

final class NoiseTests: XCTestCase {
  struct Fixture: Decodable {
    let `private`: String
    let `public`: String
    let ephemeral: String
    let first: String
    let second: String
    let host: [String]
    let phone: [String]
    let message: String
  }
  func testInvitationExpiryHostAndKeyValidation() throws {
    let key = Data(repeating: 1, count: 32).base64EncodedString()
    var fields: [String: Any] = [
      "version": 2, "endpoint": "ws://my-mac.local:8788/local",
      "public_key": key, "invitation": key, "expires": UInt64(Date().timeIntervalSince1970) - 1,
    ]
    let expired = try JSONSerialization.data(withJSONObject: fields)
    XCTAssertThrowsError(try Invitation.parse(expired))
    XCTAssertNoThrow(try Invitation.parse(expired, paired: true))
    fields["expires"] = UInt64(Date().timeIntervalSince1970) + 300
    for endpoint in ["ws://my-mac.local:8788/local", "ws://192.168.1.2:8788/local", "ws://10.0.0.2:8788/local", "ws://172.16.0.2:8788/local", "ws://127.0.0.1:8788/local"] {
      fields["endpoint"] = endpoint
      XCTAssertNoThrow(try Invitation.parse(JSONSerialization.data(withJSONObject: fields)), endpoint)
    }
    for endpoint in ["wss://relay.example/ws", "ws://relay.example:8788/local", "ws://8.8.8.8:8788/local", "ws://172.32.0.2:8788/local", "ws://mac.local.evil.com:8788/local", "ws://mac.local:8788/ws", "ws://mac.local:8788/local?token=x", "ws://user@mac.local:8788/local", "ws://mac.local/local", "ws://mac.local:80/local"] {
      fields["endpoint"] = endpoint
      XCTAssertThrowsError(try Invitation.parse(JSONSerialization.data(withJSONObject: fields)), endpoint)
    }
    fields["endpoint"] = "ws://my-mac.local:8788/local"
    fields["version"] = 1
    XCTAssertThrowsError(try Invitation.parse(JSONSerialization.data(withJSONObject: fields), paired: true))
    fields["version"] = 2
    fields["public_key"] = "invalid"
    XCTAssertThrowsError(try Invitation.parse(JSONSerialization.data(withJSONObject: fields)))
  }

  func testRustNoiseIKAndFragmentation() throws {
    let url = try XCTUnwrap(
      Bundle.module.url(forResource: "noise", withExtension: "json", subdirectory: "Fixtures"))
    let f = try JSONDecoder().decode(Fixture.self, from: Data(contentsOf: url))
    let channel = try NoiseChannel(
      privateKey: Data(base64Encoded: f.private)!, remote: Data(base64Encoded: f.public)!,
      ephemeral: Data(base64Encoded: f.ephemeral)!)
    XCTAssertEqual(
      try channel.introduce(invitation: "fixture", name: "iPhone"), Data(base64Encoded: f.first))
    try channel.complete(Data(base64Encoded: f.second)!)
    for record in f.host.dropLast() { XCTAssertNil(try channel.open(Data(base64Encoded: record)!)) }
    XCTAssertEqual(try channel.open(Data(base64Encoded: f.host.last!)!), Data(f.message.utf8))
    XCTAssertEqual(try channel.seal(Data("Ctrl+C".utf8)), f.phone.map { Data(base64Encoded: $0)! })
    XCTAssertThrowsError(try channel.open(Data(base64Encoded: f.host[0])!))
  }
}
