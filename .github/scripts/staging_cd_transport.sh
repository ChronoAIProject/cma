#!/usr/bin/env bash
set -euo pipefail

# The Ubuntu runner image ships Skopeo. The web archive requires digest-preserving
# copies. An absent or incompatible installation follows the normal package path.
if [[ ${CD_COMPONENT:?} == web ]] && skopeo --version >/dev/null 2>&1 \
    && help=$(skopeo copy --help) && [[ $help == *--preserve-digests* ]]; then
  echo 'Using runner-provided Skopeo for web publication'
  exit 0
fi

sudo apt-get update -qq
sudo apt-get install -y -qq skopeo
