# Evaluación de libghostty-vt para Vibra

Fecha: 2026-09-04. Resultado: viable para un adaptador experimental; todavía no justifica cambiar el motor de producción.

Continuación: se implementó la [migración de la aplicación a Ghostty](../ghostty.md) en `feat/ghostty-terminal`. Las mediciones de este documento corresponden al harness aislado, no al rendimiento de esa integración.

## Decisión recomendada

Evaluar Ghostty estaba justificado: compila como biblioteca estática, se integra desde Rust y su exportación ANSI funciona con SwiftTerm en las pruebas realizadas. Recomiendo avanzar con un adaptador detrás de `TerminalPort`, manteniendo Alacritty como motor predeterminado hasta demostrar paridad. Si la prioridad inmediata es entregar iOS con el menor trabajo posible, conservar Alacritty sigue siendo la opción de menor alcance.

No hace falta instalar la aplicación Ghostty en el Mac del usuario. Zig se añade al proceso de compilación, no a los requisitos de ejecución. La popularidad de la aplicación Ghostty no demuestra por sí sola mayor estabilidad de su API embebible: los headers de libghostty-vt la declaran incompleta e inestable.

## Versiones y entorno

- Ghostty: commit `492300cad104195411d12217dd22f1cd05f31376` de main, fijado para esta evaluación.
- Zig 0.16.0, optimización `ReleaseFast`, destino mínimo macOS 14.
- Alacritty Terminal 0.26.0, versión de Vibra.
- SwiftTerm 1.20.0, commit `5d14406844143538cd8f8851d2d8a67c1fe443e5`.
- Mac Apple M5 Pro, macOS 26.6.2, Rust 1.96.0, Xcode 26.6.

El código reproducible está en [tools/terminal-eval](../../tools/terminal-eval/README.md), fuera del workspace de producción.

## Verificaciones realizadas

| Verificación | Resultado |
| --- | --- |
| Biblioteca estática arm64 macOS | Compila y ejecuta desde Rust mediante un puente C pequeño |
| Biblioteca estática x86_64 macOS | Compila; no ejecutada en hardware Intel |
| Archivo universal con `lipo` | Generado, ambas arquitecturas presentes; aproximadamente 22 MB de archivo, no de incremento final de la app |
| Dependencias del ejecutable Rust con `otool -L` | Solo bibliotecas del sistema; no carga un dylib Ghostty externo |
| Siete fixtures con entrada fragmentada byte a byte | Pasan exportación ANSI, reproducción en Ghostty y comprobaciones de texto en Alacritty |
| Reproducción de esos siete fixtures en SwiftTerm | Pasa, incluyendo texto Unicode completo y RGB exacto (17, 101, 221) |
| Respuestas generadas por SwiftTerm durante esos replays | Cero bytes |
| Resize 80×24 → 40×12 → 120×40 | Conserva el texto probado |
| Ejemplo C upstream de snapshots binarios | Pasa sus assertions de restauración completa e incremental; snapshot de 25.245 bytes |
| Harness Rust | `cargo clippy -- -D warnings` pasa |

Los fixtures cubren movimiento de cursor/borrado, truecolor, Unicode, pantalla alternativa, retorno a pantalla principal, región de scroll y entrada con modos de teclado. El roundtrip Ghostty compara texto exportado; las verificaciones cruzadas comprueban contenido esperado. No demuestran equivalencia completa de todos los modos, cursor, historial o estado de ambos buffers. SwiftTerm se ejecutó sin interfaz en macOS, con su paquete Apple normal; no se probó un dispositivo iOS. La opción upstream `SWIFTTERM_EXCLUDE_APPLE=1` falló por referencias a tipos Apple y no se usó en la ejecución final.

## Rendimiento observado

Mediana de siete ejecuciones después de una de calentamiento, alternando el orden de los motores. Terminal 120×40, historial configurado en 1.000 líneas, bloques de 4.096 bytes. Se mide consumo de VT y actualización del estado; creación, captura de celdas y destrucción quedan fuera del tiempo.

| Carga sintética | Bytes | Ghostty | Alacritty | Cociente Alacritty/Ghostty |
| --- | ---: | ---: | ---: | ---: |
| Log ASCII | 4.300.000 | 5,877 ms | 16,109 ms | 2,74× |
| Log con ANSI y truecolor | 4.200.000 | 11,777 ms | 14,876 ms | 1,26× |
| Actualizaciones tipo TUI | 4.560.000 | 10,522 ms | 20,958 ms | 1,99× |

Ghostty fue más rápido en estas cargas. Esto no mide FPS, latencia de teclado, CPU de Vibra completo, memoria, consumo de batería ni tráfico remoto. Tampoco es un benchmark independiente: se comparan versiones y compiladores concretos, y la política de poda del historial puede diferir. No permite afirmar que Vibra será entre 1,26 y 2,74 veces más rápido.

## Qué aporta a iOS y qué falta

El formatter de Ghostty exporta la pantalla a secuencias VT e incluye opciones para cursor, estilos y modos. Permite reducir el serializador manual necesario para reconstruir una pantalla en SwiftTerm. Los snapshots binarios son otra API: conservan más estado, incluso entrada de parser incompleta, pero SwiftTerm no los interpreta y su formato no promete compatibilidad estable. No deben convertirse en el protocolo móvil.

La exportación ANSI por sí sola no resuelve una reconexión exacta: aún hay que coordinar snapshot y stream con una secuencia, tratar buffers alternativos y estado que no se exporte, controlar las respuestas del emulador remoto y probar resize/reflow. Un Mac seguirá siendo autoridad sobre la sesión y el PTY. Ningún motor sustituye al relay, al cifrado o al emparejamiento del plan iOS.

## Coste real de migrar Vibra

La frontera existente está en `src/ports/terminal.rs`. El adaptador actual `src/infrastructure/alacritty.rs` usa también PTY, event loop, selección y búsqueda de Alacritty; no solo su parser.

| Área | Trabajo necesario |
| --- | --- |
| Render de GPUI | Traducir render-state/celdas/daño a `TerminalSnapshot`; mantener el renderer actual. libghostty-vt no incorpora el renderer GPU de la app Ghostty |
| Procesos y PTY | Implementar o incorporar gestión de PTY, lectura/escritura, resize, cierre y procesos; libghostty-vt no la proporciona |
| FFI | Encapsular ownership, callbacks y sincronización; el puente de evaluación no es un adaptador de producción |
| Interacción | Reproducir selección, búsqueda, enlaces, modos de teclado, scroll y detección de proceso/CWD |
| Clipboard | Resolver diferencia entre callbacks de lectura síncronos y confirmación asíncrona de la UI actual |
| Distribución | Fijar Zig y commit, generar ambas arquitecturas en CI y comprobar firma/empaquetado de la aplicación |

Menos dependencias instaladas por el usuario sí es compatible con Ghostty. Menos complejidad de desarrollo que la integración existente no está demostrado.

## Siguiente paso y criterios de aceptación

1. Implementar un adaptador Ghostty experimental usando `TerminalPort` y GPUI existentes, con PTY incluido en el proceso. No incorporar una segunda UI ni el ejecutable Ghostty.
2. Verificar shell interactiva, Ctrl+C, salida/cierre, resize, scroll, selección, búsqueda, enlaces y modos de teclado; probar procesos reales y respuestas de terminal.
3. Verificar reconexión ANSI con SwiftTerm, incluyendo aplicación TUI ya abierta, cambios de pantalla alternativa, historial y snapshot concurrente con salida del PTY.
4. Medir memoria y latencia en Vibra, y compilar/empaquetar ambas arquitecturas. La revisión visual la hace el usuario.
5. Cambiar el predeterminado solo tras estas verificaciones. Hasta entonces el plan iOS conserva Alacritty y SwiftTerm.

## Fuentes primarias

- [API libghostty-vt fijada](https://github.com/ghostty-org/ghostty/blob/492300cad104195411d12217dd22f1cd05f31376/include/ghostty/vt.h).
- [Formatter](https://github.com/ghostty-org/ghostty/blob/492300cad104195411d12217dd22f1cd05f31376/include/ghostty/vt/formatter.h), [snapshots](https://github.com/ghostty-org/ghostty/blob/492300cad104195411d12217dd22f1cd05f31376/include/ghostty/vt/snapshot.h) y [render-state](https://github.com/ghostty-org/ghostty/blob/492300cad104195411d12217dd22f1cd05f31376/include/ghostty/vt/render.h).
- [Alacritty Terminal 0.26.0](https://docs.rs/alacritty_terminal/0.26.0/alacritty_terminal/).
- [SwiftTerm 1.20.0](https://github.com/migueldeicaza/SwiftTerm/tree/v1.20.0).
