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
notarize=0
prerelease=0
version=
repo_slug=${VIBRA_REPO_SLUG:-rubenrca/Vibra}
download_prefix="https://github.com/$repo_slug/releases/download"
feed_dir="$repo_root/docs"
staging_dir="$repo_root/dist/appcast"

usage() {
  print -u2 -- "usage: $script_name <version> [--prerelease] [--notarize] [--dry-run]"
  print -u2 --
  print -u2 -- "  <version>      marketing version, e.g. 0.3.0 or 0.3.1-beta.1"
  print -u2 -- "  --prerelease   GitHub prerelease only; does not update docs/appcast.xml"
  print -u2 -- "  --notarize     submit the app and disk image to Apple"
  print -u2 -- "  --dry-run      build and sign everything, publish nothing"
  print -u2 --
  print -u2 -- "Stable releases update the Sparkle feed after the DMG is live on GitHub."
  print -u2 -- "Requires the GitHub CLI and the Sparkle EdDSA private key in the keychain."
  exit 64
}

while (( $# )); do
  case "$1" in
    --notarize) notarize=1 ;;
    --dry-run) dry_run=1 ;;
    --prerelease) prerelease=1 ;;
    --stable)
      # Accepted for backwards compatibility with earlier scripts; stable is default.
      prerelease=0
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
if [[ $version == *-* ]]; then
  prerelease=1
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

# Sparkle ships generate_appcast inside the artifact SwiftPM downloads.
appcast_tool=$(
  print -r -- "$repo_root"/.build/artifacts/sparkle/Sparkle/bin/generate_appcast(N)
)
if [[ -z $appcast_tool ]]; then
  appcast_tool=$(print -r -- "$repo_root"/.build/checkouts/Sparkle/bin/generate_appcast(N))
fi
if [[ -z $appcast_tool || ! -x $appcast_tool ]]; then
  print -u2 -- "generate_appcast not found under .build/artifacts/sparkle."
  print -u2 -- "Restore the Sparkle tools (prior SPM fetch) before releasing."
  exit 69
fi

cargo_version=$(
  sed -n 's/^version = "\([^"]*\)"/\1/p' "$repo_root/Cargo.toml" | head -n 1
)
if [[ $cargo_version != $version ]]; then
  print -u2 -- "Cargo.toml declares $cargo_version, but the requested release is $version."
  exit 65
fi

if [[ -n $(git -C "$repo_root" status --porcelain) ]]; then
  print -u2 -- "working tree is dirty; commit or stash before releasing."
  exit 65
fi

branch=$(git -C "$repo_root" rev-parse --abbrev-ref HEAD)
if (( ! dry_run )) && [[ $branch != main ]]; then
  print -u2 -- "published releases are cut from main; currently on $branch."
  exit 65
fi

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

channel=stable
(( prerelease )) && channel=prerelease
print "building Vibra $version ($channel) from $(git -C "$repo_root" rev-parse --short HEAD)"

package_args=(release --universal --dmg)
(( notarize )) && package_args+=(--notarize)
VIBRA_MARKETING_VERSION=$version "$repo_root/Scripts/package_app.sh" "${package_args[@]}"

rm -rf "$staging_dir"
mkdir -p "$staging_dir"
mv "$repo_root/dist/Vibra.dmg" "$staging_dir/$dmg_name"

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

if (( ! prerelease )); then
  # Only now that the download resolves is it safe to point the feed at it.
  mkdir -p "$feed_dir"
  cp "$staging_dir/appcast.xml" "$feed_dir/appcast.xml"
  git -C "$repo_root" add "$feed_dir/appcast.xml"
  if [[ -n $(git -C "$repo_root" status --porcelain -- "$feed_dir/appcast.xml") ]]; then
    git -C "$repo_root" commit -m "Publish the Vibra $version appcast"
    git -C "$repo_root" push origin main
  fi
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
