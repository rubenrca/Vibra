#!/bin/zsh
# Local test: relay + native iPhone simulator + the repository's Mac build.
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
cd "$repo_root"
mkdir -p .build/remote-test
cargo build --locked --manifest-path services/Cargo.toml -p vibra-relay
./Scripts/build_ios.sh simulator > .build/remote-test/ios-build.log 2>&1
./Scripts/package_app.sh release --sign - > .build/remote-test/mac-build.log 2>&1
if ! curl --fail --silent http://127.0.0.1:8787/health > .build/remote-test/health.json; then
  nohup "$repo_root/services/target/debug/vibra-relay" > .build/remote-test/relay.log 2>&1 &
  print -r -- $! > .build/remote-test/relay.pid
  for attempt in {1..50}; do
    curl --fail --silent http://127.0.0.1:8787/health > .build/remote-test/health.json && break
    sleep 0.1
  done
fi
python3 -c 'import json; assert json.load(open(".build/remote-test/health.json"))["service"] == "vibra-relay", "El puerto 8787 está ocupado por otro servicio"'
simulator=${VIBRA_SIMULATOR_ID:-$(xcrun simctl list devices available -j | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(x["udid"] for group in d["devices"].values() for x in group if "iPhone" in x["name"]))')}
xcrun simctl boot "$simulator" 2>/dev/null || true
xcrun simctl bootstatus "$simulator" -b
./Scripts/verify_ios_keychain.sh "$simulator"
open -a Simulator
open -n "$repo_root/dist/Vibra.app"
print -- 'Abierto: Vibra del repo + iPhone Simulator. En Ajustes, genera y copia una invitación.'
print -- 'Pega la invitación en Vibra iOS y confirma el iPhone en el Mac. Comparte un pane desde su menú.'
print -- 'Guía completa: docs/plans/ios-remote-testing.md'
