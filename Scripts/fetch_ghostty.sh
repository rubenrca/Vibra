#!/bin/zsh
# Explicit dependency preparation; Cargo itself never downloads or builds Zig.
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
revision=492300cad104195411d12217dd22f1cd05f31376
source_dir="$repo_root/.build/ghostty/source"
zig_bin=${ZIG:-}
if [[ -z $zig_bin ]]; then
  host_arch=$(uname -m)
  case "$host_arch" in
    arm64) host_arch=aarch64; zig_sha=b23d70deaa879b5c2d486ed3316f7eaa53e84acf6fc9cc747de152450d401489 ;;
    x86_64) zig_sha=0387557ed1877bc6a2e1802c8391953baddba76081876301c522f52977b52ba7 ;;
    *) print -u2 -- 'Only macOS arm64/x86_64 hosts are supported.'; exit 1 ;;
  esac
  toolchain="$repo_root/.build/ghostty/toolchain"
  zig_bin="$toolchain/zig-$host_arch-macos-0.16.0/zig"
  if [[ ! -x $zig_bin ]]; then
    mkdir -p "$toolchain"
    archive=$(mktemp "$toolchain/zig.XXXXXX")
    trap 'rm -f "$archive"' EXIT
    curl --fail --location --retry 3 "https://ziglang.org/download/0.16.0/zig-$host_arch-macos-0.16.0.tar.xz" -o "$archive"
    if [[ $(shasum -a 256 "$archive" | cut -d ' ' -f 1) != $zig_sha ]]; then
      print -u2 -- 'Zig archive checksum mismatch'; exit 1
    fi
    tar -xJf "$archive" -C "$toolchain"
    rm -f "$archive"
    trap - EXIT
  fi
fi
arch=${1:-$(uname -m)}
case "$arch" in
  arm64|aarch64) arch=aarch64 ;;
  x86_64) ;;
  *) print -u2 -- "usage: $script_name [aarch64|x86_64]"; exit 64 ;;
esac
if [[ $("$zig_bin" version) != 0.16.0 ]]; then
  print -u2 -- 'Ghostty requires Zig 0.16.0. Set ZIG to that executable.'
  exit 1
fi
mkdir -p "${source_dir:h}"
if [[ ! -d "$source_dir/.git" ]]; then
  git init "$source_dir"
  git -C "$source_dir" remote add origin https://github.com/ghostty-org/ghostty.git
fi
if ! git -C "$source_dir" cat-file -e "$revision^{commit}" 2>/dev/null; then
  git -C "$source_dir" fetch --depth 1 origin "$revision"
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
print -- "Ghostty $revision ready for $arch. Run cargo run --features ghostty."
