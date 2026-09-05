# Backend experimental Ghostty

Rama: `feat/ghostty-terminal`. Estado: primer adaptador funcional para macOS; no es todavía el reemplazo definitivo de Alacritty.

## Compilar y ejecutar

Desde la raíz del repo:

```sh
./Scripts/fetch_ghostty.sh
cargo run --locked --features ghostty
```

El script descarga Zig 0.16.0 con SHA256 verificado y el commit Ghostty `492300cad104195411d12217dd22f1cd05f31376`, y compila libghostty-vt estáticamente. Todo queda en `.build/ghostty/`, sin instalaciones globales. Para reutilizar un Zig 0.16.0 existente, definir `ZIG=/ruta/a/zig` al ejecutar el script. Cargo no descarga ni compila Ghostty por su cuenta.

Con la feature `ghostty`, las terminales nuevas usan Ghostty. Para comparar con Alacritty usando el mismo binario:

```sh
VIBRA_TERMINAL_BACKEND=alacritty cargo run --locked --features ghostty
```

Sin la feature, Vibra conserva su backend anterior. No se convierten sesiones en ejecución entre motores.

Para generar un bundle local:

```sh
./Scripts/package_app.sh debug --ghostty --sign -
```

Resultado: `dist/Vibra.app`. El paquete contiene la biblioteca estática y su licencia; el usuario final no necesita Ghostty ni Zig. La revisión visual se hace manualmente.

Para un bundle universal, preparar antes ambas bibliotecas:

```sh
./Scripts/fetch_ghostty.sh aarch64
./Scripts/fetch_ghostty.sh x86_64
./Scripts/package_app.sh debug --universal --ghostty --sign -
```

## Qué incluye

- `TerminalPort` y `TerminalHandle` existentes, sin cambiar la UI de GPUI.
- PTY dentro de Vibra: shell de login, entorno de sesión, entrada, resize, respuestas VT, salida, señales y recogida del proceso hijo.
- Celdas Unicode y caracteres anchos, colores de tema y truecolor, estilos, cursor, pantalla alternativa y modos de teclado/ratón.
- Historial limitado a 10.000 líneas y 64 MiB, con la granularidad de páginas de Ghostty; scroll, borrado y texto reciente acotado.
- Selección normal, rectangular, por palabra y por línea; búsqueda; enlaces OSC 8 y URLs visibles.
- Título, campana y escritura de texto al portapapeles.
- Un puente C contra headers fijados. Rust serializa el acceso al motor, copia los datos prestados antes de liberarlos y conserva snapshots/filas mediante `Arc`.

Los helpers de entorno, paleta e inspección de procesos se comparten por ahora con el módulo Alacritty. No hay dos motores procesando la misma sesión. La dependencia Alacritty permanece para permitir la comparación y vuelta atrás.

## Diferencias y trabajo pendiente antes de reemplazar Alacritty

1. La búsqueda upstream es insensible a mayúsculas ASCII, mientras que el adaptador anterior busca literalmente distinguiéndolas.
2. Las lecturas OSC 52 no están conectadas al consentimiento asíncrono de Vibra: el callback Ghostty exige respuesta síncrona. Se deja sin registrar; no se autoriza acceso al portapapeles automáticamente. La escritura de texto sí se conecta al evento existente.
3. El primer adaptador reconstruye las celdas visibles cuando hay cambios y reutiliza las filas idénticas. Falta aprovechar el daño por fila para evitar recorrer toda la pantalla y medir memoria/latencia en la aplicación completa. El worker atiende entrada como máximo cada 10 ms cuando está inactivo.
4. Falta validación manual de aplicaciones TUI, selección compleja, teclado y reflow. No se ha revisado la interfaz con automatización ni probado hardware Intel/iPhone.
5. Esta rama no implementa el transporte remoto ni modifica el cliente iOS del plan. El formatter ANSI investigado sigue disponible para esa siguiente etapa.

## Validación

```sh
cargo test --locked --features ghostty
./Scripts/verify.sh
```

Las pruebas del adaptador ejercitan Unicode fragmentado, estilos, modos, pantalla alternativa, selección, búsqueda, enlaces, portapapeles de escritura, respuestas VT, historial y resize. Las pruebas de PTY real verifican entrada/entorno, tamaño, Ctrl+C, consulta de cursor, código de salida y cierre de un proceso que ignora SIGHUP. `verify.sh` conserva las pruebas del backend anterior y ejecuta Clippy con todas las features, por lo que requiere preparar Ghostty primero.

Verificado localmente el 2026-09-04: 134 pruebas con Ghostty, 127 sin la feature, `verify.sh` completo y bundle arm64 con firma ad hoc. El bundle no se abrió ni se revisó visualmente.

La CI prepara las dos arquitecturas, ejecuta las pruebas y empaqueta un bundle universal experimental. Esto configura la verificación remota; su ejecución en GitHub depende de publicar la rama.

Antecedentes y mediciones del parser: [evaluación](evaluations/ghostty-vt.md). Plan del cliente remoto: [iOS](plans/ios-remote-terminal.md).
