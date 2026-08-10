#!/bin/zsh

set -euo pipefail

script_name=${0:A}
repo_root=${script_name:h:h}
configuration=debug
universal=0
make_dmg=0
notarize=0
signing_identity=${VIBRA_SIGNING_IDENTITY:-}
force_ad_hoc=0

usage() {
  print -u2 -- "usage: $script_name [debug|release] [--universal] [--dmg] [--notarize] [--sign <identity>]"
  print -u2 --
  print -u2 -- "  --universal  build aarch64 and x86_64 slices and merge them"
  print -u2 -- "  --dmg        also produce dist/Vibra.dmg"
  print -u2 -- "  --notarize   submit the app (and DMG) to Apple and staple the ticket"
  print -u2 -- "  --sign <id>  signing identity. Defaults to \$VIBRA_SIGNING_IDENTITY, then the"
  print -u2 -- "               first Developer ID Application identity, then ad-hoc signing."
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

if [[ $signing_identity == - ]]; then
  signing_identity=
  force_ad_hoc=1
fi

app_dir="$repo_root/dist/Vibra.app"
contents_dir="$app_dir/Contents"
macos_dir="$contents_dir/MacOS"
resources_dir="$contents_dir/Resources"
frameworks_dir="$contents_dir/Frameworks"
plist="$contents_dir/Info.plist"
plist_template="$repo_root/Resources/Info.plist"
entitlements="$repo_root/Resources/Vibra.entitlements"
icon_source="$repo_root/Resources/AppIcon.png"
iconset_dir="$repo_root/target/Vibra.iconset"
icon_file="$resources_dir/Vibra.icns"
dmg_path="$repo_root/dist/Vibra.dmg"

# Sparkle checks this feed and refuses any update whose EdDSA signature does not
# verify against the public key below. The matching private key lives in the
# keychain of whoever publishes releases; Scripts/release.sh signs with it.
feed_url=${VIBRA_FEED_URL:-https://rubenrca.github.io/Vibra/appcast.xml}
public_ed_key=${VIBRA_PUBLIC_ED_KEY:-05voyXnLA9QCHMp91KonT03ysgHHfHSAElaMPiUrNOc=}

for required_path in "$plist_template" "$entitlements" "$icon_source"; do
  if [[ ! -f $required_path ]]; then
    print -u2 -- "missing packaging input: $required_path"
    exit 66
  fi
done

resolve_sparkle_framework() {
  local candidate
  for candidate in \
    "${VIBRA_SPARKLE_FRAMEWORK:-}" \
    "$repo_root/.build/artifacts/sparkle/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework" \
    "$repo_root/.build/checkouts/Sparkle/Sparkle.xcframework/macos-arm64_x86_64/Sparkle.framework" \
    "$repo_root"/third_party/sparkle-*/Sparkle.framework(N) \
    "$repo_root/third_party/Sparkle.framework" \
    "$repo_root/dist/Vibra.app/Contents/Frameworks/Sparkle.framework"
  do
    [[ -n $candidate && -d $candidate ]] && print -r -- "$candidate" && return 0
  done
  return 1
}

sparkle_source=$(resolve_sparkle_framework || true)
if [[ -z ${sparkle_source:-} ]]; then
  sparkle_source=$("$repo_root/Scripts/fetch_sparkle.sh")
fi
if [[ -z ${sparkle_source:-} || ! -d $sparkle_source ]]; then
  print -u2 -- "Sparkle.framework not found and could not be fetched."
  exit 70
fi
export VIBRA_SPARKLE_FRAMEWORK=$sparkle_source
print "using Sparkle: $sparkle_source"

marketing_version=${VIBRA_MARKETING_VERSION:-}
if [[ -z $marketing_version ]]; then
  marketing_version=$(
    sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1
  )
  marketing_version=${marketing_version:-0.3.0}
fi

build_version=${VIBRA_BUILD_VERSION:-}
if [[ -z $build_version ]]; then
  build_version=$(git -C "$repo_root" rev-list --count HEAD 2>/dev/null || true)
  build_version=${build_version:-1}
fi

if (( universal )); then
  targets=(aarch64-apple-darwin x86_64-apple-darwin)
else
  case "$(uname -m)" in
    arm64) targets=(aarch64-apple-darwin) ;;
    x86_64) targets=(x86_64-apple-darwin) ;;
    *)
      print -u2 -- "unsupported build architecture: $(uname -m)"
      exit 69
      ;;
  esac
fi

installed_targets=$(rustup target list --installed)
binaries=()
for target in $targets; do
  if ! print -r -- "$installed_targets" | grep -qx "$target"; then
    print -u2 -- "missing Rust target: $target"
    print -u2 -- "install it with: rustup target add $target"
    exit 69
  fi

  cargo_args=(build --locked --target "$target")
  profile_dir=debug
  if [[ $configuration == release ]]; then
    cargo_args+=(--release)
    profile_dir=release
  fi

  print "building Vibra ($configuration, $target)"
  # Ensure the build script can find Sparkle while compiling the ObjC bridge.
  VIBRA_SPARKLE_FRAMEWORK=$sparkle_source cargo "${cargo_args[@]}" --manifest-path "$repo_root/Cargo.toml"
  binary="$repo_root/target/$target/$profile_dir/vibra"
  if [[ ! -x $binary ]]; then
    print -u2 -- "Cargo did not produce the expected binary: $binary"
    exit 70
  fi
  binaries+=("$binary")
done

rm -rf "$app_dir"
mkdir -p "$macos_dir" "$resources_dir" "$frameworks_dir"

if (( ${#binaries} > 1 )); then
  lipo -create "${binaries[@]}" -output "$macos_dir/Vibra"
else
  cp "${binaries[1]}" "$macos_dir/Vibra"
fi
chmod 755 "$macos_dir/Vibra"

# ditto, not cp: the framework is a versioned bundle whose Versions/Current and
# top-level symlinks have to survive the copy or codesign rejects it.
ditto "$sparkle_source" "$frameworks_dir/Sparkle.framework"

rm -rf "$iconset_dir"
swift "$repo_root/Scripts/generate_app_icon.swift" "$icon_source" "$iconset_dir"
iconutil -c icns "$iconset_dir" -o "$icon_file"

cp "$plist_template" "$plist"
plutil -replace CFBundleShortVersionString -string "$marketing_version" "$plist"
plutil -replace CFBundleVersion -string "$build_version" "$plist"
plutil -replace SUFeedURL -string "$feed_url" "$plist"
plutil -replace SUPublicEDKey -string "$public_ed_key" "$plist"
plutil -replace SUEnableAutomaticChecks -bool true "$plist"
plutil -replace SUScheduledCheckInterval -integer 86400 "$plist"
plutil -lint "$plist" "$entitlements" >/dev/null

if [[ -z $signing_identity ]] && (( ! force_ad_hoc )); then
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
    exit 78
  fi
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
codesign --verify --strict --deep --verbose=2 "$app_dir"

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

  zip_path="$repo_root/dist/Vibra-notarize.zip"
  rm -f "$zip_path"
  ditto -c -k --keepParent "$app_dir" "$zip_path"
  print "notarizing Vibra.app"
  xcrun notarytool submit "$zip_path" "${notary_auth[@]}" --wait
  xcrun stapler staple "$app_dir"
  xcrun stapler validate "$app_dir"
  rm -f "$zip_path"
fi

if (( make_dmg )); then
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
  else
    codesign --force --timestamp=none --sign - "$dmg_path"
  fi

  if (( notarize )); then
    print "notarizing Vibra.dmg"
    xcrun notarytool submit "$dmg_path" "${notary_auth[@]}" --wait
    xcrun stapler staple "$dmg_path"
    xcrun stapler validate "$dmg_path"
  fi
fi

print
print "version:   $marketing_version ($build_version)"
print "arch:      $(lipo -archs "$macos_dir/Vibra")"
print "sparkle:   embedded"
print "signature: $(codesign -dv "$app_dir" 2>&1 | awk -F= '/^Signature=/ { print $2 }')"
print "$app_dir"
(( make_dmg )) && print "$dmg_path"
exit 0
