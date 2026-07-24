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

Split panes and file navigation are next.

## Install

Download the disk image from [the Vibra site](https://rubenrca.github.io/Vibra/)
or from [Releases](https://github.com/rubenrca/Vibra/releases/latest), then drag
Vibra to Applications.

Vibra is not notarized by Apple yet, so macOS blocks the first launch. Open it
once, dismiss the warning, then allow it under **System Settings → Privacy &
Security → Open Anyway**. This is needed only once; updates Vibra installs by
itself are not blocked.

Vibra checks for updates daily through [Sparkle](https://sparkle-project.org)
and can also check on demand from **Vibra → Check for Updates…**. Automatic
checks can be turned off in Settings.

## Requirements

- macOS 14 or newer

Building additionally needs:

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

Create an application bundle with:

```bash
./Scripts/package_app.sh debug
open dist/Vibra.app
```

The bundle is signed ad-hoc unless a Developer ID Application identity is in the
keychain, in which case it is signed for real with the hardened runtime and
`Resources/Vibra.entitlements`. Build a distributable universal disk image with:

```bash
./Scripts/package_app.sh release --universal --dmg --notarize
```

Notarization reads `APPLE_KEYCHAIN_PROFILE`, or `APPLE_ID`, `APPLE_TEAM_ID` and
`APPLE_APP_SPECIFIC_PASSWORD`, and is skipped when `--notarize` is omitted.

Run the model tests with:

```bash
swift test
```

## Releasing

Releases are cut from `main` with a clean working tree. Move the `Unreleased`
entries in `CHANGELOG.md` under a `## <version>` heading, commit, then run:

```bash
./Scripts/release.sh 0.2.0            # add --notarize once a Developer ID exists
./Scripts/release.sh 0.2.0 --dry-run  # build and sign without publishing
```

The script builds the universal disk image, turns that changelog section into
the release notes Sparkle shows, signs the appcast entry with the EdDSA key,
uploads the image to GitHub Releases, and only then commits `docs/appcast.xml` —
so the feed never points at a download that does not exist yet.

`docs/` is served by GitHub Pages: `docs/index.html` is the download page and
`docs/appcast.xml` is the update feed baked into every build as `SUFeedURL`.

### Signing keys

Updates are trusted by EdDSA signature, independently of Apple code signing. The
public key lives in `Scripts/package_app.sh` and is written into each build; the
private key is in the release machine's keychain, put there by Sparkle's
`generate_keys`.

Back it up somewhere safe:

```bash
.build/artifacts/sparkle/Sparkle/bin/generate_keys -x sparkle-private-key.txt
```

Losing that key means installed copies will reject every future update and each
user has to download Vibra again by hand, so keep the backup off this machine
and out of the repository.

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
