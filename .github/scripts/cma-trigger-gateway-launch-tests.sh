#!/usr/bin/env bash

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
LAUNCHER="${SCRIPT_DIR}/cma-trigger-gateway-launch.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT
mkdir -p "${TEST_ROOT}/.github/scripts"

# shellcheck disable=SC2016 # This is literal source for the isolated fixture process.
stub_source='#!/bin/bash -p
set -euo pipefail
target="${0##*/}"
case "${target}" in
  cma-trigger-gateway-cloudflare-capture.sh)
    if [[ "${1:-}" == --capture ]]; then
      [[ "${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}" == 7 && -r /dev/fd/7 ]]
      [[ ! -e /dev/fd/8 && -z "${CHRONO_RELEASE_ACCESS_TOKEN:-}" ]]
      IFS= read -r token <&7
      exec 7<&-
      [[ "${#token}" -ge 20 ]]
      printf "%s\n" cloudflare-ok >observed-cloudflare
    else
      [[ ! -e /dev/fd/7 && ! -e /dev/fd/8 ]]
      [[ -z "${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}" ]]
      printf "%s\n" cloudflare-offline-ok >observed-cloudflare-offline
    fi
    ;;
  cma-trigger-gateway-origin-evidence.sh)
    [[ "${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}" == 7 && -r /dev/fd/7 ]]
    [[ ! -e /dev/fd/8 && -z "${CHRONO_RELEASE_ACCESS_TOKEN:-}" ]]
    [[ "${CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM:-}" == writers-disabled-through-origin-probes ]]
    [[ "${CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID:-}" == change/CMA-123 ]]
    IFS= read -r token <&7
    exec 7<&-
    [[ "${#token}" -ge 20 ]]
    printf "%s\n" origin-ok >observed-origin
    ;;
  cma-trigger-gateway-restore-traffic.sh)
    if [[ "${1:-}" == --fence || "${1:-}" == --prove-fence ]]; then
      [[ ! -e /dev/fd/7 && ! -e /dev/fd/8 ]]
      [[ -z "${CMA_GATEWAY_TOKEN_FD:-}" && -z "${CHRONO_RELEASE_ACCESS_TOKEN:-}" ]]
      printf "%s\n" traffic-fence-ok >observed-traffic-fence
    else
      [[ ! -e /dev/fd/7 && "${CMA_GATEWAY_TOKEN_FD:-}" == 8 && -r /dev/fd/8 ]]
      [[ -z "${CHRONO_RELEASE_ACCESS_TOKEN:-}" ]]
      IFS= read -r token <&8
      exec 8<&-
      [[ "${#token}" -ge 20 ]]
      printf "%s\n" traffic-token-ok >observed-traffic-token
    fi
    ;;
  *)
    [[ ! -e /dev/fd/7 && ! -e /dev/fd/8 ]]
    [[ -z "${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}" && -z "${CMA_GATEWAY_TOKEN_FD:-}" ]]
    [[ -z "${CHRONO_RELEASE_ACCESS_TOKEN:-}" && -z "${CLOUDFLARE_API_TOKEN:-}" ]]
    printf "%s\n" closed-ok >"observed-${target}"
    ;;
esac
'
stub_file="${TEST_ROOT}/stub.sh"
printf '%s' "${stub_source}" >"${stub_file}"
for target in \
    cma-trigger-gateway-admission-bootstrap.sh \
    cma-trigger-gateway-projection.sh \
    cma-trigger-gateway-cloudflare-capture.sh \
    cma-trigger-gateway-origin-evidence.sh \
    cma-trigger-gateway-adoption.sh \
    cma-trigger-gateway-restore-traffic.sh; do
  cp "${stub_file}" "${TEST_ROOT}/.github/scripts/${target}"
done

trusted_path=/usr/bin:/bin
run_closed_mode() {
  local mode="$1"
  (
    cd "${TEST_ROOT}"
    exec 7</dev/null
    exec 8</dev/null
    CMA_GATEWAY_TRUSTED_PATH="${trusted_path}" \
    CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
    CHRONO_RELEASE_ACCESS_TOKEN=hostile-release-token \
    CLOUDFLARE_API_TOKEN=hostile-cloudflare-token \
      "${LAUNCHER}" "${mode}" --fixture
  )
}

run_closed_mode admission
run_closed_mode projection
run_closed_mode adoption

cloudflare_token="cloudflare-$RANDOM-$RANDOM-$RANDOM-token"
printf '%s\n' "${cloudflare_token}" | (
  cd "${TEST_ROOT}"
  exec 7<&0
  exec 8</dev/null
  CMA_GATEWAY_TRUSTED_PATH="${trusted_path}" \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7 \
  CHRONO_RELEASE_ACCESS_TOKEN=hostile-release-token \
    "${LAUNCHER}" cloudflare --capture ignored
)

origin_token="origin-$RANDOM-$RANDOM-$RANDOM-token"
printf '%s\n' "${origin_token}" | (
  cd "${TEST_ROOT}"
  exec 7<&0
  exec 8</dev/null
  CMA_GATEWAY_TRUSTED_PATH="${trusted_path}" \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7 \
  CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM=writers-disabled-through-origin-probes \
  CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID=change/CMA-123 \
    "${LAUNCHER}" origin ignored-projection.json
)

(
  cd "${TEST_ROOT}"
  exec 7</dev/null
  exec 8</dev/null
  CMA_GATEWAY_TRUSTED_PATH="${trusted_path}" \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
    "${LAUNCHER}" cloudflare --derive-policy ignored-receipt.json
)

(
  cd "${TEST_ROOT}"
  exec 7</dev/null
  exec 8</dev/null
  CMA_GATEWAY_TRUSTED_PATH="${trusted_path}" \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  CHRONO_RELEASE_ACCESS_TOKEN=must-not-be-consumed-for-fence \
    "${LAUNCHER}" traffic --fence
)

release_token="release-$RANDOM-$RANDOM-$RANDOM-token"
(
  cd "${TEST_ROOT}"
  exec 7</dev/null
  exec 8</dev/null
  CMA_GATEWAY_TRUSTED_PATH="${trusted_path}" \
  CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
  CHRONO_RELEASE_ACCESS_TOKEN="${release_token}" \
    "${LAUNCHER}" traffic --restore
)

for result in \
    observed-cma-trigger-gateway-admission-bootstrap.sh \
    observed-cma-trigger-gateway-projection.sh \
    observed-cma-trigger-gateway-adoption.sh \
    observed-cloudflare observed-cloudflare-offline observed-origin \
    observed-traffic-fence observed-traffic-token; do
  [[ -s "${TEST_ROOT}/${result}" ]]
done
if find "${TEST_ROOT}" -type f \
    \( -name 'observed-*' -o -name '*.tmp' \) -exec grep -F -l \
    -e "${cloudflare_token}" -e "${origin_token}" -e "${release_token}" {} + | grep -q .; then
  echo "a protected bearer reached launcher output or temporary evidence" >&2
  exit 1
fi

printf '%s\n' "CMA Trigger Gateway launcher tests passed (8 modes)"
