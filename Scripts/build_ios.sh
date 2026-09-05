#!/bin/zsh
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
cd "$repo_root"
platform=${1:-simulator}
case "$platform" in
  simulator) destination='generic/platform=iOS Simulator'; signing=(CODE_SIGNING_ALLOWED=YES CODE_SIGN_IDENTITY=-) ;;
  device) destination='generic/platform=iOS'; signing=(CODE_SIGNING_ALLOWED=NO) ;;
  *) print -u2 -- "Uso: Scripts/build_ios.sh [simulator|device]"; exit 64 ;;
esac
# Simulator signing is required: Xcode embeds application-identifier for Keychain.
# Disabling signing produces a launchable app whose SecItem calls fail with -34018.
# SwiftTerm's pinned build plugin generates build metadata. It is required by
# upstream; only skip Xcode's interactive plugin trust prompt in this CLI build.
xcodebuild -project ios/Vibra.xcodeproj -scheme Vibra \
  -destination "$destination" -derivedDataPath .build/ios \
  -skipPackagePluginValidation -onlyUsePackageVersionsFromResolvedFile \
  "${signing[@]}" build
