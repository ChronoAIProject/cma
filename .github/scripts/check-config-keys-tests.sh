#!/usr/bin/env bash

set -euo pipefail
umask 077

script_dir=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
readonly checker="${script_dir}/check-config-keys.sh"
suite_root=$(mktemp -d "${TMPDIR:-/tmp}/cma-config-key-tests.XXXXXX")
readonly suite_root
readonly stdout_file="${suite_root}/stdout"
readonly stderr_file="${suite_root}/stderr"

trap 'rm -rf -- "${suite_root}"' EXIT HUP INT TERM

tests=0
case_number=0
status=0
case_root=''

fail() {
  printf 'not ok - %s\n' "$*" >&2
  exit 1
}

pass() {
  tests=$((tests + 1))
  printf 'ok %d - %s\n' "${tests}" "$1"
}

new_case() {
  local suffix=${1:-fixture}

  case_number=$((case_number + 1))
  case_root="${suite_root}/case ${case_number} ${suffix}"
  mkdir -p "${case_root}/api/crates/cma/src/config" "${case_root}/infra/api"
}

write_source() {
  local path=$1
  shift

  mkdir -p "$(dirname -- "${path}")"
  printf '%s\n' "$@" >"${path}"
}

write_configmap() {
  {
    printf '%s\n' 'apiVersion: v1' 'kind: ConfigMap' 'metadata:' \
      '  name: fixture' 'data:'
    local key
    for key in "$@"; do
      printf '  %s: "fixture"\n' "${key}"
    done
    printf '%s\n' '  RUST_LOG: "info"' 'binaryData:' \
      '  CHRONO_BINARY_DATA_IS_OUT_OF_SCOPE: "Zml4dHVyZQ=="'
  } >"${case_root}/infra/api/configmap.yaml"
}

write_secret_contract() {
  {
    printf '%s\n' 'apiVersion: v1' 'kind: Secret' 'metadata:' \
      '  name: fixture' 'stringData: {}' \
      '# Keys required by api/, populated out of band.' '#'
    local key
    for key in "$@"; do
      printf '#   %s fixture secret\n' "${key}"
    done
  } >"${case_root}/infra/api/secret.yaml"
}

run_checker() {
  set +e
  "${checker}" "$@" >"${stdout_file}" 2>"${stderr_file}"
  status=$?
  set -e
}

assert_status() {
  local expected=$1
  local name=$2

  if [[ ${status} -ne ${expected} ]]; then
    fail "${name}: expected status ${expected}, got ${status}: $(<"${stderr_file}")"
  fi
}

assert_contains() {
  local file=$1
  local expected=$2
  local name=$3

  grep -F -- "${expected}" "${file}" >/dev/null || \
    fail "${name}: missing '${expected}' in ${file}: $(<"${file}")"
}

assert_not_contains() {
  local file=$1
  local unexpected=$2
  local name=$3

  if grep -F -- "${unexpected}" "${file}" >/dev/null; then
    fail "${name}: found unexpected '${unexpected}' in ${file}: $(<"${file}")"
  fi
}

new_case 'matching public and secret keys'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let public = lookup("CHRONO_PUBLIC_SETTING");' \
  'let secret = lookup("CHRONO_SECRET_SETTING");' \
  'let again = lookup("CHRONO_PUBLIC_SETTING");'
write_configmap 'CHRONO_PUBLIC_SETTING'
write_secret_contract 'CHRONO_SECRET_SETTING'
run_checker "${case_root}"
assert_status 0 'matching keys'
assert_contains "${stdout_file}" \
  'Configuration key check passed: 2 code key(s) match 2 manifest key(s).' \
  'matching keys'
pass 'ConfigMap and Secret contract keys exactly matching code pass as sets'

new_case 'missing key'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let present = lookup("CHRONO_PRESENT");' \
  'let missing = lookup("CHRONO_MISSING");'
write_configmap 'CHRONO_PRESENT'
write_secret_contract
run_checker "${case_root}"
assert_status 1 'missing key'
assert_contains "${stderr_file}" 'missing manifest key: CHRONO_MISSING' 'missing key'
assert_not_contains "${stderr_file}" 'stale manifest key:' 'missing key'
pass 'a key read by code but absent from both manifests is reported as missing'

new_case 'stale key'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let current = lookup("CHRONO_CURRENT");'
write_configmap 'CHRONO_CURRENT' 'CHRONO_STALE'
write_secret_contract 'CHRONO_STALE_SECRET'
run_checker "${case_root}"
assert_status 1 'stale key'
assert_contains "${stderr_file}" 'stale manifest key: CHRONO_STALE' 'stale key'
assert_contains "${stderr_file}" 'stale manifest key: CHRONO_STALE_SECRET' 'stale key'
assert_not_contains "${stderr_file}" 'CHRONO_BINARY_DATA_IS_OUT_OF_SCOPE' 'stale key'
assert_not_contains "${stderr_file}" 'missing manifest key:' 'stale key'
pass 'stale keys in either manifest are rejected while keys outside data are ignored'

new_case 'duplicate keys'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let local = lookup("CHRONO_LOCAL");' \
  'let repeated = lookup("CHRONO_REPEATED");' \
  'let shared = lookup("CHRONO_SHARED");' \
  'let secret_repeated = lookup("CHRONO_SECRET_REPEATED");'
write_configmap 'CHRONO_LOCAL' 'CHRONO_REPEATED' 'CHRONO_REPEATED' 'CHRONO_SHARED'
write_secret_contract 'CHRONO_SHARED' 'CHRONO_SECRET_REPEATED' 'CHRONO_SECRET_REPEATED'
run_checker "${case_root}"
assert_status 1 'duplicate keys'
assert_contains "${stderr_file}" 'duplicate manifest key: CHRONO_REPEATED' 'duplicate keys'
assert_contains "${stderr_file}" 'duplicate manifest key: CHRONO_SHARED' 'duplicate keys'
assert_contains "${stderr_file}" 'duplicate manifest key: CHRONO_SECRET_REPEATED' \
  'duplicate keys'
assert_not_contains "${stderr_file}" 'missing manifest key:' 'duplicate keys'
assert_not_contains "${stderr_file}" 'stale manifest key:' 'duplicate keys'
pass 'duplicates within either manifest and across public and secret contracts are rejected'

new_case 'nested modules'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let root = lookup("CHRONO_ROOT");' \
  'let diagnostic = "CHRONO_ROOT is invalid";' \
  '// CHRONO_COMMENT_ONLY is documentation.'
write_source "${case_root}/api/crates/cma/src/config/profiles.rs" \
  'let nested = lookup("CHRONO_NESTED");'
write_source "${case_root}/api/crates/cma/src/config/deeper/policy.rs" \
  'let deep = lookup(r#"CHRONO_DEEP"#);'
write_source "${case_root}/api/crates/cma/src/outside.rs" \
  'let outside = lookup("CHRONO_OUTSIDE_SCOPE");'
write_configmap 'CHRONO_ROOT' 'CHRONO_NESTED' 'CHRONO_DEEP'
write_secret_contract
run_checker "${case_root}"
assert_status 0 'nested modules'
assert_not_contains "${stderr_file}" 'CHRONO_COMMENT_ONLY' 'nested modules'
assert_not_contains "${stderr_file}" 'CHRONO_OUTSIDE_SCOPE' 'nested modules'
pass 'all nested config modules are scanned while sibling source and prose are excluded'

new_case 'supported configuration read patterns'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let direct = std::env::var("CHRONO_DIRECT");' \
  'let os = std::env::var_os("CHRONO_OS");' \
  'let required = required(&mut lookup, "CHRONO_REQUIRED")?;' \
  'let defaulted = value_or(&mut lookup, "CHRONO_DEFAULTED", "default")?;' \
  'let multiline = positive_u32(' \
  '    &mut lookup,' \
  '    "CHRONO_MULTILINE",' \
  '    600, 1,' \
  ')?;' \
  'let (first, second) = ("CHRONO_FIRST", "CHRONO_SECOND");' \
  'let bytes = b"CHRONO_BYTES";' \
  'let raw = r###"CHRONO_RAW"###;' \
  'const KEY: &str = "CHRONO_CONSTANT";' \
  'let via_constant = lookup(KEY);'
write_configmap 'CHRONO_DIRECT' 'CHRONO_OS' 'CHRONO_REQUIRED' 'CHRONO_DEFAULTED' \
  'CHRONO_MULTILINE' 'CHRONO_FIRST' 'CHRONO_SECOND' 'CHRONO_BYTES' 'CHRONO_RAW' \
  'CHRONO_CONSTANT'
write_secret_contract
run_checker "${case_root}"
assert_status 0 'supported configuration read patterns'
assert_contains "${stdout_file}" \
  'Configuration key check passed: 10 code key(s) match 10 manifest key(s).' \
  'supported configuration read patterns'
pass 'direct, helper, multiline, tuple, byte, raw, and constant key reads are covered'

new_case 'near miss literals'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let valid = lookup("CHRONO_VALID_9");' \
  'let prefixed = "prefix CHRONO_NOT_A_READ";' \
  'let lower = lookup("CHRONO_lower");' \
  'let other = lookup("NYXID_NOT_IN_SCOPE");'
write_configmap 'CHRONO_VALID_9'
write_secret_contract
printf '%s\n' '#  CHRONO_TWO_SPACES' '#    CHRONO_FOUR_SPACES' \
  '  #   CHRONO_INDENTED_REFERENCE' >>"${case_root}/infra/api/secret.yaml"
run_checker "${case_root}"
assert_status 0 'near miss literals'
pass 'only exact code literals and exact Secret reference entries join the contract'

new_case 'path with spaces'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let root = lookup("CHRONO_SPACE_ROOT");'
write_source "${case_root}/api/crates/cma/src/config/a module/a file.rs" \
  'let nested = lookup("CHRONO_SPACE_NESTED");'
write_configmap 'CHRONO_SPACE_ROOT' 'CHRONO_SPACE_NESTED'
write_secret_contract
run_checker "${case_root}"
assert_status 0 'path with spaces'
pass 'repository, directory, and source paths containing spaces are scanned safely'

new_case 'malformed invocation'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let key = lookup("CHRONO_ONLY");'
write_configmap 'CHRONO_ONLY'
write_secret_contract
run_checker "${case_root}" extra
assert_status 2 'extra argument'
assert_contains "${stderr_file}" 'usage: check-config-keys.sh [repository-root]' \
  'extra argument'
run_checker "${case_root}/not-a-directory"
assert_status 2 'missing root'
assert_contains "${stderr_file}" 'repository root is not a directory' 'missing root'
rm -f -- "${case_root}/infra/api/secret.yaml"
run_checker "${case_root}"
assert_status 2 'missing contract input'
assert_contains "${stderr_file}" 'Secret reference contract is not a regular file' \
  'missing contract input'
write_secret_contract
rm -f -- "${case_root}/infra/api/configmap.yaml"
run_checker "${case_root}"
assert_status 2 'missing ConfigMap input'
assert_contains "${stderr_file}" 'ConfigMap manifest is not a regular file' \
  'missing ConfigMap input'
write_configmap 'CHRONO_ONLY'
rm -f -- "${case_root}/api/crates/cma/src/config.rs"
run_checker "${case_root}"
assert_status 2 'missing source input'
assert_contains "${stderr_file}" 'configuration source is not a regular file' \
  'missing source input'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let key = lookup("CHRONO_ONLY");'
rm -rf -- "${case_root}/api/crates/cma/src/config"
printf '%s\n' 'not a directory' >"${case_root}/api/crates/cma/src/config"
run_checker "${case_root}"
assert_status 2 'invalid module layout'
assert_contains "${stderr_file}" 'configuration module path is not a directory' \
  'invalid module layout'
rm -f -- "${case_root}/api/crates/cma/src/config"
ln -s 'missing-directory' "${case_root}/api/crates/cma/src/config"
run_checker "${case_root}"
assert_status 2 'dangling module layout'
assert_contains "${stderr_file}" 'configuration module path is not a directory' \
  'dangling module layout'
rm -f -- "${case_root}/api/crates/cma/src/config"
printf '%s\n' 'data:' >>"${case_root}/infra/api/configmap.yaml"
run_checker "${case_root}"
assert_status 2 'invalid ConfigMap layout'
assert_contains "${stderr_file}" 'ConfigMap must contain one top-level data mapping' \
  'invalid ConfigMap layout'
pass 'bad invocations and required input layouts exit with usage status'

new_case 'default root discovery'
write_source "${case_root}/api/crates/cma/src/config.rs" \
  'let key = lookup("CHRONO_DEFAULT_ROOT");'
write_configmap 'CHRONO_DEFAULT_ROOT'
write_secret_contract
mkdir -p "${case_root}/.github/scripts"
cp "${checker}" "${case_root}/.github/scripts/check-config-keys.sh"
chmod 0755 "${case_root}/.github/scripts/check-config-keys.sh"
set +e
"${case_root}/.github/scripts/check-config-keys.sh" \
  >"${stdout_file}" 2>"${stderr_file}"
status=$?
set -e
assert_status 0 'default root discovery'
pass 'an installed checker discovers its repository root without an argument'

new_case 'gateway configuration'
write_source "${case_root}/api/crates/cma/src/config.rs" 'let key = lookup("CHRONO_ONLY");'
write_configmap 'CHRONO_ONLY'
write_secret_contract
mkdir -p "${case_root}/infra/cmaeg"
write_source "${case_root}/api/crates/cmaeg/src/config.rs" 'let key = lookup("CMAEG_LISTEN");'
write_source "${case_root}/api/crates/cmaeg/src/config/turns.rs" 'let key = lookup("CMAEG_DRAIN_SECONDS");'
printf '%s\n' 'data:' '  CMAEG_LISTEN: "127.0.0.1:8090"' >"${case_root}/infra/cmaeg/configmap.yaml"
printf '%s\n' '# References only.' >"${case_root}/infra/cmaeg/secret.yaml"
run_checker "${case_root}"
assert_status 1 'gateway missing key'
assert_contains "${stderr_file}" 'missing manifest key: CMAEG_DRAIN_SECONDS' 'gateway missing key'
printf '%s\n' '  CMAEG_DRAIN_SECONDS: "600"' >>"${case_root}/infra/cmaeg/configmap.yaml"
run_checker "${case_root}"
assert_status 0 'gateway complete'
pass 'gateway configuration is checked when its manifest exists, including nested modules'

printf '1..%d\n' "${tests}"
