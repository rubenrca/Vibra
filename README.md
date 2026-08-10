# Vibra

Workspace de desarrollo nativo para macOS, escrito en Rust con GPUI. Combina
terminales persistentes, proyectos, archivos, edición de texto, diff de Git y
automatización local para agentes en una sola ventana enfocada.

La rama GPUI reemplaza la implementación SwiftUI/AppKit y `libghostty` que llegó
hasta Vibra 0.2.7. El historial, identidad de aplicación y canal de distribución
continúan en este mismo repositorio.

## Estado de la migración

La primera versión GPUI es `0.3.0-beta.1`. Se distribuye únicamente como
prerelease mientras se valida la migración con usuarios existentes.

El feed estable de Sparkle permanece en Vibra 0.2.7. La versión GPUI todavía no
publica en ese feed porque aún no contiene un actualizador compatible; así se
evita que una actualización estable deje a los usuarios sin futuras updates.

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
- estado idle/working/waiting inferido desde terminal o declarado por hook;
- socket Unix local protegido por capacidades UUID;
- comandos `+pane` y `+agent` disponibles mediante `$VIBRA_CLI`.

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

## Releases GPUI

Los releases GPUI son prereleases de GitHub y requieren un árbol limpio. La
versión debe coincidir con `Cargo.toml` y tener una sección en `CHANGELOG.md`:

```bash
./Scripts/release.sh 0.3.0-beta.1 --dry-run
./Scripts/release.sh 0.3.0-beta.1 --notarize
```

El script crea un DMG universal y publica un GitHub prerelease. No modifica
`docs/appcast.xml`. Los releases estables están bloqueados hasta integrar un
actualizador en la aplicación GPUI.

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
