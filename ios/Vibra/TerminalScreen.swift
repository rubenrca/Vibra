import SwiftTerm
import SwiftUI
import VibraRemoteProtocol

final class RemoteTerminalView: TerminalView {
  var pasteText: ((String) -> Void)?
  override func paste(_ sender: Any?) {
    if let text = UIPasteboard.general.string { pasteText?(text) }
  }
}
struct TerminalScreen: UIViewRepresentable {
  @ObservedObject var model: RemoteModel
  func makeCoordinator() -> Coordinator { Coordinator(model) }
  func makeUIView(context: Context) -> RemoteTerminalView {
    let view = RemoteTerminalView(
      frame: .zero, font: UIFont.monospacedSystemFont(ofSize: 13, weight: .regular))
    view.terminalDelegate = context.coordinator
    view.pasteText = { model.paste($0) }
    view.inputAccessoryView = nil
    model.terminal = view
    let paneID = model.selected?.id
    DispatchQueue.main.async {
      _ = view.becomeFirstResponder()
      model.viewport(
        columns: view.getTerminal().cols, rows: view.getTerminal().rows, paneID: paneID)
    }
    return view
  }
  func updateUIView(_ view: RemoteTerminalView, context: Context) {}
  static func dismantleUIView(_ view: RemoteTerminalView, coordinator: Coordinator) {
    view.terminalDelegate = nil
    view.updateUiClosed()
  }
  @MainActor final class Coordinator: NSObject, @preconcurrency TerminalViewDelegate {
    let model: RemoteModel
    let paneID: UUID?
    init(_ model: RemoteModel) {
      self.model = model
      self.paneID = model.selected?.id
    }
    func sizeChanged(source: TerminalView, newCols: Int, newRows: Int) {
      guard !model.feeding else { return }
      DispatchQueue.main.async {
        self.model.viewport(columns: newCols, rows: newRows, paneID: self.paneID)
      }
    }
    func send(source: TerminalView, data: ArraySlice<UInt8>) { model.terminalInput(Array(data)) }
    func setTerminalTitle(source: TerminalView, title: String) {}
    func hostCurrentDirectoryUpdate(source: TerminalView, directory: String?) {}
    func scrolled(source: TerminalView, position: Double) {}
    func requestOpenLink(source: TerminalView, link: String, params: [String: String]) {}
    func bell(source: TerminalView) {}
    func clipboardCopy(source: TerminalView, content: Data) {}
    func rangeChanged(source: TerminalView, startY: Int, endY: Int) {}
  }
}
