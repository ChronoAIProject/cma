#!/usr/bin/env bash

set -euo pipefail
umask 077
export LC_ALL=C

script_dir=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)

usage() {
  printf 'usage: %s [repository-root]\n' "${0##*/}" >&2
}

if [[ $# -gt 1 ]]; then
  usage
  exit 2
fi

root_argument=${1:-"${script_dir}/../.."}
if [[ ! -d "${root_argument}" ]]; then
  printf 'check-config-keys: repository root is not a directory: %s\n' \
    "${root_argument}" >&2
  exit 2
fi
repository_root=$(CDPATH='' cd -- "${root_argument}" && pwd -P)
readonly repository_root

check_project() (
local crate=$1 deployment=$2
readonly config_source="${repository_root}/api/crates/${crate}/src/config.rs"
readonly config_modules="${repository_root}/api/crates/${crate}/src/config"
readonly configmap="${repository_root}/infra/${deployment}/configmap.yaml"
readonly secret_contract="${repository_root}/infra/${deployment}/secret.yaml"

require_regular_file() {
  local label=$1
  local path=$2

  if [[ ! -f "${path}" || -L "${path}" ]]; then
    printf 'check-config-keys: %s is not a regular file: %s\n' \
      "${label}" "${path}" >&2
    exit 2
  fi
}

require_regular_file 'configuration source' "${config_source}"
require_regular_file 'ConfigMap manifest' "${configmap}"
require_regular_file 'Secret reference contract' "${secret_contract}"
if [[ -L "${config_modules}" || \
  ( -e "${config_modules}" && ! -d "${config_modules}" ) ]]; then
  printf 'check-config-keys: configuration module path is not a directory: %s\n' \
    "${config_modules}" >&2
  exit 2
fi

scratch=$(mktemp -d "${TMPDIR:-/tmp}/cma-config-keys.XXXXXX")
readonly scratch
trap 'rm -rf -- "${scratch}"' EXIT HUP INT TERM

readonly nested_files="${scratch}/nested-files"
readonly code_raw="${scratch}/code-raw"
readonly code_keys="${scratch}/code-keys"
readonly manifest_raw="${scratch}/manifest-raw"
readonly manifest_sorted="${scratch}/manifest-sorted"
readonly manifest_keys="${scratch}/manifest-keys"
readonly duplicates="${scratch}/duplicates"
readonly missing="${scratch}/missing"
readonly stale="${scratch}/stale"

source_files=("${config_source}")
if [[ -d "${config_modules}" ]]; then
  if ! find "${config_modules}" -type f -name '*.rs' -print0 >"${nested_files}"; then
    printf 'check-config-keys: failed to enumerate nested configuration modules\n' >&2
    exit 2
  fi
  while IFS= read -r -d '' source; do
    source_files+=("${source}")
  done <"${nested_files}"
fi

# Only a complete quoted literal is a configuration read. This excludes key
# names embedded in diagnostics or prose while accepting ordinary, byte, and
# raw Rust string literals.
awk '
  {
    remaining = $0
    while (match(remaining, /"(CHRONO|CMAEG|CMA_IM)_[A-Z0-9_]+"/)) {
      print substr(remaining, RSTART + 1, RLENGTH - 2)
      remaining = substr(remaining, RSTART + RLENGTH)
    }
  }
' "${source_files[@]}" >"${code_raw}"
sort -u "${code_raw}" >"${code_keys}"

# The checked-in ConfigMap uses one top-level data mapping. Preserve duplicate
# entries here; YAML parsers commonly overwrite them before they can be
# diagnosed.
if ! awk '
  BEGIN { data_sections = 0; in_data = 0 }
  /^data:[[:space:]]*$/ {
    data_sections += 1
    in_data = 1
    next
  }
  in_data && /^[^[:space:]#]/ { in_data = 0 }
  in_data && match($0, /^  (CHRONO|CMAEG|CMA_IM)_[A-Z0-9_]+:/) {
    print substr($0, 3, RLENGTH - 3)
  }
  END {
    if (data_sections != 1) {
      exit 3
    }
  }
' "${configmap}" >"${manifest_raw}"; then
  printf 'check-config-keys: ConfigMap must contain one top-level data mapping: %s\n' \
    "${configmap}" >&2
  exit 2
fi

# secret.yaml intentionally contains no values. Its indented reference-list
# comments are the checked-in contract for keys populated out of band.
awk '
  {
    reference = $0
    if (sub(/^#   /, "", reference) && \
        match(reference, /^(CHRONO|CMAEG|CMA_IM)_[A-Z0-9_]+([[:space:]]|$)/)) {
      key = substr(reference, RSTART, RLENGTH)
      sub(/[[:space:]]$/, "", key)
      print key
    }
  }
' "${secret_contract}" >>"${manifest_raw}"

sort "${manifest_raw}" >"${manifest_sorted}"
sort -u "${manifest_sorted}" >"${manifest_keys}"
uniq -d "${manifest_sorted}" >"${duplicates}"
comm -23 "${code_keys}" "${manifest_keys}" >"${missing}"
comm -13 "${code_keys}" "${manifest_keys}" >"${stale}"

violations=0
while IFS= read -r key; do
  [[ -n "${key}" ]] || continue
  violations=$((violations + 1))
  printf '::error title=Missing deployment configuration::missing manifest key: %s\n' \
    "${key}" >&2
done <"${missing}"
while IFS= read -r key; do
  [[ -n "${key}" ]] || continue
  violations=$((violations + 1))
  printf '::error title=Stale deployment configuration::stale manifest key: %s\n' \
    "${key}" >&2
done <"${stale}"
while IFS= read -r key; do
  [[ -n "${key}" ]] || continue
  violations=$((violations + 1))
  printf '::error title=Duplicate deployment configuration::duplicate manifest key: %s\n' \
    "${key}" >&2
done <"${duplicates}"

code_count=$(awk 'END { print NR + 0 }' "${code_keys}")
manifest_count=$(awk 'END { print NR + 0 }' "${manifest_keys}")
if [[ ${violations} -gt 0 ]]; then
  printf 'Configuration key check failed: %d violation(s), %d code key(s), %d manifest key(s).\n' \
    "${violations}" "${code_count}" "${manifest_count}" >&2
  exit 1
fi

printf 'Configuration key check passed: %d code key(s) match %d manifest key(s).\n' \
  "${code_count}" "${manifest_count}"

)
check_project cma api
if [[ -e "${repository_root}/infra/cmaeg/configmap.yaml" || -L "${repository_root}/infra/cmaeg/configmap.yaml" ]]; then
  check_project cmaeg cmaeg
fi

if [[ -e "${repository_root}/infra/image-manager/configmap.yaml" || -L "${repository_root}/infra/image-manager/configmap.yaml" ]]; then
  check_project cma-image-manager image-manager
fi
