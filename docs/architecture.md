# Arquitectura del workspace

Vibra separa el estado persistente, los adaptadores del sistema y la coordinación
GPUI. Los módulos de una misma vista comparten su estado; no mantienen una copia
independiente del workspace.

## Responsabilidades

- `src/domain/workspace`: proyectos, tabs, panes, selección, geometría y migraciones.
- `src/domain/work_items.rs`: tareas externas, filtros, detalle remoto y marcas de
  lectura. `src/infrastructure/work_items` contiene los conectores GitHub/Linear,
  consultas acotadas, comentarios, acciones de PR y preparación del contexto para
  terminales; las credenciales no pasan por los argumentos del proceso.
  `work_inbox.rs` coordina las fuentes; `work_inbox/detail.rs` las cargas y acciones
  por URL; `work_inbox/layout` la lista, menús y detalle nativos. Las respuestas se
  validan por fuente, generación y revisión de la tarea. Cambiar de conexión
  invalida el detalle. Los borradores permanecen con su URL y una confirmación de
  merge captura el head aprobado (`expectedHeadOid`). `src/ui/markdown.rs` renderiza
  CommonMark sin ejecutar HTML remoto. La actividad local sigue en `inbox.rs`.
  Véase [la adaptación de MonoCode](inbox-monocode.md).
- `src/ports`: contratos de terminal, Git y filesystem; `src/infrastructure` implementa
  esos contratos y los repositorios de JSON.
- `src/infrastructure/paths.rs`: lecturas de archivos con límite, escrituras atómicas
  y control de revisiones. Workspace, settings, caché del Inbox y temas
  comparten el mismo lector; comprueba también el tamaño después de leer.
  La importación del preview toma el mismo lock que los guardados y comprueba
  dentro del lock que el destino siga ausente.
  Un documento ausente permite el primer arranque; un symlink roto bloquea carga
  y guardado. RevisionGuard protege revisión, recuperación y merge-input juntos.
  Workspace comparte una lectura/decode para migración y backups bajo lock.
- `src/infrastructure/process.rs`: captura acotada de comandos breves, entrada y
  salida concurrentes y cierre del grupo de procesos antes de reunir los lectores.
  Inbox, consulta de cuotas, lectura del llavero por CLI y generación de mensajes
  de commit usan este adaptador. Cada consumidor conserva su plazo y sus límites.
  Las consultas Git también usan este adaptador: 30 segundos para lecturas y
  corte al byte adicional de truncamiento. Las escrituras conservan 120 segundos.
  La cancelación incluye los workers de entrada/salida aunque un descendiente
  mantenga una tubería abierta fuera del grupo.
- `src/infrastructure/login_shell.rs`: carga el entorno de login y delega los
  scripts propios a POSIX sh. GitHub y mensajes de commit comparten esa ruta;
  los prompts lanzados dentro de una terminal conservan su PATH ya establecido.
- `src/infrastructure/git/parsing.rs`: parsers puros de status, numstat, historial
  y patches. El adaptador Git mantiene la ejecución de comandos y las cachés.
- `src/ui/workspace_view/mod.rs`: construcción y coordinación de la ventana.
- `navigation.rs`, `projects.rs` y `tabs.rs`: secciones globales, proyectos y navegación
  entre terminales y revisión. `prepare_terminal_tab` aplica la transición compartida;
  `show_terminal_tab` añade el guardado y el foco inmediato. Los atajos y sus etiquetas
  cuentan la revisión dentro de la misma fila de tabs.
- `terminals.rs`: ciclo de vida de los PTY, visibilidad, foco, nombres de panes y
  entrega de eventos. El foco diferido comprueba que su pane siga seleccionado y visible.
  Un fallo nativo o mutex poisoned invalida el motor Ghostty; el snapshot Rust sigue
  disponible y el proceso se recolecta. TerminalView conserva `Failed` tras `Exit`
  y cancela entrada, búsquedas y pegados pendientes.
- `explorer.rs`: carga y selección de archivos, filas del árbol y creación de entradas.
  `files.rs` reúne las filas visibles, el watcher y los iconos/estados de Git.
  Quick Open pide `FileSystemPort::search_files`; el adaptador local aplica ignores,
  recorre submódulos y conserva el presupuesto compartido de archivos. La captura
  de `git ls-files` pertenece al adaptador Git y comparte su lector acotado.
- `context_menu.rs`: menús de proyectos/panes y los prompts de nombres.
- `storage.rs`: debounce y errores de guardado de la vista.
  `src/infrastructure/persistence/queue.rs`: cola de escrituras fuera del hilo GPUI.
  Conserva un último estado por documento y comparte guardado y errores con la
  ruta de emergencia. El orden sigue siendo workspace y settings.
- `src/ui/diff_view/changes_panel.rs`: operaciones y controles de Changes.
  Commit, sync y staging pasan por `write_repository`, que centraliza ocupación,
  generaciones, errores y actualización posterior de la vista.
- `src/ui/diff_view/repository.rs`: generaciones, cargas asíncronas y caché de
  documentos. `rows.rs`: lista virtualizada, scroll, pliegues y tarjetas de
  comentarios. `rendering.rs`: controles y estados vacíos. `history.rs`: tabla y
  grafo de commits. Todos operan sobre el mismo `DiffView`; las revisiones remotas
  continúan compartiendo los documentos y las filas de las revisiones locales.

## Contratos que conviene conservar

La interfaz muestra una fila de tabs por proyecto. Las aperturas interactivas y los
lanzamientos desde Review usan `open_tab_in_project`. El contenedor serializado
`TerminalWorkspaceSnapshot` se conserva para leer instalaciones anteriores; al
cargar la ventana, sus sesiones antiguas se consolidan sin perder tabs ni panes.
Los campos Swift y los grupos antiguos solo sirven para migrar datos. No deben
volver a usarse para construir la sidebar.
Los constructores de varias sesiones y sus operaciones antiguas están aislados en
`legacy_fixtures.rs`, compilado solo en tests. Ni el primer arranque puede crear
contenedores ocultos: también usa `open_tab_in_project`.

La selección de un pane desactiva el zoom de otro pane; el teclado nunca debe apuntar
a una terminal oculta. Los nombres manuales y de lanzamientos pertenecen al pane,
no al proceso del agente. Solo se descartan al cerrar ese pane. Cambiar su `cwd` no
renombra los contenedores antiguos. Los recorridos de sesiones devuelven referencias,
sin clonar el workspace en cada render de la barra de estado o el Inbox.
Las ediciones del tab seleccionado resuelven su selección y normalizan el proyecto
en un solo lugar. Insertar un terminal o un layout completo usa la misma operación
del árbol, para conservar geometría y orden.
Redimensionar por teclado afecta al divisor más cercano del eje pedido, incluso
cuando ya alcanzó su límite; ese caso no debe modificar un divisor ancestro.

`AppSettings` es la fuente de visibilidad de ambos sidebars. La vista conserva solo
el progreso de la animación. Settings comparte componentes para tarjetas y etiquetas;
los temas propios usan los mismos valores por defecto de roles que el resto del
catálogo, con sus colores particulares explícitos.
El catálogo propio está en `theme/user.rs`: sus IDs independientes de colisiones
se resuelven junto a aliases legacy antes de persistir una preferencia disponible.

La raíz del proyecto define Explorer, Changes y las terminales nuevas. El `cwd`
de una terminal puede cambiar sin alterar esa raíz.

Los resultados asíncronos pertenecen al contexto que los pidió. La barra de estado
conserva la raíz junto al resumen Git. Changes usa una generación al cambiar de
proyecto: una escritura Git aceptada puede terminar en el repositorio original,
pero su respuesta no modifica el nuevo panel. Las operaciones que escriben el
repositorio comparten el estado de ocupación.
Explorer limpia inmediatamente las filas del proyecto anterior al cambiar de raíz;
su generación descarta respuestas atrasadas. Cualquier cierre de la paleta cancela
su búsqueda pendiente. Cada envío de comentarios tiene un UUID propio: una respuesta
tardía no puede confirmar ni desbloquear otra revisión, incluso si reintenta los
mismos comentarios.

Al cerrar, la cola recibe el último estado de ambos documentos y espera a que
llegue al disco. No lanzar guardados independientes ni competir con la cola
durante el cierre. Los errores de carga y de guardado están separados; un guardado
no puede borrar el bloqueo de un JSON que no se pudo cargar.

Crear archivos pertenece a `FileSystemPort`, no a la UI. El adaptador local valida
la raíz y los padres existentes, incluidos enlaces simbólicos, antes de crear
rutas anidadas, y no reemplaza archivos existentes.

## Validación

`./Scripts/verify.sh` ejecuta formato, pruebas Rust, Clippy, pruebas de release y
validación de plist/scripts, además del harness nativo del parche `block`.
Las pruebas GPUI comprueban estado, eventos y destinos
de escritura. La revisión visual del diseño se realiza manualmente.
