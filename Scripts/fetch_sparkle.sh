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

if [[ -d $framework && -x $tools_bin/generate_appcast ]]; then
  print -r -- "$framework"
  exit 0
fi

tmpdir=$(mktemp -d)
trap 'rm -rf "$tmpdir"' EXIT
url="https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz"
print -u2 -- "fetching Sparkle $version…"
curl -fsSL "$url" -o "$tmpdir/sparkle.tar.xz"
if [[ -z $expected_sha256 ]]; then
  case "$version" in
    2.9.4) expected_sha256=ce89daf967db1e1893ed3ebd67575ed82d3902563e3191ca92aaec9164fbdef9 ;;
    *)
      print -u2 -- "set VIBRA_SPARKLE_SHA256 when overriding VIBRA_SPARKLE_VERSION"
      exit 64
      ;;
  esac
fi
actual_sha256=$(shasum -a 256 "$tmpdir/sparkle.tar.xz" | awk '{ print $1 }')
if [[ $actual_sha256 != $expected_sha256 ]]; then
  print -u2 -- "Sparkle archive checksum mismatch"
  print -u2 -- "expected: $expected_sha256"
  print -u2 -- "actual:   $actual_sha256"
  exit 65
fi
mkdir -p "$dest_root"
tar -xJf "$tmpdir/sparkle.tar.xz" -C "$dest_root"
if [[ ! -d $framework ]]; then
  print -u2 -- "Sparkle.framework missing after extract"
  exit 70
fi
print -r -- "$framework"
