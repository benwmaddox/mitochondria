#!/bin/bash
# Builds crudspec's apps/todomvc for the benchmark into ~/cs-todomvc-out/app (Node 24 and Go 1.27 needed; the crudspec checkout needs its `--db-sync` flag, added in crudspec PR 30).
# usage (from Windows): wsl -e bash /mnt/d/code/Mitochondria/bench/todomvc/cs-build.sh [CRUDSPEC_CHECKOUT, default /mnt/d/code/crudspec]
set -e
export PATH=/usr/local/go/bin:/usr/local/bin:/usr/bin:/bin:$HOME/.cargo/bin
cs="${1:-/mnt/d/code/crudspec}"
cd "$cs"
rm -rf "$HOME/cs-todomvc-out"
taskset -c 0-7 node lang/cli.ts build apps/todomvc --out "$HOME/cs-todomvc-out" 2>&1 | tail -1
"$HOME/cs-todomvc-out/app" --db-sync bogus 2>&1 | head -1
