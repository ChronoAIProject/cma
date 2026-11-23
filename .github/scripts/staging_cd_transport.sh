#!/usr/bin/env bash
set -euo pipefail

# The Ubuntu runner image ships Skopeo. The web archive requires digest-preserving
# copies. The scanner also needs libarchive's ar decoder and the zstd CLI.
# Check their actual runtime interfaces so a missing bsdtar CLI alone never
# triggers a package installation on the web fast path.
if [[ ${CD_COMPONENT:?} == web ]] && skopeo --version >/dev/null 2>&1 \
    && help=$(skopeo copy --help) && [[ $help == *--preserve-digests* ]] \
    && zstd --version >/dev/null 2>&1 \
    && python3 -c 'import ctypes, ctypes.util, sys
name = ctypes.util.find_library("archive")
sys.exit(0 if name and hasattr(ctypes.CDLL(name), "archive_read_support_format_ar") else 1)' >/dev/null 2>&1; then
  echo 'Using runner-provided Skopeo for web publication'
  exit 0
fi

# Mirrors drop connections now and then; apt repeats each failed fetch first.
# A stalled mirror or a held dpkg lock must not hold the release: each apt call
# gets a per-request timeout, a bounded lock wait and a wall-clock limit, and
# the whole operation is retried once before it fails with a named error.
apt_bounded() {
  local attempt
  for attempt in 1 2; do
    if timeout --kill-after=10 180 sudo apt-get -o Acquire::Retries=3 \
        -o Acquire::http::Timeout=30 -o Acquire::https::Timeout=30 \
        -o DPkg::Lock::Timeout=60 "$@"; then
      return 0
    fi
    [[ $attempt == 2 ]] || { echo "::warning title=Registry transport retried::apt $1 did not finish; retrying once"; sleep 10; }
  done
  echo "::error title=Registry transport unavailable::apt $1 did not finish in two bounded attempts (180 s each). The package mirror or the runner's dpkg lock was unavailable; re-run the failed jobs."
  return 1
}
apt_bounded update -qq
apt_bounded install -y -qq skopeo libarchive-tools zstd
