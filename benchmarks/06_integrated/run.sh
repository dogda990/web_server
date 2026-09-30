#!/bin/sh
set -eu
cd "$(dirname "$0")/../.."
PYO3_PYTHON="$PWD/.venv/bin/python" cargo build --release
.venv/bin/python scripts/method_bench.py --profile benchmarks/06_integrated/comparison.json \
  --cycles 10 --warmup 2 --duration 5 --connections 16 --threads 4 --cooldown 1 "$@"
exec .venv/bin/python scripts/startup_bench.py \
  --profile benchmarks/06_integrated/comparison.json --runs 40
