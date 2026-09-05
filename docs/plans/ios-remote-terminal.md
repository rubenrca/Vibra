# Vibra iOS: control remoto sencillo de terminales

Fecha: 2026-09-04. Estado: implementación disponible en `feat/ios-remote-terminal`, preparada para pruebas manuales.

La [migración a Ghostty](../ghostty.md) está en `main`. El control remoto se desarrolla en su propia rama. [Guía de prueba e instalación](ios-remote-testing.md).

## Avance

- Implementados: protocolo v1, relay Rust, Noise IK interoperable Rust–Swift, fragmentación cifrada y límites.
- Implementados: exportación del área activa Ghostty con filas VT, cursor y paleta; revisiones y resync; historial independiente; teclado compartido con GPUI; arbitraje del tamaño y recuperación del control.
- Implementados: ajustes del Mac, QR de un solo uso, confirmación, Llavero, revocación y compartir panes, incluyendo terminales inferiores.
- Implementada: app SwiftUI iOS 17 con SwiftTerm, cámara QR, vinculación, lista de panes, terminal, teclas auxiliares, pegado confirmado, historial y reconexión sin repetir entradas.
- Preparados: proyecto Xcode, scripts de compilación y arranque local, Docker y configuración Fly.io de una instancia.
- Verificados por CLI: Rust/Swift, sesión cifrada a través de relay real, permisos, heartbeat, PTY/tamaño, reconstrucción Ghostty–SwiftTerm y builds iOS para simulador/dispositivo.
- Pendientes de entorno externo: firma e instalación en el iPhone del usuario, alojamiento del relay y aceptación por datos móviles. La revisión visual queda a cargo del usuario.

Ajuste de simplicidad: ante un cliente lento se cierra la conexión tras cinco segundos de envío bloqueado y se reconstruye una pantalla completa al reconectar. No se mantiene una cola de deltas atrasados. Las selecciones remotas se cierran al reconectar; hace falta elegir la terminal de nuevo.

Verificación: `./Scripts/verify.sh`, `./Scripts/verify_remote.sh`, `./Scripts/build_ios.sh simulator` y `./Scripts/build_ios.sh device`. Prueba local: `./Scripts/test_remote.sh`.

## 1. Objetivo y alcance

Crear una app nativa para iPhone que controle por internet las terminales existentes que habilites en Vibra.

La primera versión permitirá vincular el teléfono, listar panes compartidos, abrir una terminal, ver su pantalla y escribir, incluyendo responder permisos de agentes y enviar `Ctrl+C`.

**Decisiones definitivas:**

- SwiftUI para iOS 17 o posterior, con SwiftTerm.
- Servicio remoto integrado en Vibra macOS.
- Un único transporte: WebSocket mediante un relay pequeño.
- Vinculación por QR y cifrado de extremo a extremo.
- Uso personal: un Mac y un iPhone, con un pane controlado a la vez.
- Instalación del iPhone desde Xcode.

Quedan fuera: Android, WebView, React Native, WebRTC, detección LAN, Tailscale, cuentas de usuario, notificaciones push, transferencia de archivos y creación de terminales.

El Mac debe permanecer encendido, despierto y con Vibra abierto.

## 2. Componentes y dependencias

```text
Vibra macOS ↔ Relay ↔ Vibra iPhone
      └── Contenido cifrado entre dispositivos ──┘
```

**Mac:** conservar Rust, GPUI y Ghostty. Añadir el servicio remoto dentro del proceso de Vibra, independiente del socket de hooks. No instalar helpers ni modificar la configuración de la shell.

**iPhone:** proyecto Xcode independiente dentro de `ios/`, nombre Vibra y bundle ID `app.vibra.VibraMobile`. Utilizar SwiftUI, cámara y Keychain de Apple, y `URLSessionWebSocketTask`. Incorporar SwiftTerm mediante Swift Package Manager; fijar inicialmente la versión `1.20.0`.

**Relay:** binario Rust con Tokio y Axum en `services/relay/`. Solo conecta dispositivos y reenvía mensajes cifrados. Sin base de datos, almacenamiento de terminales ni interpretación de comandos.

**Cifrado:** Noise mediante `snow` en Rust y `swift-noise` en Swift, con versiones fijadas. Usar `Noise_IK_25519_ChaChaPoly_SHA256`; no diseñar un protocolo criptográfico propio.

Las dependencias se incluyen en las aplicaciones o el servidor. El usuario instala únicamente Vibra para Mac y Vibra para iPhone.

## 3. Experiencia y comportamiento

### Vinculación y permisos

- Acceso remoto desactivado inicialmente.
- Settings del Mac permite configurar el relay, mostrar el QR y revocar el iPhone.
- El QR contiene la dirección del relay, la clave pública del Mac y una invitación aleatoria de un solo uso, válida cinco minutos.
- El Mac confirma el dispositivo antes de autorizarlo. Las claves privadas y credenciales persistentes se guardan en Keychain.
- El menú contextual de cada pane permite compartirlo o retirar su acceso, incluidas las terminales inferiores.
- El iPhone solo recibe metadatos y contenido de panes compartidos.
- La vinculación persiste; compartir panes dura hasta retirarlo, cerrar el pane o reiniciar Vibra.

Separar la autorización de conexión al relay de la autenticación entre dispositivos. Registrar las credenciales de conexión desde el Mac y reconstruir ese registro al reconectar, sin base de datos. Revocar el iPhone cierra el canal activo y bloquea nuevas conexiones autorizadas.

### Pantalla y recuperación

Tomar de Orca la idea de **estado inicial explícito y actualizaciones ordenadas**, conservando Ghostty como fuente de verdad.

Para evitar modificar su lector de PTY o añadir otro parser en el Mac:

- Extender `TerminalHandle` con lectura de pantalla remota e historial, independiente del scroll y selección locales.
- Capturar el estado de Ghostty y usar su formatter VT para generar secuencias ANSI de dibujo en el Mac. SwiftTerm recibe esas secuencias para representar la pantalla.
- Enviar una pantalla completa al abrir o reconectar; después, solo filas modificadas y cursor, hasta 20 veces por segundo y únicamente mientras exista un observador.
- Mantener revisiones propias para la sincronización remota. No consumir eventos ni indicadores de cambios utilizados por GPUI.
- Ante una discontinuidad o una cola saturada, reemplazar las actualizaciones pendientes por una pantalla completa.
- Ofrecer “Historial reciente” como lectura puntual de hasta 1.000 líneas del Mac, separada de la vista en vivo.

Esto transmite actualizaciones derivadas del estado, no el flujo crudo del PTY. Evita reproducir consultas de terminal antiguas o depender de reconstruir una sesión desde un fragmento de salida.

### Escritura, tamaño y desconexión

- Definir entradas de texto, teclas con modificadores y pegado. Extraer el codificador existente de Vibra para reutilizarlo sin depender de GPUI.
- El Mac aplica el modo real de la terminal, incluyendo bracketed paste y protocolos de teclado.
- El iPhone incluye `Esc`, `Tab`, `Ctrl`, flechas y `Ctrl+C`; confirma pegados de varias líneas.
- Al abrir un pane desde el teléfono, conceder control exclusivo y adaptar sus dimensiones al iPhone.
- Centralizar el arbitraje de tamaño: GPUI conserva el último tamaño local, pero no sobrescribe el remoto.
- Mostrar en el Mac quién controla el pane y una acción para recuperar el control.
- Al salir, pasar a segundo plano, revocar o perder conexión, liberar el control y restaurar el tamaño local. Detectar conexiones perdidas con heartbeat y vencimiento de 15 segundos.
- Al reconectar, autenticar y recuperar la pantalla antes de habilitar escritura. Nunca reenviar automáticamente entradas pendientes.

## 4. Protocolo, alojamiento y entrega

Definir un protocolo versionado con: listado de panes, apertura y cierre, pantalla completa, actualización de pantalla, historial, entrada, tamaño y errores. Usar mensajes JSON dentro del canal cifrado, con fragmentación, límites de tamaño y colas acotadas.

Preparar Docker y configuración de Fly.io para una sola máquina de 512 MB en São Paulo, sin volúmenes ni escalado automático. Incluir comprobación de salud y logs de conexión y errores, excluyendo contenido y secretos.

Fly.io será la opción de alojamiento inicial. La entrega deja el despliegue preparado; activar infraestructura y facturación requiere configurar la cuenta y revisar el costo. No se considera completada la validación por internet hasta probar el servicio alojado.

**Orden de implementación:**

1. Protocolo, relay local e interoperabilidad del cifrado Rust–Swift.
2. Lectura remota de Ghostty, entrada, sincronización y arbitraje de tamaño.
3. App iOS y controles de vinculación y permisos en el Mac.
4. Preparación del despliegue e instrucciones para instalar y probar en el iPhone.

## 5. Pruebas y aceptación

- Verificaciones existentes de Vibra, pruebas Rust/Swift y compilación para iOS.
- Vinculación válida, invitación vencida o reutilizada, identidad incorrecta, revocación y mensajes manipulados o repetidos.
- Apertura a mitad de una sesión, Unicode, colores, cursor, alternate screen, pegado y cambio de orientación.
- Reconexión, cliente lento, cierre de pane, suspensión del iPhone y restauración del tamaño local.
- Confirmar que el servicio remoto no bloquea el PTY ni interfiere con los eventos y renderizado de GPUI.

**Aceptación final:** desde el iPhone usando datos móviles, abrir un pane compartido con un agente ya ejecutándose, ver su pantalla actual, responderle y enviar `Ctrl+C`; recuperar la sesión después de una interrupción sin duplicar comandos; retirar el acceso desde el Mac.

La revisión visual y la prueba física del iPhone quedan a cargo del usuario, con una guía breve. No usar automatización de navegador para comprobar el diseño.

## Referencias de la investigación

- [Wrapper](https://github.com/heycupola/wrapper/tree/0c8b6be7cbe047e3aa80f1e19fb05fcb09042ce5): transporte CLI/PTY, tickets y relay. El repositorio separado del cliente móvil no fue accesible durante la investigación.
- [Orca: sincronización inicial de terminal](https://github.com/stablyai/orca/blob/ef428d879e09a02daa3bb9ed977a7802d89da289/src/main/runtime/rpc/methods/terminal/terminal-multiplex-initial-snapshot.ts): referencia para estado inicial y actualizaciones ordenadas; no se adopta su stack React Native/WebView.
- [SwiftTerm](https://github.com/migueldeicaza/SwiftTerm/tree/v1.20.0).
- [Noise Protocol Framework](https://noiseprotocol.org/noise.html), [snow](https://github.com/mcginty/snow) y [swift-noise](https://github.com/swift-libp2p/swift-noise).
- [Precios de Fly.io](https://fly.io/docs/about/pricing/): verificar al activar el servicio.
