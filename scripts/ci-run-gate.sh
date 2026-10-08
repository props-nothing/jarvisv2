#!/usr/bin/env bash
# Runs one acceptance gate and, when it fails, turns the panic into a visible annotation.
#
# The job log needs repository authentication to read, so a gate that failed on one operating system left only "exit code 101"
# on the run page. The panic message is what says why, so it is copied into `::error::` lines, which show without logging in.
set -uo pipefail
gate="$1"
cargo test -p jarvis-acceptance --test "$gate" --locked -- --nocapture 2>&1 | tee "gate-$gate.txt"
status=${PIPESTATUS[0]}
if [ "$status" -ne 0 ]; then
  grep -a -A4 -E "panicked at" "gate-$gate.txt" | head -n 14 | while IFS= read -r line; do
    echo "::error::$gate: $line"
  done
fi
exit "$status"