# Probar control remoto iOS

Implementación en `feat/ios-remote-terminal`. Requiere macOS con Vibra abierto; iOS 17 o posterior. No requiere instalar Ghostty, tmux, SSH, Tailscale ni procesos auxiliares en el Mac del usuario. El relay es el único servicio externo.

## Prueba local, sin cuenta ni servidor contratado

Desde la raíz del repositorio:

```sh
./Scripts/test_remote.sh
```

Compila Vibra para Mac, compila/instala Vibra iOS en un simulador iPhone disponible, inicia el relay en `127.0.0.1:8787`, verifica escritura/lectura/borrado en el Llavero del simulador y abre ambas apps. El build Mac está en `dist/Vibra.app`; los logs quedan en `.build/remote-test/`. Cierra una versión anterior de Vibra antes de ejecutar el script para trabajar con una sola instancia.

1. En **Ajustes → iPhone**, pulsa **Vincular iPhone**. Esto activa el servicio y genera una invitación que dura cinco minutos. Para cambiar de iPhone, usa **Ajustes → iPhone → Desvincular iPhone** y genera otra invitación.
2. Pulsa **Copiar invitación** y pégala en **Vibra iOS → ¿No puedes escanear? → Pegar**. El portapapeles compartido del simulador debe estar activado.
3. En el Mac aparecerá el nombre del iPhone. Pulsa **Aceptar y vincular**. La invitación se consume incluso si rechazas la solicitud; para reintentar genera otra.
4. Abre el menú contextual de la terminal elegida y pulsa **Compartir con iPhone**. Funciona también en las terminales inferiores de desarrollo; puedes abrir su menú desde la pestaña.
5. Selecciona esa terminal en el iPhone. Aparece la pantalla actual y el PTY toma las dimensiones del teléfono. El Mac muestra un aviso para recuperar el control.

El relay local se limita al simulador: `127.0.0.1` en un iPhone físico apunta al propio teléfono. No se permite WebSocket sin TLS fuera de loopback.

Para detener el relay iniciado por el script, consulta `.build/remote-test/relay.pid` y termina ese PID después de verificar que sigue correspondiendo a `vibra-relay`. Cerrar Vibra iOS libera el control; el Mac conserva sus terminales.

## Pruebas manuales importantes

- Abre una terminal que ya tenga texto, Unicode/emoji y colores. Comprueba que se ve desde el primer fotograma.
- Prueba `top` o `vim`, flechas, Esc, Tab, Ctrl+C y el modificador Ctrl. Sal del TUI y vuelve a entrar.
- Cambia la orientación y abre/cierra el teclado. La terminal debe ajustarse al iPhone.
- Desplaza el historial en el Mac: la pantalla del iPhone debe seguir mostrando el área activa. El historial del teléfono se consulta con **Historial**, separado de la pantalla actual.
- Pega varias líneas y cancela primero: no debe enviarse nada. Después confirma el pegado.
- Pulsa el aviso de control remoto en el Mac o **Recuperar control del iPhone**. Debe restaurarse el último tamaño local y bloquearse la entrada anterior del teléfono.
- Usa **Dejar de compartir con iPhone**, cierra el pane, desactiva el servicio y revoca el teléfono. En todos los casos debe cesar su acceso.
- Pasa iOS a segundo plano, corta la conexión o detén el relay. El Mac debe recuperar el control de inmediato al detectar el cierre, o como máximo tras 15 segundos sin mensajes autenticados.
- Restablece la conexión: vuelve la lista de panes; debes seleccionar otra vez la terminal. No se reenvían pulsaciones pendientes ni se recupera el control automáticamente.
- Reinicia Vibra Mac: el servicio comienza desactivado y ningún pane queda compartido. La identidad del iPhone permanece en el Llavero hasta revocarla.

La inspección de diseño y distribución visual queda a cargo del usuario; no se ha automatizado una revisión visual.

## iPhone físico e internet

1. Aloja el relay con TLS. Se incluyen `services/relay/Dockerfile` y `services/relay/fly.toml`: una instancia, región `gru`, 512 MB, sin disco ni escalado automático. No se ha contratado ni desplegado un servidor.
2. En tu organización de Fly.io, crea una app con un nombre propio, reemplaza `CHANGE-ME-vibra-relay` en `fly.toml` y despliega **desde la raíz del repo**:

   ```sh
   fly deploy --config services/relay/fly.toml --ha=false
   fly scale count 1 --config services/relay/fly.toml
   ```

   El endpoint será `wss://NOMBRE.fly.dev/ws`. `/health` devuelve la identidad del relay. La instancia debe permanecer encendida; todas las conexiones de una pareja deben llegar a la misma instancia. El alojamiento tiene un coste dependiente de tu cuenta.
3. Copia ese endpoint en el Mac y pulsa **Usar dirección del relay copiada**. Cambiarlo revoca la vinculación anterior. Genera un QR nuevo.
4. Abre `ios/Vibra.xcodeproj` en Xcode, selecciona el esquema **Vibra**, tu iPhone y tu **Team** en Signing & Capabilities. Ajusta el bundle ID si tu equipo requiere otro. Ejecuta con Run y acepta la autorización del dispositivo que solicite iOS.
5. Escanea el QR desde Vibra iOS y confirma en el Mac. Comparte el pane. Desactiva el Wi-Fi del iPhone para verificar la conexión por datos móviles.

El binario para dispositivo se compila sin firma con `./Scripts/build_ios.sh device`, pero no se puede instalar ese binario sin firmarlo con un equipo de Apple. No se ha hecho una prueba física ni por datos móviles desde este entorno.

## Verificación automatizada

```sh
./Scripts/verify.sh
./Scripts/verify_remote.sh
./Scripts/build_ios.sh simulator
./Scripts/build_ios.sh device
# Con un simulador iniciado:
./Scripts/verify_ios_keychain.sh booted
```

Incluye formato, Clippy, tests existentes de Vibra, PTY real y propiedad del tamaño, viewport/selección/damage independientes, relay real + Noise + permisos + caducidad de control, autenticación del relay, invitaciones, fixtures Noise Rust–Swift en ambas direcciones, fragmentación, replay y reconstrucción Ghostty–SwiftTerm. Los fixtures criptográficos usan claves de prueba deterministas, nunca claves reales.

Para actualizar intencionalmente los fixtures tras un cambio de protocolo:

```sh
cargo run --quiet --manifest-path services/Cargo.toml -p vibra-remote --example interop > ios/VibraRemoteProtocol/Tests/VibraRemoteProtocolTests/Fixtures/noise.json
VIBRA_SCREEN_FIXTURE="$PWD/ios/VibraRemoteProtocol/Tests/VibraRemoteProtocolTests/Fixtures/screen.json" cargo test remote_export_preserves
```

SwiftTerm incluye un plugin que genera metadatos de compilación; el script acepta ese plugin fijado por versión. Xcode puede requerir descargar su componente Metal: `xcodebuild -downloadComponent MetalToolchain`.

Referencias del alojamiento: [configuración oficial de Fly.io](https://fly.io/docs/reference/configuration/) y [opciones de deploy](https://fly.io/docs/flyctl/deploy/).

### Firma del simulador

El build del simulador usa firma ad hoc (`CODE_SIGNING_ALLOWED=YES`, `CODE_SIGN_IDENTITY=-`). Xcode debe incorporar `application-identifier`: una app sin esos permisos puede abrirse pero sus llamadas al Llavero fallan con `-34018` antes de conectar al relay. `verify_ios_keychain.sh` instala el build y verifica las operaciones reales desde el proceso iOS, con un elemento temporal aislado. No lee ni modifica las credenciales de vinculación. No es una inspección visual.

### Prueba funcional en las apps · 2026-09-04

Probado mediante interacción con Vibra Mac y el simulador iPhone 17 Pro (iOS 26.5), con relay local:

- Copiar invitación, pegar en iOS y aprobar el iPhone desde Ajustes del Mac.
- Compartir el pane desde su menú contextual y abrirlo desde iOS.
- Ejecutar `echo VIBRA_REMOTE_OK_1234567890` y comprobar la salida completa en ambas apps.
- Interrumpir `sleep 30` con el botón Ctrl+C y volver al prompt.
- Recuperar el control desde el indicador del Mac y ejecutar `echo MAC_CONTROL_OK` localmente.
- Reinstalar y reiniciar iOS conservando la vinculación del Llavero.

Durante la prueba se corrigió la desconexión por ráfagas de escritura: se agrupan eventos de texto consecutivos del mismo pane, manteniendo la cola limitada a ocho mensajes y cada texto a 64 KiB. No se agrupan a través de teclas especiales, pegados o cambios de pane. La regresión Swift verifica texto Unicode y esos límites; ocho tests Swift pasan. El build corregido quedó instalado en el simulador.

Esta prueba no valida todavía un iPhone físico ni una conexión por Internet.

### Organización de Ajustes

Ajustes en macOS tiene navegación fija por General, Apariencia, iPhone, Agentes y Privacidad. La categoría iPhone reúne la vinculación y un interruptor para permitir acceso sin eliminar el dispositivo. Desvincular es una acción separada; Opciones avanzadas solo contiene la configuración del servidor. Los errores de conexión aparecen dentro de la categoría y copiar la invitación muestra una confirmación. Cada categoría conserva su propio desplazamiento.
