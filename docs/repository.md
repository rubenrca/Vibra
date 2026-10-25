# Repository

La sección Repository reúne el historial de commits del proyecto y sus pull requests e issues de GitHub. En el código conserva sus nombres internos (`WorkspaceSection::Inbox`, `work_inbox`).

## Comportamiento

| Área | Vibra |
| --- | --- |
| Estructura | La sección se llama Repository. Una barra con las pestañas History / Pull requests / Issues; en las dos últimas, lista inicial de 280 px (240–420) con búsqueda, filtros y refresco arriba, y detalle con cabecera fija y contenido desplazable. History ocupa todo el ancho: grafo de todas las ramas o solo de la actual, con el PR de cada commit y su diff debajo. |
| Lista | Dos líneas: estado, título y fecha relativa; número, lo que espera el PR (checks, cambios pedidos, conflictos, review, listo para merge) o las etiquetas del issue, tamaño, checkout local y sesión de agente. Grupos por relación con el usuario, primera tarea visible seleccionada y filtrado inmediato. |
| Filtros | Al entrar desde un proyecto, GitHub muestra solo sus tareas; All projects amplía la selección. Open y Draft son los estados predeterminados, incluidos los ajustes antiguos sin selección explícita. Closed/Merged se activan manualmente, y All statuses incluye todos. Asignación, hoy/7 días/30 días y proyectos locales; Reset filters vuelve al proyecto activo y estados predeterminados. “Hoy” usa medianoche local. |
| GitHub | Todos los remotos de los proyectos, incluidos forks/upstream. Asignación dentro de esos repositorios; cuota independiente de 100 issues y 100 PRs por repositorio, consultas con concurrencia acotada y errores parciales visibles. |
| Detalle | Título con las acciones a la derecha (Send to agent, Ask, GitHub y Merge); una fila con lo que espera, número, autor, base ← rama (menú de acciones locales), fecha y sesiones. Pestañas subrayadas Conversation/Files/Commits/Checks; Conversation usa Markdown con ancho de lectura. |
| Conversación | Comentarios de issues y PRs, reviews e hilos de revisión con contexto y respuestas. Borrador separado por tarea, publicación explícita y conservación tras errores. |
| PR | Conversation/Files/Commits/Checks; Files ocupa todo el ancho y reutiliza el componente de diff de Workspace: resaltado sintáctico, Unified/Split, Wrap, cabeceras fijas, líneas virtualizadas y comentarios de revisión. Comparte preferencias de vista y tamaño de letra. Archivos inicialmente plegados; selector Hunks/Full file y controles para expandir/colapsar. Los comentarios preparan un borrador para el agente. Checks tiene resumen, filtros Needs attention/All checks y grupos con estado, workflow y duración cuando están disponibles; links y logs de Actions bajo demanda. Fix prepara contexto con check, rama y head. |
| Acciones | Merge/squash/rebase/draft/ready/close/reopen, con confirmación del PR concreto. El merge conserva el head mostrado al preparar la confirmación. |
| Agentes | Enviar al agente prepara contexto y proyecto antes de ejecutar. Cada envío crea una sesión enlazada; Ask reutiliza una conversación por tarea en un panel lateral de 440 px, con superposición en ventanas estrechas. |
| Persistencia | Fuente, filtros, ancho, marcas de lectura y vínculos a sesiones. La última lista completa se guarda en la caché local; los detalles y borradores de comentarios se mantienen en memoria. |

## Carga inicial

La lista de GitHub empieza a cargarse en segundo plano al abrir Vibra. La precarga no selecciona tareas, no las marca como leídas ni consulta comentarios/checks hasta abrir la sección.

En aperturas posteriores se muestra la última lista completa mientras se consulta al proveedor. La caché está separada por proveedor y ligada a las credenciales locales, los proyectos y los filtros de consulta; no guarda tokens. Cada proveedor conserva como máximo un archivo de 16 MiB, con permisos privados, y las copias de más de siete días se descartan. Una caché inválida no impide consultar la red, y un error de red conserva la lista visible. La primera carga sin caché aún depende del proveedor.

GitHub omite la consulta previa de la cuenta cuando no está activo el filtro de asignación. Las cuotas de 100 issues y 100 PRs por repositorio y la concurrencia acotada se mantienen.

## Límites

El borrador de envío se prepara dentro de la sección y se convierte en una nueva terminal al pulsar Send. Ask aloja esa misma terminal nativa. Su instrucción de consulta remota guía al agente; no constituye un sandbox adicional de herramientas.

Solo se conecta GitHub.

Files muestra los patches que devuelve GitHub (hasta 500 archivos), con aviso para archivos binarios o patches omitidos. Al desplegar un archivo, carga su blob inmutable (UTF-8 de hasta 2 MiB) para preparar el resaltado y el contexto con el mismo modelo de Workspace. Reconstruye y valida el lado anterior a partir del patch; si falta contenido, conserva los hunks disponibles. La carga se encola de cuatro en cuatro y el visor mantiene hasta 16 archivos desplegados, como Workspace. Full file revela todo el contexto sin perder las líneas eliminadas. La fuente remota queda separada del repositorio local y descarta respuestas de versiones anteriores. Los modos de código, plegados y filtros de checks se mantienen por tarea durante la sesión. Checks agrupa los resultados y permite consultar logs de Actions y preparar reparaciones, sin seguimiento automático de reparaciones. Ask no incluye reinicio ni divisor propio. El Markdown es nativo: muestra imágenes como enlaces y no reproduce el render HTML, tablas avanzadas ni resaltado sintáctico del navegador.

## Comprobaciones

Las pruebas cubren filtros combinados, compatibilidad de preferencias, ámbito de consultas GitHub, parsing de threads, límites y destinos remotos, respuestas obsoletas, conservación de borradores, confirmación ligada al head original, preparación sin ejecución y visibilidad de la terminal de Ask. Las consultas de lista, detalle, revisiones, diff y checks se probaron en lectura contra un repositorio público, sin publicar comentarios ni modificar PRs.

La revisión visual manual queda a cargo del usuario, conforme a `AGENTS.md`; no se automatizó un navegador ni se tomaron capturas para validar el diseño.
