#!/bin/zsh

set -euo pipefail

# Publishes a Vibra release: universal DMG, Sparkle EdDSA appcast entry,
# GitHub Release, and (for stable builds) the live feed on docs/appcast.xml.
#
# The feed is published only after the image is downloadable, so an interrupted
# run can never point Sparkle at a URL that 404s.

script_name=${0:A}
repo_root=${script_name:h:h}
dry_run=0
# Published releases always use Developer ID and Apple notarization.
# --no-notarize is only available for local dry packaging.
notarize=1
resume_dmg=0
prerelease=0
channel_explicit=0
version=
repo_slug=${VIBRA_REPO_SLUG:-rubenrca/Vibra}
download_prefix="https://github.com/$repo_slug/releases/download"
feed_dir="$repo_root/docs"
staging_dir="$repo_root/dist/appcast"
sparkle_version=${VIBRA_SPARKLE_VERSION:-2.9.4}
signing_identity=${VIBRA_SIGNING_IDENTITY:-}
if [[ -L "$repo_root/dist" || -L $feed_dir || -L $feed_dir/appcast.xml || -L $staging_dir ]]; then
  print -u2 -- "release dist, feed and staging paths must be regular directories/files, not symlinks."
  exit 65
fi
# Used by package_app.sh when --notarize is set.
export APPLE_KEYCHAIN_PROFILE="${APPLE_KEYCHAIN_PROFILE:-Vibra-Notary}"

usage() {
  print -u2 -- "usage: $script_name <version> [--prerelease] [--notarize|--no-notarize] [--resume-dmg] [--dry-run]"
  print -u2 --
  print -u2 -- "  <version>       marketing version, e.g. 0.3.0 or 0.3.1-beta.1"
  print -u2 -- "  --prerelease    GitHub prerelease only; does not update docs/appcast.xml"
  print -u2 -- "  --stable        publish as stable, even when the version has a suffix"
  print -u2 -- "  --notarize      Developer ID sign + Apple notarization (default)"
  print -u2 -- "  --no-notarize   skip notarization in a local --dry-run only"
  print -u2 -- "  --resume-dmg    publish an existing, notarized DMG from dist/ or dist/appcast/"
  print -u2 -- "  --dry-run       build and sign everything, publish nothing"
  print -u2 --
  print -u2 -- "Stable releases update the Sparkle feed after the DMG is live on GitHub."
  print -u2 -- "Notarization uses \$APPLE_KEYCHAIN_PROFILE (default: Vibra-Notary)."
  print -u2 -- "Requires the GitHub CLI and the Sparkle EdDSA private key in the keychain."
  print -u2 -- "Published releases require VIBRA_SIGNING_IDENTITY with the full Developer ID name."
  exit 64
}

while (( $# )); do
  case "$1" in
    --notarize) notarize=1 ;;
    --no-notarize) notarize=0 ;;
    --resume-dmg) resume_dmg=1 ;;
    --dry-run) dry_run=1 ;;
    --prerelease) prerelease=1; channel_explicit=1 ;;
    --stable)
      prerelease=0
      channel_explicit=1
      ;;
    -h|--help) usage ;;
    -*)
      print -u2 -- "unknown argument: $1"
      usage
      ;;
    *)
      [[ -z $version ]] || usage
      version=$1
      ;;
  esac
  shift
done

[[ -n $version ]] || usage

if [[ ! $version =~ '^[0-9]+\.[0-9]+\.[0-9]+([.-][0-9A-Za-z.-]+)?$' ]]; then
  print -u2 -- "version must look like 1.2.3 or 1.2.3-beta.1, got: $version"
  exit 64
fi

# Versions with a pre-release suffix default to GitHub prerelease unless forced stable.
if (( ! channel_explicit )) && [[ $version == *-* ]]; then
  prerelease=1
fi

if (( ! notarize && ! dry_run )); then
  print -u2 -- "published releases must be notarized; --no-notarize is only valid with --dry-run."
  exit 64
fi
if (( notarize )); then
  if [[ ! $signing_identity =~ '^Developer ID Application: .+ \([A-Z0-9]{10}\)$' ]]; then
    print -u2 -- "set VIBRA_SIGNING_IDENTITY to the full Developer ID Application name, including its Team ID."
    exit 78
  fi
  expected_team=$(print -r -- "$signing_identity" | sed -E 's/.*\(([A-Z0-9]{10})\)$/\1/')
  if [[ -n ${APPLE_TEAM_ID:-} && $APPLE_TEAM_ID != $expected_team ]]; then
    print -u2 -- "APPLE_TEAM_ID does not match VIBRA_SIGNING_IDENTITY."
    exit 78
  fi
fi

tag="v$version"
dmg_name="Vibra-$version.dmg"

require() {
  whence -p "$1" >/dev/null || {
    print -u2 -- "missing required tool: $1"
    exit 69
  }
}

require git
require cargo
require hdiutil
(( dry_run )) || require gh

# Use the same checksum-verified Sparkle release for the bridge, bundle and
# appcast signer. Local SwiftPM caches may contain a different version.
pinned_sparkle="$repo_root/third_party/sparkle-$sparkle_version"
appcast_tool="$pinned_sparkle/bin/generate_appcast"
if (( resume_dmg && notarize )); then
  # There is no package step on resume to refresh the signer and framework.
  "$repo_root/Scripts/fetch_sparkle.sh" --refresh >/dev/null
elif [[ ! -d $pinned_sparkle/Sparkle.framework || ! -x $appcast_tool ]]; then
  "$repo_root/Scripts/fetch_sparkle.sh" >/dev/null
fi
if [[ ! -d $pinned_sparkle/Sparkle.framework || ! -x $appcast_tool ]]; then
  print -u2 -- "pinned Sparkle.framework or generate_appcast not found after fetching Sparkle."
  exit 69
fi
export VIBRA_SPARKLE_FRAMEWORK="$pinned_sparkle/Sparkle.framework"

cargo_version=$(
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1
)
if [[ $cargo_version != $version ]]; then
  print -u2 -- "Cargo.toml declares $cargo_version, but the requested release is $version."
  exit 65
fi

dirty_paths=$(git -C "$repo_root" status --porcelain --untracked-files=all)
feed_dirty=0
if [[ -n $dirty_paths ]]; then
  feed_status=$(git -C "$repo_root" status --porcelain --untracked-files=all -- docs/appcast.xml)
  if (( resume_dmg && ! prerelease )) && [[ -n $feed_status && $dirty_paths == $feed_status ]]; then
    # A run may have copied the new feed, then stopped before committing it.
    feed_dirty=1
  else
    print -u2 -- "working tree is dirty; commit or stash before releasing."
    exit 65
  fi
fi

branch=$(git -C "$repo_root" rev-parse --abbrev-ref HEAD)
if (( ! dry_run )) && [[ $branch != main ]]; then
  print -u2 -- "published releases are cut from main; currently on $branch."
  exit 65
fi

head_commit=$(git -C "$repo_root" rev-parse HEAD)
local_tag_commit=
if git -C "$repo_root" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  local_tag_commit=$(git -C "$repo_root" rev-parse "refs/tags/$tag^{commit}")
  if (( ! resume_dmg )); then
    print -u2 -- "tag $tag already exists."
    exit 65
  fi
fi

remote_tag_commit=
if (( ! dry_run )); then
  remote_tags=$(git -C "$repo_root" ls-remote --tags origin "refs/tags/$tag" "refs/tags/$tag^{}")
  remote_tag_commit=$(print -r -- "$remote_tags" | awk -v ref="refs/tags/$tag^{}" '$2 == ref { print $1; exit }')
  if [[ -z $remote_tag_commit ]]; then
    remote_tag_commit=$(print -r -- "$remote_tags" | awk -v ref="refs/tags/$tag" '$2 == ref { print $1; exit }')
  fi
  if [[ -n $remote_tag_commit ]] && (( ! resume_dmg )); then
    print -u2 -- "origin tag $tag already exists."
    exit 65
  fi
fi

if [[ -n $local_tag_commit && -n $remote_tag_commit && $local_tag_commit != $remote_tag_commit ]]; then
  print -u2 -- "Local and origin tags for $tag point to different commits."
  exit 65
fi
existing_tag_commit=${local_tag_commit:-$remote_tag_commit}
if [[ -n $existing_tag_commit && $existing_tag_commit != $head_commit ]]; then
  # A successful stable release adds exactly one appcast commit after its tag.
  # Accept that state so retries after a failed push remain possible, while
  # rejecting a DMG from an unrelated commit or later source changes.
  previous_commit=$(git -C "$repo_root" rev-parse 'HEAD^' 2>/dev/null || true)
  last_subject=$(git -C "$repo_root" log -1 --format=%s HEAD)
  last_paths=$(git -C "$repo_root" diff-tree --no-commit-id --name-only -r HEAD)
  if [[ $previous_commit != $existing_tag_commit \
      || $last_subject != "Publish the Vibra $version appcast" \
      || $last_paths != docs/appcast.xml ]]; then
    print -u2 -- "tag $tag points to $existing_tag_commit, but HEAD is $head_commit."
    exit 65
  fi
fi

source_commit=${existing_tag_commit:-$head_commit}
if [[ $(git -C "$repo_root" rev-parse --is-shallow-repository) == true ]]; then
  print -u2 -- "release builds require a full Git clone; a shallow clone gives an incorrect build number."
  exit 65
fi
source_build=$(git -C "$repo_root" rev-list --count "$source_commit")
if (( ! dry_run )); then
  remote_main=$(
    git -C "$repo_root" ls-remote origin refs/heads/main \
      | awk '$2 == "refs/heads/main" { print $1; exit }'
  )
  if [[ -z $remote_main ]]; then
    print -u2 -- "could not resolve origin/main; publication stopped."
    exit 69
  fi
  if [[ $remote_main != $head_commit && ( $head_commit == $source_commit || $remote_main != $source_commit ) ]]; then
    print -u2 -- "origin/main moved away from this release source; update main before publishing."
    exit 65
  fi
fi

if (( notarize && ! resume_dmg )); then
  if ! security find-identity -v -p codesigning 2>/dev/null \
      | awk -F'"' -v expected="$signing_identity" '$2 == expected { found = 1 } END { exit !found }'; then
    print -u2 -- "VIBRA_SIGNING_IDENTITY is not a valid identity in the keychain: $signing_identity"
    exit 78
  fi
  if ! xcrun notarytool history --keychain-profile "$APPLE_KEYCHAIN_PROFILE" >/dev/null 2>&1; then
    print -u2 -- "notarization profile '$APPLE_KEYCHAIN_PROFILE' is missing or invalid."
    print -u2 -- "Create it with: xcrun notarytool store-credentials \"$APPLE_KEYCHAIN_PROFILE\""
    print -u2 -- "Use --dry-run --no-notarize for local packaging without notarization."
    exit 78
  fi
fi

verify_developer_signature() {
  local artifact=$1
  local verify_args=(--verify --strict)
  [[ $artifact == *.app ]] && verify_args+=(--deep)
  codesign "${verify_args[@]}" "$artifact" >/dev/null
  local details=$(codesign -dv --verbose=4 "$artifact" 2>&1)
  if ! print -r -- "$details" | grep -Fx "Authority=$signing_identity" >/dev/null \
      || ! print -r -- "$details" | grep -Fx "TeamIdentifier=$expected_team" >/dev/null; then
    print -u2 -- "$artifact was not signed by $signing_identity (Team $expected_team)."
    return 65
  fi
}

verify_bundle_metadata() {
  local bundled_plist=$1
  local actual_version actual_build actual_source actual_bundle_id expected_bundle_id
  actual_version=$(plutil -extract CFBundleShortVersionString raw -o - "$bundled_plist")
  actual_build=$(plutil -extract CFBundleVersion raw -o - "$bundled_plist")
  actual_source=$(plutil -extract VibraSourceCommit raw -o - "$bundled_plist" 2>/dev/null || true)
  actual_bundle_id=$(plutil -extract CFBundleIdentifier raw -o - "$bundled_plist")
  expected_bundle_id=$(plutil -extract CFBundleIdentifier raw -o - "$repo_root/Resources/Info.plist")
  if [[ $actual_version != $version || $actual_build != $source_build \
      || $actual_source != $source_commit || $actual_bundle_id != $expected_bundle_id ]]; then
    print -u2 -- \
      "The DMG contains $actual_bundle_id $actual_version build $actual_build commit $actual_source;" \
      "expected $expected_bundle_id $version build $source_build commit $source_commit."
    return 65
  fi
}

verify_resumed_dmg() (
  local dmg=$1
  verify_developer_signature "$dmg"
  local mount_point=$(mktemp -d)
  local mounted=0
  cleanup_mount() {
    if (( mounted )); then
      hdiutil detach -quiet "$mount_point" || print -u2 -- "Could not detach $mount_point"
    fi
    rmdir "$mount_point" 2>/dev/null || true
  }
  trap cleanup_mount EXIT
  hdiutil attach -readonly -nobrowse -quiet -mountpoint "$mount_point" "$dmg"
  mounted=1

  local bundled_plist="$mount_point/Vibra.app/Contents/Info.plist"
  if [[ ! -f $bundled_plist ]]; then
    print -u2 -- "The DMG has no Vibra.app/Contents/Info.plist: $dmg"
    exit 65
  fi
  verify_bundle_metadata "$bundled_plist"
  verify_developer_signature "$mount_point/Vibra.app"
)

resume_source=
if (( resume_dmg )); then
  if (( ! notarize )); then
    print -u2 -- "--resume-dmg requires a notarized DMG; do not combine it with --no-notarize."
    exit 64
  fi
  dist_dmg="$repo_root/dist/Vibra.dmg"
  staged_dmg="$staging_dir/$dmg_name"
  if [[ -f $dist_dmg && -f $staged_dmg ]] && ! cmp -s "$dist_dmg" "$staged_dmg"; then
    print -u2 -- "Two different DMGs exist at $dist_dmg and $staged_dmg; choose one before resuming."
    exit 65
  fi
  if [[ -f $dist_dmg ]]; then
    resume_source=$dist_dmg
  elif [[ -f $staged_dmg ]]; then
    resume_source=$staged_dmg
  else
    print -u2 -- "--resume-dmg needs $dist_dmg or $staged_dmg."
    exit 66
  fi
  verify_resumed_dmg "$resume_source"
  if ! xcrun stapler validate "$resume_source"; then
    submission_record="$repo_root/dist/notarization/Vibra.dmg.submission-id"
    if [[ ! -f $submission_record ]]; then
      print -u2 -- "The DMG is not stapled and no notarization submission ID was found at $submission_record."
      exit 65
    fi
    submission_id=$(< "$submission_record")
    if [[ ! $submission_id =~ '^[0-9A-Fa-f-]{36}$' ]]; then
      print -u2 -- "Invalid notarization submission ID in $submission_record."
      exit 65
    fi
    notary_response=$(
      xcrun notarytool info "$submission_id" \
        --keychain-profile "$APPLE_KEYCHAIN_PROFILE" --output-format json
    )
    notary_status=$(print -r -- "$notary_response" | plutil -extract status raw -o - -)
    if [[ $notary_status != Accepted ]]; then
      print -u2 -- "Notarization is $notary_status for $submission_id; resume after Apple accepts it."
      exit 65
    fi
    xcrun stapler staple "$resume_source"
    xcrun stapler validate "$resume_source"
  fi
  spctl --assess --type open --context context:primary-signature --verbose=2 "$resume_source"
fi

notes_markdown=$(
  awk -v version="$version" '
    # Compare text literally: dots in a version must not act as regex wildcards.
    {
      heading = "## " version
      suffix = substr($0, length(heading) + 1)
      if (substr($0, 1, length(heading)) == heading && (suffix == "" || substr(suffix, 1, 1) == " ")) {
        capture = 1
        next
      }
    }
    capture && /^## / { exit }
    capture { print }
  ' "$repo_root/CHANGELOG.md"
)
if [[ -z ${notes_markdown//[[:space:]]/} ]]; then
  print -u2 -- "CHANGELOG.md has no '## $version' section."
  exit 65
fi

channel=stable
(( prerelease )) && channel=prerelease
print "building Vibra $version ($channel) from $(git -C "$repo_root" rev-parse --short HEAD)"

if (( resume_dmg )); then
  print "reusing notarized DMG at $resume_source"
else
  package_args=(release --universal --dmg)
  if (( notarize )); then
    package_args+=(--notarize --sign "$signing_identity")
  else
    package_args+=(--sign -)
  fi
  VIBRA_MARKETING_VERSION=$version VIBRA_BUILD_VERSION=$source_build \
    VIBRA_SOURCE_COMMIT=$source_commit "$repo_root/Scripts/package_app.sh" "${package_args[@]}"
  if (( notarize )); then
    verify_resumed_dmg "$repo_root/dist/Vibra.dmg"
  fi
fi

if [[ $resume_source == "$staging_dir/$dmg_name" ]]; then
  # A previous run already moved the DMG. Preserve only that validated image;
  # generate_appcast must not include an older release left in this directory.
  for entry in "$staging_dir"/*(DN); do
    [[ $entry == "$resume_source" ]] || rm -rf -- "$entry"
  done
else
  rm -rf "$staging_dir"
  mkdir -p "$staging_dir"
  mv "$repo_root/dist/Vibra.dmg" "$staging_dir/$dmg_name"
fi

# generate_appcast picks up a sibling HTML file as the release notes shown in
# Sparkle's update dialog.
print -r -- "$notes_markdown" | awk '
  BEGIN { print "<ul>" }
  /^- / {
    line = substr($0, 3)
    gsub(/&/, "\\&amp;", line)
    gsub(/</, "\\&lt;", line)
    printf "  <li>%s</li>\n", line
  }
  END { print "</ul>" }
' > "$staging_dir/Vibra-$version.html"

print "signing the appcast entry"
"$appcast_tool" \
  --download-url-prefix "$download_prefix/$tag/" \
  --link "https://github.com/$repo_slug/releases/tag/$tag" \
  "$staging_dir"
if (( feed_dirty )) && ! cmp -s "$feed_dir/appcast.xml" "$staging_dir/appcast.xml"; then
  print -u2 -- "The uncommitted docs/appcast.xml differs from the regenerated feed; resolve it before resuming."
  exit 65
fi

if (( dry_run )); then
  print
  print "dry run: nothing published"
  print "disk image: $staging_dir/$dmg_name"
  print "appcast:    $staging_dir/appcast.xml"
  exit 0
fi

release_exists=0
if release_response=$(gh api -i "repos/$repo_slug/releases/tags/$tag" 2>/dev/null); then
  release_exists=1
else
  response_status=$(print -r -- "$release_response" | awk 'NR == 1 && $1 ~ /^HTTP\// { print $2 }')
  if [[ $response_status != 404 ]]; then
    print -u2 -- "Could not check whether GitHub release $tag exists; publication stopped."
    exit 69
  fi
fi

if [[ -z $local_tag_commit && -z $remote_tag_commit ]]; then
  git -C "$repo_root" tag -a "$tag" -m "Vibra $version"
fi
if [[ -z $remote_tag_commit ]]; then
  git -C "$repo_root" push origin "$tag"
fi

verify_published_asset() (
  local download_dir=$(mktemp -d)
  trap 'rm -rf "$download_dir"' EXIT
  gh release download "$tag" --repo "$repo_slug" --pattern "$dmg_name" --dir "$download_dir"
  if ! cmp -s "$download_dir/$dmg_name" "$staging_dir/$dmg_name"; then
    print -u2 -- "The published $dmg_name differs from the validated local DMG."
    exit 65
  fi
)

if (( release_exists )); then
  published_channel=$(gh release view "$tag" --repo "$repo_slug" --json isPrerelease --jq '.isPrerelease')
  expected_channel=false
  (( prerelease )) && expected_channel=true
  if [[ $published_channel != $expected_channel ]]; then
    print -u2 -- "GitHub release $tag has isPrerelease=$published_channel; expected $expected_channel."
    exit 65
  fi
  asset_names=$(gh release view "$tag" --repo "$repo_slug" --json assets --jq '.assets[].name')
  if print -r -- "$asset_names" | grep -Fx "$dmg_name" >/dev/null; then
    verify_published_asset
    print "reusing the existing GitHub release and verified DMG"
  else
    print "uploading the missing DMG to the existing GitHub release"
    gh release upload "$tag" "$staging_dir/$dmg_name" --repo "$repo_slug"
  fi
else
  print "creating the GitHub release"
  gh_args=(
    release create "$tag"
    --repo "$repo_slug"
    --title "Vibra $version"
    --notes "$notes_markdown"
    "$staging_dir/$dmg_name"
  )
  if (( prerelease )); then
    gh_args+=(--prerelease)
  else
    gh_args+=(--latest)
  fi
  gh "${gh_args[@]}"
fi

if (( ! prerelease )); then
  # Only now that the download resolves is it safe to point the feed at it.
  mkdir -p "$feed_dir"
  cp "$staging_dir/appcast.xml" "$feed_dir/appcast.xml"
  git -C "$repo_root" add "$feed_dir/appcast.xml"
  if [[ -n $(git -C "$repo_root" status --porcelain -- "$feed_dir/appcast.xml") ]]; then
    git -C "$repo_root" commit -m "Publish the Vibra $version appcast"
  fi
  git -C "$repo_root" push origin main
  print
  print "published Vibra $version as the stable release"
  print "download: $download_prefix/$tag/$dmg_name"
  print "feed:     https://${repo_slug%%/*}.github.io/${repo_slug##*/}/appcast.xml"
else
  print
  print "published Vibra $version as a prerelease (Sparkle feed unchanged)"
  print "download: $download_prefix/$tag/$dmg_name"
fi
exit 0
