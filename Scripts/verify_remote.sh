#!/bin/zsh
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
cd "$repo_root"
cargo fmt --manifest-path services/Cargo.toml --all --check
cargo test --locked --manifest-path services/Cargo.toml
cargo clippy --locked --manifest-path services/Cargo.toml --all-targets -- -D warnings
fixtures=$(mktemp)
noise_fixture=$(mktemp)
screen_fixture=$(mktemp)
trap 'rm -f "$fixtures" "$noise_fixture" "$screen_fixture"' EXIT
cargo run --quiet --locked --manifest-path services/Cargo.toml -p vibra-remote-protocol --example fixtures > "$fixtures"
cargo run --quiet --locked --manifest-path services/Cargo.toml -p vibra-remote --example interop > "$noise_fixture"
cmp "$noise_fixture" ios/VibraRemoteProtocol/Tests/VibraRemoteProtocolTests/Fixtures/noise.json
VIBRA_SCREEN_FIXTURE="$screen_fixture" cargo test --locked remote_export_preserves
cmp "$screen_fixture" ios/VibraRemoteProtocol/Tests/VibraRemoteProtocolTests/Fixtures/screen.json
VIBRA_PROTOCOL_FIXTURES="$fixtures" swift test --package-path ios/VibraRemoteProtocol

cargo test --locked remote_local_
