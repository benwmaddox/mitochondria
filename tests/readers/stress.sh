#!/bin/bash
# Readers with unlogged state (Linux / WSL2). Build the program with `mito build tests/readers/readers.mito -o /tmp/readers_app` first.
# Starts it with --workers 4 (the note "reads on 4 readers" must appear: a program with unlogged state has readers because the compiler marks read-only endpoints),
# sends 4000 mixed requests 8 at a time (reads, peeks, a GET that writes unlogged scratch, POSTs): every answer must be 200, the final state right, the server alive.
export PATH="$HOME/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
/tmp/readers_app --port 18232 --workers 4 > /tmp/readers_out2.txt 2>&1 &
PID=$!
sleep 1
curl -s -X POST localhost:18232/set/5 > /dev/null
# 4000 requests, 8 at a time: reads, a scratch-writing GET, peeks, POSTs
seq 1 4000 | taskset -c 0-5 xargs -P 8 -I{} sh -c 'case $(( {} % 8 )) in 0) curl -s -X POST localhost:18232/set/5 -o /dev/null -w "%{http_code}\n";; 1) curl -s localhost:18232/scratch/3 -o /dev/null -w "%{http_code}\n";; 2|3) curl -s localhost:18232/peek -o /dev/null -w "%{http_code}\n";; *) curl -s localhost:18232/read -o /dev/null -w "%{http_code}\n";; esac' | sort | uniq -c
curl -s localhost:18232/read; echo
curl -s localhost:18232/peek; echo
if kill -0 $PID 2>/dev/null; then echo "server alive"; else echo "SERVER DIED"; fi
kill $PID 2>/dev/null
tail -2 /tmp/readers_out2.txt
