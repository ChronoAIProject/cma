#!/usr/bin/env bash
set -euo pipefail

scanner=${1:?usage: scan-secret-history.sh gitleaks-binary [repository]}
repository=${2:-.}

# checkout fetches every remote branch at depth zero. Scan the complete history
# of the candidate merge, including every merged parent, rather than unrelated
# branches that are not part of this change. Removed secrets still fail.
exec "$scanner" detect --source "$repository" --redact --verbose \
  --log-opts='--full-history HEAD' --exit-code 1
