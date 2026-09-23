#!/bin/zsh

# Downloads Sparkle 2.9.4 into third_party/ so package/release builds work
# without a prior SwiftPM checkout.

set -euo pipefail

script_name=${0:A}
repo_root=${script_name:h:h}
version=${VIBRA_SPARKLE_VERSION:-2.9.4}
expected_sha256=${VIBRA_SPARKLE_SHA256:-}
dest_root="$repo_root/third_party/sparkle-$version"
framework="$dest_root/Sparkle.framework"
tools_bin="$dest_root/bin"
refresh=0

if (( $# )); then
  if [[ $# != 1 || $1 != --refresh ]]; then
    print -u2 -- "usage: $script_name [--refresh]"
    exit 64
  fi
  refresh=1
fi

if [[ -L $dest_root ]]; then
  print -u2 -- "Sparkle cache path must not be a symlink: $dest_root"
  exit 65
fi

if (( ! refresh )) && [[ -d $framework && -x $tools_bin/generate_appcast ]]; then
  print -r -- "$framework"
  exit 0
fi

case "$version" in
  2.9.4)
    pinned_sha256=ce89daf967db1e1893ed3ebd67575ed82d3902563e3191ca92aaec9164fbdef9
    if [[ -n $expected_sha256 && $expected_sha256 != $pinned_sha256 ]]; then
      print -u2 -- "Sparkle 2.9.4 uses a pinned checksum; VIBRA_SPARKLE_SHA256 must match it."
      exit 64
    fi
    expected_sha256=$pinned_sha256
    ;;
  *)
    if [[ -z $expected_sha256 ]]; then
      print -u2 -- "set VIBRA_SPARKLE_SHA256 when overriding VIBRA_SPARKLE_VERSION"
      exit 64
    fi
    ;;
esac

mkdir -p "$repo_root/third_party"
tmpdir=$(mktemp -d "$repo_root/third_party/.sparkle-$version.XXXXXX")
trap 'rm -rf "$tmpdir"' EXIT
url="https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz"
print -u2 -- "fetching Sparkle $version…"
curl -fsSL "$url" -o "$tmpdir/sparkle.tar.xz"
actual_sha256=$(shasum -a 256 "$tmpdir/sparkle.tar.xz" | awk '{ print $1 }')
if [[ $actual_sha256 != $expected_sha256 ]]; then
  print -u2 -- "Sparkle archive checksum mismatch"
  print -u2 -- "expected: $expected_sha256"
  print -u2 -- "actual:   $actual_sha256"
  exit 65
fi
mkdir "$tmpdir/unpacked"
tar -xJf "$tmpdir/sparkle.tar.xz" -C "$tmpdir/unpacked"
if [[ ! -d $tmpdir/unpacked/Sparkle.framework || ! -x $tmpdir/unpacked/bin/generate_appcast ]]; then
  print -u2 -- "Sparkle.framework or generate_appcast missing after extract"
  exit 70
fi
backup="$repo_root/third_party/.sparkle-$version.backup-$$"
if [[ -e $dest_root ]]; then
  mv "$dest_root" "$backup"
fi
if ! mv "$tmpdir/unpacked" "$dest_root"; then
  if [[ -e $backup ]]; then
    mv "$backup" "$dest_root"
  fi
  print -u2 -- "could not replace the Sparkle cache"
  exit 70
fi
if [[ -e $backup ]]; then
  rm -rf "$backup"
fi
print -r -- "$framework"
