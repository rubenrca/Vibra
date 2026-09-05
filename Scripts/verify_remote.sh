#!/bin/zsh
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
cd "$repo_root"
cargo fmt --manifest-path services/Cargo.toml --check
cargo test --locked --manifest-path services/Cargo.toml
cargo clippy --locked --manifest-path services/Cargo.toml --all-targets -- -D warnings
fixtures=$(mktemp)
trap 'rm -f "$fixtures"' EXIT
cargo run --quiet --locked --manifest-path services/Cargo.toml -p vibra-remote-protocol --example fixtures > "$fixtures"
VIBRA_PROTOCOL_FIXTURES="$fixtures" swift test --package-path ios/VibraRemoteProtocol
