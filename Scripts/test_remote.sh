#!/bin/zsh
# Local test: native iPhone simulator + the repository's Mac build. No relay.
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
cd "$repo_root"
mkdir -p .build/remote-test
./Scripts/build_ios.sh simulator > .build/remote-test/ios-build.log 2>&1
./Scripts/package_app.sh release --sign - > .build/remote-test/mac-build.log 2>&1
simulator=${VIBRA_SIMULATOR_ID:-$(xcrun simctl list devices available -j | python3 -c 'import json,sys; d=json.load(sys.stdin); print(next(x["udid"] for group in d["devices"].values() for x in group if "iPhone" in x["name"]))')}
xcrun simctl boot "$simulator" 2>/dev/null || true
xcrun simctl bootstatus "$simulator" -b
./Scripts/verify_ios_keychain.sh "$simulator"
open -a Simulator
open -n "$repo_root/dist/Vibra.app"
print -- 'Abierto: Vibra del repo + iPhone Simulator. En Ajustes, genera y copia una invitación.'
print -- 'Pega la invitación en Vibra iOS y confirma el iPhone en el Mac. Comparte un pane desde su menú.'
print -- 'Guía completa: docs/plans/ios-remote-testing.md'
