#!/usr/bin/env bash
# Validate and extract the Release Notes section of a pull request body.
#
# Reads the body on stdin. Writes the extracted section to stdout, and every
# problem it found to stderr — every problem, not just the first, so a pull
# request is not corrected one line at a time across three CI runs.
#
# One script rather than the same parser copied into two workflows. The gate
# that runs before the merge and the workflow that publishes after it have to
# agree on what the notes are; two copies that drift would let a pull request
# pass review and then fail on merge, or publish notes nobody approved.
#
# The format is exact on purpose. These notes are the only description a
# release carries, and they are read by people who were not in the review — so
# what they must contain is not left to whoever is writing them that day.
#
# No version number appears anywhere in the body. The version is derived from
# the release label and the previous tag, and is written into the notes by the
# publisher. An author who types one has guessed at something the merge decides.

set -euo pipefail

readonly EXPECTED=("What's New" "What's Fixed" "What's Changed")

failed=0

fail() {
  printf '::error::%s\n' "$1" >&2
  failed=1
}

expected_shape() {
  cat >&2 <<'USAGE'
::group::Required shape
The pull request body must contain this section, exactly:

  ## Release Notes

  ### What's New
  - ...

  ### What's Fixed
  - ...

  ### What's Changed
  - ...

All three subsections, spelled and capitalised as above, in that order, each
followed by at least one bullet. Only bullets may appear under a subsection.

The section ends at the next "##" heading, so anything addressed to the
reviewer goes outside it and is never published. Do not write a version
number: the merge derives it and the publisher adds it.
::endgroup::
USAGE
}

# GitHub returns pull request bodies with CRLF line endings, which would
# otherwise leave a trailing carriage return on every heading and stop all of
# them matching.
body=$(tr -d '\r')

headings=0
in_section=0
subsection=""
notes=""
declare -a found=()
declare -a bullets=()

while IFS= read -r line || [ -n "$line" ]; do
  # A level-two heading either opens the section or closes it. "###" is not
  # matched here: the subsections have to survive.
  if [ "${line:0:3}" = "## " ] || [ "$line" = "##" ]; then
    title=${line#\#\#}
    title=${title# }
    title=${title%"${title##*[![:space:]]}"}
    if [ "$title" = "Release Notes" ]; then
      headings=$((headings + 1))
      in_section=1
      subsection=""
    else
      in_section=0
    fi
    continue
  fi

  [ "$in_section" -eq 1 ] || continue

  if [ "${line:0:4}" = "### " ]; then
    title=${line#\#\#\# }
    title=${title%"${title##*[![:space:]]}"}
    found+=("$title")
    bullets+=(0)
    subsection="$title"
    notes+="$line"$'\n'
    continue
  fi

  stripped=${line#"${line%%[![:space:]]*}"}

  # Between "## Release Notes" and the first subsection nothing belongs.
  if [ -z "$subsection" ]; then
    if [ -n "$stripped" ]; then
      case "$stripped" in
        v[0-9]*.[0-9]*|[0-9]*.[0-9]*)
          fail "remove the version number '$stripped': the version is derived from the release label and written by the publisher, so one typed here would be a guess at what the merge decides"
          ;;
        *)
          fail "nothing may sit between '## Release Notes' and '### ${EXPECTED[0]}' (found: $stripped)"
          ;;
      esac
    fi
    continue
  fi

  case "$stripped" in
    '- '* | '* '*)
      last=$((${#bullets[@]} - 1))
      bullets[last]=$((bullets[last] + 1))
      notes+="$line"$'\n'
      ;;
    '')
      notes+=$'\n'
      ;;
    *)
      fail "only bullets may appear under '### ${subsection}' (found: $stripped)"
      ;;
  esac
done <<EOF
$body
EOF

if [ "$headings" -eq 0 ]; then
  fail "the pull request body must carry a '## Release Notes' section"
elif [ "$headings" -gt 1 ]; then
  fail "the body carries ${headings} '## Release Notes' headings; there must be exactly one"
fi

# Compared as a whole so a wrong order is reported as a wrong order, rather
# than as three separate complaints about sections that are all present.
joined=$(printf '%s|' ${found[@]+"${found[@]}"})
wanted=$(printf '%s|' "${EXPECTED[@]}")
if [ "$headings" -gt 0 ] && [ "$joined" != "$wanted" ]; then
  fail "the subsections must be exactly ${EXPECTED[0]}, ${EXPECTED[1]}, ${EXPECTED[2]} in that order (found: ${joined//|/, })"
  # A smart-quoting editor turns the apostrophe into U+2019, which fails the
  # comparison while looking identical in the pull request.
  case "$joined" in
    *$'’'*) fail "use a plain apostrophe ('), not a typographic one" ;;
  esac
fi

index=0
while [ "$index" -lt "${#found[@]}" ]; do
  if [ "${bullets[index]}" -eq 0 ]; then
    fail "'### ${found[index]}' has no bullets; write at least one, or say so in one"
  fi
  index=$((index + 1))
done

if [ "$failed" -ne 0 ]; then
  expected_shape
  exit 1
fi

# Trim the blank lines the section picked up at either end, so the publisher
# can place this between a heading it adds and an appendix it appends.
printf '%s\n' "$notes" | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
