#!/usr/bin/env bash

set -euo pipefail
umask 077

readonly SOURCE_LINE_LIMIT=500
readonly TEST_SOURCE_LINE_LIMIT=2000
readonly GENERATED_FIXTURE_LINE_LIMIT=2000

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
  printf 'check-file-size: repository root is not a directory: %s\n' \
    "${root_argument}" >&2
  exit 2
fi
repository_root=$(CDPATH='' cd -- "${root_argument}" && pwd -P)
readonly repository_root

is_test_source() {
  local basename=$1

  case "${basename}" in
    tests.rs | *_tests.rs | *.test.ts | *.test.tsx) return 0 ;;
    *) return 1 ;;
  esac
}

is_generated_fixture() {
  local relative_path=$1

  # Generated fixtures get a larger bound only in an unambiguous generated
  # fixture directory. A generic fixtures/ directory is still ordinary source.
  case "/${relative_path}/" in
    */fixtures/generated/* | */generated/fixtures/* | \
      */generated-fixtures/* | */generated_fixtures/*) return 0 ;;
    *) return 1 ;;
  esac
}

github_escape() {
  local value=$1

  value=${value//'%'/'%25'}
  value=${value//$'\r'/'%0D'}
  value=${value//$'\n'/'%0A'}
  printf '%s' "${value}"
}

source_roots=()
for directory in "${repository_root}"/api/crates/*/src; do
  [[ -d "${directory}" ]] && source_roots+=("${directory}")
done
[[ -d "${repository_root}/web/src" ]] && \
  source_roots+=("${repository_root}/web/src")

checked=0
violations=0
if [[ ${#source_roots[@]} -eq 0 ]]; then
  printf 'Source file size check passed: 0 files checked.\n'
  exit 0
fi

file_list=$(mktemp "${TMPDIR:-/tmp}/cma-file-size.XXXXXX")
readonly file_list
trap 'rm -f -- "${file_list}"' EXIT HUP INT TERM
if ! find "${source_roots[@]}" -type f \
  \( -name '*.rs' -o -name '*.ts' -o -name '*.tsx' \) \
  -print0 >"${file_list}"; then
  printf 'check-file-size: failed to enumerate source files\n' >&2
  exit 1
fi

while IFS= read -r -d '' file; do
  basename=${file##*/}
  relative_path=${file#"${repository_root}/"}
  limit=${SOURCE_LINE_LIMIT}
  category='source'
  if is_test_source "${basename}"; then
    limit=${TEST_SOURCE_LINE_LIMIT}
    category='test source'
  elif is_generated_fixture "${relative_path}"; then
    limit=${GENERATED_FIXTURE_LINE_LIMIT}
    category='generated fixture'
  fi

  # awk counts the final logical line even when the file has no trailing
  # newline; wc -l would undercount that valid source-file shape.
  line_count=$(awk 'END { print NR + 0 }' "${file}")
  checked=$((checked + 1))
  if [[ ${line_count} -le ${limit} ]]; then
    continue
  fi

  violations=$((violations + 1))
  annotation_path=$(github_escape "${relative_path}")
  annotation_message=$(github_escape \
    "${relative_path} has ${line_count} lines; the ${category} limit is ${limit}")
  printf '::error file=%s,line=%d,title=Source file too large::%s\n' \
    "${annotation_path}" "$((limit + 1))" "${annotation_message}" >&2
done <"${file_list}"

if [[ ${violations} -gt 0 ]]; then
  printf 'Source file size check failed: %d of %d checked files exceed their limit.\n' \
    "${violations}" "${checked}" >&2
  exit 1
fi

printf 'Source file size check passed: %d files checked.\n' "${checked}"
