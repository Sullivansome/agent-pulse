#!/usr/bin/env bash
# Build a local installable plugin package; does not install hooks or publish it.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
cargo build --release --locked
PACKAGE="$ROOT/dist/com.sullivansome.agent-pulse"
mkdir -p "$PACKAGE"
cp island-plugin.toml LICENSE README.md "$PACKAGE/"
for folder in assets market; do
  if [[ -d "$folder" ]]; then cp -R "$folder" "$PACKAGE/"; fi
done
install -m 755 target/release/island-plugin-agent-pulse "$PACKAGE/island-plugin-agent-pulse"
echo "Package ready: $PACKAGE"
