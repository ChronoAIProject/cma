#!/usr/bin/env bash

set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
readonly checker="${script_dir}/check-file-size.sh"
suite_root=$(mktemp -d "${TMPDIR:-/tmp}/cma-file-size-tests.XXXXXX")
readonly suite_root
readonly stdout_file="${suite_root}/stdout"
readonly stderr_file="${suite_root}/stderr"

trap 'rm -rf "${suite_root}"' EXIT HUP INT TERM

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
  case_number=$((case_number + 1))
  case_root="${suite_root}/case-${case_number}"
  mkdir -p "${case_root}"
}

make_lines() {
  local path=$1
  local count=$2
  local final_newline=${3:-yes}

  mkdir -p "$(dirname -- "${path}")"
  awk -v count="${count}" -v final_newline="${final_newline}" 'BEGIN {
    for (line = 1; line <= count; line += 1) {
      if (final_newline == "no" && line == count) {
        printf "// fixture line %d", line
      } else {
        printf "// fixture line %d\n", line
      }
    }
  }' >"${path}"
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

new_case
make_lines "${case_root}/api/crates/core/src/model.rs" 500
make_lines "${case_root}/web/src/components/surface.tsx" 500 no
make_lines "${case_root}/sdk/typescript/src/client.ts" 500
make_lines "${case_root}/sdk/rust/src/lib.rs" 1
run_checker "${case_root}"
assert_status 0 'ordinary boundary'
assert_contains "${stdout_file}" 'Source file size check passed: 4 files checked.' \
  'ordinary boundary'
pass 'ordinary Rust, TypeScript, and TSX files accept exactly 500 logical lines'

new_case
make_lines "${case_root}/api/crates/core/src/too_large.rs" 501 no
make_lines "${case_root}/web/src/too_large.ts" 502
make_lines "${case_root}/sdk/typescript/src/too_large.tsx" 503
run_checker "${case_root}"
assert_status 1 'ordinary overflow'
assert_contains "${stderr_file}" \
  'api/crates/core/src/too_large.rs has 501 lines; the source limit is 500' \
  'ordinary overflow'
assert_contains "${stderr_file}" \
  'web/src/too_large.ts has 502 lines; the source limit is 500' \
  'ordinary overflow'
assert_contains "${stderr_file}" \
  'sdk/typescript/src/too_large.tsx has 503 lines; the source limit is 500' \
  'ordinary overflow'
assert_contains "${stderr_file}" \
  'Source file size check failed: 3 of 3 checked files exceed their limit.' \
  'ordinary overflow'
pass 'all ordinary-source overflows are reported in one run'

new_case
make_lines "${case_root}/api/crates/core/src/model_tests.rs" 2501
make_lines "${case_root}/web/src/model.test.ts" 2501
make_lines "${case_root}/web/src/model.test.tsx" 2501
make_lines "${case_root}/sdk/typescript/src/client.test.ts" 2501
make_lines "${case_root}/api/crates/core/tests/not_source.rs" 2501
make_lines "${case_root}/web/scripts/not_source.ts" 2501
make_lines "${case_root}/sdk/rust/tests/not_source.rs" 2501
make_lines "${case_root}/web/src/not_source.js" 2501
run_checker "${case_root}"
assert_status 0 'scope and test exclusions'
assert_contains "${stdout_file}" 'Source file size check passed: 0 files checked.' \
  'scope and test exclusions'
pass 'named test sources, files outside source roots, and other extensions are excluded'

new_case
make_lines "${case_root}/api/crates/core/src/fixtures/generated/wire.rs" 2000 no
make_lines "${case_root}/web/src/generated/fixtures/events.ts" 2000
make_lines "${case_root}/web/src/generated-fixtures/replies.tsx" 2000
make_lines "${case_root}/sdk/typescript/src/generated_fixtures/problems.ts" 2000
run_checker "${case_root}"
assert_status 0 'generated fixture boundary'
assert_contains "${stdout_file}" 'Source file size check passed: 4 files checked.' \
  'generated fixture boundary'
pass 'explicit generated-fixture trees accept exactly 2,000 logical lines'

new_case
make_lines "${case_root}/web/src/fixtures/generated/wire.ts" 2001 no
make_lines "${case_root}/web/src/fixtures/handwritten.ts" 501
run_checker "${case_root}"
assert_status 1 'fixture overflow and handwritten fixture'
assert_contains "${stderr_file}" \
  'web/src/fixtures/generated/wire.ts has 2001 lines; the generated fixture limit is 2000' \
  'fixture overflow and handwritten fixture'
assert_contains "${stderr_file}" \
  'web/src/fixtures/handwritten.ts has 501 lines; the source limit is 500' \
  'fixture overflow and handwritten fixture'
pass 'generated fixtures are capped at 2,000 and handwritten fixtures remain capped at 500'

new_case
make_lines "${case_root}/api/crates/core/src/model_test.rs" 501
make_lines "${case_root}/web/src/model.tests.ts" 501
make_lines "${case_root}/web/src/model.test.js" 2501
run_checker "${case_root}"
assert_status 1 'near-miss exclusions'
assert_contains "${stderr_file}" 'model_test.rs has 501 lines; the source limit is 500' \
  'near-miss exclusions'
assert_contains "${stderr_file}" 'model.tests.ts has 501 lines; the source limit is 500' \
  'near-miss exclusions'
assert_not_contains "${stderr_file}" 'model.test.js' 'near-miss exclusions'
pass 'only the specified test filename forms bypass the production-source limit'

new_case
make_lines "${case_root}/api/crates/a crate/src/a file.rs" 501
run_checker "${case_root}"
assert_status 1 'paths with spaces'
assert_contains "${stderr_file}" \
  'api/crates/a crate/src/a file.rs has 501 lines; the source limit is 500' \
  'paths with spaces'
pass 'source paths containing spaces are scanned without word splitting'

new_case
mkdir -p "${case_root}/.github/scripts"
cp "${checker}" "${case_root}/.github/scripts/check-file-size.sh"
chmod 0755 "${case_root}/.github/scripts/check-file-size.sh"
make_lines "${case_root}/web/src/default-root.ts" 500
set +e
"${case_root}/.github/scripts/check-file-size.sh" \
  >"${stdout_file}" 2>"${stderr_file}"
status=$?
set -e
assert_status 0 'default repository root'
assert_contains "${stdout_file}" 'Source file size check passed: 1 files checked.' \
  'default repository root'
pass 'the checker discovers the repository root from its installed location'

new_case
run_checker "${case_root}/missing"
assert_status 2 'missing root'
assert_contains "${stderr_file}" 'repository root is not a directory' 'missing root'
run_checker "${case_root}" extra
assert_status 2 'extra argument'
assert_contains "${stderr_file}" 'usage: check-file-size.sh [repository-root]' \
  'extra argument'
pass 'invalid invocations fail before scanning'

new_case
run_checker "${case_root}"
assert_status 0 'empty source tree'
assert_contains "${stdout_file}" 'Source file size check passed: 0 files checked.' \
  'empty source tree'
pass 'a repository with no matching source files passes'

printf '1..%d\n' "${tests}"
