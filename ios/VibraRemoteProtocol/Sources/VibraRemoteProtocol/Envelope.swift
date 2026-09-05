import Foundation

/// Application envelope inside an authenticated encrypted channel. No networking
/// or cryptography is performed here. Payload models are supplied by the client.
public struct Envelope<Payload: Codable & Sendable>: Codable, Sendable {
    public let version: UInt16
    public let requestID: UInt64
    public let message: Payload
    enum CodingKeys: String, CodingKey { case version, requestID = "request_id", message }

    public init(requestID: UInt64, message: Payload) {
        self.version = 1
        self.requestID = requestID
        self.message = message
    }
    public static func decode(_ data: Data) throws -> Self {
        guard data.count <= 1_048_576 else { throw WireError.tooLarge }
        let envelope = try JSONDecoder().decode(Self.self, from: data)
        guard envelope.version == 1 else { throw WireError.unsupportedVersion }
        return envelope
    }
    public func encode() throws -> Data {
        guard version == 1 else { throw WireError.unsupportedVersion }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
        let data = try encoder.encode(self)
        guard data.count <= 1_048_576 else { throw WireError.tooLarge }
        return data
    }
}
public enum WireError: Error { case tooLarge, unsupportedVersion }
