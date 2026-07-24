# Vibra

A fast, native macOS workspace for agentic coding.

Vibra keeps terminal sessions, projects, and coding agents in one focused
workspace. It is an independent implementation powered by `libghostty`.

## Status

Version `0.1.0` established the application and terminal foundation. Current
development builds provide:

- native SwiftUI/AppKit application shell;
- project workspaces and long-lived terminal tabs;
- `libghostty` terminal rendering;
- explicit visible/background session lifecycle;
- workspace restoration;
- keyboard commands for projects and terminal tabs.
- an event-driven Git sidebar with a native diff renderer;
- stage, unstage, stage-all, and unstage-all actions;
- bounded Git output so unusually large repositories cannot grow memory
  without limit.

Split panes, file navigation, and release packaging are next.

## Requirements

- macOS 14 or newer
- Xcode 26 or newer
- Swift 6.2 or newer

## Run

```bash
swift run Vibra
```

Open a specific project directly with:

```bash
swift run Vibra -- /path/to/project
```

Create an ad-hoc signed application bundle with:

```bash
./Scripts/package_app.sh debug
open dist/Vibra.app
```

Run the model tests with:

```bash
swift test
```

The first build downloads the prebuilt GhosttyKit binary used by
`libghostty-spm`.

## Shortcuts

- `⌘T`: new terminal
- `⌘W`: close the selected terminal
- `⌘O`: open a project
- `⇧⌘G`: toggle the Git sidebar

## Principles

- Hidden sessions keep their shell alive without rendering frames.
- Polling is avoided unless an underlying API has no event-driven mechanism.
- Every long-lived runtime object has an explicit shutdown path.
- Performance regressions are treated as correctness bugs.

## License and acknowledgements

Vibra is MIT licensed. See [NOTICE.md](NOTICE.md) for acknowledgements.
