#!/bin/zsh

set -euo pipefail

# Publishes a Vibra release: builds the universal disk image, signs an appcast
# entry for it with the Sparkle EdDSA key, uploads the image to GitHub Releases
# and pushes the feed that installed copies poll.
#
# The feed is published only after the image is downloadable, so an interrupted
# run can never point Sparkle at a URL that 404s.

script_name=${0:A}
repo_root=${script_name:h:h}
dry_run=0
notarize=0
version=

repo_slug=${VIBRA_REPO_SLUG:-rubenrca/Vibra}
download_prefix="https://github.com/$repo_slug/releases/download"
feed_dir="$repo_root/docs"
staging_dir="$repo_root/dist/appcast"

usage() {
  print -u2 -- "usage: $script_name <version> [--notarize] [--dry-run]"
  print -u2 --
  print -u2 -- "  <version>    marketing version without a leading v, e.g. 0.2.0"
  print -u2 -- "  --notarize   submit the app and disk image to Apple before publishing"
  print -u2 -- "  --dry-run    build and sign everything, publish nothing"
  print -u2 --
  print -u2 -- "Requires the GitHub CLI, and the Sparkle private key in the keychain."
  exit 64
}

while (( $# )); do
  case "$1" in
    --notarize) notarize=1 ;;
    --dry-run) dry_run=1 ;;
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

if [[ ! $version =~ '^[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$' ]]; then
  print -u2 -- "version must look like 1.2.3 or 1.2.3-beta.1, got: $version"
  exit 64
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
require hdiutil
(( dry_run )) || require gh

# Sparkle ships generate_appcast inside the artifact SwiftPM downloads, so a
# build has to have happened at least once for it to exist.
appcast_tool=$(
  print -r -- "$repo_root"/.build/artifacts/sparkle/Sparkle/bin/generate_appcast(N)
)
if [[ -z $appcast_tool ]]; then
  print -u2 -- "generate_appcast not found. Run 'swift build' once to fetch Sparkle."
  exit 69
fi

if [[ -n $(git -C "$repo_root" status --porcelain) ]]; then
  print -u2 -- "working tree is dirty; commit or stash before releasing."
  exit 65
fi

if git -C "$repo_root" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  print -u2 -- "tag $tag already exists."
  exit 65
fi

branch=$(git -C "$repo_root" rev-parse --abbrev-ref HEAD)
if [[ $branch != main ]]; then
  print -u2 -- "releases are cut from main; currently on $branch."
  exit 65
fi

print "releasing Vibra $version from $(git -C "$repo_root" rev-parse --short HEAD)"

package_args=(release --universal --dmg)
(( notarize )) && package_args+=(--notarize)

VIBRA_MARKETING_VERSION=$version "$repo_root/Scripts/package_app.sh" "${package_args[@]}"

rm -rf "$staging_dir"
mkdir -p "$staging_dir"
mv "$repo_root/dist/Vibra.dmg" "$staging_dir/$dmg_name"

# generate_appcast picks up a sibling HTML file as the release notes shown in
# Sparkle's update dialog. Take them from the matching CHANGELOG section so the
# changelog stays the single place they are written.
notes_markdown=$(
  awk -v version="$version" '
    $0 ~ "^## " version { capture = 1; next }
    capture && /^## / { exit }
    capture { print }
  ' "$repo_root/CHANGELOG.md"
)

if [[ -z ${notes_markdown//[[:space:]]/} ]]; then
  print -u2 -- "CHANGELOG.md has no '## $version' section; add one before releasing."
  exit 65
fi

# Sparkle renders a fragment, not a document. Bullets are the only markup the
# changelog uses, so translating them is enough and keeps this dependency-free.
print -r -- "$notes_markdown" | awk '
  BEGIN { print "<ul>" }
  /^- / { line = substr($0, 3); gsub(/&/, "\\&amp;", line); gsub(/</, "\\&lt;", line); printf "  <li>%s</li>\n", line }
  END { print "</ul>" }
' > "$staging_dir/Vibra-$version.html"

print "signing the appcast entry"
"$appcast_tool" \
  --download-url-prefix "$download_prefix/$tag/" \
  --link "https://github.com/$repo_slug/releases/tag/$tag" \
  "$staging_dir"

if (( dry_run )); then
  print
  print "dry run: nothing published"
  print "disk image: $staging_dir/$dmg_name"
  print "appcast:    $staging_dir/appcast.xml"
  exit 0
fi

git -C "$repo_root" tag -a "$tag" -m "Vibra $version"
git -C "$repo_root" push origin "$tag"

print "creating the GitHub release"
gh release create "$tag" \
  --repo "$repo_slug" \
  --title "Vibra $version" \
  --notes "$notes_markdown" \
  "$staging_dir/$dmg_name"

# Only now that the download resolves is it safe to point the feed at it.
mkdir -p "$feed_dir"
cp "$staging_dir/appcast.xml" "$feed_dir/appcast.xml"
git -C "$repo_root" add "$feed_dir/appcast.xml"
git -C "$repo_root" commit -m "Publish the Vibra $version appcast"
git -C "$repo_root" push origin main

print
print "published Vibra $version"
print "download: $download_prefix/$tag/$dmg_name"
print "feed:     https://${repo_slug%%/*}.github.io/${repo_slug##*/}/appcast.xml"
exit 0
