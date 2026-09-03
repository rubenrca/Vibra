# Vibra

Workspace de desarrollo nativo para macOS, escrito en Rust con GPUI. Combina
terminales persistentes, proyectos, archivos, edición de texto, diff de Git e
integración local para agentes en una sola ventana enfocada.

La rama GPUI reemplaza la implementación SwiftUI/AppKit y `libghostty` que llegó
hasta Vibra 0.2.7. El historial, identidad de aplicación y canal de distribución
continúan en este mismo repositorio.

## Estado de la migración

A partir de **Vibra 0.3.0**, el runtime oficial es GPUI (Rust + Metal). La
última build Swift es 0.2.7.

La app empaqueta de nuevo **Sparkle** con el mismo feed EdDSA, de modo que
instalaciones previas en 0.2.7 pueden actualizarse al canal estable:

```text
https://rubenrca.github.io/Vibra/appcast.xml
```

La identidad `app.vibra.Vibra` y la migración de `workspace.json` se mantienen.

## Funciones principales

### Workspaces, tabs y panes

- múltiples proyectos, workspaces y tabs persistentes;
- terminales divididas recursivamente en cuatro direcciones;
- reordenar tabs, panes y sesiones de la sidebar arrastrándolos; saltar a un tab con `⌘1`–`⌘8` y al último con `⌘9`;
- foco geométrico, resize por teclado o arrastrando, reparto equitativo y zoom;
- sidebar de sesiones con CWD, rama Git en vivo (ahead/behind/dirty), agente activo, estado y modelo cuando el CLI lo reporta;
- menús contextuales en sesiones y panes (renombrar, cerrar, dividir, zoom);
- command palette (`⇧⌘P`), apertura rápida de archivos (`⌘P`) y Settings modal (`⌘,`).

### Terminal

- PTY nativo y emulación ANSI con `alacritty_terminal`;
- render GPUI/Metal, truecolor, estilos, cursores, scrollback e IME;
- teclado xterm y Kitty, mouse SGR, bracketed paste y alternate screen;
- pegado estilo Warp (`⌘V`): texto con bracketed paste; con imagen en el clipboard y un agente CLI en foco, se envía Ctrl+V para adjuntar capturas;
- selección, búsqueda, enlaces OSC 8 y clipboard OSC 52 protegido;
- JetBrains Mono Variable incluida en la aplicación;
- consola inferior persistente (`⌘J`) para servidores de desarrollo: al ocultarla el proceso sigue vivo y no cambia el pane seleccionado; cada sesión de la sidebar tiene las suyas y `+` abre terminales extra.

### Files, Git y Servers

- panel derecho unificado con las vistas `Files`, `Git` y `Servers`;
- árbol de archivos confinado al proyecto, con iconos SVG de carpetas/archivos y guías de indentación;
- árbol de archivos que conserva el terminal como superficie central y enfoca los archivos modificados directamente en Git;
- panel Git con tres vistas: working tree, cambios de la rama frente a la base por defecto, e historial de commits con grafo de lanes;
- diffs de solo lectura estilo Warp: tarjetas expandibles, gutter de línea y resaltado de sintaxis (Rust, JS/TS, Python, Swift, Go, shell y configs comunes);
- las mutaciones Git se realizan desde la terminal integrada;
- pestaña `Servers` con procesos en escucha TCP de los PTY (vite, next, etc.), salto al pane, abrir URL y detener.

### Agentes y seguimiento

- detección de Codex, Claude, Gemini, Goose, Grok, OpenCode, Cursor, Aider, Amp y Pi;
- estados de actividad en vivo para los agentes que se ejecutan dentro de una terminal de Vibra;
- avisos de sistema cuando un agente termina o pide permiso fuera de la sesión visible;
- identidad resuelta por proceso foreground, sesión, título y texto reciente;
- nombres personalizados para panes desde su menú contextual;
- hooks estructurados de Claude y Codex para estados de trabajo, espera, permisos y fin de sesión;
- socket Unix local protegido por capacidades UUID;

La CLI de Vibra no orquesta agentes ni layouts desde una terminal: no crea panes o
tabs, no lanza agentes en otras sesiones y no envía prompts a otros procesos. Los
agentes se ejecutan manualmente en la terminal y Vibra conserva su detección,
estado y notificaciones.

Para activar el seguimiento preciso de actividad, usa **Settings → Seguimiento
de agentes → Activar seguimiento** o la CLI de instalación:

```bash
Vibra agent setup
Vibra agent status
Vibra agent uninstall codex
```

El instalador añade únicamente los handlers de Vibra a `~/.claude/settings.json`
y `~/.codex/hooks.json`, guarda scripts en `~/.vibra/agent-hooks/` y conserva los
hooks existentes. Los scripts no hacen nada fuera de un pane de Vibra y los eventos
se procesan en orden para evitar estados atrasados. Después de instalar el hook de
Codex, ábrelo una vez y apruébalo en `/hooks`.

Claude y Codex tienen seguimiento estructurado mediante esos hooks. En Gemini,
Goose, Grok, OpenCode, Cursor, Aider, Amp y Pi, Vibra sólo infiere la actividad
desde el proceso y el texto visible.

## Requisitos

- macOS 14 o posterior;
- Xcode;
- Rust 1.96.

## Ejecutar durante desarrollo

```bash
cargo run
cargo run -- /ruta/al/proyecto
```

Cada cambio requiere cerrar la aplicación y ejecutar nuevamente `cargo run`.

Verificar formato, tests, Clippy, plist y scripts:

```bash
./Scripts/verify.sh
```

## Crear Vibra.app

Bundle de desarrollo firmado ad-hoc:

```bash
./Scripts/package_app.sh debug --sign -
open dist/Vibra.app
```

Bundle universal, DMG, firma Developer ID y notarización:

```bash
./Scripts/package_app.sh release --universal --dmg --notarize
```

La notarización usa `APPLE_KEYCHAIN_PROFILE`, o `APPLE_ID`, `APPLE_TEAM_ID` y
`APPLE_APP_SPECIFIC_PASSWORD`. La identidad puede definirse con
`VIBRA_SIGNING_IDENTITY` o `--sign`. El release normal notariza solo el DMG,
que es el archivo distribuido. La espera está limitada a dos horas por defecto;
si Apple demora más, conserva la solicitud y el ID queda en
`dist/notarization/Vibra.dmg.submission-id` para consultarlo después.

## Releases

Requiere un árbol limpio, `Cargo.toml` y una sección en `CHANGELOG.md` con la
misma versión, más las herramientas Sparkle en `.build/artifacts/sparkle`
(clave EdDSA en el llavero):

```bash
./Scripts/release.sh 0.3.6 --dry-run
./Scripts/release.sh 0.3.6
./Scripts/release.sh 0.3.6-beta.1 --prerelease
./Scripts/release.sh 0.3.6 --no-notarize   # solo si hace falta omitir notarización
./Scripts/release.sh 0.3.6 --resume-dmg    # publica un DMG ya notarizado tras una espera interrumpida
```

Un release **estable** crea el DMG universal, firma con Developer ID, notariza
por defecto (opt-out con `--no-notarize`), firma el appcast, publica en GitHub
como Latest y actualiza `docs/appcast.xml`. Un **prerelease** no toca el feed
de Sparkle.

## Migración de datos

Vibra GPUI conserva la identidad `app.vibra.Vibra` y usa:

```text
~/Library/Application Support/Vibra/workspace.json
```

Antes de escribir un workspace creado por Swift, guarda una copia única en:

```text
~/Library/Application Support/Vibra/workspace.swift-v0.2.7.backup.json
```

Si no existe un workspace de Vibra, importa automáticamente el creado durante
el preview independiente de VibraGPUI. Las preferencias del preview también se
importan una sola vez.

## Atajos principales

| Atajo | Acción |
| --- | --- |
| `⌘N` / `⌘T` / `⌘W` | Nuevo workspace / nuevo tab / cerrar terminal |
| `⌘1`–`⌘8` / `⌘9` | Ir al tab 1–8 / ir al último tab |
| `⌘D` / `⇧⌘D` | Dividir a la derecha / abajo |
| `⌃⌥⌘` + flechas | Dividir en cualquier dirección |
| `⌥⌘` + flechas | Enfocar pane vecino |
| `⌘[` / `⌘]` | Pane anterior / siguiente |
| `⌃⌥` + flechas | Cambiar proporción del pane |
| `⌃⌥E` / `⇧⌘↵` | Igualar panes / alternar zoom |
| `⇧⌘P` / `⌘P` | Paleta de comandos / quick open |
| `⇧⌘E` | Abrir la carpeta activa en un IDE externo |
| `⌘,` | Abrir Settings (modal centrado) |
| `⌘B` | Mostrar u ocultar sidebar de sesiones |
| `⌥⌘B` | Mostrar u ocultar panel Files, Git y Servers |
| `⌘J` | Mostrar u ocultar la terminal inferior de la sesión actual |
| `⌘U` | Buscar actualizaciones (Sparkle) |
| `⌘V` | Pegar (bracketed paste; Ctrl+V con imagen en agentes CLI) |
| `⌘F`, `⌘G`, `⇧⌘G` | Buscar / siguiente / anterior en terminal |
| `⌘=`, `⌘-`, `⌘0` | Ajustar o restablecer fuente |
| `⌘K` | Limpiar pantalla y scrollback |

## Arquitectura

```text
GPUI views
   │
WorkspaceSnapshot + acciones de dominio
   ├── WorkspaceRepository ── JSON versionado y migración Swift
   ├── SettingsRepository  ── preferencias persistentes
   ├── TerminalPort        ── AlacrittyTerminal (PTY + parser)
   ├── FileSystemPort      ── filesystem local confinado
   ├── GitPort             ── status y diff mediante git CLI
   └── AutomationServer    ── socket Unix + capacidades por pane
```

## Licencia y reconocimientos

Vibra usa licencia MIT. Consulta [NOTICE.md](NOTICE.md) para dependencias y
reconocimientos.
