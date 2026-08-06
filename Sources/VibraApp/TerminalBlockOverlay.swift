import GhosttyTerminal
import SwiftUI

/// Draws Warp-style command groups over Ghostty without intercepting terminal
/// input, selection, links, or scrolling.
struct TerminalBlockOverlay: View {
    @ObservedObject var session: TerminalSession
    @ObservedObject private var state: TerminalViewState
    @Environment(\.appChrome) private var chrome

    init(session: TerminalSession) {
        self.session = session
        _state = ObservedObject(wrappedValue: session.state)
    }

    var body: some View {
        GeometryReader { proxy in
            if let scrollbar = state.scrollbar, scrollbar.len > 0 {
                ForEach(Array(session.historyBlocks.enumerated()), id: \.element.id) {
                    index, block in
                    if let rect = visibleRect(
                        for: block,
                        at: index,
                        scrollbar: scrollbar,
                        viewportSize: proxy.size
                    ) {
                        blockSurface(for: block)
                            .frame(width: rect.width, height: rect.height)
                            .position(x: rect.midX, y: rect.midY)
                    }
                }
            }
        }
        .clipped()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
    }

    private func blockSurface(for block: TerminalHistoryBlock) -> some View {
        let latest = block.id == session.historyBlocks.last?.id
        let failed = block.exitCode.map { $0 != 0 } ?? false
        let borderOpacity: Double = latest ? 0.16 : failed ? 0.14 : 0.09
        let fillOpacity: Double = latest ? 0.028 : 0.018

        return RoundedRectangle(cornerRadius: 7, style: .continuous)
            .fill(chrome.foreground.opacity(fillOpacity))
            .overlay {
                RoundedRectangle(cornerRadius: 7, style: .continuous)
                    .stroke(chrome.foreground.opacity(borderOpacity), lineWidth: 1)
            }
    }

    private func visibleRect(
        for block: TerminalHistoryBlock,
        at index: Int,
        scrollbar: TerminalScrollbar,
        viewportSize: CGSize
    ) -> CGRect? {
        guard let startRow = block.promptRow else { return nil }

        let nextPromptRow: UInt64? = {
            let nextIndex = index + 1
            guard session.historyBlocks.indices.contains(nextIndex) else { return nil }
            return session.historyBlocks[nextIndex].promptRow
        }()
        let explicitEnd = nextPromptRow.map { $0 > 0 ? $0 - 1 : 0 }
            ?? block.outputEndRow
        let fallbackEnd = scrollbar.offset + scrollbar.len - 1
        let endRow = max(startRow, explicitEnd ?? fallbackEnd)

        let viewportStart = scrollbar.offset
        let viewportEnd = scrollbar.offset + scrollbar.len
        let blockEnd = endRow == UInt64.max ? endRow : endRow + 1
        guard blockEnd > viewportStart, startRow < viewportEnd else { return nil }

        let visibleStart = max(startRow, viewportStart)
        let visibleEnd = min(blockEnd, viewportEnd)
        guard visibleEnd > visibleStart else { return nil }

        let rowHeight = viewportSize.height / CGFloat(scrollbar.len)
        let top = CGFloat(visibleStart - viewportStart) * rowHeight + 2
        let bottom = CGFloat(visibleEnd - viewportStart) * rowHeight - 2
        let height = max(rowHeight - 4, bottom - top)
        let horizontalInset: CGFloat = 5

        return CGRect(
            x: horizontalInset,
            y: top,
            width: max(0, viewportSize.width - horizontalInset * 2),
            height: height
        )
    }
}
