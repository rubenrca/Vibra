# Notices

Vibra is built with [GPUI](https://www.gpui.rs/), the GPU-accelerated UI
framework distributed under the Apache-2.0 license.

Terminal parsing and state are provided by
[alacritty_terminal](https://github.com/alacritty/alacritty), distributed under
the Apache-2.0 license.

Builds with the experimental `ghostty` feature use
[libghostty-vt](https://github.com/ghostty-org/ghostty), pinned to
`492300cad104195411d12217dd22f1cd05f31376`, under the MIT license.
Its copyright and license are included in `Resources/Licenses/Ghostty-MIT.txt`.
Alacritty remains available as a fallback in these builds.

The embedded JetBrains Mono Variable fonts in `Resources/Fonts` are distributed
under the SIL Open Font License 1.1. The complete font license is included as
`Resources/Fonts/OFL.txt`.

Dependency copyright and license notices remain with their respective projects
and versions recorded in `Cargo.lock`.

Several bundled palettes (Nord, Gruvbox, Solarized, Dracula, Catppuccin,
Tokyo Night, One, GitHub, Ayu, Everforest, Kanagawa, Rosé Pine, Monokai, and
Warp) use color values published in
[warpdotdev/themes](https://github.com/warpdotdev/themes) (Apache-2.0) and
their upstream schemes. Palette names remain trademarks of their respective
authors.

## Agent marks

The compatibility marks in `Resources/AgentMarks` identify the coding agents
Vibra can detect. OpenAI/Codex, Claude, Gemini, OpenCode, Amp, and Cursor
marks are supplied by [Simple Icons](https://simpleicons.org/) (CC0-1.0).
The Aider and Goose marks are sourced from their public upstream repositories.
The Grok mark is the current icon served by [Grok](https://grok.com),
the xAI assistant.
Their names and logos remain trademarks of their respective owners; their use
here does not imply endorsement or affiliation.
