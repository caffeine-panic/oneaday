#!/usr/bin/env bash
set -euo pipefail

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) target="aarch64-apple-darwin" ;;
  Darwin-x86_64) target="x86_64-apple-darwin" ;;
  Linux-x86_64) target="x86_64-unknown-linux-gnu" ;;
  MINGW*-x86_64|MSYS*-x86_64) target="x86_64-pc-windows-msvc" ;;
  *) echo "Unsupported build host: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac

mkdir -p src-tauri/binaries .cache/go-build
output="src-tauri/binaries/registry-core-${target}"
if [[ "$target" == *windows* ]]; then output="${output}.exe"; fi
GOCACHE="$(pwd)/.cache/go-build" go build -o "$output" ./sidecar/cmd/registry-core
echo "Built $output"
