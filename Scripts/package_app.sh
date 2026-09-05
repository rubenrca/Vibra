#!/bin/zsh

set -euo pipefail

script_name=${0:A}
repo_root=${script_name:h:h}
configuration=debug
universal=0
ghostty=0
make_dmg=0
notarize=0
signing_identity=${VIBRA_SIGNING_IDENTITY:-}
force_ad_hoc=0

usage() {
  print -u2 -- "usage: $script_name [debug|release] [--universal] [--ghostty] [--dmg] [--notarize] [--sign <identity>]"
  print -u2 --
  print -u2 -- "  --universal  build aarch64 and x86_64 slices and merge them"
  print -u2 -- "  --ghostty    use the experimental Ghostty backend (run fetch_ghostty.sh first)"
  print -u2 -- "  --dmg        also produce dist/Vibra.dmg"
  print -u2 -- "  --notarize   notarize the distributable artifact with Apple and staple its ticket"
  print -u2 -- "  --sign <id>  signing identity. Defaults to \$VIBRA_SIGNING_IDENTITY, then the"
  print -u2 -- "               first Developer ID Application identity, then ad-hoc signing."
  print -u2 --
  print -u2 -- "Notarization reads APPLE_KEYCHAIN_PROFILE, or APPLE_ID + APPLE_TEAM_ID +"
  print -u2 -- "APPLE_APP_SPECIFIC_PASSWORD. Set VIBRA_NOTARY_WAIT_TIMEOUT (default: 2h)"
  print -u2 -- "to bound how long the command waits; the submission continues at Apple after a timeout."
  exit 64
}

while (( $# )); do
  case "$1" in
    debug|release) configuration=$1 ;;
    --universal) universal=1 ;;
    --ghostty) ghostty=1 ;;
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
notarization_dir="$repo_root/dist/notarization"
notary_wait_timeout=${VIBRA_NOTARY_WAIT_TIMEOUT:-2h}

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
  if (( ghostty )); then
    cargo_args+=(--features ghostty)
  fi
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
if (( ghostty )); then
  mkdir -p "$resources_dir/Licenses"
  cp "$repo_root/Resources/Licenses/Ghostty-MIT.txt" "$resources_dir/Licenses/Ghostty-MIT.txt"
fi

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
fi

if (( make_dmg )); then
  staging_dir=$(mktemp -d)
  # Preserve the bundle's signature, symlinks and extended attributes.
  ditto "$app_dir" "$staging_dir/Vibra.app"
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
fi

if (( notarize )); then
  # Apple recommends notarizing only the outermost file users download. For a
  # normal release that is the DMG; sending both the app ZIP and its DMG adds a
  # second queue wait without improving the distributed artifact.
  if (( make_dmg )); then
    notarize_path=$dmg_path
    notarize_label=Vibra.dmg
  else
    notarize_path="$repo_root/dist/Vibra-notarize.zip"
    notarize_label=Vibra.app
    rm -f "$notarize_path"
    ditto -c -k --keepParent "$app_dir" "$notarize_path"
  fi

  mkdir -p "$notarization_dir"
  print "submitting $notarize_label to Apple for notarization"
  submission=$(
    xcrun notarytool submit "$notarize_path" "${notary_auth[@]}" \
      --output-format json --no-progress
  )
  submission_id=$(print -r -- "$submission" | plutil -extract id raw -o - -)
  if [[ ! $submission_id =~ '^[0-9A-Fa-f-]{36}$' ]]; then
    print -u2 -- "Apple did not return a valid notarization submission ID."
    print -u2 -- "$submission"
    exit 70
  fi
  submission_record="$notarization_dir/${notarize_label}.submission-id"
  print -r -- "$submission_id" > "$submission_record"
  print "submitted: $submission_id"
  print "waiting for Apple (timeout: $notary_wait_timeout)"
  xcrun notarytool wait "$submission_id" "${notary_auth[@]}" \
    --timeout "$notary_wait_timeout" || {
      wait_status=$?
      print -u2 -- "Notarization did not complete successfully. Apple keeps processing after a timeout."
      print -u2 -- "Submission ID: $submission_id"
      print -u2 -- "Check:  xcrun notarytool info $submission_id --keychain-profile \"${APPLE_KEYCHAIN_PROFILE:-Vibra-Notary}\""
      print -u2 -- "Record: $submission_record"
      exit "$wait_status"
    }

  print "stapling Apple's ticket to $notarize_label"
  xcrun stapler staple "$notarize_path"
  xcrun stapler validate "$notarize_path"
  if (( make_dmg )); then
    spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path"
  else
    spctl --assess --type execute --verbose=2 "$app_dir"
    rm -f "$notarize_path"
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
