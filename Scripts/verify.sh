#!/bin/zsh

set -euo pipefail

script_name=${0:A}
repo_root=${script_name:h:h}

cd "$repo_root"
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets --all-features -- -D warnings
plutil -lint Resources/Info.plist Resources/Vibra.entitlements
zsh -n Scripts/package_app.sh Scripts/release.sh
