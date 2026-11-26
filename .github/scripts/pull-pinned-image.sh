#!/usr/bin/env bash
# Pull one digest-pinned image with bounded retries before a job starts a test
# container from it. `docker run` pulls implicitly and fails the whole job on
# one registry or CDN hiccup; a pinned digest makes repeating the pull safe.
set -euo pipefail

image=${1:-}
if [[ ! "$image" =~ ^[a-z0-9][a-z0-9./:_-]*@sha256:[0-9a-f]{64}$ ]]; then
  echo "::error title=Image pull refused::pull-pinned-image.sh pulls only one digest-pinned image" >&2
  exit 2
fi
name=${image%%@*}

# Already present: the digest names exact bytes, so there is nothing to fetch.
if docker image inspect "$image" >/dev/null 2>&1; then
  exit 0
fi

attempts=4
attempt_seconds=120
waits=(5 15 45)
bounded() {
  if command -v timeout >/dev/null 2>&1; then
    timeout "$attempt_seconds" "$@"
  else
    "$@"
  fi
}
for ((attempt = 1; attempt <= attempts; attempt++)); do
  if bounded docker pull --quiet "$image" >/dev/null 2>&1; then
    (( attempt == 1 )) || echo "Pulled ${name} on attempt ${attempt} of ${attempts}"
    exit 0
  fi
  if (( attempt < attempts )); then
    wait=${waits[attempt - 1]}
    echo "::warning title=Image pull retried::${name} attempt ${attempt} of ${attempts} failed; retrying in ${wait}s" >&2
    sleep "$wait"
  fi
done
echo "::error title=Image pull failed::${name} (pinned digest) could not be pulled in ${attempts} attempts of up to ${attempt_seconds}s each. The registry or network was unavailable; re-run the job." >&2
exit 1
