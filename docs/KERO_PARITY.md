# Paridad funcional con Kero

Esta matriz parte de una revisión del código de Kero 0.1.45, no solo de su README.
Su objetivo es escoger conceptos que mejoran Vibra sin copiar código GPL ni
forzar dependencias contrarias a GPUI.

## Implementado

| Área de Kero | Estado en Vibra |
| --- | --- |
| Proyectos, workspaces y tabs | Modelo persistente con selección y migración de snapshots. |
| Split panes recursivos | Cuatro direcciones, foco geométrico, ciclo, resize, drag, equalize, zoom y collapse al cerrar. |
| Terminal avanzada | PTY/Alacritty, truecolor, teclado Kitty/xterm, IME, mouse reporting, selección, búsqueda, scrollback, OSC 8 y OSC 52. |
| Sidebar de sesiones | Sesiones vivas, cwd/título y presencia/estado de agentes. |
| Explorador de archivos | Árbol, ocultos, crear, renombrar y Papelera, siempre confinado al proyecto. |
| Editor de texto | UTF-8, números de línea, cursor, búsqueda, undo/redo, guardado atómico y control de conflictos externos. |
| Git enfocado | Status y diff por archivo en un panel de solo lectura, junto al explorador de archivos. |
| Command palette | Comandos, selección de workspace y quick open indexado. |
| Automatización por pane | CLI propia, socket Unix con capacidad, send/run/split/focus/close/zoom y hooks de agente. |
| Preferencias | Fuente y visibilidad de archivos/sidebars persistentes. |
| Distribución macOS | Bundle, entitlements, firma ad-hoc/Developer ID, zip, notarización y staple opcionales. |

## Implementado con una forma distinta

| Función | Decisión de Vibra |
| --- | --- |
| CWD por OSC 7 | Se consulta el directorio del proceso foreground; funciona también con shells que no emiten OSC 7. |
| Diff como contenido de pane | Vive junto a Files en el sidebar derecho para mantener el terminal visible sin sumar controles Git secundarios. |
| Estado de agente | Combina detección de título/pantalla con hooks explícitos, sin acoplarse a un proveedor. |
| Clipboard OSC 52 | Las escrituras se aceptan; las lecturas requieren consentimiento explícito con preview para evitar exfiltración. |
| Borrado de archivos | Siempre usa Papelera recuperable mediante Finder; no existe delete permanente en la UI. |
| Pull | Siempre `--ff-only`; no crea merges implícitos desde la aplicación. |

## Parcial, candidato a evolución

| Área | Cobertura actual | Próximo corte razonable |
| --- | --- | --- |
| Tipos de pane | El árbol recursivo contiene terminales; el editor ocupa el centro y Git el sidebar. | Generalizar `PaneContent` cuando haya al menos dos contenidos que necesiten split. |
| Editor | Texto UTF-8, navegación, búsqueda y edición segura. | Tree-sitter, selección de rangos, múltiples buffers y syntax highlighting. |
| Integración shell | CWD, títulos, bell, enlaces y automatización. | OSC 133/777, bloques semánticos de comandos y progreso. |
| Git | Inspección de status y diff; las mutaciones se realizan en la terminal. | Añadir acciones solo cuando el flujo justifique su coste visual. |
| Apariencia | Tema oscuro coherente y fuente configurable. | Temas serializables, light mode y ajustes de contraste. |
| Notificaciones | Bell visual y estado de agente en sidebar. | Centro de notificaciones con opt-in y throttling. |

## Diferido deliberadamente

- **Browser embebido:** GPUI 0.2.2 no ofrece una API pública estable para alojar
  un `NSView` arbitrario. Integrar `WKWebView` con un fork o una segunda event loop
  comprometería foco, IME, accesibilidad y lifecycle. Véase ADR 0001.
- **Kitty graphics e imágenes:** exigirían un pipeline de texturas, límites de
  memoria y cache invalidation que merece un componente independiente.
- **Multi-window:** el modelo ya separa proyectos/workspaces, pero el lifecycle
  actual es de una ventana; se priorizó la profundidad del flujo en una ventana.
- **Actualizador automático:** la firma/notarización y los prereleases están
  listos; el feed estable de Sparkle permanece en la última versión Swift hasta
  que el runtime GPUI incorpore un actualizador compatible.
- **Localización completa:** la UI está en español y los identificadores internos
  en inglés; se extraerán catálogos cuando exista un segundo idioma objetivo.
- **Finder extension/servicio:** abrir una ruta explícita y la integración con
  Papelera cubren el caso actual sin sumar un target nativo adicional.

## Criterio para futuras incorporaciones

Una función de Kero entra en Vibra cuando mejora el flujo terminal/agente,
puede aislarse detrás de un puerto o acción de dominio, tiene una frontera de
seguridad verificable y no exige sostener un fork frágil de GPUI. Esta regla evita
una copia superficial de features y mantiene la aplicación comprobable.
