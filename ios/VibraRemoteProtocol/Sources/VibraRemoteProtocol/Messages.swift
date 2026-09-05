import Foundation

public struct Pane: Codable, Sendable, Identifiable {
  public let id: UUID
  public let title: String
  public let size: Size
}
public struct Size: Codable, Sendable, Equatable {
  public let columns: Int
  public let rows: Int
  public init(columns: Int, rows: Int) {
    self.columns = columns
    self.rows = rows
  }
}
public enum KeyValue: Codable, Sendable {
  case named(String)
  case character(String)
  public init(from decoder: Decoder) throws {
    let c = try decoder.singleValueContainer()
    if let name = try? c.decode(String.self) {
      self = .named(name)
    } else {
      self = .character(try c.decode([String: String].self)["character"] ?? "")
    }
  }
  public func encode(to encoder: Encoder) throws {
    var c = encoder.singleValueContainer()
    switch self {
    case .named(let name): try c.encode(name)
    case .character(let char): try c.encode(["character": char])
    }
  }
}
public struct Input: Codable, Sendable {
  public let kind: String
  public var text: String?
  public var key: KeyValue?
  public var modifiers: [String]?
  public init(kind: String, text: String? = nil, key: KeyValue? = nil, modifiers: [String]? = nil) {
    self.kind = kind
    self.text = text
    self.key = key
    self.modifiers = modifiers
  }
}
public struct Message: Codable, Sendable {
  public let kind: String
  public var pane_id: UUID?
  public var size: Size?
  public var input: Input?
  public var panes: [Pane]?
  public var revision: UInt64?
  public var base_revision: UInt64?
  public var ansi: String?
  public var nonce: UInt64?
  public var lines: Int?
  public var text: String?
  public var reason: String?
  public var code: String?
  public init(
    _ kind: String, pane: UUID? = nil, size: Size? = nil, input: Input? = nil, nonce: UInt64? = nil,
    lines: Int? = nil
  ) {
    self.kind = kind
    self.pane_id = pane
    self.size = size
    self.input = input
    self.nonce = nonce
    self.lines = lines
  }
}
extension Message {
  /// Combine adjacent typing events without crossing keys, paste boundaries, or panes.
  /// This keeps fast keyboard bursts within the bounded transport queue.
  public mutating func appendTyping(_ next: Message) -> Bool {
    guard kind == "input", next.kind == "input", pane_id == next.pane_id,
      input?.kind == "text", next.input?.kind == "text",
      let current = input?.text, let suffix = next.input?.text,
      current.utf8.count + suffix.utf8.count <= 65_536
    else { return false }
    input?.text = current + suffix
    return true
  }
}

public struct FrameTracker: Sendable {
  public private(set) var revision: UInt64?
  public private(set) var needsFull = true
  public init() {}
  public mutating func full(_ next: UInt64) -> Bool {
    if let old = revision, next < old || (next == old && !needsFull) { return false }
    revision = next
    needsFull = false
    return true
  }
  public mutating func patch(base: UInt64, next: UInt64) -> Bool {
    if let old = revision, next <= old { return false }
    guard !needsFull, revision == base, next > base else {
      needsFull = true
      return false
    }
    revision = next
    return true
  }
}

extension Input {
  /// Decode only user keyboard output from a neutral SwiftTerm viewport. Callers
  /// must suppress terminal-generated replies while feeding display frames.
  public static func userEvent(_ bytes: [UInt8]) -> Input? {
    guard !bytes.isEmpty, bytes.count <= 65_536,
      let text = String(bytes: bytes, encoding: .utf8)
    else { return nil }
    let named: [String: String] = [
      "\u{1b}": "escape", "\r": "enter", "\n": "enter", "\t": "tab", "\u{7f}": "backspace",
      "\u{1b}[A": "up", "\u{1b}[B": "down", "\u{1b}[C": "right", "\u{1b}[D": "left",
      "\u{1b}[H": "home", "\u{1b}[F": "end", "\u{1b}[3~": "delete", "\u{1b}[5~": "page_up",
      "\u{1b}[6~": "page_down",
      "\u{1b}OA": "up", "\u{1b}OB": "down", "\u{1b}OC": "right", "\u{1b}OD": "left",
    ]
    if let name = named[text] { return Input(kind: "key", key: .named(name), modifiers: []) }
    if text == "\u{1b}[Z" { return Input(kind: "key", key: .named("tab"), modifiers: ["shift"]) }
    if bytes.count == 1, let byte = bytes.first, byte < 32 {
      let scalar: UInt8 = byte == 0 ? 32 : (byte < 27 ? byte + 96 : byte + 64)
      return Input(
        kind: "key", key: .character(String(UnicodeScalar(scalar))), modifiers: ["control"])
    }
    if text.hasPrefix("\u{1b}["), let end = text.last {
      let parameters = text.dropFirst(2).dropLast().split(separator: ";")
      if parameters.count == 2, let modifier = Int(parameters[1]), (1...16).contains(modifier) {
        let name: String?
        if end == "~" {
          name = ["3": "delete", "5": "page_up", "6": "page_down"][String(parameters[0])]
        } else if parameters[0] == "1" {
          name =
            ["A": "up", "B": "down", "C": "right", "D": "left", "H": "home", "F": "end"][
              String(end)]
        } else {
          name = nil
        }
        if let name {
          let bits = modifier - 1
          let modifiers = [(1, "shift"), (2, "alt"), (4, "control"), (8, "super")].filter {
            bits & $0.0 != 0
          }.map { $0.1 }
          return Input(kind: "key", key: .named(name), modifiers: modifiers)
        }
      }
    }
    if bytes.first == 27 {
      let value = String(text.dropFirst())
      if value.unicodeScalars.count == 1,
        !value.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains)
      {
        return Input(kind: "key", key: .character(value), modifiers: ["alt"])
      }
      return nil
    }
    guard !text.unicodeScalars.contains(where: CharacterSet.controlCharacters.contains) else {
      return nil
    }
    return Input(kind: "text", text: text)
  }
}
