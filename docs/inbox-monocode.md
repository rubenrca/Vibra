# Inbox: referencia de MonoCode y adaptación a Vibra

Referencia inspeccionada: [hardbeat920/monocode, commit c576783ac14a50a0998222146fd9838d85d52ffb](https://github.com/hardbeat920/monocode/tree/c576783ac14a50a0998222146fd9838d85d52ffb), 27 de septiembre de 2026. Se revisaron componentes, modelos, handlers de la aplicación, backend Rust y pruebas; no solo capturas de pantalla. La atribución de los iconos adaptados y la licencia MIT están en `third_party/monocode`.

## Fuentes revisadas

- [`InboxView.tsx`](https://github.com/hardbeat920/monocode/blob/c576783ac14a50a0998222146fd9838d85d52ffb/src/features/inbox/ui/InboxView.tsx): columnas, lista, cabecera fija, pestañas de PR, sesiones relacionadas y acciones.
- [`InboxFiltersMenu.tsx`](https://github.com/hardbeat920/monocode/blob/c576783ac14a50a0998222146fd9838d85d52ffb/src/features/inbox/ui/InboxFiltersMenu.tsx) y `model/inboxFilters.ts`: filtros de selección múltiple y valores iniciales (todos los estados; asignación desactivada).
- [`InboxDiscussionPanel.tsx`](https://github.com/hardbeat920/monocode/blob/c576783ac14a50a0998222146fd9838d85d52ffb/src/features/inbox/ui/InboxDiscussionPanel.tsx), `model/inboxAsk.ts` y `src/instructions/inbox.md`: Ask aloja una sesión normal y establece un contexto de consulta remota.
- `InboxComments.tsx`, `InboxPrDiff.tsx`, `InboxPrChecks.tsx`: conversación y respuestas, revisión de cambios, resultados de CI y preparación de reparaciones.
- [`App.tsx`](https://github.com/hardbeat920/monocode/blob/c576783ac14a50a0998222146fd9838d85d52ffb/src/app/App.tsx), handler `onStartInboxItem`: crea una sesión con tarjeta de contexto pendiente de envío.
- `model/githubTasks.ts`, `src-tauri/src/fs.rs`, `src-tauri/src/linear.rs` y las pruebas de Inbox: remotos de proyectos, consultas, comentarios y mutaciones.

## Comportamiento trasladado

| Área | Vibra |
| --- | --- |
| Estructura | Cabecera de 40 px; lista inicial de 280 px (240–420), pestañas de conexiones y segunda fila de búsqueda/acciones; detalle con cabecera fija y contenido desplazable. |
| Lista | Identificador, tipo y estado, fecha relativa, marca de lectura, título, repositorio/proyecto y etiquetas. Primera tarea visible seleccionada y filtrado inmediato. |
| Filtros | Al entrar desde un proyecto, GitHub muestra solo sus tareas; All projects amplía la selección. Open y Draft son los estados predeterminados, incluidos los ajustes antiguos sin selección explícita. Closed/Merged se activan manualmente, y All statuses incluye todos. Asignación, hoy/7 días/30 días, exclusión de tipos, proyectos locales y grupos de Linear; Reset filters vuelve al proyecto activo y estados predeterminados. “Hoy” usa medianoche local. |
| GitHub | Todos los remotos de los proyectos, incluidos forks/upstream. Asignación dentro de esos repositorios; cuota independiente de 100 issues y 100 PRs por repositorio, consultas con concurrencia acotada y errores parciales visibles. |
| Detalle | Cabecera con tipo, número y estado, título destacado, autor/responsables, fechas y ramas en una fila adaptable. Acciones Ask/Review on GitHub con iconos; pestañas subrayadas y estado de checks. Summary usa Markdown con tipografía y espaciado de lectura. |
| Conversación | Comentarios de issues y PRs, reviews e hilos de revisión con contexto y respuestas; comentarios y respuestas de Linear. Borrador separado por tarea, publicación explícita y conservación tras errores. |
| PR | Summary/Code/Checks; Code reutiliza el componente de diff de Workspace: resaltado sintáctico, Unified/Split, Wrap, cabeceras fijas, líneas virtualizadas y comentarios de revisión. Comparte preferencias de vista y tamaño de letra. Archivos inicialmente plegados; selector Hunks/Full file y controles para expandir/colapsar. Los comentarios preparan un borrador en Inbox. Checks tiene resumen, filtros Needs attention/All checks y grupos con estado, workflow y duración cuando están disponibles; links y logs de Actions bajo demanda. Fix prepara contexto con check, rama y head. |
| Acciones | Merge/squash/rebase/draft/ready/close/reopen, con confirmación del PR concreto. El merge conserva el head mostrado al preparar la confirmación. |
| Agentes | Enviar al agente prepara contexto y proyecto antes de ejecutar. Cada envío crea una sesión enlazada; Ask reutiliza una conversación por tarea en un panel lateral de 440 px, con superposición en ventanas estrechas. |
| Persistencia | Fuente, filtros, ancho, marcas de lectura y vínculos a sesiones. La última lista completa se guarda en la caché local; los detalles y borradores de comentarios se mantienen en memoria. |

## Carga inicial

Las listas de GitHub y Linear empiezan a cargarse en segundo plano al abrir Vibra. La precarga no selecciona tareas, no las marca como leídas ni consulta comentarios/checks hasta abrir Inbox.

En aperturas posteriores se muestra la última lista completa mientras se consulta al proveedor. La caché está separada por proveedor y ligada a las credenciales locales, los proyectos y los filtros de consulta; no guarda tokens. Cada proveedor conserva como máximo un archivo de 16 MiB, con permisos privados, y las copias de más de siete días se descartan. Una caché inválida no impide consultar la red, y un error de red conserva la lista visible. La primera carga sin caché aún depende del proveedor.

GitHub omite la consulta previa de la cuenta cuando no está activo el filtro de asignación. Las cuotas de 100 issues y 100 PRs por repositorio y la concurrencia acotada se mantienen.

## Diferencias de esta adaptación

Vibra usa GPUI y terminales CLI; MonoCode usa React/Tauri y su propia interfaz de chat. El borrador de envío se prepara dentro de Inbox y se convierte en una nueva terminal al pulsar Enviar. Ask aloja esa misma terminal nativa. Su instrucción de consulta remota guía al agente; no constituye un sandbox adicional de herramientas.

Esta versión conecta GitHub y Linear. No incorpora los proveedores Jira, GitLab y Azure DevOps presentes en la revisión de MonoCode. Los grupos de Linear se filtran por equipo/proyecto entre las tareas cargadas; no hay un catálogo remoto completo de proyectos.

Code muestra los patches que devuelve GitHub (hasta 500 archivos), con aviso para archivos binarios o patches omitidos. Al desplegar un archivo, carga su blob inmutable (UTF-8 de hasta 2 MiB) para preparar el resaltado y el contexto con el mismo modelo de Workspace. Reconstruye y valida el lado anterior a partir del patch; si falta contenido, conserva los hunks disponibles. La carga se encola de cuatro en cuatro y el visor mantiene hasta 16 archivos desplegados, como Workspace. Full file revela todo el contexto sin perder las líneas eliminadas. La fuente remota queda separada del repositorio local y descarta respuestas de versiones anteriores. Los modos de código, plegados y filtros de checks se mantienen por tarea durante la sesión. Checks agrupa los resultados y permite consultar logs de Actions y preparar reparaciones, sin seguimiento automático de reparaciones. Ask no incluye reinicio ni divisor propio. El Markdown es nativo: muestra imágenes como enlaces y no reproduce el render HTML, tablas avanzadas ni resaltado sintáctico del navegador.

## Comprobaciones

Las pruebas cubren filtros combinados, compatibilidad de preferencias, ámbito de consultas GitHub, parsing de threads, límites y destinos remotos, respuestas obsoletas, conservación de borradores, confirmación ligada al head original, preparación sin ejecución y visibilidad de la terminal de Ask. Las consultas de lista, detalle, revisiones, diff y checks se probaron en lectura contra el repositorio público de MonoCode. No se publicaron comentarios ni se modificaron PRs durante la validación.

La revisión visual manual queda a cargo del usuario, conforme a `AGENTS.md`; no se automatizó un navegador ni se tomaron capturas para validar el diseño.
