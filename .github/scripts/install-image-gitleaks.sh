#!/usr/bin/env bash
set -euo pipefail

# Keep the image scanner's executable outside every Docker build context.
install_dir=$(mktemp -d "${RUNNER_TEMP:?}/image-gitleaks.XXXXXX")
archive="$install_dir/gitleaks.tar.gz"
curl --fail --silent --show-error --location \
  https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz \
  --output "$archive"
printf '%s  %s\n' \
  551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb "$archive" \
  | sha256sum --check --status
tar -xzf "$archive" -C "$install_dir" gitleaks
rm "$archive"
test "$("$install_dir/gitleaks" version)" = '8.30.1'
printf '%s\n' "$install_dir" >> "${GITHUB_PATH:?}"
