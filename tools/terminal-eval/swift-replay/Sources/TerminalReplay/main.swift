import Foundation
import SwiftTerm

final class Delegate: TerminalDelegate {
    var replies = 0
    func send(source: Terminal, data: ArraySlice<UInt8>) { replies += data.count }
}

let directory = CommandLine.arguments[1]
let cases = ["cursor-erase": "new", "truecolor": "COLOR", "unicode": "Español 日本語 e\u{301} 🦀", "alternate-screen": "alt-screen", "alternate-return": "normal", "scrolling-region": "LAST", "keyboard-modes": "READY"]
for (name, expected) in cases.sorted(by: { $0.key < $1.key }) {
    let delegate = Delegate()
    let terminal = Terminal(delegate: delegate, options: TerminalOptions(cols: 80, rows: 24))
    let data = try Data(contentsOf: URL(fileURLWithPath: directory).appendingPathComponent(name + ".ansi"))
    for byte in data { terminal.feed(byteArray: [byte]) }
    let text = (0..<24).map { row in
        terminal.getLine(row: row)!.translateToString(trimRight: true, skipNullCellsFollowingWide: true, characterProvider: { terminal.getCharacter(for: $0) })
    }.joined(separator: "\n")
    precondition(text.contains(expected), "\(name): \(text)")
    if name == "truecolor" {
        precondition(terminal.getLine(row: 0)![0].attribute.fg == .trueColor(red: 17, green: 101, blue: 221))
    }
    precondition(delegate.replies == 0, "Unexpected terminal replies during replay")
    print("PASS SwiftTerm \(name)")
}
