#!/usr/bin/env bash
# Build the native hook receiver and connect all three agents (or pass --remove).
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
cargo build -p island-plugin-agent-pulse --locked
exec "$ROOT/target/debug/island-plugin-agent-pulse" setup "$@"
