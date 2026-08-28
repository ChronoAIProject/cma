#!/bin/bash -p

{ set +x; } 2>/dev/null
if ! umask 077; then
  echo "the Gateway launcher requires umask 077" >&2
  exit 1
fi
if ! ulimit -c 0; then
  echo "the Gateway launcher requires core dumps to be disabled" >&2
  exit 1
fi
set -euo pipefail

# This launcher intentionally uses Bash builtins only until any bearer has been
# copied to a private descriptor and removed from the inherited environment.
unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH
for cma_launcher_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_launcher_name}"
done
unset cma_launcher_name

readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0
readonly CMA_LAUNCHER_ATTESTATION=reviewed-host-toolchain

cma_launcher_die() {
  printf 'cma-trigger Gateway trusted launcher: %s\n' "$*" >&2
  exit 1
}

[[ "$#" -ge 1 ]] || \
  cma_launcher_die "select admission, projection, cloudflare, origin, adoption, or traffic"
cma_launcher_mode="$1"
shift
: "${CMA_GATEWAY_TRUSTED_PATH:?set the reviewed absolute command-search path}"
[[ "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" == "${CMA_LAUNCHER_ATTESTATION}" ]] || \
  cma_launcher_die \
    "set CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain only after approving the host toolchain"

IFS=: read -r -a cma_launcher_path_parts <<<"${CMA_GATEWAY_TRUSTED_PATH}"
[[ "${#cma_launcher_path_parts[@]}" -gt 0 ]] || cma_launcher_die "reviewed PATH is empty"
for cma_launcher_path_part in "${cma_launcher_path_parts[@]}"; do
  [[ "${cma_launcher_path_part}" == /* && \
    "${cma_launcher_path_part}" != *$'\n'* && \
    "${cma_launcher_path_part}" != *$'\r'* ]] || \
    cma_launcher_die "every reviewed PATH component must be absolute and single-line"
done
unset cma_launcher_path_parts cma_launcher_path_part

cma_launcher_token=""
cma_launcher_token_required=0
cma_launcher_cloudflare_token_required=0
if [[ "${cma_launcher_mode}" == traffic && "${1:-}" != --fence && \
      "${1:-}" != --prove-fence ]]; then
  : "${CHRONO_RELEASE_ACCESS_TOKEN:?traffic restoration requires the short-lived release bearer}"
  cma_launcher_token="${CHRONO_RELEASE_ACCESS_TOKEN}"
  unset CHRONO_RELEASE_ACCESS_TOKEN
  case "${cma_launcher_token}" in
    *$'\r'*|*$'\n'*)
      unset cma_launcher_token
      cma_launcher_die "the release bearer must not contain CR or LF"
      ;;
  esac
  cma_launcher_token_required=1
fi
if [[ "${cma_launcher_mode}" == origin || \
      ( "${cma_launcher_mode}" == cloudflare && "${1:-}" == --capture ) ]]; then
  cma_launcher_cloudflare_token_required=1
fi

cma_launcher_target=""
case "${cma_launcher_mode}" in
  admission) cma_launcher_target=.github/scripts/cma-trigger-gateway-admission-bootstrap.sh ;;
  projection) cma_launcher_target=.github/scripts/cma-trigger-gateway-projection.sh ;;
  cloudflare) cma_launcher_target=.github/scripts/cma-trigger-gateway-cloudflare-capture.sh ;;
  origin) cma_launcher_target=.github/scripts/cma-trigger-gateway-origin-evidence.sh ;;
  adoption) cma_launcher_target=.github/scripts/cma-trigger-gateway-adoption.sh ;;
  traffic) cma_launcher_target=.github/scripts/cma-trigger-gateway-restore-traffic.sh ;;
  *) cma_launcher_die "unknown launcher mode ${cma_launcher_mode}" ;;
esac
[[ -f "${cma_launcher_target}" && ! -L "${cma_launcher_target}" ]] || \
  cma_launcher_die "launcher target is not a regular reviewed file"
[[ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" == 0 ]] || \
  cma_launcher_die "the checked-in production mutation gate is unexpectedly open"

cma_launcher_environment=(
  "PATH=${CMA_GATEWAY_TRUSTED_PATH}"
  "LC_ALL=C"
  "CMA_GATEWAY_TRUSTED_PATH=${CMA_GATEWAY_TRUSTED_PATH}"
  "CMA_GATEWAY_TRUSTED_LAUNCH=env-i-v1"
  "CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=${CMA_LAUNCHER_ATTESTATION}"
)
for cma_launcher_variable in \
    HOME CLOUDSDK_CONFIG KUBECONFIG \
    CMA_GATEWAY_RELEASE_REVISION CMA_GATEWAY_CONFIRM \
    CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM \
    CMA_GATEWAY_ADMISSION_CUSTODY_ROOT CMA_GATEWAY_ADMISSION_CUSTODY_CONFIRM \
    CMA_GATEWAY_ADMISSION_RECEIPT CMA_GATEWAY_ADMISSION_RECEIPT_SHA256 \
    CMA_GATEWAY_FENCE_LABEL_RESERVATION_CONFIRM \
    CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM \
    CMA_GATEWAY_ADOPTED_PROJECTION_PARENT \
    CMA_GATEWAY_ROLLBACK_DIR CMA_GATEWAY_ROLLBACK_SHA256 \
    CMA_GATEWAY_FENCE_RECEIPT_DIR CMA_GATEWAY_FENCE_RECEIPT_CONFIRM \
    CMA_GATEWAY_FENCE_RECEIPT_SHA256 \
    CMA_GATEWAY_FENCED_PROJECTION_PARENT CMA_GATEWAY_FINAL_PROJECTION_PARENT \
    CMA_GATEWAY_SOURCE_PROJECTION \
    CMA_GATEWAY_DESTRUCTIVE_INTENT CMA_FINAL_POD CMA_FINAL_POD_UID CMA_FINAL_POD_IP \
    CMA_GATEWAY_PROJECTION_DIR CMA_GATEWAY_PROJECTION_SHA256 \
    CMA_GATEWAY_PROJECTION_STAGE CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256 \
    CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256 CMA_GATEWAY_CLOUDFLARE_POLICY_SHA256 \
    CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM \
    CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID CMA_GATEWAY_ORIGIN_EVIDENCE_DIR; do
  if declare -p "${cma_launcher_variable}" >/dev/null 2>&1; then
    cma_launcher_environment+=(
      "${cma_launcher_variable}=${!cma_launcher_variable}"
    )
  fi
done
unset cma_launcher_variable
if [[ "${cma_launcher_cloudflare_token_required}" == 1 ]]; then
  [[ "${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}" == 7 && -r /dev/fd/7 ]] || \
    cma_launcher_die "${cma_launcher_mode} requires its short-lived Cloudflare bearer on protected descriptor 7"
  cma_launcher_environment+=("CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7")
fi
if [[ "${cma_launcher_token_required}" == 1 ]]; then
  cma_launcher_environment+=("CMA_GATEWAY_TOKEN_FD=8")
fi

if [[ "${cma_launcher_token_required}" == 1 ]]; then
  exec 7<&-
  exec 8<&-
  set +e
  printf '%s\n' "${cma_launcher_token}" | (
    exec 8<&0
    exec 0</dev/null
    unset cma_launcher_token
    exec /usr/bin/env -i "${cma_launcher_environment[@]}" \
      /bin/bash --noprofile --norc -p "${cma_launcher_target}" "$@"
  )
  cma_launcher_status=$?
  set -e
  cma_launcher_token=""
  unset cma_launcher_token
  exit "${cma_launcher_status}"
fi

if [[ "${cma_launcher_cloudflare_token_required}" == 1 ]]; then
  exec 8<&-
else
  exec 7<&-
  exec 8<&-
fi
unset cma_launcher_token
exec /usr/bin/env -i "${cma_launcher_environment[@]}" \
  /bin/bash --noprofile --norc -p "${cma_launcher_target}" "$@"
