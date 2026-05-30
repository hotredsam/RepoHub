#!/usr/bin/env bash
# Run RepoHub in development: Rust backend (127.0.0.1:8787) + Vite frontend (5173).
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cleanup() { kill 0 2>/dev/null || true; }
trap cleanup EXIT INT TERM

echo "▶ starting backend (cargo run) …"
( cd "$ROOT/backend" && cargo run ) &

echo "▶ starting frontend (vite) …"
( cd "$ROOT/frontend" && npm run dev ) &

wait
