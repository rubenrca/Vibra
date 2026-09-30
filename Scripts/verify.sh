#!/bin/zsh

set -euo pipefail

script_name=${0:A}
repo_root=${script_name:h:h}

cd "$repo_root"
cargo fmt --check
cargo test --locked
cargo test --locked --manifest-path third_party/block/Cargo.toml
cargo clippy --locked --all-targets --all-features -- -D warnings
python3 Scripts/test_release.py
plutil -lint Resources/Info.plist Resources/Vibra.entitlements
for script in Scripts/*.{sh,zsh}; do
  zsh -n "$script"
done
