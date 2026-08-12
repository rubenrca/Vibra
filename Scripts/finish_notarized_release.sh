#!/bin/zsh
set -euo pipefail
# Resume the 0.3.4 release after Apple accepts the app ZIP submitted before the
# release flow switched to notarizing only the final DMG. This is intentionally
# separate from package_app.sh: rebuilding the app would change the accepted
# artifact and invalidate the first submission.
# Usage: APPLE_KEYCHAIN_PROFILE=Vibra-Notary ./Scripts/finish_notarized_release.sh <submission-id>

script_name=${0:A}
repo_root=${script_name:h:h}
profile=${APPLE_KEYCHAIN_PROFILE:-Vibra-Notary}
submission_id=${1:-}
app_dir="$repo_root/dist/Vibra.app"
version=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1)
identity=$(security find-identity -v -p codesigning 2>/dev/null | awk -F'"' '/Developer ID Application/ { print $2; exit }')
wait_timeout=${VIBRA_NOTARY_WAIT_TIMEOUT:-2h}

usage() {
  print -u2 -- "usage: $script_name <submission-id>"
  print -u2 -- "The submission must be a ZIP of dist/Vibra.app accepted by Apple."
  exit 64
}

[[ -n $submission_id ]] || usage
[[ $submission_id =~ '^[0-9A-Fa-f-]{36}$' ]] || usage

status=$(xcrun notarytool info "$submission_id" --keychain-profile "$profile" 2>/dev/null | awk -F': ' '/status:/ {print $2; exit}')
print "submission $submission_id → $status"
if [[ $status != Accepted ]]; then
  print -u2 -- "Not ready (want Accepted). Current: ${status:-unknown}"
  print -u2 -- "Wait with: xcrun notarytool wait $submission_id --keychain-profile $profile --timeout $wait_timeout"
  if [[ $status == Invalid ]]; then
    print -u2 -- "Inspect Apple's report: xcrun notarytool log $submission_id --keychain-profile $profile"
  fi
  exit 1
fi

[[ -d $app_dir ]] || { print -u2 -- "missing $app_dir"; exit 1 }
[[ -n $identity ]] || { print -u2 -- "no Developer ID identity"; exit 1 }

print "stapling ticket to Vibra.app"
xcrun stapler staple "$app_dir"
xcrun stapler validate "$app_dir"
spctl --assess --type execute --verbose=2 "$app_dir"

print "building DMG"
dmg_path="$repo_root/dist/Vibra.dmg"
staging_dir=$(mktemp -d)
cleanup() { rm -rf "$staging_dir"; }
trap cleanup EXIT
rm -f "$dmg_path"
ditto "$app_dir" "$staging_dir/Vibra.app"
ln -s /Applications "$staging_dir/Applications"
hdiutil create -volname "Vibra $version" -srcfolder "$staging_dir" -ov -format UDZO "$dmg_path"
codesign --force --timestamp --sign "$identity" "$dmg_path"

print "notarizing DMG"
xcrun notarytool submit "$dmg_path" --keychain-profile "$profile" --wait --timeout "$wait_timeout"
xcrun stapler staple "$dmg_path"
xcrun stapler validate "$dmg_path"
spctl --assess --type open --context context:primary-signature --verbose=2 "$dmg_path"

print "done: $dmg_path"
print "Next: from a clean tree that matches this app, run:"
print "  $repo_root/Scripts/release.sh $version --resume-dmg"
