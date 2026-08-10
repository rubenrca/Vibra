#!/bin/zsh

# Downloads Sparkle 2.9.4 into third_party/ so package/release builds work
# without a prior SwiftPM checkout.

set -euo pipefail

script_name=${0:A}
repo_root=${script_name:h:h}
version=${VIBRA_SPARKLE_VERSION:-2.9.4}
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
mkdir -p "$dest_root"
tar -xJf "$tmpdir/sparkle.tar.xz" -C "$dest_root"
if [[ ! -d $framework ]]; then
  print -u2 -- "Sparkle.framework missing after extract"
  exit 70
fi
print -r -- "$framework"
