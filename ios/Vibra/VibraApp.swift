import SwiftUI

@main struct VibraApp: App {
  @StateObject private var model = RemoteModel()
  @Environment(\.scenePhase) private var scenePhase
  var body: some Scene {
    WindowGroup {
      ContentView(model: model).onChange(of: scenePhase) { _, phase in
        if phase == .active { model.resume() } else if phase == .background { model.suspend() }
      }
    }
  }
}
struct ContentView: View {
  @ObservedObject var model: RemoteModel
  @State private var scanning = false
  @State private var invitation = ""
  @State private var settings = false
  @State private var confirmForget = false
  var body: some View {
    NavigationStack {
      VStack(spacing: 0) {
        Text(model.status).font(.caption).foregroundStyle(.secondary).padding(8)
        if let pane = model.selected {
          TerminalScreen(model: model).id(pane.id)
          ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 18) {
              Button("Esc") { model.key("escape") }
              Button("Tab") { model.key("tab") }
              Toggle("Ctrl", isOn: $model.control).toggleStyle(.button)
              Button("Ctrl+C") { model.key("c", control: true) }
              Button("Ctrl+D") { model.key("d", control: true) }
              Button("↑") { model.key("up") }
              Button("↓") { model.key("down") }
              Button("←") { model.key("left") }
              Button("→") { model.key("right") }
              Button("Pegar") { if let text = UIPasteboard.general.string { model.paste(text) } }
            }.buttonStyle(.bordered).padding(8)
          }.disabled(!model.ready)
        } else {
          List {
            if model.paired || model.connected {
              Section {
                Label(
                  model.connected ? "Tu Mac está conectado" : "Conectando con tu Mac",
                  systemImage: model.connected ? "checkmark.circle.fill" : "desktopcomputer"
                )
                .foregroundStyle(model.connected ? Color.green : Color.secondary)
                if !model.connected {
                  Text("Conecta el iPhone y el Mac a la misma red Wi-Fi y mantén Vibra abierto. Permite el acceso a la red local en Ajustes de iOS.")
                    .font(.subheadline).foregroundStyle(.secondary)
                  Button("Intentar de nuevo") { model.retry() }
                }
              }
              if model.connected {
                Section {
                  if model.panes.isEmpty {
                    VStack(alignment: .leading, spacing: 12) {
                      Label("Comparte tu primera terminal", systemImage: "terminal")
                        .font(.headline)
                      Text(
                        "En el Mac, haz clic derecho dentro de una terminal y elige Compartir con iPhone."
                      )
                      .foregroundStyle(.secondary)
                      Text("Aparecerá aquí automáticamente.")
                        .font(.caption).foregroundStyle(.secondary)
                    }.padding(.vertical, 8)
                  }
                  ForEach(model.panes) { pane in
                    Button {
                      model.open(pane)
                    } label: {
                      HStack(spacing: 12) {
                        Image(systemName: "terminal").font(.title3)
                        Text(pane.title).foregroundStyle(.primary)
                        Spacer()
                        Image(systemName: "chevron.right").font(.caption).foregroundStyle(.tertiary)
                      }.padding(.vertical, 6)
                    }
                  }
                } header: {
                  Text("Tus terminales")
                } footer: {
                  Text("Toca una terminal para controlarla. Al cerrarla, el control vuelve al Mac.")
                }
              }
            } else {
              Section {
                VStack(alignment: .leading, spacing: 16) {
                  Image(systemName: "laptopcomputer.and.iphone")
                    .font(.largeTitle).foregroundStyle(.tint)
                  Text("Tu terminal, contigo").font(.title2.bold())
                  Text("Controla tu Mac desde el iPhone en la misma red Wi-Fi. Sin servidor ni cuenta adicional.")
                    .foregroundStyle(.secondary)
                  Label("Abre Ajustes → iPhone en el Mac", systemImage: "1.circle")
                  Label("Pulsa Vincular iPhone", systemImage: "2.circle")
                  Label("Escanea el código y acepta en el Mac", systemImage: "3.circle")
                  Button {
                    scanning = true
                  } label: {
                    Label("Escanear código QR", systemImage: "qrcode.viewfinder")
                      .frame(maxWidth: .infinity).padding(.vertical, 4)
                  }.buttonStyle(.borderedProminent)
                }.padding(.vertical, 12)
              }
              Section {
                DisclosureGroup("¿No puedes escanear?") {
                  Text(
                    "Copia la invitación desde los ajustes de Vibra en el Mac y pégala aquí. También sirve para el simulador."
                  )
                  .font(.subheadline).foregroundStyle(.secondary)
                  PasteButton(payloadType: String.self) { values in
                    if let value = values.first { model.scan(value) }
                  }
                  DisclosureGroup("Introducir manualmente") {
                    TextField("Invitación del Mac", text: $invitation, axis: .vertical)
                      .lineLimit(3...5).font(.system(.caption, design: .monospaced))
                      .textInputAutocapitalization(.never).autocorrectionDisabled()
                    Button("Conectar con el Mac") {
                      model.scan(invitation)
                      invitation = ""
                    }.disabled(invitation.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                  }
                }
              } footer: {
                Text("Solo tú decides qué terminales compartir. La conexión está cifrada.")
              }
            }
          }

        }
      }
      .navigationTitle("Vibra").navigationBarTitleDisplayMode(.inline)
      .toolbar {
        if model.selected == nil && model.paired {
          ToolbarItem(placement: .topBarTrailing) {
            Button {
              settings = true
            } label: {
              Image(systemName: "gearshape")
            }
            .accessibilityLabel("Ajustes")
          }
        }
        if model.selected != nil {
          ToolbarItem(placement: .topBarLeading) { Button("Cerrar") { model.close() } }
          ToolbarItem(placement: .topBarTrailing) { Button("Historial") { model.requestHistory() } }
        }
      }
      .sheet(isPresented: $settings) {
        NavigationStack {
          Form {
            Section("Tu Mac") {
              Label(model.connected ? "Conectado" : "Sin conexión", systemImage: "desktopcomputer")
              Text(model.status).font(.caption).foregroundStyle(.secondary)
              if !model.connected { Button("Reconectar") { model.retry() } }
            }
            Section {
              Button("Desvincular este Mac", role: .destructive) { confirmForget = true }
            } footer: {
              Text(
                "Para conectar con otro Mac, desvincula este primero. Tus terminales seguirán abiertas en el Mac."
              )
            }
          }.navigationTitle("Ajustes").navigationBarTitleDisplayMode(.inline)
            .toolbar { Button("Listo") { settings = false } }
            .confirmationDialog(
              "¿Desvincular este Mac?", isPresented: $confirmForget, titleVisibility: .visible
            ) {
              Button("Desvincular Mac", role: .destructive) {
                model.forget()
                settings = false
              }
            } message: {
              Text("Necesitarás escanear un nuevo código para volver a conectarte.")
            }
        }
      }
      .sheet(isPresented: $scanning) {
        NavigationStack {
          Scanner { value in
            scanning = false
            model.scan(value)
          }.navigationTitle("Escanear QR").toolbar { Button("Cancelar") { scanning = false } }
        }
      }
      .sheet(
        isPresented: Binding(get: { model.history != nil }, set: { if !$0 { model.history = nil } })
      ) {
        NavigationStack {
          ScrollView {
            Text(model.history ?? "").font(.system(.caption, design: .monospaced)).textSelection(
              .enabled
            ).frame(maxWidth: .infinity, alignment: .leading).padding()
          }.navigationTitle("Historial reciente").toolbar {
            Button("Cerrar") { model.history = nil }
          }
        }
      }
      .alert(
        "Pegar varias líneas",
        isPresented: Binding(
          get: { model.pendingPaste != nil }, set: { if !$0 { model.pendingPaste = nil } })
      ) {
        Button("Pegar", role: .destructive) { model.confirmPaste() }
        Button("Cancelar", role: .cancel) { model.pendingPaste = nil }
      } message: {
        Text("El texto contiene saltos de línea y puede ejecutar comandos en la terminal.")
      }
      .alert(
        "Vibra",
        isPresented: Binding(get: { model.error != nil }, set: { if !$0 { model.error = nil } })
      ) {
        Button("Aceptar") { model.error = nil }
      } message: {
        Text(model.error ?? "")
      }
      .task { model.resume() }
    }
  }
}
