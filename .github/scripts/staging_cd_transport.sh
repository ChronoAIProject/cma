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
sudo apt-get -o Acquire::Retries=5 update -qq
sudo apt-get -o Acquire::Retries=5 install -y -qq skopeo libarchive-tools zstd
