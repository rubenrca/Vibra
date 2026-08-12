# Vibra

Workspace de desarrollo nativo para macOS, escrito en Rust con GPUI. Combina
terminales persistentes, proyectos, archivos, edición de texto, diff de Git y
automatización local para agentes en una sola ventana enfocada.

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
- foco geométrico, resize por teclado o arrastrando, reparto equitativo y zoom;
- command palette (`⇧⌘P`) y apertura rápida de archivos (`⌘P`).

### Terminal

- PTY nativo y emulación ANSI con `alacritty_terminal`;
- render GPUI/Metal, truecolor, estilos, cursores, scrollback e IME;
- teclado xterm y Kitty, mouse SGR, bracketed paste y alternate screen;
- selección, búsqueda, enlaces OSC 8 y clipboard OSC 52 protegido;
- JetBrains Mono Variable incluida en la aplicación.

### Files y Diff

- panel derecho unificado con las vistas `Files` y `Diff`;
- árbol de archivos confinado al proyecto, con operaciones recuperables;
- editor UTF-8 con búsqueda, undo/redo, guardado atómico y `⌘S`;
- estado Git y diff virtualizado por archivo en una interfaz de solo lectura;
- las mutaciones Git se realizan desde la terminal integrada.

### Agentes y automatización

- detección de Codex, Claude, Gemini, Grok, OpenCode, Cursor, Aider, Amp y Pi;
- identidad resuelta por proceso foreground, título y texto visible; estado por hooks o heurística de terminal;
- socket Unix local protegido por capacidades UUID;
- comandos `+pane` y `+agent` disponibles mediante `$VIBRA_CLI`;
- un agente (o script) dentro de un pane puede abrir otro agente en split o tab:

```bash
# One-shot: crea layout y arranca el kind indicado (default: split right, sin robar foco)
"$VIBRA_CLI" +agent open codex --split right --no-focus --name reviewer
"$VIBRA_CLI" +agent open claude --tab --no-focus --cwd "$PWD"
"$VIBRA_CLI" +agent open codex --name builder -- -m o3

# Prompt / wait / read por nombre o pane id
"$VIBRA_CLI" +agent prompt reviewer "Review the current diff" --wait --timeout 120000
"$VIBRA_CLI" +agent wait reviewer --until waiting
"$VIBRA_CLI" +agent read reviewer --lines 120
"$VIBRA_CLI" +agent list
"$VIBRA_CLI" +agent status reviewer

# Primitivas de layout
"$VIBRA_CLI" +pane split right --no-focus --cwd "$PWD"
"$VIBRA_CLI" +pane tab --no-focus
"$VIBRA_CLI" +pane run --pane <id> "npm test"
"$VIBRA_CLI" +agent start --kind codex --pane <id> --name reviewer
"$VIBRA_CLI" +agent kinds
"$VIBRA_CLI" +skill
```

Para instalar los adaptadores de estado de Claude y Codex, usa **Settings →
Integraciones de agentes** o la CLI:

```bash
"$VIBRA_CLI" agent setup
"$VIBRA_CLI" agent status
"$VIBRA_CLI" agent uninstall codex
```

El instalador añade únicamente los handlers de Vibra a `~/.claude/settings.json`
y `~/.codex/hooks.json`, guarda scripts en `~/.vibra/agent-hooks/` y conserva los
hooks existentes. Los scripts no hacen nada fuera de un pane de Vibra. Después de
instalar el hook de Codex, ábrelo una vez y apruébalo en `/hooks`.

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
`VIBRA_SIGNING_IDENTITY` o `--sign`.

## Releases

Requiere un árbol limpio, `Cargo.toml` y una sección en `CHANGELOG.md` con la
misma versión, más las herramientas Sparkle en `.build/artifacts/sparkle`
(clave EdDSA en el llavero):

```bash
./Scripts/release.sh 0.3.0 --dry-run
./Scripts/release.sh 0.3.0 --notarize
./Scripts/release.sh 0.3.1-beta.1 --prerelease
```

Un release **estable** crea el DMG universal, firma el appcast, publica en
GitHub como Latest y actualiza `docs/appcast.xml`. Un **prerelease** no toca el
feed de Sparkle.

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
| `⌘N` / `⌘T` / `⌘W` | Nuevo workspace / nuevo tab / cerrar editor o terminal |
| `⌘D` / `⇧⌘D` | Dividir a la derecha / abajo |
| `⌃⌥⌘` + flechas | Dividir en cualquier dirección |
| `⌥⌘` + flechas | Enfocar pane vecino |
| `⌘[` / `⌘]` | Pane anterior / siguiente |
| `⌃⌥` + flechas | Cambiar proporción del pane |
| `⌃⌥E` / `⇧⌘↵` | Igualar panes / alternar zoom |
| `⇧⌘P` / `⌘P` | Paleta de comandos / quick open |
| `⌘B` | Mostrar u ocultar sidebar de sesiones |
| `⌥⌘B` | Mostrar u ocultar panel Files y Diff |
| `⌘U` | Buscar actualizaciones (Sparkle) |
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
