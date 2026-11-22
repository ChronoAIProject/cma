#!/usr/bin/env bash
set -euo pipefail
umask 077
[[ $# -eq 0 ]] || exit 2

# Keep the image scanner's executable outside every Docker build context.
install_dir=$(mktemp -d "${RUNNER_TEMP:?}/image-gitleaks.XXXXXX")
trap 'status=$?; if (( status != 0 )); then rm -rf -- "$install_dir"; fi' EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
archive="$install_dir/gitleaks.tar.gz"
# Repeat transient download failures; the pinned checksum below stays the authority.
curl --fail --silent --show-error --location --retry 5 --retry-all-errors --connect-timeout 30 \
  https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz \
  --output "$archive"
printf '%s  %s\n' \
  551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb "$archive" \
  | sha256sum --check --status
tar -xzf "$archive" -C "$install_dir" gitleaks
rm "$archive"
test "$("$install_dir/gitleaks" version)" = '8.30.1'
python3 - "$install_dir" <<'PYTHON'
import hashlib, json, platform, sys
from pathlib import Path
root = Path(sys.argv[1]).resolve()
binary = root / "gitleaks"
receipt = {"kind": "checksum-verified upstream release", "version": "8.30.1",
    "archive_sha256": "551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb",
    "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
    "artifact_platform": "linux/amd64", "runner_system": platform.system(),
    "runner_machine": platform.machine(), "executable": str(binary)}
(root / "identity.json").write_text(json.dumps(receipt, sort_keys=True) + "\n")
PYTHON
printf '%s\n' "$install_dir" >> "${GITHUB_PATH:?}"
if [[ -n "${GITHUB_OUTPUT:-}" ]]; then
  printf 'directory=%s\nexecutable=%s\nreceipt=%s\n' \
    "$install_dir" "$install_dir/gitleaks" "$install_dir/identity.json" >> "$GITHUB_OUTPUT"
fi
