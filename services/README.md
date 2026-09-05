# Base del control remoto iOS

Primer paso del [plan](../docs/plans/ios-remote-terminal.md). Este workspace Rust está separado de la app GPUI para permitir compilar y probar los servicios sin Xcode ni Ghostty. El paquete `protocol` no inicia servidores ni modifica terminales.

## Contrato v1

Cada mensaje tiene `version`, `request_id` y `message`. `request_id` sirve para correlacionar respuestas; no autentica mensajes ni sustituye la protección contra replay de Noise.

El cuerpo usa `kind`: `list_panes`, `panes`, `open`, `close`, `resize`, `input`, `screen`, `patch`, `history`, `history_result`, `resync`, `control_released`, `ping`, `pong`, `error`. Los mensajes de pane llevan UUID; tamaño y revisiones acompañan a las pantallas. La entrada diferencia texto, tecla con modificadores y pegado. El Mac interpretará esos valores según el modo real de Ghostty.

Ejemplo de Ctrl+C:

```json
{"version":1,"request_id":3,"message":{"kind":"input","pane_id":"00000000-0000-0000-0000-000000000000","input":{"kind":"key","key":{"character":"c"},"modifiers":["control"]}}}
```

Los límites iniciales son 1 MiB por mensaje JSON descifrado, 64 KiB por entrada, 128 panes y 1.000 líneas por consulta de historial. La pantalla admite entre 1×1 y 500×300 celdas. Las constantes de 20 Hz, ocho mensajes pendientes y heartbeat de 15 segundos son requisitos del futuro servicio, no colas ni timers ya implementados.

`FrameTracker` se crea por pane y conexión autenticada. Requiere una pantalla completa inicial, ignora revisiones antiguas y bloquea deltas tras una discontinuidad hasta recibir otra pantalla completa. No autoriza escritura ni valida identidades.

El sobre Swift en `ios/VibraRemoteProtocol` usa Foundation, con destino iOS 17/macOS 14. Es genérico sobre el payload; los modelos y controles completos del cliente se añadirán con la app iOS. La validación del cuerpo y los límites semánticos actuales están implementados en Rust. El contrato sigue en desarrollo y aún no está desplegado.

## Verificación

```sh
./Scripts/verify_remote.sh
```

Ejecuta formato, tests y Clippy de Rust; genera mensajes con el codec real; Swift decodifica una pantalla, comprueba ANSI/Unicode y vuelve a codificarla sin diferencias de contenido. `swift test --package-path ios/VibraRemoteProtocol` también funciona con fixtures incluidos.

**Siguiente hito:** relay WebSocket local e interoperabilidad del cifrado Noise. Estos mensajes solo deben circular dentro del canal autenticado y cifrado. La conexión, fragmentación cifrada, autorización, cuotas, control exclusivo y revocación todavía no están implementados.
