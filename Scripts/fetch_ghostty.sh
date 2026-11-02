#!/bin/zsh
# Explicit dependency preparation; Cargo itself never downloads or builds Zig.
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
revision=7b267cd9ac7863b0bd72e808daa021381e72dc21
source_dir="$repo_root/.build/ghostty/source"
zig_bin=${ZIG:-}
if [[ -z $zig_bin ]]; then
  host_arch=$(uname -m)
  case "$host_arch" in
    arm64)
      host_arch=aarch64
      zig_sha=b607e9b9234790a008116ae5bdb71c6243b84b9fb42a53a9e70fde41c06c536a
      ;;
    x86_64)
      zig_sha=4f9a1c5269aa17ebda5e6d3c2b89d6cbf36f7d2b22a0306e9ab98f25f95529c6
      ;;
    *)
      print -u2 -- 'Only macOS arm64/x86_64 hosts are supported.'
      exit 1
      ;;
  esac
  toolchain="$repo_root/.build/ghostty/toolchain"
  zig_bin="$toolchain/zig-$host_arch-macos-0.17.0/zig"
  if [[ ! -x $zig_bin ]]; then
    mkdir -p "$toolchain"
    archive=$(mktemp "$toolchain/zig.XXXXXX")
    trap 'rm -f "$archive"' EXIT
    curl --fail --location --retry 3 \
      "https://ziglang.org/download/0.17.0/zig-$host_arch-macos-0.17.0.tar.xz" \
      -o "$archive"
    if [[ $(shasum -a 256 "$archive" | cut -d ' ' -f 1) != $zig_sha ]]; then
      print -u2 -- 'Zig archive checksum mismatch'
      exit 1
    fi
    tar -xJf "$archive" -C "$toolchain"
    rm -f "$archive"
    trap - EXIT
  fi
fi
arch=${1:-$(uname -m)}
case "$arch" in
  arm64|aarch64)
    arch=aarch64
    ;;
  x86_64) ;;
  *)
    print -u2 -- "usage: $script_name [aarch64|x86_64]"
    exit 64
    ;;
esac
if [[ $("$zig_bin" version) != 0.17.0 ]]; then
  print -u2 -- 'Ghostty requires Zig 0.17.0. Set ZIG to that executable.'
  exit 1
fi
mkdir -p "${source_dir:h}"
if [[ ! -d "$source_dir/.git" ]]; then
  git init "$source_dir"
fi
# Fetch by URL so a stale or local `origin` in an old checkout cannot block updates.
if ! git -C "$source_dir" cat-file -e "$revision^{commit}" 2>/dev/null; then
  git -C "$source_dir" fetch --depth 1 https://github.com/ghostty-org/ghostty.git "$revision"
fi
# Refuse to overwrite local upstream edits.
if [[ -n $(git -C "$source_dir" status --porcelain --untracked-files=no) ]]; then
  print -u2 -- "Local edits in $source_dir; restore or relocate them before preparing Ghostty."
  exit 1
fi
git -C "$source_dir" checkout --detach "$revision"
cd "$source_dir"
"$zig_bin" build -Demit-lib-vt -Demit-xcframework=false -Doptimize=ReleaseFast \
  "-Dtarget=$arch-macos.14.0" --prefix "$repo_root/.build/ghostty/$arch"
print -- "Ghostty $revision ready for $arch. Run cargo run."
