#!/bin/zsh
# Runtime regression test of the installed Simulator app, not a UI test.
set -euo pipefail
script_name=${0:A}
repo_root=${script_name:h:h}
cd "$repo_root"
simulator=${1:-booted}
probe_id=$(uuidgen)
app=.build/ios/Build/Products/Debug-iphonesimulator/Vibra.app
xcrun simctl install "$simulator" "$app"
SIMCTL_CHILD_VIBRA_KEYCHAIN_PROBE="$probe_id" xcrun simctl launch --terminate-running-process "$simulator" app.vibra.VibraMobile
container=$(xcrun simctl get_app_container "$simulator" app.vibra.VibraMobile data)
report="$container/Library/Caches/keychain-probe-$probe_id.json"
trap 'rm -f "$report"' EXIT
for attempt in {1..100}; do
  [[ -f "$report" ]] && break
  sleep 0.1
done
python3 - "$report" <<'PY'
import json,sys
with open(sys.argv[1]) as report:
    result=json.load(report)
assert result.get('ok') is True, f"Simulator Keychain failed: {result}"
print('Simulator Keychain: write/read/delete passed')
PY
