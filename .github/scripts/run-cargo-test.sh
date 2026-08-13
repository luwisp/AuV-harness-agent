#!/usr/bin/env bash

set -u -o pipefail

log_file=$(mktemp)
trap 'rm -f "$log_file"' EXIT

set +e
"$@" 2>&1 | tee "$log_file"
status=${PIPESTATUS[0]}
set -e

if ((status == 0)); then
    exit 0
fi

# GitHub's public Checks API exposes annotations even when full logs require login.
details=$(tail -n 80 "$log_file" | sed -e 's/%/%25/g' -e 's/\r/%0D/g' -e ':a;N;$!ba;s/\n/%0A/g')
printf '::error title=Cargo test failed::%s\n' "$details"
exit "$status"
