# Changelog

## Unreleased

- Fix the header `IDE` button (and ⇧⌘E) so it opens the active console directory instead of the project root (often `$HOME` for ungrouped tabs).

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
