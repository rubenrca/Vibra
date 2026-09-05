# Control remoto Vibra · iOS

Workspace independiente de GPUI: `protocol` define y valida mensajes; `remote` implementa Noise y fragmentación; `relay` reenvía WebSockets cifrados. La app Mac integra el servicio en `src/infrastructure/remote.rs`. Cliente SwiftUI/SwiftTerm en `ios/Vibra.xcodeproj`.

[Guía para probar y alojar](../docs/plans/ios-remote-testing.md) · [Plan y decisiones](../docs/plans/ios-remote-terminal.md).

## Conexión y autorización

- WSS a `/ws`; WS solo para loopback de pruebas. El primer mensaje JSON de routing lleva rol, canal y token. El Mac registra además el token del teléfono. El relay guarda registros en RAM y permite reconectar al dueño durante una hora desde la desconexión; un reinicio pierde todos los registros y el Mac los vuelve a crear.
- El relay responde `ready` al Mac y `peer` a ambos al unirlos. Después reenvía exclusivamente mensajes binarios y ping/pong. Solo hay un teléfono por conexión; un cierre termina ambas puntas. No registra contenido ni credenciales en logs.
- Noise `IK_25519_ChaChaPoly_SHA256`, prólogo UTF-8 `Vibra remote v1`; iOS inicia con la clave pública del Mac obtenida del QR. Su primer payload cifrado lleva invitación y nombre. El Mac verifica la identidad vinculada o consume la invitación de cinco minutos y pide confirmación local. Responde `approved` dentro del segundo mensaje Noise.
- Claves privadas y vinculación en Llavero del sistema. Desactivado al iniciar; panes compartidos solo durante la ejecución actual. Revocar y cambiar relay rotan todas las credenciales. QR y confirmación no se sustituyen por autenticación del relay.

## Transporte y contrato v1

Un mensaje WebSocket binario contiene un registro Noise (máximo 65.535 bytes). Cada fragmento descifrado contiene un byte `0` (continúa) o `1` (último), seguido de hasta 60.000 bytes. Se reconstruye un único mensaje a la vez, hasta 1 MiB. No se permiten registros fuera de orden ni repeticiones: los nonces AEAD avanzan por dirección y conexión. Ante un error se cierra el canal.

El JSON reconstruido tiene `version`, `request_id`, `message`. Los requests del teléfono comienzan en 1 y crecen estrictamente; 0 se reserva para eventos del Mac. El cuerpo usa `kind`: `list_panes`, `panes`, `open`, `close`, `resize`, `input`, `screen`, `patch`, `history`, `history_result`, `resync`, `control_released`, `ping`, `pong`, `error`.

```json
{"version":1,"request_id":3,"message":{"kind":"input","pane_id":"00000000-0000-0000-0000-000000000000","input":{"kind":"key","key":{"character":"c"},"modifiers":["control"]}}}
```

El Mac interpreta las teclas con el mismo encoder que GPUI y el modo real de Ghostty, incluyendo Kitty y pegado entre marcadores. Solo se permite input tras enviar una pantalla completa de una terminal compartida y bajo control remoto.

## Pantalla y límites

Ghostty exporta cada fila del área activa con su formatter VT, además de cursor y paleta. No se transmiten bytes crudos del PTY ni se mueve el viewport/selección ni se limpia el daño de GPUI. Se comparan filas a un máximo de 20 Hz mientras existe un observador. `screen` inicia la revisión; `patch` incluye base y revisión nueva. SwiftTerm dibuja y no devuelve respuestas de protocolo durante ese dibujo. Un salto exige `resync`; una reconexión exige selección y pantalla completa nuevas.

Límites: JSON 1 MiB, input 64 KiB, 128 panes, 1.000 líneas de historial, tamaño entre 1×1 y 500×300, 200 solicitudes y 256 KiB de mensajes entrantes por segundo. La cola de salida iOS admite ocho mensajes; al llenarse se corta la conexión y se descartan los pendientes. En Mac cada envío tiene un plazo de cinco segundos; un cliente lento se desconecta y obtiene pantalla completa al volver. Los snapshots excepcionalmente complejos que excedan 1 MiB también cierran el canal; no se envían fotogramas truncados.

Solo una terminal tiene control remoto por conexión. Su PTY adopta el tamaño del iPhone y recuerda los cambios de tamaño locales para restaurarlos. Recuperar control invalida también resizes pendientes. El heartbeat autenticado caduca a los 15 segundos. Cerrar, descompartir, revocar, desconectar o pasar iOS a segundo plano libera el control; los errores también lo liberan mediante RAII.

El relay limita sockets a 512, registros a 1.024, buffers de websocket y plazo de introducción a cinco segundos. No hay base de datos, cuentas, archivos, creación de terminales ni acceso al socket de automatización.

## Verificar

`./Scripts/verify_remote.sh` ejecuta tests Rust, Clippy, fixtures actuales contra Swift y una sesión cifrada mediante el relay real. `./Scripts/build_ios.sh simulator` y `device` compilan la app nativa. El servicio se puede ejecutar solo con `cargo run --locked --manifest-path services/Cargo.toml -p vibra-relay`; por defecto escucha únicamente en `127.0.0.1:8787`.
