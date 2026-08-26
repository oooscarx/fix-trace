#!/bin/sh
set -eu

repository=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
screenshot_dir=$repository/docs/screenshots

python3 "$repository/demo/capture_tui_snapshot.py"
cd "$repository/apps/fixtrace-desktop"
for snapshot in tui-main tui-approval; do
    npx playwright screenshot \
        --channel chrome \
        --timeout 5000 \
        --viewport-size='1400,900' \
        "file://$screenshot_dir/$snapshot.svg" \
        "$screenshot_dir/$snapshot.png"
done
FIXTRACE_SCREENSHOT_DIR=$screenshot_dir npm run e2e

printf '%s\n' "Screenshots written to $screenshot_dir"
