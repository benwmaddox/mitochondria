#!/bin/bash
# Linux (WSL2): build the compiler and run its tests, off the benchmark cores (8-21).
# usage (from Windows): wsl -e bash /mnt/d/code/Mitochondria/bench/langarena/linux-test.sh
set -e
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
cd "$(dirname "$0")/../.."
export CARGO_TARGET_DIR="$HOME/mito-target"
taskset -c 0-7 cargo build --release 2>&1 | tail -1
taskset -c 0-7 cargo test --release 2>&1 | grep -E "^test result|FAILED|panicked|differs"
