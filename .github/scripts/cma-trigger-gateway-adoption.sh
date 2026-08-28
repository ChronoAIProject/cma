#!/bin/bash -p
set -euo pipefail
umask 077

unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH
for cma_gateway_startup_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_gateway_startup_name}"
done
unset cma_gateway_startup_name

readonly CMA_GATEWAY_CONTEXT="gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke"
readonly CMA_GATEWAY_NAMESPACE="chrono-bot"
readonly CMA_GATEWAY_CHECKER=".github/scripts/cma-trigger-gateway-state.sh"
readonly CMA_GATEWAY_SOURCE_CONTRACT=".github/scripts/cma-trigger-gateway-source-contract.rb"
readonly CMA_GATEWAY_PROJECTION_CONTRACT=".github/scripts/cma-trigger-gateway-projection.rb"
readonly CMA_GATEWAY_PROJECTION_CAPTURE=".github/scripts/cma-trigger-gateway-projection.sh"
readonly CMA_GATEWAY_ADMISSION_BOOTSTRAP=".github/scripts/cma-trigger-gateway-admission-bootstrap.sh"
readonly CMA_GATEWAY_TRUSTED_LAUNCHER=".github/scripts/cma-trigger-gateway-launch.sh"
readonly CMA_GATEWAY_CHECK_TIMEOUT="90s"
readonly CMA_GATEWAY_COMMAND_TIMEOUT="25s"
readonly CMA_GATEWAY_WAIT_SECONDS=900
readonly CMA_GATEWAY_OBJECTS="cma-api-route cma-web-route cma-http-redirect-route cma-api-health"
readonly CMA_GATEWAY_SOURCE_PATHS="infra/api/healthcheckpolicy.yaml infra/api/httproute.yaml infra/api/cutover-fence-policy.yaml .github/scripts/cma-trigger-gateway-source-contract.rb .github/scripts/cma-trigger-gateway-admission-bootstrap.sh .github/scripts/cma-trigger-gateway-projection.rb .github/scripts/cma-trigger-gateway-projection.sh .github/scripts/cma-trigger-gateway-tls-sanitize.rb .github/scripts/cma-trigger-gateway-live-policy.json .github/scripts/cma-trigger-gateway-cloudflare-capture.sh .github/scripts/cma-trigger-gateway-cloudflare-policy.json .github/scripts/cma-trigger-gateway-origin-evidence.sh .github/scripts/cma-trigger-gateway-launch.sh .github/scripts/cma-trigger-gateway-state.sh .github/scripts/cma-trigger-gateway-adoption.sh .github/scripts/cma-trigger-gateway-restore-traffic.sh"
readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0

CMA_GATEWAY_MUTATED=0
CMA_GATEWAY_PROBE_DIR=""
CMA_GATEWAY_WORK_DIR=""
CMA_GATEWAY_ROLLBACK_DIR=""
CMA_GATEWAY_ROLLBACK_SHA256="${CMA_GATEWAY_ROLLBACK_SHA256:-}"
CMA_GATEWAY_ROLLBACK_SEALED=0
CMA_GATEWAY_DESIRED_DIR=""
CMA_GATEWAY_DESIRED_SHA256=""
CMA_GATEWAY_SOURCE_SHA256=""

cma_gateway_die() {
  printf 'cma-trigger Gateway adoption: %s\n' "$*" >&2
  exit 1
}

cma_gateway_capture_projection() {
  "${CMA_GATEWAY_PROJECTION_CAPTURE}" "$@"
}

cma_gateway_verify_admission_receipt() {
  "${CMA_GATEWAY_ADMISSION_BOOTSTRAP}" --verify-receipt >/dev/null
}

cma_gateway_require_production_mutation_enabled() {
  [[ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" == 1 ]] || \
    cma_gateway_die \
      "production Gateway mutation is disabled until the reviewed Compute, origin-chain, and client-IP evidence gates close"
}

cma_gateway_require_privileged_bash() {
  [[ "$-" == *p* ]] || \
    cma_gateway_die "production Gateway mutation requires the reviewed /bin/bash -p entrypoint"
}

cma_gateway_require_trusted_launcher() {
  [[ "${CMA_GATEWAY_TRUSTED_LAUNCH:-}" == env-i-v1 ]] || \
    cma_gateway_die "production Gateway mutation requires the reviewed env-i launcher"
  [[ "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" == reviewed-host-toolchain ]] || \
    cma_gateway_die "production Gateway mutation requires the reviewed host-toolchain attestation"
  [[ -n "${CMA_GATEWAY_TRUSTED_PATH:-}" && "${PATH}" == "${CMA_GATEWAY_TRUSTED_PATH}" ]] || \
    cma_gateway_die "the production PATH differs from the reviewed launcher PATH"
}

cma_gateway_sha256_file() {
  local source_file="$1"

  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "${source_file}" | awk '{print $1}'
  else
    shasum -a 256 "${source_file}" | awk '{print $1}'
  fi
}

cma_gateway_sha256_stream() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  else
    shasum -a 256 | awk '{print $1}'
  fi
}

cma_gateway_sha256_valid() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]]
}

cma_gateway_named_files_digest() {
  local source_dir="$1"
  local object_name object_sha256 bundle_sha256

  bundle_sha256="$(
    set -o pipefail
    for object_name in ${CMA_GATEWAY_OBJECTS}; do
      object_sha256="$(cma_gateway_sha256_file \
        "${source_dir}/${object_name}.json")" || exit 1
      cma_gateway_sha256_valid "${object_sha256}" || exit 1
      printf '%s  %s.json\n' "${object_sha256}" "${object_name}"
    done | cma_gateway_sha256_stream
  )" || return 1
  cma_gateway_sha256_valid "${bundle_sha256}" || return 1
  printf '%s\n' "${bundle_sha256}"
}

cma_gateway_source_digest() {
  local source_path source_sha256 bundle_sha256

  bundle_sha256="$(
    set -o pipefail
    for source_path in ${CMA_GATEWAY_SOURCE_PATHS}; do
      source_sha256="$(cma_gateway_sha256_file "${source_path}")" || exit 1
      cma_gateway_sha256_valid "${source_sha256}" || exit 1
      printf '%s  %s\n' "${source_sha256}" "${source_path}"
    done | cma_gateway_sha256_stream
  )" || return 1
  cma_gateway_sha256_valid "${bundle_sha256}" || return 1
  printf '%s\n' "${bundle_sha256}"
}

cma_gateway_reviewed_source_digest() {
  local release_revision="$1"
  local source_path committed_sha256 bundle_sha256

  bundle_sha256="$(
    set -o pipefail
    for source_path in ${CMA_GATEWAY_SOURCE_PATHS}; do
      committed_sha256="$(
        timeout 10s git --no-replace-objects cat-file blob \
          "${release_revision}:${source_path}" | \
          cma_gateway_sha256_stream
      )" || exit 1
      cma_gateway_sha256_valid "${committed_sha256}" || exit 1
      printf '%s  %s\n' "${committed_sha256}" "${source_path}"
    done | cma_gateway_sha256_stream
  )" || return 1
  cma_gateway_sha256_valid "${bundle_sha256}" || return 1
  printf '%s\n' "${bundle_sha256}"
}

cma_gateway_file_mode() {
  local source_path="$1"
  local detected_mode

  detected_mode="$(stat -f '%Lp' "${source_path}" 2>/dev/null)" || detected_mode=""
  if [[ "${detected_mode}" =~ ^[0-7]{3,4}$ ]]; then
    printf '%s\n' "${detected_mode}"
    return 0
  fi
  stat -c '%a' "${source_path}"
}

cma_gateway_kubectl() {
  timeout --signal=TERM --kill-after=5s "${CMA_GATEWAY_COMMAND_TIMEOUT}" \
    kubectl --request-timeout=15s --context "${CMA_GATEWAY_CONTEXT}" \
      --namespace "${CMA_GATEWAY_NAMESPACE}" "$@"
}

cma_gateway_check() {
  cma_gateway_assert_source_seal || return 1
  timeout --signal=TERM --kill-after=5s "${CMA_GATEWAY_CHECK_TIMEOUT}" \
    /bin/bash --noprofile --norc -p "${CMA_GATEWAY_CHECKER}" "$@"
}

cma_gateway_source_contract() {
  timeout --signal=TERM --kill-after=2s 10s env -i \
    PATH="${PATH}" HOME="${HOME:-}" LC_ALL=C \
    ruby --disable-gems "${CMA_GATEWAY_SOURCE_CONTRACT}" "$@"
}

cma_gateway_deadline_reached() {
  [[ "${SECONDS}" -ge "$1" ]]
}

cma_gateway_require_checkout() {
  local command_name head_revision dirty_paths source_path committed_sha256
  local reviewed_source_sha256 live_source_sha256 live_file_sha256

  cma_gateway_require_trusted_launcher
  for command_name in awk chmod cmp cp curl find git jq kubectl mktemp rm ruby \
      sleep stat timeout tr wc; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_gateway_die "required command is unavailable: ${command_name}"
  done
  if ! command -v sha256sum >/dev/null 2>&1 && \
     ! command -v shasum >/dev/null 2>&1; then
    cma_gateway_die "required SHA-256 command is unavailable"
  fi
  [[ -x "${CMA_GATEWAY_CHECKER}" ]] || \
    cma_gateway_die "Gateway state checker is missing or not executable"
  [[ -x "${CMA_GATEWAY_SOURCE_CONTRACT}" ]] || \
    cma_gateway_die "Gateway source contract is missing or not executable"
  [[ -x "${CMA_GATEWAY_PROJECTION_CONTRACT}" && \
    -x "${CMA_GATEWAY_PROJECTION_CAPTURE}" && \
    -x "${CMA_GATEWAY_TRUSTED_LAUNCHER}" ]] || \
    cma_gateway_die "closed projection or trusted-launcher tooling is missing or not executable"
  [[ "${CMA_GATEWAY_RELEASE_REVISION:-}" =~ ^[0-9a-f]{40}$ ]] || \
    cma_gateway_die "set CMA_GATEWAY_RELEASE_REVISION to the reviewed full commit"
  [[ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:-}" == sole-spec-writer ]] || \
    cma_gateway_die \
      "set CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer after fencing direct writes to the checked routing graph"

  head_revision="$(timeout 10s git --no-replace-objects \
    rev-parse --verify 'HEAD^{commit}')" || \
    cma_gateway_die "the checkout HEAD could not be resolved"
  [[ "${head_revision}" == "${CMA_GATEWAY_RELEASE_REVISION}" ]] || \
    cma_gateway_die "the checkout is not the reviewed release revision"
  dirty_paths="$(timeout 10s git --no-replace-objects \
    -c core.fsmonitor=false -c core.hooksPath=/dev/null \
    status --porcelain=v1 --untracked-files=all)" || \
    cma_gateway_die "the checkout state could not be read"
  [[ -z "${dirty_paths}" ]] || \
    cma_gateway_die \
      "the release checkout has a tracked change or non-ignored untracked path"
  if ! timeout 10s git --no-replace-objects -c core.fsmonitor=false \
      -c core.hooksPath=/dev/null ls-files -v | \
      awk 'substr($0, 1, 1) ~ /[a-zS]/ { exit 1 }'; then
    cma_gateway_die \
      "the release index has an assume-unchanged, skip-worktree, or unreadable tracked entry"
  fi

  reviewed_source_sha256="$(cma_gateway_reviewed_source_digest \
    "${CMA_GATEWAY_RELEASE_REVISION}")" || \
    cma_gateway_die "the reviewed release source bundle could not be resolved"

  for source_path in ${CMA_GATEWAY_SOURCE_PATHS}; do
    [[ -f "${source_path}" && ! -L "${source_path}" ]] || \
      cma_gateway_die "release source is not a regular checked-in file: ${source_path}"
    committed_sha256="$(
      timeout 10s git --no-replace-objects cat-file blob \
        "${CMA_GATEWAY_RELEASE_REVISION}:${source_path}" | cma_gateway_sha256_stream
    )" || cma_gateway_die "release source is absent from the reviewed commit: ${source_path}"
    cma_gateway_sha256_valid "${committed_sha256}" || \
      cma_gateway_die "reviewed release source hash is malformed: ${source_path}"
    live_file_sha256="$(cma_gateway_sha256_file "${source_path}")" || \
      cma_gateway_die "release source could not be hashed: ${source_path}"
    cma_gateway_sha256_valid "${live_file_sha256}" || \
      cma_gateway_die "live release source hash is malformed: ${source_path}"
    [[ "${live_file_sha256}" == "${committed_sha256}" ]] || \
      cma_gateway_die "release source differs byte-for-byte from the reviewed commit: ${source_path}"
  done

  cma_gateway_source_contract >/dev/null || \
    cma_gateway_die "the release source contract failed validation"
  live_source_sha256="$(cma_gateway_source_digest)" || \
    cma_gateway_die "the live release source bundle could not be hashed"
  cma_gateway_sha256_valid "${live_source_sha256}" || \
    cma_gateway_die "the live release source bundle hash is malformed"
  [[ "${live_source_sha256}" == "${reviewed_source_sha256}" ]] || \
    cma_gateway_die "release source changed while its contract was validated"
  CMA_GATEWAY_SOURCE_SHA256="${reviewed_source_sha256}"
}

cma_gateway_validate_projection_receipt() {
  local projection_dir projection_file projection_digest_file
  local entry_count recorded_digest live_digest work_dir candidate_file projection_entry

  projection_dir="${CMA_GATEWAY_PROJECTION_DIR:-}"
  [[ "${projection_dir}" == /* && "${projection_dir}" != / && \
    -d "${projection_dir}" && ! -L "${projection_dir}" && -O "${projection_dir}" ]] || \
    cma_gateway_die "CMA_GATEWAY_PROJECTION_DIR must name the reviewed owned capture directory"
  [[ "${CMA_GATEWAY_PROJECTION_SHA256:-}" =~ ^[0-9a-f]{64}$ ]] || \
    cma_gateway_die "CMA_GATEWAY_PROJECTION_SHA256 must be the externally recorded projection digest"
  entry_count="$(find "${projection_dir}" -mindepth 1 -maxdepth 1 -print | \
    wc -l | tr -d ' ')" || cma_gateway_die "projection directory could not be enumerated"
  [[ "${entry_count}" == 3 ]] || cma_gateway_die "projection directory must contain exactly three receipts"
  projection_file="${projection_dir}/projection.json"
  projection_digest_file="${projection_dir}/projection.sha256"
  for projection_entry in capture.json projection.json projection.sha256; do
    [[ -f "${projection_dir}/${projection_entry}" && \
      ! -L "${projection_dir}/${projection_entry}" && \
      -O "${projection_dir}/${projection_entry}" ]] || \
      cma_gateway_die "projection receipt is not an owned regular file: ${projection_entry}"
    [[ "$(cma_gateway_file_mode "${projection_dir}/${projection_entry}")" == 400 ]] || \
      cma_gateway_die "projection receipt mode is not 0400: ${projection_entry}"
  done
  recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" { print $1 }' \
    "${projection_digest_file}")" || cma_gateway_die "projection digest receipt could not be read"
  live_digest="$(cma_gateway_sha256_file "${projection_file}")" || \
    cma_gateway_die "projection receipt could not be hashed"
  if ! cma_gateway_sha256_valid "${recorded_digest}" || \
     ! cma_gateway_sha256_valid "${live_digest}"; then
    cma_gateway_die "projection receipt digest is malformed"
  fi
  [[ "${recorded_digest}" == "${live_digest}" && \
    "${live_digest}" == "${CMA_GATEWAY_PROJECTION_SHA256}" ]] || \
    cma_gateway_die "projection receipt differs from the externally recorded digest"

  work_dir="$(mktemp -d)" || cma_gateway_die "projection validation work directory failed"
  chmod 700 "${work_dir}" || cma_gateway_die "projection validation work directory is not private"
  candidate_file="${work_dir}/projection.json"
  cma_gateway_assert_source_seal || cma_gateway_die "source seal drifted before projection validation"
  timeout --signal=TERM --kill-after=2s 15s env -i \
    PATH="${PATH}" LC_ALL=C ruby --disable-gems \
    "${CMA_GATEWAY_PROJECTION_CONTRACT}" --emit \
    "${projection_dir}/capture.json" "${candidate_file}" >/dev/null || \
    cma_gateway_die "closed projection capture failed its reviewed contract"
  cmp "${candidate_file}" "${projection_file}" >/dev/null || \
    cma_gateway_die "projection receipt was not derived from its sealed capture"
  rm -rf "${work_dir}"
  "${CMA_GATEWAY_PROJECTION_CAPTURE}" --check-live "${projection_file}" >/dev/null || \
    cma_gateway_die "live Gateway graph no longer equals the reviewed closed projection"
}

cma_gateway_assert_source_seal() {
  local live_source_sha256

  cma_gateway_sha256_valid "${CMA_GATEWAY_SOURCE_SHA256}" || return 1
  live_source_sha256="$(cma_gateway_source_digest)" || return 1
  cma_gateway_sha256_valid "${live_source_sha256}" || return 1
  [[ "${live_source_sha256}" == "${CMA_GATEWAY_SOURCE_SHA256}" ]]
}

cma_gateway_prepare_desired() {
  CMA_GATEWAY_DESIRED_DIR="$(mktemp -d)" || \
    cma_gateway_die "a private desired-state directory could not be created"
  [[ -n "${CMA_GATEWAY_DESIRED_DIR}" && \
    -d "${CMA_GATEWAY_DESIRED_DIR}" && ! -L "${CMA_GATEWAY_DESIRED_DIR}" ]] || \
    cma_gateway_die "the desired-state directory is not an exact private directory"
  chmod 700 "${CMA_GATEWAY_DESIRED_DIR}" || \
    cma_gateway_die "the desired-state directory could not be protected"
  cma_gateway_source_contract --emit "${CMA_GATEWAY_DESIRED_DIR}" >/dev/null || \
    cma_gateway_die "the desired Gateway bundle could not be emitted"
  chmod 400 "${CMA_GATEWAY_DESIRED_DIR}"/*.json || \
    cma_gateway_die "the desired Gateway bundle could not be made read-only"
  CMA_GATEWAY_DESIRED_SHA256="$(cma_gateway_named_files_digest \
    "${CMA_GATEWAY_DESIRED_DIR}")" || \
    cma_gateway_die "the desired Gateway bundle could not be hashed"
  cma_gateway_sha256_valid "${CMA_GATEWAY_DESIRED_SHA256}" || \
    cma_gateway_die "the desired Gateway bundle hash is malformed"
}

cma_gateway_assert_desired_seal() {
  [[ -n "${CMA_GATEWAY_DESIRED_DIR}" && -n "${CMA_GATEWAY_DESIRED_SHA256}" ]] || \
    return 1
  cma_gateway_validate_files_seal \
    "${CMA_GATEWAY_DESIRED_DIR}" "${CMA_GATEWAY_DESIRED_SHA256}"
}

cma_gateway_capture_snapshot() {
  local route_name custody_root custody_mode

  [[ "${CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM:-}" == durable-off-host ]] || \
    cma_gateway_die \
      "set CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host after provisioning durable custody"
  custody_root="${CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT:-}"
  [[ "${custody_root}" == /* && "${custody_root}" != / && \
    -d "${custody_root}" && ! -L "${custody_root}" && -O "${custody_root}" ]] || \
    cma_gateway_die \
      "CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT must be an absolute, owned, pre-existing directory"
  custody_mode="$(cma_gateway_file_mode "${custody_root}")" || \
    cma_gateway_die "Gateway rollback custody permissions could not be read"
  [[ "${custody_mode}" == 700 ]] || \
    cma_gateway_die "Gateway rollback custody root must have mode 0700"

  CMA_GATEWAY_ROLLBACK_DIR="$(
    mktemp -d "${custody_root%/}/cma-gateway-route.XXXXXXXXXX"
  )" || cma_gateway_die "a rollback custody directory could not be created"
  [[ -n "${CMA_GATEWAY_ROLLBACK_DIR}" && \
    "${CMA_GATEWAY_ROLLBACK_DIR}" == \
      "${custody_root%/}/cma-gateway-route."* && \
    -d "${CMA_GATEWAY_ROLLBACK_DIR}" && ! -L "${CMA_GATEWAY_ROLLBACK_DIR}" ]] || \
    cma_gateway_die "the rollback custody directory is not an exact private directory"
  chmod 700 "${CMA_GATEWAY_ROLLBACK_DIR}" || \
    cma_gateway_die "the rollback custody directory could not be protected"
  for route_name in cma-api cma-web cma-http-redirect; do
    cma_gateway_kubectl get httproute "${route_name}" -o json \
      > "${CMA_GATEWAY_ROLLBACK_DIR}/${route_name}-route.json" || \
      cma_gateway_die "the ${route_name} rollback snapshot could not be read"
  done
  cma_gateway_kubectl get healthcheckpolicy cma-api -o json \
    > "${CMA_GATEWAY_ROLLBACK_DIR}/cma-api-health.json" || \
    cma_gateway_die "the cma-api health rollback snapshot could not be read"
  chmod 600 "${CMA_GATEWAY_ROLLBACK_DIR}"/*.json || \
    cma_gateway_die "the rollback snapshot files could not be protected"
  cma_gateway_source_contract \
    --seal-bundle "${CMA_GATEWAY_ROLLBACK_DIR}" >/dev/null || \
    cma_gateway_die "the rollback snapshot could not be sealed"
  CMA_GATEWAY_ROLLBACK_SHA256="$(cma_gateway_named_files_digest \
    "${CMA_GATEWAY_ROLLBACK_DIR}")" || \
    cma_gateway_die "the rollback snapshot could not be hashed"
  cma_gateway_sha256_valid "${CMA_GATEWAY_ROLLBACK_SHA256}" || \
    cma_gateway_die "the rollback snapshot hash is malformed"
  printf 'Gateway rollback material: %s\n' "${CMA_GATEWAY_ROLLBACK_DIR}" || \
    cma_gateway_die "the rollback custody path could not be reported"
  printf 'Gateway rollback SHA-256: %s\n' "${CMA_GATEWAY_ROLLBACK_SHA256}" || \
    cma_gateway_die "the rollback custody digest could not be reported"
  cma_gateway_validate_snapshot_seal \
    "${CMA_GATEWAY_ROLLBACK_DIR}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || \
    cma_gateway_die "the rollback snapshot seal could not be revalidated"
  cma_gateway_check --validate-snapshot "${CMA_GATEWAY_ROLLBACK_DIR}" >/dev/null || \
    cma_gateway_die "the rollback snapshot schema could not be validated"
  cma_gateway_check --check-snapshot "${CMA_GATEWAY_ROLLBACK_DIR}" >/dev/null || \
    cma_gateway_die "the rollback snapshot state could not be checked"
  CMA_GATEWAY_ROLLBACK_SEALED=1
  printf 'Gateway rollback custody acknowledged: %s\n' \
    "${CMA_GATEWAY_ROLLBACK_SHA256}" || \
    cma_gateway_die "durable rollback custody acknowledgement could not be reported"
}

cma_gateway_validate_files_seal() {
  local sealed_dir="$1"
  local expected_sha256="$2"
  local object_name entry_count observed_mode observed_sha256

  [[ "${expected_sha256}" =~ ^[0-9a-f]{64}$ ]] || return 1
  [[ -d "${sealed_dir}" && ! -L "${sealed_dir}" && -O "${sealed_dir}" ]] || \
    return 1
  observed_mode="$(cma_gateway_file_mode "${sealed_dir}")" || return 1
  [[ "${observed_mode}" == 700 ]] || return 1
  entry_count="$(
    find "${sealed_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' '
  )" || return 1
  [[ "${entry_count}" == 4 ]] || return 1
  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    [[ -f "${sealed_dir}/${object_name}.json" && \
      ! -L "${sealed_dir}/${object_name}.json" && \
      -O "${sealed_dir}/${object_name}.json" ]] || return 1
    observed_mode="$(cma_gateway_file_mode \
      "${sealed_dir}/${object_name}.json")" || return 1
    [[ "${observed_mode}" == 400 ]] || return 1
  done
  observed_sha256="$(cma_gateway_named_files_digest "${sealed_dir}")" || return 1
  cma_gateway_sha256_valid "${observed_sha256}" || return 1
  [[ "${observed_sha256}" == "${expected_sha256}" ]]
}

cma_gateway_validate_snapshot_seal() {
  local snapshot_dir="$1"
  local expected_sha256="$2"

  cma_gateway_validate_files_seal "${snapshot_dir}" "${expected_sha256}"
}

cma_gateway_probe_old_images() {
  local expected_app_discovery_status="$1"
  local app_auth direct_auth app_redirect direct_redirect direct_ready
  local app_openapi_status direct_openapi_status app_card_status direct_card_status

  if [[ -n "${CMA_GATEWAY_PROBE_DIR}" ]]; then
    rm -rf "${CMA_GATEWAY_PROBE_DIR}" >/dev/null 2>&1 || :
  fi
  CMA_GATEWAY_PROBE_DIR="$(mktemp -d)" || return 1
  [[ -n "${CMA_GATEWAY_PROBE_DIR}" ]] || return 1
  chmod 700 "${CMA_GATEWAY_PROBE_DIR}" || return 1

  curl --disable --noproxy '*' --fail --show-error --silent --max-time 15 --output /dev/null \
    https://bot.chrono-ai.fun/ || return 1
  curl --disable --noproxy '*' --fail --show-error --silent --max-time 15 \
    --output "${CMA_GATEWAY_PROBE_DIR}/app-kernels.json" \
    https://bot.chrono-ai.fun/api/v1/kernels || return 1
  curl --disable --noproxy '*' --fail --show-error --silent --max-time 15 \
    --output "${CMA_GATEWAY_PROBE_DIR}/direct-kernels.json" \
    https://bot-api.chrono-ai.fun/api/v1/kernels || return 1
  cmp "${CMA_GATEWAY_PROBE_DIR}/app-kernels.json" \
    "${CMA_GATEWAY_PROBE_DIR}/direct-kernels.json" || return 1

  app_auth="$(curl --disable --noproxy '*' --show-error --silent --max-time 15 \
    --write-out '|%{http_code}' https://bot.chrono-ai.fun/auth/session)" || return 1
  direct_auth="$(curl --disable --noproxy '*' --show-error --silent --max-time 15 \
    --write-out '|%{http_code}' https://bot-api.chrono-ai.fun/auth/session)" || return 1
  [[ "${app_auth}" == '{"authenticated":false}|401' ]] || return 1
  [[ "${direct_auth}" == '{"authenticated":false}|401' ]] || return 1

  direct_ready="$(curl --disable --noproxy '*' --fail --show-error --silent --max-time 15 \
    https://bot-api.chrono-ai.fun/ready)" || return 1
  printf '%s' "${direct_ready}" | \
    jq -e '. == {"status":"ready"}' >/dev/null || return 1

  app_openapi_status="$(curl --disable --noproxy '*' --show-error --silent \
    --max-time 15 --output /dev/null --write-out '%{http_code}' \
    https://bot.chrono-ai.fun/openapi.json)" || return 1
  direct_openapi_status="$(curl --disable --noproxy '*' --show-error --silent \
    --max-time 15 --output /dev/null --write-out '%{http_code}' \
    https://bot-api.chrono-ai.fun/openapi.json)" || return 1
  app_card_status="$(curl --disable --noproxy '*' --show-error --silent \
    --max-time 15 --output /dev/null --write-out '%{http_code}' \
    https://bot.chrono-ai.fun/.well-known/agent-card.json)" || return 1
  direct_card_status="$(curl --disable --noproxy '*' --show-error --silent \
    --max-time 15 --output /dev/null --write-out '%{http_code}' \
    https://bot-api.chrono-ai.fun/.well-known/agent-card.json)" || return 1
  [[ "${app_openapi_status}" == "${expected_app_discovery_status}" ]] || return 1
  [[ "${direct_openapi_status}" == 404 ]] || return 1
  [[ "${app_card_status}" == "${expected_app_discovery_status}" ]] || return 1
  [[ "${direct_card_status}" == 404 ]] || return 1

  app_redirect="$(curl --disable --noproxy '*' --show-error --silent --max-time 15 --output /dev/null \
    --write-out '%{http_code}|%{redirect_url}' http://bot.chrono-ai.fun/)" || return 1
  direct_redirect="$(curl --disable --noproxy '*' --show-error --silent --max-time 15 --output /dev/null \
    --write-out '%{http_code}|%{redirect_url}' http://bot-api.chrono-ai.fun/)" || return 1
  [[ "${app_redirect}" == '301|https://bot.chrono-ai.fun:443/' ]] || return 1
  [[ "${direct_redirect}" == '301|https://bot-api.chrono-ai.fun:443/' ]] || return 1

  rm -rf "${CMA_GATEWAY_PROBE_DIR}"
  CMA_GATEWAY_PROBE_DIR=""
}

cma_gateway_snapshot_discovery_status() {
  local snapshot_dir="$1"
  local request_path

  request_path="$(jq -er '.spec.default.config.httpHealthCheck.requestPath' \
    "${snapshot_dir}/cma-api-health.json")" || return 1
  case "${request_path}" in
    /livez) printf '%s\n' 200 ;;
    /ready) printf '%s\n' 404 ;;
    *) return 1 ;;
  esac
}

cma_gateway_get_current_object() {
  local object_name="$1"
  local output_file="$2"

  case "${object_name}" in
    cma-api-route)
      cma_gateway_kubectl get httproute cma-api -o json > "${output_file}" || return 1
      ;;
    cma-web-route)
      cma_gateway_kubectl get httproute cma-web -o json > "${output_file}" || return 1
      ;;
    cma-http-redirect-route)
      cma_gateway_kubectl get httproute cma-http-redirect -o json > "${output_file}" || \
        return 1
      ;;
    cma-api-health)
      cma_gateway_kubectl get healthcheckpolicy cma-api -o json > "${output_file}" || \
        return 1
      ;;
    *) return 1 ;;
  esac
  chmod 600 "${output_file}" || return 1
}

cma_gateway_current_allowed() {
  local current_file="$1"
  local snapshot_file="$2"
  local desired_file="$3"

  jq -e --slurpfile snapshot "${snapshot_file}" --slurpfile desired "${desired_file}" '
    ((keys - ["apiVersion","kind","metadata","spec","status"]) | length) == 0 and
    (((.metadata | keys) - [
      "annotations","creationTimestamp","finalizers","generation","labels",
      "managedFields","name","namespace","ownerReferences","resourceVersion","uid"
    ]) | length) == 0 and
    .metadata.uid == $snapshot[0].metadata.uid and
    ($snapshot[0].metadata.uid | type) == "string" and
    ($snapshot[0].metadata.resourceVersion | type) == "string" and
    (.metadata.resourceVersion | type) == "string" and
    .metadata.deletionTimestamp == null and
    (.metadata.creationTimestamp == null or
     (.metadata.creationTimestamp | type) == "string") and
    (.metadata.generation == null or (.metadata.generation | type) == "number") and
    (.metadata.managedFields == null or (.metadata.managedFields | type) == "array") and
    (.status == null or (.status | type) == "object") and
    .apiVersion == $snapshot[0].apiVersion and
    .kind == $snapshot[0].kind and
    .metadata.name == $snapshot[0].metadata.name and
    .metadata.namespace == $snapshot[0].metadata.namespace and
    (.metadata.annotations // {}) == ($snapshot[0].metadata.annotations // {}) and
    (.metadata.finalizers // []) == ($snapshot[0].metadata.finalizers // []) and
    (.metadata.ownerReferences // []) == ($snapshot[0].metadata.ownerReferences // []) and
    ((.spec == $snapshot[0].spec and
      (.metadata.labels // {}) == ($snapshot[0].metadata.labels // {})) or
     (.spec == $desired[0].spec and
      (.metadata.labels // {}) == ($desired[0].metadata.labels // {})))
  ' "${current_file}" >/dev/null
}

cma_gateway_capture_allowed_current_set() {
  local snapshot_dir="$1"
  local current_dir="$2"
  local object_name

  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    cma_gateway_get_current_object \
      "${object_name}" "${current_dir}/${object_name}.json" || return 1
    cma_gateway_current_allowed \
      "${current_dir}/${object_name}.json" \
      "${snapshot_dir}/${object_name}.json" \
      "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json" || return 1
  done
}

cma_gateway_build_replace_object() {
  local current_file="$1"
  local target_file="$2"
  local output_file="$3"

  jq -e --slurpfile target "${target_file}" '
    .apiVersion = $target[0].apiVersion |
    .kind = $target[0].kind |
    .metadata.name = $target[0].metadata.name |
    .metadata.namespace = $target[0].metadata.namespace |
    .metadata.labels = ($target[0].metadata.labels // {}) |
    .spec = $target[0].spec |
    del(.metadata.generation, .metadata.managedFields, .status) |
    select((.metadata.uid | type) == "string" and
           (.metadata.resourceVersion | type) == "string")
  ' "${current_file}" > "${output_file}" || return 1
  chmod 600 "${output_file}" || return 1
}

cma_gateway_object_matches_target() {
  local current_file="$1"
  local target_file="$2"

  jq -e --slurpfile target "${target_file}" '
    .spec == $target[0].spec and
    (.metadata.labels // {}) == ($target[0].metadata.labels // {})
  ' "${current_file}" >/dev/null
}

cma_gateway_validate_response() {
  local response_file="$1"
  local target_file="$2"
  local snapshot_file="$3"
  local desired_file="$4"

  cma_gateway_validate_snapshot_seal \
    "${CMA_GATEWAY_ROLLBACK_DIR}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || return 1
  cma_gateway_assert_source_seal || return 1
  cma_gateway_assert_desired_seal || return 1
  cma_gateway_current_allowed \
    "${response_file}" "${snapshot_file}" "${desired_file}" || return 1
  cma_gateway_object_matches_target "${response_file}" "${target_file}"
}

cma_gateway_replace_cas() {
  local current_file="$1"
  local target_file="$2"
  local response_file="$3"
  local snapshot_file="$4"
  local desired_file="$5"
  local payload_file

  cma_gateway_validate_snapshot_seal \
    "${CMA_GATEWAY_ROLLBACK_DIR}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || return 1
  cma_gateway_assert_source_seal || return 1
  cma_gateway_assert_desired_seal || return 1
  cma_gateway_current_allowed \
    "${current_file}" "${snapshot_file}" "${desired_file}" || return 1
  if cma_gateway_object_matches_target "${current_file}" "${target_file}"; then
    cp "${current_file}" "${response_file}" || return 1
    chmod 600 "${response_file}" || return 1
    return 0
  fi
  payload_file="${response_file}.payload"
  cma_gateway_build_replace_object \
    "${current_file}" "${target_file}" "${payload_file}" || return 1
  cma_gateway_assert_source_seal || return 1
  cma_gateway_assert_desired_seal || return 1
  cma_gateway_kubectl replace -f "${payload_file}" -o json > "${response_file}" || \
    return 1
  chmod 600 "${response_file}" || return 1
  cma_gateway_validate_response \
    "${response_file}" "${target_file}" "${snapshot_file}" "${desired_file}" || \
    return 1
  rm -f "${payload_file}"
}

cma_gateway_preflight_replacements() {
  local current_dir="$1"
  local snapshot_dir="$2"
  local target_dir="${3:-${CMA_GATEWAY_DESIRED_DIR}}"
  local object_name payload_file dry_run_response

  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    cma_gateway_validate_snapshot_seal \
      "${snapshot_dir}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || return 1
    cma_gateway_assert_source_seal || return 1
    cma_gateway_assert_desired_seal || return 1
    cma_gateway_current_allowed \
      "${current_dir}/${object_name}.json" \
      "${snapshot_dir}/${object_name}.json" \
      "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json" || return 1
    payload_file="${current_dir}/${object_name}.dry-run.json"
    cma_gateway_build_replace_object \
      "${current_dir}/${object_name}.json" \
      "${target_dir}/${object_name}.json" \
      "${payload_file}" || return 1
    dry_run_response="${payload_file}.response"
    cma_gateway_kubectl replace --dry-run=server -f "${payload_file}" -o json \
      > "${dry_run_response}" || return 1
    chmod 600 "${dry_run_response}" || return 1
    cma_gateway_validate_response \
      "${dry_run_response}" \
      "${target_dir}/${object_name}.json" \
      "${snapshot_dir}/${object_name}.json" \
      "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json" || return 1
    rm -f "${payload_file}" "${dry_run_response}"
  done
}

cma_gateway_wait_for_state() {
  local state_mode="$1"
  local snapshot_dir="${2:-}"
  local deadline=$((SECONDS + CMA_GATEWAY_WAIT_SECONDS))

  while ! cma_gateway_deadline_reached "${deadline}"; do
    if [[ "${state_mode}" == adopted ]]; then
      if cma_gateway_check \
          --check-adopted-from-snapshot "${snapshot_dir}" >/dev/null; then
        cma_gateway_deadline_reached "${deadline}" && return 1
        return 0
      fi
    elif [[ "${state_mode}" == snapshot ]]; then
      if cma_gateway_check --check-snapshot "${snapshot_dir}" >/dev/null; then
        cma_gateway_deadline_reached "${deadline}" && return 1
        return 0
      fi
    elif [[ "${state_mode}" == ready-transition ]]; then
      if cma_gateway_check --check-ready-transition "${snapshot_dir}" >/dev/null; then
        cma_gateway_deadline_reached "${deadline}" && return 1
        return 0
      fi
    elif [[ "${state_mode}" == rollback-routes ]]; then
      if cma_gateway_check --check-rollback-routes "${snapshot_dir}" >/dev/null; then
        cma_gateway_deadline_reached "${deadline}" && return 1
        return 0
      fi
    else
      return 1
    fi
    cma_gateway_deadline_reached "${deadline}" && return 1
    sleep 5 || return 1
    cma_gateway_deadline_reached "${deadline}" && return 1
  done
  return 1
}

cma_gateway_restore_one() {
  local object_name="$1"
  local snapshot_dir="$2"
  local restore_dir="$3"
  local attempt=0 current_file response_file

  current_file="${restore_dir}/${object_name}.current.json"
  response_file="${restore_dir}/${object_name}.response.json"
  while [[ "${attempt}" -lt 5 ]]; do
    cma_gateway_validate_snapshot_seal \
      "${snapshot_dir}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || return 1
    cma_gateway_assert_source_seal || return 1
    cma_gateway_assert_desired_seal || return 1
    cma_gateway_get_current_object "${object_name}" "${current_file}" || return 1
    cma_gateway_current_allowed \
      "${current_file}" \
      "${snapshot_dir}/${object_name}.json" \
      "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json" || return 1
    if cma_gateway_replace_cas \
        "${current_file}" \
        "${snapshot_dir}/${object_name}.json" \
        "${response_file}" \
        "${snapshot_dir}/${object_name}.json" \
        "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json"; then
      return 0
    fi
    attempt=$((attempt + 1))
  done
  return 1
}

cma_gateway_restore() {
  local snapshot_dir="$1"
  local require_preflight="${2:-0}"
  local restore_dir restore_status=0 object_name expected_status health_current

  cma_gateway_validate_snapshot_seal \
    "${snapshot_dir}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || return 1
  cma_gateway_check --validate-snapshot "${snapshot_dir}" >/dev/null || return 1
  expected_status="$(cma_gateway_snapshot_discovery_status "${snapshot_dir}")" || \
    return 1

  restore_dir="$(mktemp -d)" || return 1
  [[ -n "${restore_dir}" ]] || return 1
  chmod 700 "${restore_dir}" || {
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  }
  # All identities and allowed source/adopted specs are checked before the
  # first restore write. Each write then uses the freshly read resourceVersion.
  cma_gateway_capture_allowed_current_set "${snapshot_dir}" "${restore_dir}" || {
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  }
  if [[ "${require_preflight}" == 1 ]]; then
    cma_gateway_preflight_replacements \
      "${restore_dir}" "${snapshot_dir}" "${snapshot_dir}" || {
      rm -rf "${restore_dir}" >/dev/null 2>&1 || :
      return 1
    }
  elif [[ "${require_preflight}" != 0 ]]; then
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  fi

  for object_name in cma-api-route cma-web-route cma-http-redirect-route; do
    cma_gateway_restore_one "${object_name}" "${snapshot_dir}" "${restore_dir}" || \
      restore_status=1
  done
  if ! cma_gateway_wait_for_state rollback-routes "${snapshot_dir}" || \
     ! cma_gateway_probe_old_images "${expected_status}"; then
    printf '%s\n' \
      "route rollback is unproved; refusing to change the API health policy" >&2 || :
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  fi
  health_current="${restore_dir}/cma-api-health.proof.json"
  cma_gateway_get_current_object cma-api-health "${health_current}" || {
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  }
  cma_gateway_current_allowed \
    "${health_current}" \
    "${snapshot_dir}/cma-api-health.json" \
    "${CMA_GATEWAY_DESIRED_DIR}/cma-api-health.json" || {
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  }
  if cma_gateway_object_matches_target \
      "${health_current}" "${snapshot_dir}/cma-api-health.json"; then
    :
  else
    cma_gateway_restore_one cma-api-health \
      "${snapshot_dir}" "${restore_dir}" || restore_status=1
  fi

  # A reported replace failure is not final if another actor completed the
  # exact rollback. Only the effective source-state proof decides success.
  if ! cma_gateway_wait_for_state snapshot "${snapshot_dir}" || \
     ! cma_gateway_probe_old_images "${expected_status}" || \
     ! cma_gateway_check --check-snapshot "${snapshot_dir}" >/dev/null; then
    rm -rf "${restore_dir}" >/dev/null 2>&1 || :
    return 1
  fi
  if [[ "${restore_status}" -ne 0 ]]; then
    printf '%s\n' \
      "one restore request reported failure, but exact effective source state was independently proved" >&2
  fi
  rm -rf "${restore_dir}" >/dev/null 2>&1 || :
}

cma_gateway_forward_one() {
  local object_name="$1"
  local current_file response_file

  current_file="${CMA_GATEWAY_WORK_DIR}/${object_name}.json"
  response_file="${CMA_GATEWAY_WORK_DIR}/${object_name}.response.json"
  cma_gateway_get_current_object "${object_name}" "${current_file}" || return 1
  cma_gateway_replace_cas \
    "${current_file}" \
    "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json" \
    "${response_file}" \
    "${CMA_GATEWAY_ROLLBACK_DIR}/${object_name}.json" \
    "${CMA_GATEWAY_DESIRED_DIR}/${object_name}.json"
}

cma_gateway_on_exit() {
  trap '' HUP INT QUIT TERM
  local original_status="$1"
  local rollback_status=0
  trap - EXIT
  set +e
  if [[ "${CMA_GATEWAY_MUTATED}" -eq 1 ]]; then
    if [[ "${original_status}" -eq 0 ]]; then
      original_status=1
    fi
    cma_gateway_restore "${CMA_GATEWAY_ROLLBACK_DIR}"
    rollback_status=$?
  fi

  if [[ -n "${CMA_GATEWAY_PROBE_DIR}" ]]; then
    rm -rf "${CMA_GATEWAY_PROBE_DIR}" >/dev/null 2>&1 || :
  fi
  if [[ -n "${CMA_GATEWAY_WORK_DIR}" ]]; then
    rm -rf "${CMA_GATEWAY_WORK_DIR}" >/dev/null 2>&1 || :
  fi
  if [[ "${CMA_GATEWAY_MUTATED}" -eq 1 ]]; then
    if [[ "${rollback_status}" -eq 0 ]]; then
      printf '%s\n' \
        "Gateway rollback is effective; retained directory: ${CMA_GATEWAY_ROLLBACK_DIR}; SHA-256: ${CMA_GATEWAY_ROLLBACK_SHA256}" >&2 || :
    else
      printf '%s\n' \
        "CRITICAL: Gateway rollback is unproved; retained directory: ${CMA_GATEWAY_ROLLBACK_DIR}; SHA-256: ${CMA_GATEWAY_ROLLBACK_SHA256}; keep v0.7.0, stop the release, and run the sealed --restore command" >&2 || :
    fi
  elif [[ -n "${CMA_GATEWAY_ROLLBACK_DIR}" && \
      "${CMA_GATEWAY_ROLLBACK_SEALED}" -eq 0 ]]; then
    rm -rf "${CMA_GATEWAY_ROLLBACK_DIR}" >/dev/null 2>&1 || :
  elif [[ -n "${CMA_GATEWAY_ROLLBACK_DIR}" && \
      "${CMA_GATEWAY_ROLLBACK_SEALED}" -eq 1 ]]; then
    printf '%s\n' \
      "Gateway adoption stopped before mutation; retained sealed custody at ${CMA_GATEWAY_ROLLBACK_DIR}; SHA-256: ${CMA_GATEWAY_ROLLBACK_SHA256}" >&2 || :
  fi
  if [[ -n "${CMA_GATEWAY_DESIRED_DIR}" ]]; then
    rm -rf "${CMA_GATEWAY_DESIRED_DIR}" >/dev/null 2>&1 || :
  fi
  trap - HUP INT QUIT TERM
  exit "${original_status}"
}

cma_gateway_on_signal() {
  trap '' HUP INT QUIT TERM
  exit 1
}

cma_gateway_adopt() {
  local expected_status object_name adopted_projection_parent adopted_projection_dir

  unset CURL_CA_BUNDLE CURL_SSL_BACKEND SSL_CERT_DIR SSL_CERT_FILE SSLKEYLOGFILE
  [[ "${CMA_GATEWAY_CONFIRM:-}" == adopt-v0.10-route ]] || \
    cma_gateway_die "set CMA_GATEWAY_CONFIRM=adopt-v0.10-route for authorized adoption"
  cma_gateway_require_privileged_bash
  cma_gateway_require_production_mutation_enabled || \
    cma_gateway_die "the production mutation gate could not be proved"
  cma_gateway_require_checkout || \
    cma_gateway_die "the reviewed release checkout could not be proved"
  trap 'cma_gateway_on_exit "$?"' EXIT
  trap cma_gateway_on_signal HUP INT QUIT TERM
  cma_gateway_prepare_desired || \
    cma_gateway_die "the desired Gateway bundle could not be prepared"
  cma_gateway_verify_admission_receipt || \
    cma_gateway_die "the permanent admission bootstrap receipt/live proof is invalid"
  cma_gateway_validate_projection_receipt || \
    cma_gateway_die "the pre-adoption closed projection could not be proved"
  cma_gateway_capture_snapshot || \
    cma_gateway_die "the rollback snapshot could not be captured"
  expected_status="$(cma_gateway_snapshot_discovery_status \
    "${CMA_GATEWAY_ROLLBACK_DIR}")" || \
    cma_gateway_die "the rollback discovery contract could not be read"
  cma_gateway_probe_old_images "${expected_status}" || \
    cma_gateway_die "the pre-adoption public route probes failed"

  CMA_GATEWAY_WORK_DIR="$(mktemp -d)" || \
    cma_gateway_die "a private adoption work directory could not be created"
  [[ -n "${CMA_GATEWAY_WORK_DIR}" && \
    -d "${CMA_GATEWAY_WORK_DIR}" && ! -L "${CMA_GATEWAY_WORK_DIR}" ]] || \
    cma_gateway_die "the adoption work directory is not an exact private directory"
  chmod 700 "${CMA_GATEWAY_WORK_DIR}" || \
    cma_gateway_die "the adoption work directory could not be protected"
  cma_gateway_capture_allowed_current_set \
    "${CMA_GATEWAY_ROLLBACK_DIR}" "${CMA_GATEWAY_WORK_DIR}" || \
    cma_gateway_die "the current Gateway objects could not be captured"
  cma_gateway_preflight_replacements \
    "${CMA_GATEWAY_WORK_DIR}" "${CMA_GATEWAY_ROLLBACK_DIR}" \
    "${CMA_GATEWAY_DESIRED_DIR}" || \
    cma_gateway_die "the forward compare-and-swap preflight failed"
  cma_gateway_preflight_replacements \
    "${CMA_GATEWAY_WORK_DIR}" "${CMA_GATEWAY_ROLLBACK_DIR}" \
    "${CMA_GATEWAY_ROLLBACK_DIR}" || \
    cma_gateway_die "the rollback compare-and-swap preflight failed"
  cma_gateway_validate_snapshot_seal \
    "${CMA_GATEWAY_ROLLBACK_DIR}" "${CMA_GATEWAY_ROLLBACK_SHA256}" || \
    cma_gateway_die "the rollback snapshot seal drifted before mutation"
  cma_gateway_assert_source_seal || \
    cma_gateway_die "the reviewed source bundle drifted before mutation"
  cma_gateway_assert_desired_seal || \
    cma_gateway_die "the desired Gateway bundle drifted before mutation"

  CMA_GATEWAY_MUTATED=1
  cma_gateway_forward_one cma-api-health || \
    cma_gateway_die "the cma-api health-policy adoption failed"
  cma_gateway_wait_for_state ready-transition "${CMA_GATEWAY_ROLLBACK_DIR}" || \
    cma_gateway_die "the /ready health transition did not converge"
  cma_gateway_probe_old_images "${expected_status}" || \
    cma_gateway_die "the public route probes failed after the health transition"
  for object_name in cma-api-route cma-web-route cma-http-redirect-route; do
    cma_gateway_forward_one "${object_name}" || \
      cma_gateway_die "the ${object_name} adoption failed"
  done

  cma_gateway_wait_for_state adopted "${CMA_GATEWAY_ROLLBACK_DIR}" || \
    cma_gateway_die "the adopted Gateway state did not converge"
  cma_gateway_probe_old_images 404 || \
    cma_gateway_die "the post-adoption public route probes failed"
  cma_gateway_check \
    --check-adopted-from-snapshot "${CMA_GATEWAY_ROLLBACK_DIR}" >/dev/null || \
    cma_gateway_die "the adopted Gateway state failed its final proof"

  adopted_projection_parent="${CMA_GATEWAY_ADOPTED_PROJECTION_PARENT:-}"
  [[ "${adopted_projection_parent}" == /* && "${adopted_projection_parent}" != / && \
    -d "${adopted_projection_parent}" && ! -L "${adopted_projection_parent}" && \
    -O "${adopted_projection_parent}" ]] || \
    cma_gateway_die "CMA_GATEWAY_ADOPTED_PROJECTION_PARENT must be a preselected owned absolute custody directory"
  adopted_projection_dir="${adopted_projection_parent%/}/adopted-${CMA_GATEWAY_RELEASE_REVISION}"
  cma_gateway_assert_source_seal || \
    cma_gateway_die "the reviewed source bundle drifted before adopted projection capture"
  CMA_GATEWAY_PROJECTION_STAGE=adopted \
    CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="${CMA_GATEWAY_PROJECTION_SHA256}" \
    cma_gateway_capture_projection \
      --capture "${adopted_projection_dir}" source >/dev/null || \
    cma_gateway_die "the stable adopted closed projection could not be captured while rollback remained armed"
  cma_gateway_assert_source_seal || \
    cma_gateway_die "the reviewed source bundle drifted during adopted projection capture"
  [[ -f "${adopted_projection_dir}/projection.sha256" ]] || \
    cma_gateway_die "the adopted projection digest receipt is absent"

  trap '' HUP INT QUIT TERM
  printf '%s\n' \
    "CMA Gateway route and /ready health check are adopted and effective; retain sealed rollback custody at ${CMA_GATEWAY_ROLLBACK_DIR} with SHA-256 ${CMA_GATEWAY_ROLLBACK_SHA256}; adopted projection ${adopted_projection_dir}" || \
    cma_gateway_die "the successful Gateway adoption could not be acknowledged"
  CMA_GATEWAY_MUTATED=0
  trap - EXIT
  rm -rf "${CMA_GATEWAY_WORK_DIR}" "${CMA_GATEWAY_DESIRED_DIR}" \
    >/dev/null 2>&1 || :
  CMA_GATEWAY_WORK_DIR=""
  CMA_GATEWAY_DESIRED_DIR=""
  trap - HUP INT QUIT TERM
}

cma_gateway_manual_restore() {
  local snapshot_dir="$1"

  unset CURL_CA_BUNDLE CURL_SSL_BACKEND SSL_CERT_DIR SSL_CERT_FILE SSLKEYLOGFILE
  [[ "${CMA_GATEWAY_CONFIRM:-}" == restore-v0.10-route ]] || \
    cma_gateway_die "set CMA_GATEWAY_CONFIRM=restore-v0.10-route for authorized restore"
  [[ "${CMA_GATEWAY_ROLLBACK_SHA256:-}" =~ ^[0-9a-f]{64}$ ]] || \
    cma_gateway_die "set CMA_GATEWAY_ROLLBACK_SHA256 to the printed snapshot seal"
  cma_gateway_require_privileged_bash
  cma_gateway_require_production_mutation_enabled || \
    cma_gateway_die "the production mutation gate could not be proved"
  cma_gateway_require_checkout || \
    cma_gateway_die "the reviewed release checkout could not be proved"
  cma_gateway_prepare_desired || \
    cma_gateway_die "the desired Gateway bundle could not be prepared"
  CMA_GATEWAY_ROLLBACK_DIR="${snapshot_dir}"
  trap '' HUP INT QUIT TERM
  if ! cma_gateway_restore "${snapshot_dir}" 1; then
    trap - HUP INT QUIT TERM
    [[ -z "${CMA_GATEWAY_PROBE_DIR}" ]] || \
      rm -rf "${CMA_GATEWAY_PROBE_DIR}" >/dev/null 2>&1 || :
    [[ -z "${CMA_GATEWAY_DESIRED_DIR}" ]] || \
      rm -rf "${CMA_GATEWAY_DESIRED_DIR}" >/dev/null 2>&1 || :
    cma_gateway_die "restored Gateway state could not be proved"
  fi
  rm -rf "${CMA_GATEWAY_DESIRED_DIR}" >/dev/null 2>&1 || :
  CMA_GATEWAY_DESIRED_DIR=""
  printf '%s\n' "CMA Gateway rollback is exact and effective" || \
    cma_gateway_die "the successful Gateway rollback could not be acknowledged"
  trap - HUP INT QUIT TERM
}

cma_gateway_self_test() {
  local test_document

  test_document='{"openapi":"3.1.0","paths":{"/api/v1/sessions":{"get":{}}}}'
  printf '%s' "${test_document}" | \
    jq -e '.openapi == "3.1.0" and
           (.paths["/api/v1/sessions"].get | type) == "object"' >/dev/null
  if printf '%s' '{"openapi":"3.1.0","paths":{}}' | \
      jq -e '.openapi == "3.1.0" and
             (.paths["/api/v1/sessions"].get | type) == "object"' >/dev/null; then
    cma_gateway_die "self-test accepted an unrelated OpenAPI document"
  fi
  printf '%s\n' "CMA Trigger Gateway adoption self-test passed"
}

cma_gateway_usage() {
  cat >&2 <<'USAGE'
usage:
  CMA_GATEWAY_RELEASE_REVISION=<40-hex> CMA_GATEWAY_CONFIRM=adopt-v0.10-route \
    CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer \
    CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT=<durable-absolute-directory> \
    CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host \
    cma-trigger-gateway-launch.sh adoption --adopt
  CMA_GATEWAY_RELEASE_REVISION=<40-hex> CMA_GATEWAY_CONFIRM=restore-v0.10-route \
    CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer \
    CMA_GATEWAY_ROLLBACK_SHA256=<64-hex> \
    cma-trigger-gateway-launch.sh adoption --restore ROLLBACK_DIR
  cma-trigger-gateway-adoption.sh --self-test

--adopt and --restore are production mutations and both are hard-disabled by
the same reviewed source constant. Forward and restore writes use
resourceVersion compare-and-swap; ordinary termination signals are masked while
rollback is in progress. Rollback files must be synchronously durable off-host
before mutation; SIGKILL or host loss uses that sealed manual path.
The sole-spec-writer confirmation attests that direct changes to the managed
routes/policy, shared Gateway, cma-api/cma-web Services and BackendPolicies, and
generated Compute health/backend resources are fenced for the whole operation;
normal GKE controller reconciliation remains allowed.
The trusted launcher executes through /usr/bin/env -i with a single explicitly
reviewed absolute PATH; its attestation covers that host's binaries and dynamic
loader. The scripts refuse a caller-inherited PATH or an unapproved launcher.
USAGE
}

if [[ "${BASH_SOURCE[0]}" != "$0" ]]; then
  return 0
fi

case "${1:-}" in
  --adopt)
    [[ "$#" -eq 1 ]] || cma_gateway_die "--adopt takes no arguments"
    cma_gateway_adopt
    ;;
  --restore)
    [[ "$#" -eq 2 ]] || cma_gateway_die "--restore requires one directory"
    cma_gateway_manual_restore "$2"
    ;;
  --self-test)
    [[ "$#" -eq 1 ]] || cma_gateway_die "--self-test takes no arguments"
    cma_gateway_self_test
    ;;
  *)
    cma_gateway_usage
    exit 2
    ;;
esac
