#!/usr/bin/env bash
# Regressions for pull-pinned-image.sh with an inert docker and sleep on PATH.
set -euo pipefail

script_dir=$(CDPATH='' cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)
readonly puller="${script_dir}/pull-pinned-image.sh"
suite=$(mktemp -d "${TMPDIR:-/tmp}/cma-pull-pinned.XXXXXX")
readonly suite
trap 'rm -rf "${suite}"' EXIT HUP INT TERM
readonly image="mongo:8.0.13@sha256:$(printf 'a%.0s' {1..64})"
tests=0

fail() { printf 'not ok - %s\n' "$*" >&2; exit 1; }
pass() { tests=$((tests + 1)); printf 'ok %d - %s\n' "${tests}" "$1"; }

# present: 1 when the image is already local; failures: pulls that fail first.
setup() {
  local present=$1 failures=$2
  rm -rf "${suite}/bin" "${suite}/log" && mkdir -p "${suite}/bin" && : > "${suite}/log"
  printf '%s' "$failures" > "${suite}/remaining"
  cat > "${suite}/bin/docker" <<FAKE
#!/usr/bin/env bash
echo "docker \$*" >> "${suite}/log"
if [[ "\$1 \$2" == "image inspect" ]]; then exit \$(( ${present} == 1 ? 0 : 1 )); fi
if [[ "\$1" == "pull" ]]; then
  left=\$(cat "${suite}/remaining")
  if (( left > 0 )); then echo \$((left - 1)) > "${suite}/remaining"; exit 1; fi
  exit 0
fi
exit 0
FAKE
  printf '#!/usr/bin/env bash\necho "sleep $*" >> "%s/log"\n' "${suite}" > "${suite}/bin/sleep"
  chmod +x "${suite}/bin/docker" "${suite}/bin/sleep"
}
run() { PATH="${suite}/bin:${PATH}" bash "$puller" "$@" > "${suite}/out" 2> "${suite}/err"; }
count() { grep -c -- "$1" "${suite}/log" || true; }

setup 0 0
if run "mongo:8.0.13"; then fail "an unpinned image must be refused"; else [[ $? -eq 2 ]] || fail "refusal exit code"; fi
[[ $(count "docker") -eq 0 ]] || fail "a refused image must not reach docker"
grep -q "digest-pinned" "${suite}/err" || fail "the refusal must say why"
pass "refuses an image without a pinned digest before calling docker"

setup 1 0
run "$image" || fail "a present image must succeed"
[[ $(count "docker pull") -eq 0 ]] || fail "a present image must not be pulled"
pass "an image already present is not pulled again"

setup 0 2
run "$image" || fail "a pull that succeeds on attempt 3 must succeed"
[[ $(count "docker pull") -eq 3 ]] || fail "expected 3 pulls, got $(count 'docker pull')"
[[ "$(grep '^sleep' "${suite}/log" | tr '\n' ' ')" == "sleep 5 sleep 15 " ]] || fail "backoff must be 5 then 15"
[[ $(grep -c "::warning title=Image pull retried::mongo:8.0.13 attempt" "${suite}/err") -eq 2 ]] || fail "each retry warns"
grep -q "Pulled mongo:8.0.13 on attempt 3 of 4" "${suite}/out" || fail "the recovery is reported"
pass "a transient pull failure is retried with bounded backoff"

setup 0 9
if run "$image"; then fail "a pull that never succeeds must fail"; else [[ $? -eq 1 ]] || fail "final exit code"; fi
[[ $(count "docker pull") -eq 4 ]] || fail "expected 4 bounded attempts"
[[ "$(grep '^sleep' "${suite}/log" | tr '\n' ' ')" == "sleep 5 sleep 15 sleep 45 " ]] || fail "backoff must be 5, 15, 45"
grep -q "::error title=Image pull failed::mongo:8.0.13 (pinned digest) could not be pulled in 4 attempts" "${suite}/err" \
  || fail "the final failure must name the image and the bound"
! grep -q "sha256:aaaa" "${suite}/err" || fail "the annotation names the image, not its digest"
pass "a persistent failure stops after 4 attempts with a named error"

printf '1..%d\n' "$tests"
