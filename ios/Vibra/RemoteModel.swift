import Foundation
import SwiftTerm
import UIKit
import VibraRemoteProtocol

@MainActor final class RemoteModel: ObservableObject {
  @Published var status = "Escanea el QR de Vibra en tu Mac"
  @Published var panes: [Pane] = []
  @Published var selected: Pane?
  @Published var connected = false
  @Published var ready = false
  @Published var control = false
  @Published var paired = false
  @Published var history: String?
  @Published var pendingPaste: String?
  @Published var error: String?
  weak var terminal: RemoteTerminalView?
  var feeding = false
  private var socket: URLSessionWebSocketTask?
  private var connection: Task<Void, Never>?
  private var heartbeat: Task<Void, Never>?
  private var writer: Task<Void, Never>?
  private var cipher: NoiseChannel?
  private var queue: [VibraRemoteProtocol.Message] = []
  private var request: UInt64 = 0
  private var generation = UUID()
  private var requestedSize: Size?
  private var tracker = FrameTracker()
  private var lastReceived = Date()
  private var invitation: Invitation?
  private var reconnect = true
  init() {
    #if targetEnvironment(simulator)
      Keychain.runSimulatorProbeIfRequested()
    #endif
    do {
      if let data = try Keychain.read("pairing") {
        paired = (try? Invitation.parse(data, paired: true)) != nil
        if !paired { status = "Actualiza Vibra en el Mac y escanea un QR nuevo para la conexión local" }
      }
    } catch {
      self.status = "No se pudo acceder al Llavero"
      self.error = Self.connectionError(error)
    }
  }
  private static func connectionError(_ error: Error) -> String {
    let code = error as NSError
    if code.domain == NSOSStatusErrorDomain && code.code == -34018 {
      return
        "Esta versión de Vibra no tiene los permisos de firma necesarios para usar el Llavero. Reinstala el build corregido; no es un problema de la invitación."
    }
    if code.domain == NSOSStatusErrorDomain {
      return
        "No se pudo acceder al Llavero (código \(code.code)). Desbloquea el dispositivo y vuelve a intentarlo."
    }
    if code.domain == NSURLErrorDomain {
      return
        "No se pudo conectar con el Mac (código \(code.code)). Conecta ambos dispositivos a la misma red Wi-Fi, permite el acceso a la red local y mantén Vibra abierto en el Mac."
    }
    return "No se pudo vincular con el Mac. Si la invitación expiró o fue utilizada, genera otra."
  }
  func scan(_ text: String) {
    do {
      let invite = try Invitation.parse(Data(text.utf8))
      disconnect()
      invitation = invite
      connect(invite)
    } catch { self.error = "La invitación no es compatible o expiró. Actualiza Vibra en ambos dispositivos y genera un QR nuevo en el Mac." }
  }
  func resume() {
    reconnect = true
    guard connection == nil else { return }
    if let invite = invitation {
      connect(invite)
    } else if let data = try? Keychain.read("pairing"),
      let invite = try? Invitation.parse(data, paired: true)
    {
      invitation = invite
      connect(invite)
    }
  }
  func retry() {
    disconnect()
    resume()
  }
  func suspend() {
    reconnect = false
    disconnect()
    status = "En pausa · el Mac recuperó el control"
  }
  func forget() {
    reconnect = false
    disconnect()
    do {
      try Keychain.remove("pairing")
    } catch {
      self.error = Self.connectionError(error)
      status = "No se pudo eliminar la vinculación"
      return
    }
    // Keep this device's identity; deleting the pairing is enough to forget the Mac.
    paired = false
    invitation = nil
    panes = []
    status = "Escanea un nuevo QR"
  }
  func disconnect() {
    generation = UUID()
    connection?.cancel()
    connection = nil
    heartbeat?.cancel()
    heartbeat = nil
    writer?.cancel()
    writer = nil
    socket?.cancel(with: .goingAway, reason: nil)
    socket = nil
    cipher = nil
    queue = []
    connected = false
    ready = false
    selected = nil
    resetPaneActions()
    panes = []
    tracker = FrameTracker()
  }
  private static func phoneName() -> String {
    var name = UIDevice.current.name.unicodeScalars.filter {
      !$0.properties.isDefaultIgnorableCodePoint && !CharacterSet.controlCharacters.contains($0)
    }.map(String.init).joined()
    while name.utf8.count > 80 { name.removeLast() }
    return name.isEmpty ? "iPhone" : name
  }
  private func connect(_ invite: Invitation) {
    disconnect()
    reconnect = true
    let token = generation
    connection = Task { [weak self] in
      guard let self else { return }
      do {
        self.status = "Conectando al Mac…"
        let privateKey: Data
        if let existing = try Keychain.read("identity") {
          privateKey = existing
        } else {
          privateKey = NoiseChannel.privateKey()
          try Keychain.write("identity", privateKey)
        }
        let channel = try NoiseChannel(
          privateKey: privateKey, remote: Data(base64Encoded: invite.public_key)!)
        let config = URLSessionConfiguration.ephemeral
        config.waitsForConnectivity = true
        config.timeoutIntervalForRequest = 130
        config.timeoutIntervalForResource = 86_400
        let session = URLSession(configuration: config)
        defer { session.invalidateAndCancel() }
        let ws = session.webSocketTask(with: URL(string: invite.endpoint)!)
        ws.maximumMessageSize = 65_535
        self.socket = ws
        ws.resume()
        let handshakeDeadline = Task {
          try? await Task.sleep(for: .seconds(130))
          guard !Task.isCancelled else { return }
          ws.cancel(with: .goingAway, reason: nil)
        }
        defer { handshakeDeadline.cancel() }
        try await ws.send(
          .data(
            channel.introduce(
              invitation: invite.invitation, name: Self.phoneName())))
        guard self.generation == token else { return }
        self.status = "Confirma este iPhone en Ajustes del Mac"
        guard case .data(let response) = try await ws.receive() else {
          throw RemoteError.unauthenticated
        }
        try channel.complete(response)
        handshakeDeadline.cancel()
        guard self.generation == token else { return }
        try Keychain.write("pairing", JSONEncoder().encode(invite))
        self.paired = true
        self.cipher = channel
        self.connected = true
        self.request = 0
        self.lastReceived = Date()
        self.status = "Selecciona una terminal compartida"
        self.enqueue(.init("list_panes"))
        self.heartbeat = Task { [weak self] in
          while !Task.isCancelled {
            try? await Task.sleep(for: .seconds(5))
            guard let self, self.generation == token, self.connected else { return }
            if Date().timeIntervalSince(self.lastReceived) > 15 {
              self.socket?.cancel(with: .goingAway, reason: nil)
              return
            }
            self.enqueue(.init("ping", nonce: UInt64(Date().timeIntervalSince1970)))
            if self.selected == nil { self.enqueue(.init("list_panes")) }
          }
        }
        while !Task.isCancelled {
          guard case .data(let record) = try await ws.receive() else {
            throw RemoteError.invalidFrame
          }
          guard self.generation == token else { return }
          if let data = try channel.open(record) {
            self.lastReceived = Date()
            try self.receive(Envelope<VibraRemoteProtocol.Message>.decode(data).message)
          }
        }
      } catch {
        guard self.generation == token else { return }
        self.disconnect()
        self.status = "Desconectado · el Mac recuperó el control"
        if self.reconnect && self.paired {
          self.connection = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled, let self else { return }
            self.connection = nil
            self.resume()
          }
        } else {
          self.error = Self.connectionError(error)
        }
      }
    }
  }
  func enqueue(_ message: VibraRemoteProtocol.Message) {
    guard connected else { return }
    if !queue.isEmpty, queue[queue.count - 1].appendTyping(message) { return }
    guard queue.count < 8 else {
      socket?.cancel(with: .goingAway, reason: nil)
      return
    }
    queue.append(message)
    guard writer == nil else { return }
    let token = generation
    writer = Task { [weak self] in
      guard let self else { return }
      do {
        while !self.queue.isEmpty {
          guard self.generation == token, let socket = self.socket, let cipher = self.cipher else {
            return
          }
          let message = self.queue.removeFirst()
          self.request += 1
          let records = try cipher.seal(
            Envelope(requestID: self.request, message: message).encode())
          for record in records { try await socket.send(.data(record)) }
        }
        if self.generation == token { self.writer = nil }
      } catch {
        if self.generation == token {
          self.socket?.cancel(with: .goingAway, reason: nil)
          self.writer = nil
        }
      }
    }
  }
  private func resetPaneActions() {
    pendingPaste = nil
    history = nil
    control = false
    requestedSize = nil
  }
  func open(_ pane: Pane) {
    resetPaneActions()
    selected = pane
    ready = false
    tracker = FrameTracker()
    status = "Abriendo terminal…"
  }
  func viewport(columns: Int, rows: Int, paneID: UUID?) {
    guard let pane = selected, pane.id == paneID, columns > 0, rows > 0 else { return }
    let size = Size(columns: min(columns, 500), rows: min(rows, 300))
    guard requestedSize != size else { return }
    let kind = requestedSize == nil ? "open" : "resize"
    requestedSize = size
    enqueue(.init(kind, pane: pane.id, size: size))
    ready = false
  }
  func close() {
    resetPaneActions()
    if let pane = selected { enqueue(.init("close", pane: pane.id)) }
    selected = nil
    ready = false
    tracker = FrameTracker()
    enqueue(.init("list_panes"))
    status = "Selecciona una terminal compartida"
  }
  func input(_ input: VibraRemoteProtocol.Input) {
    guard ready, let pane = selected else { return }
    enqueue(.init("input", pane: pane.id, input: input))
  }
  func key(_ name: String, control: Bool = false) {
    input(
      .init(
        kind: "key", key: control ? .character(name) : .named(name),
        modifiers: control ? ["control"] : []))
  }
  func paste(_ text: String) {
    guard ready, selected != nil else { return }
    guard text.utf8.count <= 65_536 else {
      error = "El texto supera 64 KB"
      return
    }
    if text.contains("\n") || text.contains("\r") {
      pendingPaste = text
    } else {
      input(.init(kind: "paste", text: text))
    }
  }
  func confirmPaste() {
    if let text = pendingPaste { input(.init(kind: "paste", text: text)) }
    pendingPaste = nil
  }
  func requestHistory() {
    if let pane = selected { enqueue(.init("history", pane: pane.id, lines: 1000)) }
  }
  private func receive(_ message: VibraRemoteProtocol.Message) throws {
    switch message.kind {
    case "ping":
      guard let nonce = message.nonce else { throw RemoteError.invalidFrame }
      enqueue(.init("pong", nonce: nonce))
    case "pong": break
    case "panes":
      guard let panes = message.panes, panes.count <= 128 else { throw RemoteError.invalidFrame }
      self.panes = panes
    case "screen", "patch":
      guard let pane = selected, message.pane_id == pane.id else { return }
      guard let revision = message.revision, let ansi = message.ansi else {
        throw RemoteError.invalidFrame
      }
      if message.kind == "screen" {
        guard let size = message.size, (1...500).contains(size.columns),
          (1...300).contains(size.rows)
        else { throw RemoteError.invalidFrame }
        guard tracker.full(revision) else { return }
      } else {
        guard let base = message.base_revision else { throw RemoteError.invalidFrame }
        guard tracker.patch(base: base, next: revision) else {
          if tracker.needsFull {
            ready = false
            enqueue(.init("resync", pane: pane.id))
          }
          return
        }
      }
      guard let terminal else {
        ready = false
        tracker = FrameTracker()
        return
      }
      feeding = true
      terminal.feed(text: ansi)
      feeding = false
      ready = true
      status = "Controlando \(pane.title)"
    case "control_released":
      if message.pane_id == selected?.id {
        selected = nil
        resetPaneActions()
        ready = false
        tracker = FrameTracker()
        status = "El Mac recuperó el control"
      }
    case "history_result": if message.pane_id == selected?.id { history = message.text }
    case "error":
      error = "El Mac rechazó la operación: \(message.code ?? "desconocida")"
      if message.code == "not_controller" || message.code == "not_shared" { close() }
    default: throw RemoteError.invalidFrame
    }
  }
  /// SwiftTerm uses a neutral keyboard mode. Convert its user events back to semantic input;
  /// protocol replies during feed are ignored and never injected into the host PTY.
  func terminalInput(_ bytes: [UInt8]) {
    guard !feeding, ready, var event = VibraRemoteProtocol.Input.userEvent(bytes) else { return }
    if control, event.kind == "text", let text = event.text, text.unicodeScalars.count == 1 {
      control = false
      event = .init(
        kind: "key", key: .character(text.utf8.count == 1 ? text.lowercased() : text),
        modifiers: ["control"])
    }
    input(event)
  }
}
