import Crypto
import Foundation
import Noise

public enum RemoteError: Error {
  case invalidFrame, invalidInvitation, unauthenticated
}
public struct Invitation: Codable, Sendable {
  public let version: Int
  public let endpoint: String
  public let public_key: String
  public let invitation: String
  public let expires: UInt64
  public static func parse(_ data: Data, paired: Bool = false) throws -> Self {
    guard data.count < 2048 else { throw RemoteError.invalidInvitation }
    let value = try JSONDecoder().decode(Self.self, from: data)
    guard value.version == 2, let url = URLComponents(string: value.endpoint), let host = url.host,
      localHost(host), url.url != nil, url.path == "/local", url.port.map({ (1024...65535).contains($0) }) == true,
      url.user == nil, url.password == nil, url.query == nil, url.fragment == nil,
      url.scheme == "ws",
      [value.public_key, value.invitation].allSatisfy({ Data(base64Encoded: $0)?.count == 32 }),
      paired || value.expires > UInt64(Date().timeIntervalSince1970)
    else { throw RemoteError.invalidInvitation }
    return value
  }
  private static func localHost(_ host: String) -> Bool {
    if host == "localhost" || host == "[::1]" || host == "::1" { return true }
    if host.hasSuffix(".local") {
      let name = host.dropLast(6)
      return !name.isEmpty && name.count <= 63 && name.first != "-" && name.last != "-"
        && name.utf8.allSatisfy { (65...90).contains($0) || (97...122).contains($0) || (48...57).contains($0) || $0 == 45 }
    }
    let parts = host.split(separator: ".", omittingEmptySubsequences: false)
    guard parts.count == 4 else { return false }
    let bytes = parts.compactMap { part -> UInt8? in
      guard part.allSatisfy({ $0.isASCII && $0.isNumber }), String(Int(part) ?? -1) == part else { return nil }
      return UInt8(part)
    }
    guard bytes.count == 4 else { return false }
    return bytes[0] == 10 || bytes[0] == 127 || (bytes[0] == 172 && (16...31).contains(bytes[1]))
      || (bytes[0] == 192 && bytes[1] == 168) || (bytes[0] == 169 && bytes[1] == 254)
  }

}
/// Confine this object to one actor. Noise transport nonces are strictly ordered.
public final class NoiseChannel {
  private let handshake: Noise.HandshakeState
  private var outbound: Noise.CipherState?
  private var inbound: Noise.CipherState?
  private var partial = Data()
  public static func privateKey() -> Data { Curve25519.KeyAgreement.PrivateKey().rawRepresentation }
  public init(privateKey: Data, remote: Data, ephemeral: Data? = nil) throws {
    handshake = try Noise.HandshakeState(
      config: Noise.Config(
        cipherSuite: Noise.CipherSuite(
          keyCurve: .x25519, cipher: .ChaChaPoly1305, hashFunction: .sha256),
        handshake: .IK_Initiator(
          remoteStatic: try Curve25519.KeyAgreement.PublicKey(rawRepresentation: remote)),
        prologue: Array("Vibra remote v1".utf8),
        staticKeypair: try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: privateKey),
        ephemeralKeypair: try ephemeral.map {
          try Curve25519.KeyAgreement.PrivateKey(rawRepresentation: $0)
        }
      ))
  }
  public func introduce(invitation: String, name: String) throws -> Data {
    struct Intro: Codable {
      let invitation: String
      let name: String
    }
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
    return Data(
      try handshake.writeMessage(
        payload: Array(encoder.encode(Intro(invitation: invitation, name: name)))
      ).buffer)
  }
  public func complete(_ record: Data) throws {
    let result = try handshake.readMessage(Array(record))
    guard String(bytes: result.payload, encoding: .utf8) == "approved", let c1 = result.c1,
      let c2 = result.c2
    else { throw RemoteError.unauthenticated }
    outbound = c1
    inbound = c2
  }
  public func seal(_ message: Data) throws -> [Data] {
    guard let outbound, !message.isEmpty, message.count <= 1_048_576 else {
      throw RemoteError.invalidFrame
    }
    return try stride(from: 0, to: message.count, by: 60_000).map { offset in
      let end = min(offset + 60_000, message.count)
      return Data(
        try outbound.encrypt(plaintext: [end == message.count ? 1 : 0] + message[offset..<end]))
    }
  }
  public func open(_ record: Data) throws -> Data? {
    guard let inbound, record.count <= 65_535 else { throw RemoteError.invalidFrame }
    let bytes = try inbound.decrypt(ciphertext: Array(record))
    guard bytes.count > 1, bytes.count <= 60_001, bytes[0] <= 1,
      partial.count + bytes.count - 1 <= 1_048_576
    else { throw RemoteError.invalidFrame }
    partial.append(contentsOf: bytes.dropFirst())
    if bytes[0] == 1 {
      let result = partial
      partial.removeAll(keepingCapacity: true)
      return result
    }
    return nil
  }
}
