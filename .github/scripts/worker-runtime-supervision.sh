#!/usr/bin/env bash
# Opt-in host-native proof; the runner owns its tools, cache and temporary state.
set -euo pipefail
umask 077
export CARGO_INCREMENTAL=0
repository=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd -P)
exec node "$repository/.github/scripts/worker-runtime-supervision.mjs" "$@"
