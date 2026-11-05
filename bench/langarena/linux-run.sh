#!/bin/bash
# Linux (WSL2) run: build the compiler, run its tests, then the LangArena ports (Mitochondria only; Rust and Crystal keep their
# recorded numbers) under the shared benchmark lock. Builds and tests stay off the benchmark cores (8-21).
# usage (from Windows): wsl -e bash /mnt/d/code/Mitochondria/bench/langarena/linux-run.sh [run.py args...]
set -e
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
cd "$(dirname "$0")/../.."
export CARGO_TARGET_DIR="$HOME/mito-target"
taskset -c 0-7 cargo build --release 2>&1 | tail -1
taskset -c 0-7 cargo test --release 2>&1 | grep -E "^test result|FAILED|panicked"
cd bench/langarena
MITO_EXE="$CARGO_TARGET_DIR/release/mito" flock -x /home/ben/linuxbench/bench.lock python3 -I run.py --mito-only "$@"
