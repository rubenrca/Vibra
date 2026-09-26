# Arquitectura del workspace

Vibra separa el estado persistente, los adaptadores del sistema y la coordinación
GPUI. Los módulos de una misma vista comparten su estado; no mantienen una copia
independiente del workspace.

## Responsabilidades

- `src/domain/workspace`: proyectos, tabs, panes, selección, geometría y migraciones.
- `src/domain/library.rs`: notas, automatizaciones y reglas del horario.
- `src/ports`: contratos de terminal, Git y filesystem; `src/infrastructure` implementa
  esos contratos y los repositorios de JSON.
- `src/ui/workspace_view/mod.rs`: construcción y coordinación de la ventana.
- `navigation.rs`, `projects.rs` y `tabs.rs`: secciones globales, proyectos y navegación
  entre terminales y revisión. `prepare_terminal_tab` aplica la transición compartida;
  `show_terminal_tab` añade el guardado y el foco inmediato. Los atajos y sus etiquetas
  cuentan la revisión dentro de la misma fila de tabs.
- `terminals.rs`: ciclo de vida de los PTY, visibilidad, foco, nombres de panes y
  entrega de eventos. El foco diferido comprueba que su pane siga seleccionado y visible.
- `explorer.rs`: carga y selección de archivos, filas del árbol y creación de entradas.
  `files.rs` reúne el recorrido del filesystem, el watcher y los iconos/estados de Git.
- `context_menu.rs`: menús de proyectos/panes y los prompts de nombres.
- `storage.rs`: debounce y errores de guardado de la vista. `persistence.rs`: una cola
  que ordena las escrituras de workspace, settings y library fuera del hilo GPUI.
- `src/ui/diff_view/changes_panel.rs`: operaciones y controles de Changes.

## Contratos que conviene conservar

La interfaz muestra una fila de tabs por proyecto. Las aperturas interactivas y las
automatizaciones usan `open_tab_in_project`. El contenedor serializado
`TerminalWorkspaceSnapshot` se conserva para leer instalaciones anteriores; al
cargar la ventana, sus sesiones antiguas se consolidan sin perder tabs ni panes.
Los campos Swift y los grupos antiguos solo sirven para migrar datos. No deben
volver a usarse para construir la sidebar.
Los constructores de varias sesiones y sus operaciones antiguas están aislados en
`legacy_fixtures.rs`, compilado solo en tests. Ni el primer arranque puede crear
contenedores ocultos: también usa `open_tab_in_project`.

La selección de un pane desactiva el zoom de otro pane; el teclado nunca debe apuntar
a una terminal oculta. Los nombres manuales y de automatizaciones pertenecen al pane,
no al proceso del agente. Solo se descartan al cerrar ese pane. Cambiar su `cwd` no
renombra los contenedores antiguos. Los recorridos de sesiones devuelven referencias,
sin clonar el workspace en cada render de la barra de estado o el Inbox.

La raíz del proyecto define Explorer, Changes y las terminales nuevas. El `cwd`
de una terminal puede cambiar sin alterar esa raíz. Las notas y automatizaciones
con un proyecto explícito no deben usar otro proyecto si el destino falta.
Al quitar un proyecto se conservan sus notas y comandos, y sus automatizaciones
quedan pausadas.

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

Al cerrar, la cola recibe el último estado de los tres documentos y espera a que
llegue al disco. No lanzar guardados independientes de library ni competir con la
cola durante el cierre. Los errores de carga, guardado y acciones de notas están
separados; una acción no puede borrar el bloqueo de un JSON que no se pudo cargar.

Crear archivos pertenece a `FileSystemPort`, no a la UI. El adaptador local valida
la raíz y los padres existentes, incluidos enlaces simbólicos, antes de crear
rutas anidadas, y no reemplaza archivos existentes.

## Validación

`./Scripts/verify.sh` ejecuta formato, pruebas Rust, Clippy, pruebas de release y
validación de plist/scripts. Las pruebas GPUI comprueban estado, eventos y destinos
de escritura. La revisión visual del diseño se realiza manualmente.
