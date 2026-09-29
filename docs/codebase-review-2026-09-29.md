# Revisión de codebase — 29 de septiembre de 2026

Se aplicó una limpieza de estructura y de lógica repetida, y se corrigieron los
problemas concretos descritos abajo. La aplicación conserva sus flujos, migraciones
y presentación. La revisión visual sigue a cargo del usuario, según `AGENTS.md`.

## Alcance y evidencia

El inventario inicial contiene 109 archivos y 56.541 líneas de Rust propios de la
app. El análisis comprende estructura y referencias sobre `src`, dependencias
directas, textos de la aplicación, código repetido, uso de procesos, lecturas y
escrituras, y revisión de los caminos críticos de persistencia, terminales, Git,
Inbox, cuotas y coordinación asíncrona. Se revisaron también los bridges nativos,
el build, los scripts de distribución y las comprobaciones de CI.

La primera pasada quedó en 56.755 líneas al separar responsabilidades y agregar
pruebas. La simplificación posterior elimina **388 líneas netas** de ese estado:
el inventario final contiene **115 archivos y 56.367 líneas**, incluyendo tests.
Frente al inicio de la revisión, el balance es de **174 líneas menos**, contando
los lectores compartidos y las siete pruebas nuevas. No se eliminaron pruebas
existentes ni funcionalidades para obtener esa reducción.

| Área | Resultado |
| --- | --- |
| Dominio y migraciones | Las pruebas cubren IDs repetidos, selección, geometría, consolidación de sesiones y conservación de proyectos. Se conservaron los formatos y fixtures de migración. |
| Persistencia | La cola pertenece a infraestructura. Los tres documentos comparten captura de estado, procesamiento y errores, incluyendo la ruta de emergencia. Se conservan orden, guardado final, escrituras atómicas, locks y copias de recuperación. |
| Terminal y FFI | Se revisaron propiedad del motor, callbacks, colas acotadas, PTY y cierre del proceso. La suite mantiene cobertura de entrada, búsqueda, selección, clipboard y reaping. |
| Git y revisiones | Se separaron parsing y ejecución. Se conservan el índice privado de capturas, los límites de patches, los cambios por repositorio y el descarte de respuestas atrasadas. |
| Inbox y cuotas | Se comparte la captura de comandos sin incluir credenciales en diagnósticos nuevos. Se mantienen generaciones, sesiones por tarea, cachés por cuenta y cooldowns. |
| Interfaz | Se eliminan copias de preferencias, wrappers de atajos y constructores repetidos de Settings. Las 12 paletas propias conservaron todos sus valores en una comparación programática antes/después. Se mantienen las pruebas de foco, tabs, pliegues, comentarios, scroll y revisiones remotas. |
| Dependencias y recursos | Todas las dependencias directas tienen referencias en la aplicación o el build. No se identificó una dependencia eliminable con evidencia suficiente. Se conserva el vendor GPUI y su parche documentado. |
| Distribución | Las siete pruebas de release, los plist y la sintaxis de scripts pasan. No se generó ni publicó una distribución nueva. |

## Problemas corregidos

1. **Generación de mensajes de commit: procesos y memoria.** Las salidas de stdout
   y stderr se acumulaban sin límite. El timeout mataba solo el proceso principal
   y retornaba sin reunir los lectores; un descendiente podía conservar las
   tuberías. Ahora se usa un grupo privado, captura con límite de 64 KiB por
   salida y cleanup antes de reunir los workers. El plazo sigue siendo 120 s.
2. **Lectura de temas: límite incompleto.** Se comprobaba metadata y después se
   hacía un `fs::read` sin límite. Si el archivo crecía entre ambos pasos, la
   lectura podía superar 64 KiB. El lector compartido acota la lectura sobre el
   archivo abierto y verifica el tamaño final.
3. **Verificación bloqueada por Clippy.** `decode_hex` usaba `% 2 != 0`, rechazado
   por la configuración de Rust 1.96 con `-D warnings`. Se usa `is_multiple_of`.
4. **Texto de arranque.** El error propio de inicio estaba en español. Ahora está
   en inglés, de acuerdo con `AGENTS.md`. Se conservan textos de usuario, fixtures
   multilingües y reconocimiento de respuestas en distintos idiomas.

## Limpieza aplicada

- `diff_view.rs` pasa de 4.259 a 966 líneas. Las cargas y cachés están en
  `repository.rs`; filas y comentarios en `rows.rs`; controles en `rendering.rs`;
  historial y grafo en `history.rs`. Las dependencias de los nuevos módulos son
  explícitas y sus métodos internos conservan visibilidad limitada.
- Los parsers de Git pasan a `git/parsing.rs`. `run_git` y `run_git_diff` comparten
  la captura con límites, conservando el byte adicional usado para detectar
  truncamiento. Los diffs ya no heredan stdin.
- Inbox, cuotas, llavero por CLI y generación de commits comparten el manejo de
  procesos. Cada llamada especifica sus límites; los consumidores siguen
  decidiendo cómo tratar fallos y respuestas demasiado grandes.
- Workspace, settings, library, caché de Inbox y temas comparten la lectura
  acotada. Se conserva la detección de archivos ausentes y el bloqueo ante una
  carga fallida.
- Se elimina la implementación manual de `Clone` de `WorkspaceRepository` y la
  creación redundante de directorios en el guardado de library.
- Las comparaciones entre revisiones guardadas evitan consultar metadata del
  working tree que luego se descartaba.
- Las operaciones de panes comparten resolución de selección y normalización;
  insertar un pane y fusionar layouts comparten la implementación del árbol.
- Se eliminan 14 métodos que solo reenviaban atajos. Las asociaciones del teclado
  llaman a las operaciones correspondientes directamente.
- La vista deja de duplicar la visibilidad de los sidebars: lee `AppSettings` y
  conserva el estado necesario para la animación.
- Los guardados se agrupan por documento; una sola ruta procesa resultados y
  fallos. La cola pasa de la vista a infraestructura con las mismas pruebas.
- Commit, sync y staging comparten `write_repository`, incluyendo el descarte de
  respuestas de otro proyecto y la exclusión de escrituras simultáneas.
- Quick Open usa `FileSystemPort::search_files`. La UI deja de ejecutar Git y
  recorrer archivos para el índice. Se conservan ignores, submódulos, límites y
  resultados parciales; sus pruebas pasan al adaptador de filesystem.
- Settings comparte tarjetas y etiquetas de controles. Los temas propios heredan
  roles comunes del constructor ya usado por el catálogo y solo declaran sus
  diferencias; las paletas ANSI son arrays explícitos, sin wrapper de 16 argumentos.
- Se actualiza `architecture.md` para reflejar los límites de responsabilidad.

## Observaciones para futuras refactorizaciones

Estas son oportunidades o riesgos de mantenimiento identificados; no son fallos
reproducidos por la suite actual.

| Prioridad | Evidencia | Siguiente cambio razonable |
| --- | --- | --- |
| Media | `infrastructure/git.rs`: comandos de escritura y del índice privado usan `Command::output`; las lecturas Git no tienen deadline. | Definir cancelación y límites por operación, teniendo en cuenta que cortar una escritura puede dejar locks de Git. No aplicar el timeout de consultas a mutaciones sin esa política. |
| Media | `infrastructure/ghostty.rs` y `terminal_support.rs` reciben paleta/generación desde `ui::theme`. | Publicar esos datos a través del contrato de terminal para reducir la dependencia del backend sobre la UI. |
| Media | `infrastructure/ghostty.rs`: locks del motor y construcción de snapshot contienen `unwrap`/`expect`. | Definir un estado de terminal fallida y su propagación por el puerto. Recuperar un mutex envenenado sin conocer el estado nativo sería una decisión insegura. |
| Baja | `ui/terminal.rs` tiene 2.527 líneas; `infrastructure/git.rs` mantiene 1.820. | Simplificar responsabilidades y repetición de entrada/render de terminal y referencias/operaciones Git con sus pruebas existentes. El tamaño por sí solo no justifica reescribirlos. |
| Dependencia | Cargo reporta incompatibilidad futura en `block 0.1.6`, transitiva de GPUI/Cocoa/Metal. | Mantener seguimiento del árbol nativo. Cambiarlo exige validar FFI y el parche Metal de GPUI; el toolchain actual compila. |

## Validación final

- `cargo fmt --check`: correcto.
- `cargo test --locked -- --quiet`: **390 pasan, 0 fallan y 2 ignoradas**.
- `cargo clippy --locked --all-targets --all-features -- -D warnings`: correcto.
- `python3 Scripts/test_release.py`: **7 pruebas correctas**.
- `plutil -lint Resources/Info.plist Resources/Vibra.entitlements`: correcto.
- `zsh -n Scripts/*.sh` y `git diff --check`: correctos.
- Comparación programática de las 12 paletas propias antes/después: todos sus
  campos coinciden. La comprobación temporal no forma parte de la codebase final.

Se agregaron siete pruebas: cinco de procesos y tuberías, una de límites de
archivos y una de lectura de temas. Las dos ignoradas conservan sus motivos:
medición manual de rendimiento y consulta opt-in de cuotas con sesiones reales.
Las pruebas de sockets requieren ejecución fuera del sandbox; al repetirlas así
pasaron tanto antes como después de los cambios.
