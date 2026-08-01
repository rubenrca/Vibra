#!/bin/zsh

set -euo pipefail

# zsh rebinds $0 to the function name inside a function, so capture the script
# path while still at top level for usage() to report.
script_name=${0:A}
repo_root=${script_name:h:h}
configuration=debug
universal=0
make_dmg=0
notarize=0
signing_identity=${VIBRA_SIGNING_IDENTITY:-}

# Every message goes through `print -u2 --`: zsh's print parses a leading `-` as
# an option bundle, so an unguarded "--notarize ..." string aborts the script.
usage() {
  print -u2 -- "usage: $script_name [debug|release] [--universal] [--dmg] [--notarize] [--sign <identity>]"
  print -u2 --
  print -u2 -- "  --universal  build arm64 and x86_64 separately and merge them"
  print -u2 -- "  --dmg        also produce dist/Vibra.dmg"
  print -u2 -- "  --notarize   submit the app (and DMG) to Apple and staple the ticket"
  print -u2 -- "  --sign <id>  signing identity. Defaults to \$VIBRA_SIGNING_IDENTITY, then the"
  print -u2 -- "               first Developer ID Application identity in the keychain, then an"
  print -u2 -- "               ad-hoc signature so local builds work without a paid account."
  print -u2 --
  print -u2 -- "Notarization reads APPLE_KEYCHAIN_PROFILE, or APPLE_ID + APPLE_TEAM_ID +"
  print -u2 -- "APPLE_APP_SPECIFIC_PASSWORD."
  exit 64
}

while (( $# )); do
  case "$1" in
    debug|release) configuration=$1 ;;
    --universal) universal=1 ;;
    --dmg) make_dmg=1 ;;
    --notarize) notarize=1 ;;
    --sign)
      shift
      (( $# )) || usage
      signing_identity=$1
      ;;
    -h|--help) usage ;;
    *)
      print -u2 -- "unknown argument: $1"
      usage
      ;;
  esac
  shift
done

app_dir="$repo_root/dist/Vibra.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
frameworks_dir="$contents_dir/Frameworks"
plist="$contents_dir/Info.plist"
entitlements="$repo_root/Resources/Vibra.entitlements"
icon_source="$repo_root/Resources/AppIcon.png"
agent_marks_source="$repo_root/Sources/VibraApp/Resources/AgentMarks"
iconset_dir="$repo_root/.build/Vibra.iconset"
icon_file="$resources_dir/Vibra.icns"
dmg_path="$repo_root/dist/Vibra.dmg"

# Sparkle checks this feed and refuses any update whose EdDSA signature does not
# verify against the public key below. The matching private key lives in the
# keychain of whoever publishes releases; Scripts/release.sh signs with it.
feed_url=${VIBRA_FEED_URL:-https://rubenrca.github.io/Vibra/appcast.xml}
public_ed_key=${VIBRA_PUBLIC_ED_KEY:-05voyXnLA9QCHMp91KonT03ysgHHfHSAElaMPiUrNOc=}

# Sparkle compares CFBundleVersion and refuses an update whose build number is
# not higher than the installed one, so it has to grow monotonically. The commit
# count does that on a linear history; override either value when building
# outside a git checkout.
marketing_version=${VIBRA_MARKETING_VERSION:-}
if [[ -z $marketing_version ]]; then
  marketing_version=$(git -C "$repo_root" describe --tags --abbrev=0 2>/dev/null || true)
  marketing_version=${marketing_version#v}
  marketing_version=${marketing_version:-0.1.0}
fi

build_version=${VIBRA_BUILD_VERSION:-}
if [[ -z $build_version ]]; then
  build_version=$(git -C "$repo_root" rev-list --count HEAD 2>/dev/null || true)
  build_version=${build_version:-1}
fi

# `swift build --arch arm64 --arch x86_64` cannot link libghostty: SwiftPM
# switches to the xcodebuild-style layout for multi-arch builds and stops
# resolving the GhosttyKit binary target ("library not found for -lghostty").
# Building each slice on its own and merging them works, because the libghostty.a
# that ships in the XCFramework is already fat.
if (( universal )); then
  archs=(arm64 x86_64)
else
  archs=($(uname -m))
fi

binaries=()
sparkle_source=
resource_bundle_source=
for arch in $archs; do
  print "building Vibra ($configuration, $arch)"
  swift build --package-path "$repo_root" -c "$configuration" --arch "$arch" --product Vibra
  bin_path=$(swift build --package-path "$repo_root" -c "$configuration" --arch "$arch" --show-bin-path)
  binaries+=("$bin_path/Vibra")
  resource_bundle_source="$bin_path/Vibra_VibraApp.bundle"
  # Every slice gets the same framework: the XCFramework SwiftPM downloads
  # already carries a fat arm64+x86_64 Sparkle, so there is nothing to merge.
  sparkle_source="$bin_path/Sparkle.framework"
done

if [[ ! -d $sparkle_source ]]; then
  print -u2 -- "Sparkle.framework not found at: $sparkle_source"
  exit 70
fi

rm -rf "$app_dir"
mkdir -p "$macos_dir" "$resources_dir" "$frameworks_dir"
if [[ -d $resource_bundle_source ]]; then
  ditto "$resource_bundle_source" "$resources_dir/Vibra_VibraApp.bundle"
elif [[ -d $agent_marks_source ]]; then
  # This supports package builds made before SwiftPM resource bundles existed.
  ditto "$agent_marks_source" "$resources_dir/AgentMarks"
fi

if (( ${#binaries} > 1 )); then
  lipo -create "${binaries[@]}" -output "$macos_dir/Vibra"
else
  cp "${binaries[1]}" "$macos_dir/Vibra"
fi

# ditto, not cp: the framework is a versioned bundle whose Versions/Current and
# top-level symlinks have to survive the copy or codesign rejects it.
ditto "$sparkle_source" "$frameworks_dir/Sparkle.framework"

swift "$repo_root/Scripts/generate_app_icon.swift" "$icon_source" "$iconset_dir"
iconutil -c icns "$iconset_dir" -o "$icon_file"

plutil -create xml1 "$plist"
plutil -insert CFBundleDisplayName -string Vibra "$plist"
plutil -insert CFBundleExecutable -string Vibra "$plist"
plutil -insert CFBundleIdentifier -string app.vibra.Vibra "$plist"
plutil -insert CFBundleIconFile -string Vibra "$plist"
plutil -insert CFBundleInfoDictionaryVersion -string 6.0 "$plist"
plutil -insert CFBundleName -string Vibra "$plist"
plutil -insert CFBundlePackageType -string APPL "$plist"
plutil -insert CFBundleShortVersionString -string "$marketing_version" "$plist"
plutil -insert CFBundleVersion -string "$build_version" "$plist"
plutil -insert LSApplicationCategoryType -string public.app-category.developer-tools "$plist"
plutil -insert LSMinimumSystemVersion -string 14.0 "$plist"
plutil -insert NSHighResolutionCapable -bool true "$plist"
plutil -insert NSHumanReadableCopyright -string "Vibra is MIT licensed." "$plist"

# Sparkle. UpdaterModel refuses to start the updater when SUFeedURL is absent,
# which is what keeps `swift run` builds from checking for updates.
plutil -insert SUFeedURL -string "$feed_url" "$plist"
plutil -insert SUPublicEDKey -string "$public_ed_key" "$plist"
plutil -insert SUEnableAutomaticChecks -bool true "$plist"
plutil -insert SUScheduledCheckInterval -integer 86400 "$plist"

if [[ -z $signing_identity ]]; then
  signing_identity=$(
    security find-identity -v -p codesigning 2>/dev/null \
      | awk -F'"' '/Developer ID Application/ { print $2; exit }'
  )
fi

if [[ -n $signing_identity ]]; then
  print "signing with: $signing_identity"
  sign_flags=(--options runtime --timestamp --sign "$signing_identity")
else
  if (( notarize )); then
    print -u2 -- "--notarize needs a Developer ID Application identity; none was found."
    print -u2 -- "Install one from developer.apple.com, or pass --sign <identity>."
    exit 78
  fi
  # Ad-hoc keeps this working without a paid Apple Developer account. The bundle
  # still runs elsewhere, but with no hardened runtime and no secure timestamp
  # Gatekeeper blocks it on first launch until the user allows it by hand, and
  # Apple rejects it for notarization.
  print "signing ad-hoc (no Developer ID identity found)"
  sign_flags=(--timestamp=none --sign -)
fi

# Signing runs inside out: sealing a nested bundle after its container
# invalidates the container's signature. Sparkle's helpers ship entitlements of
# their own — the installer and downloader XPC services especially — so theirs
# are preserved rather than replaced with Vibra's.
sparkle_versioned_dir="$frameworks_dir/Sparkle.framework/Versions/B"
for helper in \
  "$sparkle_versioned_dir/XPCServices/Downloader.xpc" \
  "$sparkle_versioned_dir/XPCServices/Installer.xpc" \
  "$sparkle_versioned_dir/Updater.app" \
  "$sparkle_versioned_dir/Autoupdate"; do
  codesign --force --preserve-metadata=entitlements "${sign_flags[@]}" "$helper"
done
codesign --force "${sign_flags[@]}" "$sparkle_versioned_dir"
codesign --force --entitlements "$entitlements" "${sign_flags[@]}" "$app_dir"

codesign --verify --strict --deep "$app_dir"

notary_auth=()
if (( notarize )); then
  if [[ -n ${APPLE_KEYCHAIN_PROFILE:-} ]]; then
    notary_auth=(--keychain-profile "$APPLE_KEYCHAIN_PROFILE")
  elif [[ -n ${APPLE_ID:-} && -n ${APPLE_TEAM_ID:-} && -n ${APPLE_APP_SPECIFIC_PASSWORD:-} ]]; then
    notary_auth=(
      --apple-id "$APPLE_ID"
      --team-id "$APPLE_TEAM_ID"
      --password "$APPLE_APP_SPECIFIC_PASSWORD"
    )
  else
    print -u2 -- "--notarize needs APPLE_KEYCHAIN_PROFILE, or APPLE_ID + APPLE_TEAM_ID +"
    print -u2 -- "APPLE_APP_SPECIFIC_PASSWORD."
    exit 78
  fi

  # Notarization only accepts archives, so submit a zip of the bundle and staple
  # the returned ticket onto the bundle itself.
  zip_path="$repo_root/dist/Vibra-notarize.zip"
  rm -f "$zip_path"
  ditto -c -k --keepParent "$app_dir" "$zip_path"
  print "notarizing Vibra.app"
  xcrun notarytool submit "$zip_path" "${notary_auth[@]}" --wait
  xcrun stapler staple "$app_dir"
  rm -f "$zip_path"
fi

if (( make_dmg )); then
  # hdiutil keeps this dependency-free. Reach for `create-dmg` instead if you
  # want a styled window with background art and placed icons.
  staging_dir=$(mktemp -d)
  cp -R "$app_dir" "$staging_dir/Vibra.app"
  ln -s /Applications "$staging_dir/Applications"
  rm -f "$dmg_path"
  hdiutil create \
    -volname "Vibra $marketing_version" \
    -srcfolder "$staging_dir" \
    -format UDZO \
    -quiet \
    "$dmg_path"
  rm -rf "$staging_dir"

  if [[ -n $signing_identity ]]; then
    codesign --force --timestamp --sign "$signing_identity" "$dmg_path"
  fi

  if (( notarize )); then
    print "notarizing Vibra.dmg"
    xcrun notarytool submit "$dmg_path" "${notary_auth[@]}" --wait
    xcrun stapler staple "$dmg_path"
  fi
fi

print
print "version:   $marketing_version ($build_version)"
print "arch:      $(lipo -archs "$macos_dir/Vibra")"
print "signature: $(codesign -dv "$app_dir" 2>&1 | awk -F= '/^Signature=/ { print $2 }')"
print "$app_dir"
(( make_dmg )) && print "$dmg_path"
exit 0
