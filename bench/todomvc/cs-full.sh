#!/bin/bash
# Builds crudspec's apps/todomvc twice for the benchmark: as it ships (~/cs-todomvc-out/app, `PRAGMA synchronous = NORMAL` in its Go runtime) and with `synchronous = FULL`
# (~/cs-todomvc-full/app), so that an answered change survives a power cut as in Mitochondria's default. The crudspec checkout is not changed: the one line is edited in a copy of
# its lang/gogen module, which the second build links instead.
# usage (from Windows): wsl -e bash /mnt/d/code/Mitochondria/bench/todomvc/cs-full.sh [CRUDSPEC_CHECKOUT, default /mnt/d/code/crudspec]
set -e
export PATH=/usr/local/go/bin:/usr/local/bin:/usr/bin:/bin:$HOME/.cargo/bin
cs="${1:-/mnt/d/code/crudspec}"
cd "$cs"
rm -rf "$HOME/cs-todomvc-out" "$HOME/cs-todomvc-full" "$HOME/cs-gogen-full"
taskset -c 0-7 node lang/cli.ts build apps/todomvc --out "$HOME/cs-todomvc-out" 2>&1 | tail -1
cp -r "$cs/lang/gogen" "$HOME/cs-gogen-full"
sed -i 's/PRAGMA synchronous = NORMAL/PRAGMA synchronous = FULL/' "$HOME/cs-gogen-full/rt/store.go"
grep -c "synchronous = FULL" "$HOME/cs-gogen-full/rt/store.go" | sed 's/^/lines with FULL: /'
cp -r "$HOME/cs-todomvc-out" "$HOME/cs-todomvc-full"
sed -i "s#^replace crudspec/gogen => .*#replace crudspec/gogen => $HOME/cs-gogen-full#" "$HOME/cs-todomvc-full/go.mod"
(cd "$HOME/cs-todomvc-full" && taskset -c 0-7 go build -o app . && ls -la app)
