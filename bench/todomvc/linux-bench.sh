#!/bin/bash
# TodoMVC against crudspec on Linux (WSL2): builds this compiler and examples/todomvc, then runs bench.py under the shared bench lock.
# usage (from Windows): wsl -e bash /mnt/d/code/Mitochondria/bench/todomvc/linux-bench.sh CRUDSPEC_APP CRUDSPEC_ASSETS [bench.py args]
set -e
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
cd "$(dirname "$0")/../.."
export CARGO_TARGET_DIR="$HOME/mito-target"
taskset -c 0-7 cargo build --release 2>&1 | tail -1
mkdir -p "$HOME/mito-out"
taskset -c 0-7 "$CARGO_TARGET_DIR/release/mito" build examples/todomvc/todomvc.mito -o "$HOME/mito-out/todomvc"
app="$1"; assets="$2"; shift 2
cd bench/todomvc
flock -x /home/ben/linuxbench/bench.lock python3 -I bench.py --mito "$HOME/mito-out/todomvc" --crudspec "$app" --crudspec-assets "$assets" "$@"
