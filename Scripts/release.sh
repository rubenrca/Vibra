#!/bin/zsh

set -euo pipefail

# GPUI releases start on a GitHub prerelease channel. The stable Sparkle feed
# remains pinned to the final Swift build until Vibra has an updater again.

script_name=${0:A}
repo_root=${script_name:h:h}
dry_run=0
notarize=0
stable=0
version=
repo_slug=${VIBRA_REPO_SLUG:-rubenrca/Vibra}

usage() {
  print -u2 -- "usage: $script_name <version> [--notarize] [--dry-run]"
  print -u2 --
  print -u2 -- "  <version>    prerelease version, e.g. 0.3.0-beta.1"
  print -u2 -- "  --notarize   submit the app and disk image to Apple"
  print -u2 -- "  --dry-run    build and validate everything, publish nothing"
  print -u2 --
  print -u2 -- "Stable publication is intentionally disabled until the GPUI app"
  print -u2 -- "contains an updater compatible with the existing Sparkle channel."
  exit 64
}

while (( $# )); do
  case "$1" in
    --notarize) notarize=1 ;;
    --dry-run) dry_run=1 ;;
    --stable) stable=1 ;;
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

if [[ ! $version =~ '^[0-9]+\.[0-9]+\.[0-9]+-[0-9A-Za-z.-]+$' ]]; then
  print -u2 -- "GPUI releases must be prereleases such as 0.3.0-beta.1; got: $version"
  exit 64
fi

if (( stable )); then
  print -u2 -- "Stable releases are blocked until the GPUI build has an updater."
  print -u2 -- "The existing docs/appcast.xml must remain on Vibra 0.2.7."
  exit 78
fi

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

if [[ -n $(git -C "$repo_root" status --porcelain) ]]; then
  print -u2 -- "working tree is dirty; commit or stash before releasing."
  exit 65
fi

branch=$(git -C "$repo_root" rev-parse --abbrev-ref HEAD)
if (( ! dry_run )) && [[ $branch != main ]]; then
  print -u2 -- "published releases are cut from main; currently on $branch."
  exit 65
fi

tag="v$version"
if git -C "$repo_root" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  print -u2 -- "tag $tag already exists."
  exit 65
fi

notes_markdown=$(
  awk -v version="$version" '
    $0 ~ "^## " version { capture = 1; next }
    capture && /^## / { exit }
    capture { print }
  ' "$repo_root/CHANGELOG.md"
)
if [[ -z ${notes_markdown//[[:space:]]/} ]]; then
  print -u2 -- "CHANGELOG.md has no '## $version' section."
  exit 65
fi

print "building Vibra $version prerelease from $(git -C "$repo_root" rev-parse --short HEAD)"
package_args=(release --universal --dmg)
(( notarize )) && package_args+=(--notarize)
VIBRA_MARKETING_VERSION=$version "$repo_root/Scripts/package_app.sh" "${package_args[@]}"

dmg_path="$repo_root/dist/Vibra.dmg"
versioned_dmg="$repo_root/dist/Vibra-$version.dmg"
rm -f "$versioned_dmg"
mv "$dmg_path" "$versioned_dmg"

if (( dry_run )); then
  print
  print "dry run: nothing published"
  print "$versioned_dmg"
  exit 0
fi

git -C "$repo_root" tag -a "$tag" -m "Vibra $version"
git -C "$repo_root" push origin "$tag"

print "creating the GitHub prerelease"
gh release create "$tag" \
  --repo "$repo_slug" \
  --prerelease \
  --title "Vibra $version" \
  --notes "$notes_markdown" \
  "$versioned_dmg"

print
print "published Vibra $version as a prerelease"
print "legacy Sparkle feed unchanged: https://${repo_slug%%/*}.github.io/${repo_slug##*/}/appcast.xml"
exit 0
