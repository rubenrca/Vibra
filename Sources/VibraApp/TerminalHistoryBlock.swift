import Foundation

struct TerminalHistoryBlock: Identifiable, Equatable, Sendable {
    let id: Int
    let startedAt: Date
    var finishedAt: Date?
    let promptRow: UInt64?
    var outputEndRow: UInt64?
    var exitCode: Int?
    var durationNanos: UInt64?

    init(id: Int, startedAt: Date, promptRow: UInt64?) {
        self.id = id
        self.startedAt = startedAt
        self.finishedAt = nil
        self.promptRow = promptRow
        self.outputEndRow = nil
        self.exitCode = nil
        self.durationNanos = nil
    }

    var isFinished: Bool {
        finishedAt != nil
    }
}
