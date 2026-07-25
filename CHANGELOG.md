# Changelog

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
