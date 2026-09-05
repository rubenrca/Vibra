import Foundation
import SwiftTerm
import XCTest

@testable import VibraRemoteProtocol

final class ScreenTests: XCTestCase {
  final class Delegate: TerminalDelegate { func send(source: Terminal, data: ArraySlice<UInt8>) {} }
  func testGhosttyActiveScreenAndRowPatch() throws {
    let url = try XCTUnwrap(
      Bundle.module.url(forResource: "screen", withExtension: "json", subdirectory: "Fixtures"))
    let fixture = try JSONDecoder().decode([String: String].self, from: Data(contentsOf: url))
    let delegate = Delegate()
    let terminal = Terminal(delegate: delegate, options: TerminalOptions(cols: 80, rows: 24))
    terminal.feed(text: try XCTUnwrap(fixture["full"]))
    func line(_ row: Int) -> String {
      terminal.getLine(row: row)!.translateToString(
        trimRight: true, skipNullCellsFollowingWide: true,
        characterProvider: { terminal.getCharacter(for: $0) })
    }
    XCTAssertTrue(line(0).contains("Español 日本語 🦀"))
    XCTAssertTrue(line(1).hasPrefix("BLUE"))
    XCTAssertEqual(
      terminal.getLine(row: 1)![0].attribute.fg, .trueColor(red: 17, green: 101, blue: 221))
    terminal.feed(text: try XCTUnwrap(fixture["patch"]))
    XCTAssertTrue(line(0).hasPrefix("Zspañol 日本語 🦀"))
    XCTAssertTrue(line(1).hasPrefix("BLUE"))
  }
  func testSemanticKeyboardAndReplyRejection() throws {
    let arrow = try XCTUnwrap(Input.userEvent(Array("\u{1b}[1;5A".utf8)))
    XCTAssertEqual(arrow.modifiers, ["control"])
    guard case .named("up") = arrow.key else { return XCTFail("modified arrow") }
    let control = try XCTUnwrap(Input.userEvent([3]))
    guard case .character("c") = control.key else { return XCTFail("Ctrl+C") }
    XCTAssertEqual(Input.userEvent(Array("日本語 🦀".utf8))?.text, "日本語 🦀")
    XCTAssertNil(Input.userEvent([]))
    XCTAssertNil(Input.userEvent(Array("\u{1b}[12;30R".utf8)))
    XCTAssertNil(Input.userEvent(Array("\u{1b}]52;c;secret\u{7}".utf8)))
  }
  func testRevisionGapRequiresFull() {
    var tracker = FrameTracker()
    XCTAssertFalse(tracker.patch(base: 1, next: 2))
    XCTAssertTrue(tracker.full(5))
    XCTAssertTrue(tracker.patch(base: 5, next: 6))
    XCTAssertFalse(tracker.patch(base: 7, next: 8))
    XCTAssertFalse(tracker.patch(base: 6, next: 7))
    XCTAssertTrue(tracker.full(9))
    XCTAssertFalse(tracker.full(8))
  }
}
