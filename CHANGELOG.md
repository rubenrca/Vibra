# Changelog

## 0.3.14 — 2026-09-03

- Organize sessions into persistent, collapsible sidebar spaces that can be created from the empty sidebar, renamed, removed without deleting their sessions, and reordered with precise drag-and-drop insertion.
- Open the active workspace folder in an installed external IDE directly from Vibra.
- Persist window dimensions and let background refresh intervals be configured from Settings.
- Cache prepared Git diff documents so unchanged diffs avoid repeated parsing, highlighting, and layout work.

## 0.3.13 — 2026-09-02

- Render Git diffs more like an editor: JetBrains Mono at 12px, a dedicated gutter, a color bar for added/removed lines, full-brightness syntax, and tab expansion.
- Expand the theme picker with dual light/dark palettes for Nord, Gruvbox, Solarized, Dracula, Catppuccin, Tokyo Night, One, GitHub, Ayu, Everforest, Kanagawa, Rosé Pine, Monokai, and Warp, keeping the original Vibra families.
- Paint the terminal grid from the active theme (background, text, cursor, ANSI, and selection) instead of leaving the pane black while only chrome borders change.
- Tint syntax highlighting, modal overlays, and the terminal scrollbar from the same palette so light and dark styles stay consistent.
- Enrich the sessions sidebar with the highest-priority agent across every tab and split in a workspace, its live state, and its model when the CLI reports it explicitly.
- Keep the right utility tabs icon-only and let the file tree focus changed files in Git without replacing the terminal.
- Toggle a persistent bottom terminal with ⌘J for dev servers; hiding it keeps the process running without changing the selected pane. Each sidebar session has its own consoles, and `+` opens additional ones.

## 0.3.12 — 2026-08-24

- Retain agent detection, activity states, lifecycle hooks, and notifications while removing the agent/layout orchestration CLI; Vibra no longer creates panes/tabs, launches agents, or routes prompts between sessions.
- Show macOS notifications only when Vibra is in the background; when an agent finishes in a different foreground pane, play a short completion sound without a banner.

## 0.3.11 — 2026-08-23

- Clarify agent capabilities and require structured activity hooks for reliable prompt waits instead of timing out after heuristic-only submissions.
- Detect the current Cursor CLI through its preserved invocation name so Cursor sessions show their bundled icon even when the CLI runs on Node.js.
- Keep the Servers panel stable during unchanged background scans instead of repainting its populated list.

## 0.3.10 — 2026-08-21

- Drag terminal tabs and session-sidebar entries to reorder them, and drag split panes by their handle to swap places.
- Keep terminal-tab drags from moving the app window on macOS.
- Jump to tabs with ⌘1–⌘8, and to the last tab with ⌘9, matching Ghostty.
- Give every split pane a compact identity header with its alias, live command or directory, agent status, and focused-pane identity across tabs, Sessions, Servers, and context menus.

## 0.3.9 — 2026-08-21

- Add a Servers tab in the right sidebar that lists TCP listeners from workspace terminals and from processes whose working directory is inside an open project, with jump-to-pane, open URL, and stop.

## 0.3.8 — 2026-08-18

- Pause terminal, Git, and sidebar work when those surfaces are hidden so idle CPU stays low.
- Notify when an agent finishes even if the process exits or the session ends, and keep `cargo run` from crashing on notification setup.
- Save `workspace.json` as compact JSON, skip unchanged writes, and stop duplicating legacy Swift session data.
- Use less memory in the editor, file tree, and terminal grid on large files and long sessions.

## 0.3.7 — 2026-08-15

- Turn the right-sidebar Diff tab into a Git panel with three views: working tree, branch changes versus the default base, and a commit history with a lane graph.
- Keep a single user-resizable width for both sidebars instead of changing the Git panel width per view.

## 0.3.6 — 2026-08-14

- Open ⌘N (new session) and ⌘T (new tab) in the current terminal directory instead of the launch/project root.
- Restyle terminal tabs as compact pills: directory/command labels, close on every tab, split count, and agent status.
- Distinguish agent attention in the sidebar and tabs: permission requests use a red mark instead of a generic waiting dot.
- Restore macOS notifications when an agent finishes or needs attention in a hidden session or while Vibra is in the background.
- Show Claude and Codex hook status separately in Settings, with a toggle for agent notifications.
- Detect Goose sessions and show a Pi mark for the Pi coding agent.

## 0.3.5 — 2026-08-12

- Track terminal `cd` end-to-end: automatic session titles, titlebar, Files tree, and Diff root follow the live working directory.
- Keep cmux-style sidebar tabs in sync (directory name, branch with ahead/behind/dirty, and path).
- Add dual light/dark app themes (Midnight, Moss, Harbor, Cinder, Violet, Bloom) with Sistema/Claro/Oscuro mode in Settings.
- Persist theme preference and follow the system appearance when mode is Sistema.
- Bind agent aliases and waits to the live process/session so stale panes cannot receive prompts intended for a previous agent.
- Make launches transactional, preserve the user's selection with `--no-focus`, and report launch/wait timeouts as real errors.
- Read the live terminal tail independently of scroll position and sanitize automated prompt input.
- Process Claude and Codex lifecycle hooks in order, repair incomplete installations, and expose one intuitive activity-tracking control in Settings.

## 0.3.4 — 2026-08-11

- Sign release builds with Developer ID Application and notarize with Apple so Gatekeeper no longer blocks install.
- Make notarization the default path in `Scripts/release.sh` (opt out with `--no-notarize`).

## 0.3.3 — 2026-08-11

- Open Settings as a centered modal (⌘,) instead of embedding preferences in the sessions sidebar.
- Add right-click context menus on sessions and panes: rename, delete/close, split, and zoom.
- Route system paste (⌘V) like Warp: inject text with bracketed paste, and for CLI agents with an image on the clipboard send Ctrl+V so tools like Claude Code can attach screenshots.
- Show live working directory and git branch metadata (ahead/behind/dirty) on each sessions sidebar tab.
- Remove the left sidebar title bar; collapse sessions with the titlebar control or ⌘B.
- Restyle the Files tree with open/closed folder SVG icons, expand chevrons, indent guides, and clearer file-type glyphs.

## 0.3.2 — 2026-08-11

- Restore the macOS application menu (Vibra → Settings…, Check for Updates…, Quit) and basic File / Edit / View / Window menus.
- Open preferences from the menu or with `⌘,` (same settings panel as the command palette).
- Rebuild the Diff panel as Warp-style expandable file cards: accordion multi-expand, inline diffs, single line-number gutter, and an “Uncommitted changes” header.
- Add IDE-style syntax highlighting in the Diff panel (keywords, strings, comments, types, numbers) for Rust, JS/TS, Python, Swift, Go, shell, and common config formats.
- Resolve coding-agent identity from foreground TTY process, terminal title, then screen text so the Codex mark appears as soon as `codex` starts (not only after a screen banner).
- Add structured `SetAgentPresence` automation and idempotent Claude/Codex hook setup, so hooks report working, waiting-for-permission, idle, and session end states.
- Manage Claude/Codex hook integrations from Settings, with installed-state feedback and explicit install/update or uninstall actions.
- Let agents open and coordinate other agents via `$VIBRA_CLI`: `+agent open/start` (split/tab, `--cwd`, shell-ready wait), named targets, `+agent prompt/wait/read/list/rename/status`, `+pane split/tab --no-focus`, cross-pane `+pane run --pane`, `+agent kinds`, and `+skill`.

## 0.3.1 — 2026-08-11

- Restore coding-agent brand marks in the sessions sidebar with live idle/working/waiting status.
- Sanitize monochrome host environment variables (`NO_COLOR`, `FORCE_COLOR`, etc.) when spawning PTYs so agent colors match a dock-launched app.
- Paint terminal cell backgrounds as a full-bleed grid so full-screen agent TUIs no longer leave a cut-off strip at the pane edge.
- Keep the shell theme intact: surface fill follows the live TUI color instead of forcing pure black.

## 0.3.0 — 2026-08-09

- Make the GPUI runtime the official stable Vibra release, replacing SwiftUI/AppKit and `libghostty`.
- Re-embed Sparkle with the existing EdDSA feed so installed copies can update again from the stable appcast.
- Replace `libghostty` with `alacritty_terminal`, preserving long-lived PTYs, terminal search, selection, mouse protocols and modern keyboard modes.
- Add recursive terminal splits, persistent ratios, geometric focus, resizing and pane zoom.
- Combine Files and read-only Git diffs in one focused right sidebar.
- Add a built-in file explorer and UTF-8 editor with atomic saves and conflict detection.
- Simplify the chrome toward a Herdr-like layout: no focus ring on panes, sessions-only left sidebar, and a full-height terminal when a single tab is open.
- Restyle Files/Diff as subtle underline tabs; move file actions into a compact header with double-click to open.
- Rebuild the Diff panel as a Warp-style master/detail view with a compact change list and full remaining height for the selected file.
- Toggle sidebars with `⌘B` (sessions) and `⌥⌘B` (Files and Diff); check for updates with `⌘U`.
- Preserve the `Vibra.app` identity and import the Swift workspace with a one-time backup before conversion.
- Import settings and workspaces created by the standalone VibraGPUI preview when no canonical Vibra state exists.
- Port signing, universal builds, notarization and DMG packaging to Cargo.

## 0.3.0-beta.1 — 2026-08-09

- First public GPUI prerelease for migration validation.

## 0.2.7 — 2026-08-06

- Restore the terminal's normal rendering by removing the experimental command-block overlay.
- Replace the xAI mark with the current Grok icon in agent badges.

## 0.2.6 — 2026-08-06

- Fix the header `IDE` button (and ⇧⌘E) so it opens the active console directory instead of the project root (often `$HOME` for ungrouped tabs).
- Upgrade Git changes with filtering, file-type icons, syntax highlighting, word-level edits, and unified or split diff layouts.
- Group terminal history into subtle command blocks without interfering with selection, links, or scrolling.
- Redesign sidebar sessions with clearer hierarchy, metadata, agent marks, and live activity states.
- Add optional macOS notifications when an agent finishes in a hidden session or while Vibra is in the background.
- Detect Grok sessions and display the official Grok mark alongside the other supported coding agents.

## 0.2.5 — 2026-08-03

- Simplify the terminal sidebar into a compact, route-aware tab list with drag-and-drop ordering.
- Add drag-and-drop ordering for nested terminal tabs.
- Add a session context view to the right sidebar with Git state and the active process tree.
- Keep the Git branch and change summary visible across the right sidebar views.
- Add a compact `IDE ↗` launcher in the window header.

## 0.2.4 — 2026-07-31

- Open the current project or a selected file in an external IDE (Cursor, VS Code, Zed, Windsurf, Xcode, and others).
- Prefer a default editor in Settings → General; reach it from the session header, File menu (⇧⌘E), and context menus.
- Polish sidebar geometry and resizing.

## 0.2.3 — 2026-07-27

- Unify app chrome and terminal under one surface: Ghostty config by default (Match Ghostty), with Catalog and Vibra as options.
- Drive sidebars, headers, and accents from the same resolved Ghostty/catalog/Vibra colors as the terminal.
- Add an Appearance settings tab with font, cursor, background opacity, and 485 catalog themes.
- Apply appearance changes live to open terminal sessions.
- Fix duplicate menu bar items caused by registering commands on multiple window groups.

## 0.2.2 — 2026-07-27

- Redesign terminal workspace navigation with a resizable terminal sidebar.
- Organize vertical tabs into folders, with create, rename, remove, and drag-and-drop.
- Clarify tab shortcuts: ⌘N for a new tab, ⌘T for a nested terminal tab, ⇧⌘N for a new window, and ⌘O to open a directory.
- Show branch, path, and nested-tab count in the terminal sidebar.
- Restore the Vibra purple accent color.

## 0.2.1 — 2026-07-25

- Add cmux-style workspaces with independent terminal tabs inside each workspace.
- Show coding-agent activity in the sidebar, including ready, working, attention, and finished states.
- Add optional Codex lifecycle hooks with a transcript-based fallback for status detection.
- Add collapsible inline Git diffs while keeping the expanded in-app diff viewer.
- Align terminal typography and spacing more closely with the user's Ghostty configuration.
- Reduce background polling, transcript reads, and unnecessary SwiftUI updates.

## 0.2.0 — 2026-07-24

- Add in-app updates over Sparkle, checked daily and on demand.
- Publish signed universal disk images through GitHub Releases.
- Add a download page and update feed served from GitHub Pages.
- Add an event-driven Git changes sidebar.
- Render bounded unified diffs with native SwiftUI views.
- Add stage and unstage actions for individual files and the repository.
- Render terminal tabs from the state of the session they own.
- Reduce the visible brand treatment to a text-only application name.

## 0.1.0 — 2026-07-24

- Create the independent Vibra repository.
- Add the native macOS application shell.
- Integrate long-lived libghostty terminal sessions.
- Add projects, terminal tabs, workspace restoration, and lifecycle tests.
