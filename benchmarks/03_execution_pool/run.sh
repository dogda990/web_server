#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
PYO3_PYTHON="$PWD/.venv/bin/python" cargo build --release
exec .venv/bin/python scripts/method_bench.py --profile benchmarks/03_execution_pool/comparison.json \
  --cycles 10 --warmup 2 --duration 5 --connections 64 --threads 4 --cooldown 1 "$@"
