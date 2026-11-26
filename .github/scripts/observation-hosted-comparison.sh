#!/usr/bin/env bash
set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
output=$1
mkdir "$output"
readonly target='store::sandbox_mongo_integration_tests::resource_access_tests::history_measurement::observation_budget::observation_two_serving_replicas_keep_idle_work_independent_of_history'
readonly runner="$script_dir/mongo-contract-tests.sh"
readonly companion_partition='count:40/76'
primary=$(mktemp "$script_dir/observation-exact.XXXXXX.sh")
primary_pid=
companion_pid=
sampler_pid=

record() {
  printf '%s %s\n' "$(date -u +%s%3N)" "$*" | tee -a "$output/schedule.log"
}

cleanup() {
  local result=$? pid
  trap - EXIT
  trap '' INT TERM
  for pid in "$primary_pid" "$companion_pid" "$sampler_pid"; do
    [[ -n "$pid" ]] || continue
    kill -TERM -- "-$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  done
  rm -f -- "$primary"
  exit "$result"
}
trap cleanup EXIT
trap 'record event=cancelled signal=INT; exit 130' INT
trap 'record event=cancelled signal=TERM; exit 143' TERM

# The primary uses the ordinary fixture, profile and time limits. Select only
# the target test through a disposable copy of the runner.
python3 - "$runner" "$primary" "$target" <<'PY'
from pathlib import Path
import sys

runner, destination = map(Path, sys.argv[1:3])
source = runner.read_text()
marker = '-E "$selection_filter & not test(=auth::'
if source.count(marker) != 1:
    raise SystemExit("ordinary filter insertion point changed")
destination.write_text(source.replace(
    marker, f'-E "package(=cma) and test(={sys.argv[3]}) & '
    '$selection_filter & not test(=auth::',
))
PY

binary_metadata="$script_dir/../../api/target/nextest/binaries-metadata.json"
binary=$(jq -er '.["rust-binaries"]["cma::bin/cma"]["binary-path"]' "$binary_metadata")
[[ -x "$binary" ]]
sha256sum "$binary" | tee "$output/test-binary.sha256"
{
  printf 'runner_cpus=%s\n' "$(nproc)"
  sed -n '/^MemTotal:/p' /proc/meminfo
  uname -srmo
  df -h .
} | tee "$output/runner.txt"
setsid vmstat -w -t 5 > "$output/host-vmstat.log" &
sampler_pid=$!

run_primary() {
  local arm=$1 result=0
  record "arm=$arm role=primary event=start"
  setsid bash "$primary" count:1/1 > "$output/$arm.log" 2>&1 &
  primary_pid=$!
  wait "$primary_pid" || result=$?
  primary_pid=
  record "arm=$arm role=primary event=finish status=$result"
  cat "$output/$arm.log"
  return "$result"
}

alone_status=0
run_primary alone || alone_status=$?
[[ "$alone_status" != 125 ]] || exit 125
sha256sum --check "$output/test-binary.sha256"

record "arm=overlap role=companion event=start partition=$companion_partition"
setsid bash "$runner" "$companion_partition" > "$output/companion.log" 2>&1 &
companion_pid=$!
deadline=$((SECONDS + 90))
until rg -q '^Mongo fixture .*"event": "started"' "$output/companion.log"; do
  if ! kill -0 "$companion_pid" 2>/dev/null || (( SECONDS >= deadline )); then
    cat "$output/companion.log"
    exit 1
  fi
  sleep 0.25
done
record "arm=overlap role=companion event=ready"
overlap_status=0
run_primary overlap || overlap_status=$?
companion_status=0
wait "$companion_pid" || companion_status=$?
companion_pid=
record "arm=overlap role=companion event=finish status=$companion_status"
sha256sum --check "$output/test-binary.sha256"

# Exit codes and every test's original output remain available in the artifact.
[[ "$alone_status" == 0 && "$overlap_status" == 0 && "$companion_status" == 0 ]]
