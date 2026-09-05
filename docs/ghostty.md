# Motor Ghostty de Vibra

Rama: `feat/ghostty-terminal`. Ghostty es el motor único de la aplicación. Se retiraron el adaptador Alacritty y su dependencia del workspace principal; el harness histórico de evaluación sigue aislado en `tools/terminal-eval`.

## Compilar y ejecutar

```sh
./Scripts/fetch_ghostty.sh
cargo run --locked
```

El script prepara Zig 0.16.0 con SHA256 verificado y Ghostty `492300cad104195411d12217dd22f1cd05f31376`. Todo queda en `.build/ghostty/`, sin instalaciones globales. Para utilizar un Zig existente, definir `ZIG=/ruta/a/zig`. Cargo enlaza el archivo estático; la aplicación distribuida no requiere Zig ni la aplicación Ghostty.

Ya no hace falta `--features ghostty` y se retiró `VIBRA_TERMINAL_BACKEND`. Las sesiones existentes en un proceso abierto continúan usando su binario hasta reiniciar Vibra.

```sh
./Scripts/package_app.sh release --universal --sign -
```

Genera `dist/Vibra.app` con slices arm64 y x86_64 y firma ad hoc. El empaquetador prepara las bibliotecas faltantes. Para preparar ambas explícitamente:

```sh
./Scripts/fetch_ghostty.sh aarch64
./Scripts/fetch_ghostty.sh x86_64
```

## Integración

- GPUI conserva el dibujo de texto y la interfaz; libghostty-vt aporta emulación y estado, no el renderer de la app Ghostty.
- PTY en el proceso de Vibra: shell de login, entorno de sesión, entrada, resize, consultas/respuestas VT, códigos de salida, señales y recogida del proceso hijo.
- Entrada, resize y cierre despiertan al worker mediante un socket local. No hay espera fija de 10 ms para enviar teclas. El sondeo periódico restante solo permite recoger procesos y escalar su cierre.
- El render-state indica qué filas cambiaron. Solo esas celdas cruzan el puente C/Rust; las filas sin cambios conservan su `Arc`. Cursor, scroll, selección, resize y colores se actualizan con el mismo snapshot.
- Historial configurado a 10.000 líneas y 64 MiB, sujeto a granularidad de páginas de Ghostty. La lectura de texto reciente está acotada y no depende del scroll visible.
- Selección normal, rectangular, por palabra y línea; búsqueda literal sensible a mayúsculas; enlaces OSC 8 y URLs visibles; teclado Kitty, ratón, foco y pantalla alternativa.
- Los helpers de entorno, paleta e inspección de procesos viven en `terminal_support.rs` y no dependen de otro emulador.

## Portapapeles con consentimiento

Ghostty exige contestar su callback dentro de la llamada. Para OSC 52 generamos una respuesta vacía y retenemos sus bytes en vez de escribirlos al PTY. Esto proporciona la selección de portapapeles y el terminador exactos del protocolo. Vibra copia esa plantilla y presenta el diálogo existente sin bloquear el parser.

Al permitir, el formatter incorpora el texto codificado en Base64 y lo envía por el canal de protocolo. Al denegar, envía la respuesta vacía. Ningún puntero prestado por Ghostty sobrevive al callback y no se transmite contenido antes del consentimiento. El bridge valida la plantilla y no recuerda permisos implícitos. Los protocolos de lectura distintos de OSC 52 conservan su denegación explícita; no se anuncian permisos para ellos. La escritura de texto al portapapeles usa el evento existente de Vibra.

## Búsqueda literal

El índice de Ghostty compara letras ASCII sin distinguir mayúsculas. El adaptador recorre sus candidatos y compara el texto exacto antes de seleccionar. Así conserva la búsqueda literal, incluyendo espacios y metacaracteres, sin añadir otro motor de expresiones regulares. Si no hay coincidencia exacta, restaura el viewport anterior.

## Validación y medición

```sh
./Scripts/verify.sh
cargo test --locked --release infrastructure::ghostty::tests::profile_sessions -- --ignored --nocapture
```

Las pruebas cubren fragmentación UTF-8/ANSI, truecolor, caracteres anchos, modos, pantalla alternativa, selección, búsqueda exacta, enlaces, permiso y denegación de portapapeles, reutilización de filas, borrado, historial y resize. Las pruebas con PTY real cubren entrada y entorno, Ctrl+C, respuestas de cursor, salida sostenida de 12.000 líneas, cierre de un proceso que ignora SIGHUP y entrada/salida de Vim. Los tests GPUI verifican el flujo de consentimiento sin inspección visual automatizada.

Resultado local: 127 pruebas funcionales aprobadas, perfil manual aprobado y `verify.sh` completo sin errores. Se generó el bundle release universal (arm64 y x86_64), build 136, con firma ad hoc y sin un dylib Ghostty externo.

Medición local del 2026-09-04, Apple M5 Pro, macOS 26.6.2, Rust release y Zig ReleaseFast. Ocho motores de 120×40 con 4.000 líneas de historial cada uno; 800 actualizaciones de una fila:

| Métrica | Resultado |
| --- | ---: |
| Consumir actualización y obtener snapshot, mediana | 0,005 ms |
| Consumir actualización y obtener snapshot, p95 | 0,010 ms |
| Celdas exportadas por el puente | 96.960 |
| Celdas si se exportara toda la pantalla cada vez | 3.840.000 |
| Reducción de celdas exportadas en esta carga | 97,475 % |
| Pico RSS del proceso antes/después de crear y ejercitar los ocho motores | 11.141.120 / 50.806.784 bytes |
| Ida y vuelta a una shell real, mediana / p95 de 30 muestras | 0,137 / 0,150 ms |

La latencia incluye la observación del resultado con sondeo de 100 µs. El RSS es el máximo del proceso de pruebas, no memoria exclusiva de cada terminal. Estas cifras miden el adaptador y el PTY; no miden FPS ni latencia visual de GPUI, batería o red. El perfil se ejecuta explícitamente y queda fuera de CI para no imponer umbrales de tiempo inestables.

El usuario validó visualmente la primera integración. La nueva implementación no se revisó con automatización de navegador. La compilación Intel no sustituye una prueba en hardware Intel.

Esta migración no implementa todavía el transporte ni el cliente iOS: [plan remoto](plans/ios-remote-terminal.md). La [evaluación original](evaluations/ghostty-vt.md) conserva los antecedentes y las mediciones comparativas de los parsers.
