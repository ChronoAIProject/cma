#!/bin/bash -p

# Never inherit xtrace into a shell that handles the release bearer credential.
{ set +x; } 2>/dev/null
if ! umask 077; then
  echo "a restrictive file-creation mask is required before traffic restoration" >&2
  exit 1
fi
if ! ulimit -c 0; then
  echo "core dumps must be disabled before traffic-restoration startup" >&2
  exit 1
fi
set -euo pipefail

unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB
unset PYTHONHOME PYTHONPATH
for cma_step10_startup_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_step10_startup_name}"
done
unset cma_step10_startup_name

readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0
cma_pinned_api_root_identity=""
CMA_GATEWAY_RELEASE_TAG=""
readonly CMA_GATEWAY_REVIEWED_SOURCE_PATHS="infra/api/deployment.yaml infra/web/deployment.yaml infra/api/configmap.yaml .github/scripts/cma-trigger-gateway-cutover-fence-policy.yaml .github/scripts/cma-trigger-gateway-admission-bootstrap.sh .github/scripts/cma-trigger-gateway-live-policy.json .github/scripts/cma-trigger-gateway-cloudflare-capture.sh .github/scripts/cma-trigger-gateway-cloudflare-policy.json .github/scripts/cma-trigger-gateway-projection.rb .github/scripts/cma-trigger-gateway-projection.sh .github/scripts/cma-trigger-gateway-tls-sanitize.rb .github/scripts/cma-trigger-gateway-origin-evidence.sh .github/scripts/cma-trigger-gateway-launch.sh .github/scripts/cma-trigger-gateway-state.sh .github/scripts/cma-trigger-gateway-adoption.sh .github/scripts/cma-trigger-gateway-restore-traffic.sh"

cma_step10_require_trusted_launcher() {
  if [ "${CMA_GATEWAY_TRUSTED_LAUNCH:-}" != env-i-v1 ] || \
     [ "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" != reviewed-host-toolchain ] || \
     [ -z "${CMA_GATEWAY_TRUSTED_PATH:-}" ] || \
     [ "${PATH}" != "${CMA_GATEWAY_TRUSTED_PATH}" ]; then
    echo "traffic restoration requires the reviewed env-i host-toolchain launcher" >&2
    return 1
  fi
}
cma_step10_reject_unexpected_token_fd() {
  if [ -n "${CMA_GATEWAY_TOKEN_FD:-}" ] || [ -e /dev/fd/8 ]; then
    exec 8<&- 2>/dev/null || :
    unset CMA_GATEWAY_TOKEN_FD CHRONO_RELEASE_ACCESS_TOKEN
    echo "read-only/fence mode received an unexpected bearer descriptor" >&2
    return 1
  fi
}

cma_step10_sha256_stream() {
  if command -v sha256sum >/dev/null 2>&1; then
    timeout --signal=TERM --kill-after=2s 10s sha256sum
  elif command -v shasum >/dev/null 2>&1; then
    timeout --signal=TERM --kill-after=2s 10s shasum -a 256
  else
    echo "a SHA-256 command is required" >&2
    return 1
  fi
}
cma_step10_sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    timeout --signal=TERM --kill-after=2s 10s sha256sum "$1"
  elif command -v shasum >/dev/null 2>&1; then
    timeout --signal=TERM --kill-after=2s 10s shasum -a 256 "$1"
  else
    echo "a SHA-256 command is required" >&2
    return 1
  fi
}
cma_step10_sha256_valid() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]]
}
cma_step10_resolve_release_tag() {
  local cma_release_revision="$1"
  local cma_tag_record cma_tag_name cma_tag_object_type
  local cma_peeled_object_type cma_peeled_revision

  cma_tag_record="$(timeout --signal=TERM --kill-after=2s 10s \
    git --no-replace-objects for-each-ref \
      --points-at "${cma_release_revision}" \
      --format='%(refname:strip=2)|%(objecttype)|%(*objecttype)|%(*objectname)' \
      'refs/tags/v*')" || return 1
  [[ "${cma_tag_record}" != *$'\n'* ]] || return 1
  IFS='|' read -r cma_tag_name cma_tag_object_type \
    cma_peeled_object_type cma_peeled_revision <<<"${cma_tag_record}"
  [[ "${cma_tag_name}" =~ ^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ && \
    "${cma_tag_object_type}" == tag && "${cma_peeled_object_type}" == commit && \
    "${cma_peeled_revision}" == "${cma_release_revision}" ]] || return 1
  printf '%s\n' "${cma_tag_name}"
}
cma_step10_require_confirmation() {
  local cma_action="$1"
  local cma_scope="$2"
  local cma_expected_confirmation="${cma_action}-${CMA_GATEWAY_RELEASE_TAG}-${cma_scope}"

  if [ -z "${CMA_GATEWAY_RELEASE_TAG}" ] || \
     [ "${CMA_GATEWAY_CONFIRM}" != "${cma_expected_confirmation}" ]; then
    echo "CMA_GATEWAY_CONFIRM must equal ${cma_expected_confirmation}, derived from the reviewed release tag" >&2
    return 1
  fi
}
cma_step10_forbidden_index_entries() {
  awk 'substr($0, 1, 1) ~ /[a-zS]/ { print }'
}
cma_step10_require_checkout() {
  if [ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM}" != sole-spec-writer ]; then
    echo "CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM must attest the checked-routing-graph freeze" >&2
    return 1
  fi
  if [ "${#CMA_GATEWAY_RELEASE_REVISION}" -ne 40 ]; then
    echo "CMA_GATEWAY_RELEASE_REVISION is not a full commit SHA" >&2
    return 1
  fi
  case "${CMA_GATEWAY_RELEASE_REVISION}" in
    *[!0-9a-f]*)
      echo "CMA_GATEWAY_RELEASE_REVISION is not lowercase hexadecimal" >&2
      return 1
      ;;
  esac
  if ! cma_step10_head_revision="$(timeout --signal=TERM --kill-after=2s 10s \
    git --no-replace-objects rev-parse --verify 'HEAD^{commit}')"; then
    echo "the release checkout HEAD could not be resolved" >&2
    return 1
  fi
  if [ "${cma_step10_head_revision}" != "${CMA_GATEWAY_RELEASE_REVISION}" ]; then
    echo "the traffic-restoration checkout is not the reviewed release commit" >&2
    return 1
  fi
  if ! CMA_GATEWAY_RELEASE_TAG="$(cma_step10_resolve_release_tag \
    "${CMA_GATEWAY_RELEASE_REVISION}")"; then
    echo "the reviewed release revision must have exactly one annotated SemVer release tag" >&2
    return 1
  fi
  if ! cma_step10_worktree="$(timeout --signal=TERM --kill-after=2s 10s \
    git --no-replace-objects rev-parse --show-toplevel)"; then
    echo "the release checkout root could not be resolved" >&2
    return 1
  fi
  if [ "$(pwd -P)" != "$(cd "${cma_step10_worktree}" && pwd -P)" ]; then
    echo "traffic restoration must run from the reviewed checkout root" >&2
    return 1
  fi
  if ! cma_step10_dirty_paths="$(timeout --signal=TERM --kill-after=2s 10s \
    git --no-replace-objects -c core.fsmonitor=false \
      -c core.hooksPath=/dev/null status \
      --porcelain=v1 --untracked-files=all)"; then
    echo "the release checkout state could not be read" >&2
    return 1
  fi
  if [ -n "${cma_step10_dirty_paths}" ]; then
    echo "the traffic-restoration checkout is not completely clean" >&2
    return 1
  fi
  if ! cma_step10_index_flags="$(timeout --signal=TERM --kill-after=2s 10s \
    git --no-replace-objects ls-files -v | \
      cma_step10_forbidden_index_entries)"; then
    echo "the release checkout index flags could not be read" >&2
    return 1
  fi
  if [ -n "${cma_step10_index_flags}" ]; then
    echo "the traffic-restoration checkout contains assume-unchanged or skip-worktree entries" >&2
    return 1
  fi
  for cma_step10_source_path in ${CMA_GATEWAY_REVIEWED_SOURCE_PATHS}; do
    if [ ! -f "${cma_step10_source_path}" ] || [ -L "${cma_step10_source_path}" ]; then
      echo "reviewed release source is not a regular non-symlink file: ${cma_step10_source_path}" >&2
      return 1
    fi
    if ! cma_step10_committed_source_hash="$({
      git --no-replace-objects cat-file blob \
        "${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_source_path}" | cma_step10_sha256_stream
    })" || ! cma_step10_live_source_hash="$(cma_step10_sha256_file "${cma_step10_source_path}")"; then
      echo "reviewed release source could not be independently hashed: ${cma_step10_source_path}" >&2
      return 1
    fi
    if [ "${cma_step10_committed_source_hash%% *}" != "${cma_step10_live_source_hash%% *}" ]; then
      echo "reviewed release source differs from the commit blob: ${cma_step10_source_path}" >&2
      return 1
    fi
  done
  cma_step10_checker_path=.github/scripts/cma-trigger-gateway-state.sh
  if [ ! -f "${cma_step10_checker_path}" ] || \
     [ -L "${cma_step10_checker_path}" ] || \
     [ ! -x "${cma_step10_checker_path}" ]; then
    echo "the reviewed Gateway state checker is not an executable regular non-symlink file" >&2
    return 1
  fi
  if ! cma_step10_committed_hash_output="$(
    timeout --signal=TERM --kill-after=2s 10s \
      git --no-replace-objects cat-file blob \
      "${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_checker_path}" | \
      cma_step10_sha256_stream
  )"; then
    echo "the reviewed Gateway state-checker blob could not be hashed" >&2
    return 1
  fi
  if ! cma_step10_live_hash_output="$(
    cma_step10_sha256_file "${cma_step10_checker_path}"
  )"; then
    echo "the live Gateway state checker could not be hashed" >&2
    return 1
  fi
  cma_step10_committed_checker_sha256="${cma_step10_committed_hash_output%% *}"
  cma_step10_live_checker_sha256="${cma_step10_live_hash_output%% *}"
  if ! cma_step10_sha256_valid "${cma_step10_committed_checker_sha256}" || \
     ! cma_step10_sha256_valid "${cma_step10_live_checker_sha256}" || \
     [ "${cma_step10_live_checker_sha256}" != \
       "${cma_step10_committed_checker_sha256}" ]; then
    echo "the Gateway state checker differs from the reviewed commit blob" >&2
    return 1
  fi
  cma_step10_adoption_path=.github/scripts/cma-trigger-gateway-adoption.sh
  if [ ! -f "${cma_step10_adoption_path}" ] || \
     [ -L "${cma_step10_adoption_path}" ] || \
     [ ! -x "${cma_step10_adoption_path}" ]; then
    echo "the reviewed Gateway adoption helper is not an executable regular non-symlink file" >&2
    return 1
  fi
  if ! cma_step10_committed_adoption_hash_output="$(
    timeout --signal=TERM --kill-after=2s 10s \
      git --no-replace-objects cat-file blob \
      "${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_adoption_path}" | \
      cma_step10_sha256_stream
  )"; then
    echo "the reviewed Gateway adoption-helper blob could not be hashed" >&2
    return 1
  fi
  if ! cma_step10_live_adoption_hash_output="$(
    cma_step10_sha256_file "${cma_step10_adoption_path}"
  )"; then
    echo "the live Gateway adoption helper could not be hashed" >&2
    return 1
  fi
  cma_step10_committed_adoption_sha256="${cma_step10_committed_adoption_hash_output%% *}"
  cma_step10_live_adoption_sha256="${cma_step10_live_adoption_hash_output%% *}"
  if ! cma_step10_sha256_valid "${cma_step10_committed_adoption_sha256}" || \
     ! cma_step10_sha256_valid "${cma_step10_live_adoption_sha256}" || \
     [ "${cma_step10_live_adoption_sha256}" != \
       "${cma_step10_committed_adoption_sha256}" ]; then
    echo "the Gateway adoption helper differs from the reviewed commit blob" >&2
    return 1
  fi
  cma_step10_restore_path=.github/scripts/cma-trigger-gateway-restore-traffic.sh
  if [ ! -f "${cma_step10_restore_path}" ] || \
     [ -L "${cma_step10_restore_path}" ] || \
     [ ! -x "${cma_step10_restore_path}" ]; then
    echo "the reviewed traffic-restoration program is not an executable regular non-symlink file" >&2
    return 1
  fi
  if ! cma_step10_committed_restore_hash_output="$(
    timeout --signal=TERM --kill-after=2s 10s \
      git --no-replace-objects cat-file blob \
      "${CMA_GATEWAY_RELEASE_REVISION}:${cma_step10_restore_path}" | \
      cma_step10_sha256_stream
  )"; then
    echo "the reviewed traffic-restoration blob could not be hashed" >&2
    return 1
  fi
  if ! cma_step10_live_restore_hash_output="$(
    cma_step10_sha256_file "${cma_step10_restore_path}"
  )"; then
    echo "the live traffic-restoration program could not be hashed" >&2
    return 1
  fi
  cma_step10_committed_restore_sha256="${cma_step10_committed_restore_hash_output%% *}"
  cma_step10_live_restore_sha256="${cma_step10_live_restore_hash_output%% *}"
  if ! cma_step10_sha256_valid "${cma_step10_committed_restore_sha256}" || \
     ! cma_step10_sha256_valid "${cma_step10_live_restore_sha256}" || \
     [ "${cma_step10_live_restore_sha256}" != \
       "${cma_step10_committed_restore_sha256}" ]; then
    echo "the traffic-restoration program differs from the reviewed commit blob" >&2
    return 1
  fi
}
cma_step10_kubectl() {
  timeout --signal=TERM --kill-after=5s 25s \
    kubectl --request-timeout=15s \
      --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke \
      --namespace chrono-bot "$@"
}
cma_step10_gcloud() {
  # HOME intentionally selects the operator's default Cloud SDK configuration
  # and its active-config file. No ambient Cloud SDK override reaches gcloud.
  timeout --signal=TERM --kill-after=5s 25s \
    env -i \
      HOME="${HOME}" \
      PATH="${PATH}" \
      LC_ALL=C \
      CLOUDSDK_CORE_HTTP_TIMEOUT=15 \
      gcloud --quiet "$@"
}
cma_step10_deadline_reached() {
  [ "${SECONDS}" -ge "$1" ]
}
cma_step10_require_production_mutation_enabled() {
  if [ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" != 1 ]; then
    echo "production traffic restoration is disabled until the reviewed Compute, origin-chain, and client-IP evidence gates close" >&2
    return 1
  fi
}
cma_step10_require_privileged_bash() {
  if [[ "$-" != *p* ]]; then
    echo "production traffic restoration requires the reviewed /bin/bash -p entrypoint" >&2
    return 1
  fi
}
cma_step10_authenticated_curl() {
  local cma_auth_status=0
  export -n cma_auth_status

  curl --disable --noproxy '*' --fail --show-error --silent --max-time 15 \
    --header @/dev/fd/9 "$@" \
    9<<<"Authorization: Bearer ${cma_release_access_token}" || cma_auth_status=$?
  return "${cma_auth_status}"
}
cma_step10_validate_snapshot_seal() {
  # Positional arguments expand only inside the inner Bash process.
  # shellcheck disable=SC2016
  timeout --signal=TERM --kill-after=2s 15s \
    /bin/bash --noprofile --norc -p -c '
      set -euo pipefail
      source .github/scripts/cma-trigger-gateway-adoption.sh
      cma_gateway_validate_snapshot_seal "$1" "$2"
    ' cma-step10 "${CMA_GATEWAY_ROLLBACK_DIR}" \
      "${CMA_GATEWAY_ROLLBACK_SHA256}"
}
cma_step10_projection_capture() {
  local cma_projection_parent="$1"
  local cma_projection_label="$2"
  local cma_projection_mode="$3"
  local cma_projection_stage="$4"
  local cma_predecessor_file="$5"
  local cma_expected_predecessor_stage="$6"
  local cma_expected_predecessor_digest="$7"
  local cma_expected_predecessor_predecessor_digest="$8"
  local cma_projection_dir cma_predecessor_digest cma_recorded_digest

  if [ "${cma_projection_parent}" != "${cma_projection_parent%/}" ] || \
     [[ "${cma_projection_parent}" != /* ]] || [ "${cma_projection_parent}" = / ] || \
     [ ! -d "${cma_projection_parent}" ] || [ -L "${cma_projection_parent}" ] || \
     [ ! -O "${cma_projection_parent}" ]; then
    echo "projection custody parent is not an exact owned absolute directory" >&2
    return 1
  fi
  if [ ! -f "${cma_predecessor_file}" ] || [ -L "${cma_predecessor_file}" ]; then
    echo "projection predecessor is not a regular non-symlink receipt" >&2
    return 1
  fi
  [ "$(jq -er .lifecycle.stage "${cma_predecessor_file}")" = \
    "${cma_expected_predecessor_stage}" ] || {
    echo "projection predecessor has the wrong lifecycle stage" >&2
    return 1
  }
  cma_predecessor_digest="$(cma_step10_sha256_file "${cma_predecessor_file}")" || return 1
  cma_predecessor_digest="${cma_predecessor_digest%% *}"
  cma_step10_sha256_valid "${cma_expected_predecessor_digest}" && \
    [ "${cma_predecessor_digest}" = "${cma_expected_predecessor_digest}" ] || {
    echo "projection predecessor differs from its independently retained digest" >&2
    return 1
  }
  cma_recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" {print $1}' \
    "${cma_predecessor_file%/*}/projection.sha256")" || return 1
  [ "${cma_recorded_digest}" = "${cma_predecessor_digest}" ] || {
    echo "projection predecessor differs from its durable digest receipt" >&2
    return 1
  }
  case "${cma_expected_predecessor_stage}" in
    adopted)
      cma_step10_require_projection_file "${cma_predecessor_file}" \
        "${cma_expected_predecessor_digest}" \
        "${cma_expected_predecessor_predecessor_digest}" && \
        cma_step10_source_projection_matches_active_receipt \
          "${cma_predecessor_file}" || return 1
      ;;
    fenced-pre-scale | fenced-post-scale | fenced-pre-reopen)
      cma_step10_validate_fence_projection_receipt \
        "${cma_predecessor_file%/projection.json}" \
        "${cma_expected_predecessor_stage}" \
        "${cma_expected_predecessor_predecessor_digest}" || return 1
      ;;
    *) return 1 ;;
  esac
  cma_projection_dir="${cma_projection_parent}/${cma_projection_label}-${CMA_GATEWAY_RELEASE_REVISION}"
  CMA_GATEWAY_PROJECTION_STAGE="${cma_projection_stage}" \
    CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="${cma_predecessor_digest}" \
    .github/scripts/cma-trigger-gateway-projection.sh \
      --capture "${cma_projection_dir}" "${cma_projection_mode}" >/dev/null || return 1
  jq -e --arg stage "${cma_projection_stage}" \
    --arg predecessor "${cma_predecessor_digest}" '
      .lifecycle == {stage:$stage,predecessorSha256:$predecessor}
    ' "${cma_projection_dir}/projection.json" >/dev/null || return 1
  printf '%s\n' "${cma_projection_dir}"
}
cma_step10_projection_check_live() {
  .github/scripts/cma-trigger-gateway-projection.sh --check-live "$1" >/dev/null
}
cma_step10_projection_emit() {
  ruby --disable-gems .github/scripts/cma-trigger-gateway-projection.rb --emit "$1" "$2" >/dev/null
}
cma_step10_require_projection_file() {
  local cma_projection_file="$1"
  local cma_expected_digest="$2"
  local cma_expected_predecessor_digest="$3"
  local cma_projection_dir="${cma_projection_file%/*}"
  local cma_projection_digest_file="${cma_projection_dir}/projection.sha256"
  local cma_recorded_digest cma_live_digest cma_entry_count
  local cma_projection_entry cma_projection_work_dir

  [[ "${cma_projection_file}" == /*/projection.json ]] && \
    [ -d "${cma_projection_dir}" ] && [ ! -L "${cma_projection_dir}" ] && \
    [ -O "${cma_projection_dir}" ] || return 1
  cma_entry_count="$(find "${cma_projection_dir}" -mindepth 1 -maxdepth 1 -print | \
    wc -l | tr -d ' ')" || return 1
  [ "${cma_entry_count}" = 3 ] || return 1
  for cma_projection_entry in capture.json projection.json projection.sha256; do
    [ -f "${cma_projection_dir}/${cma_projection_entry}" ] && \
      [ ! -L "${cma_projection_dir}/${cma_projection_entry}" ] && \
      [ -O "${cma_projection_dir}/${cma_projection_entry}" ] || return 1
  done
  cma_recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" {print $1}' \
    "${cma_projection_digest_file}")" || return 1
  [ "$(wc -l < "${cma_projection_digest_file}" | tr -d ' ')" = 1 ] || return 1
  cma_live_digest="$(cma_step10_sha256_file "${cma_projection_file}")" || return 1
  cma_live_digest="${cma_live_digest%% *}"
  cma_step10_sha256_valid "${cma_expected_digest}" && \
    cma_step10_sha256_valid "${cma_expected_predecessor_digest}" && \
    cma_step10_sha256_valid "${cma_recorded_digest}" && \
    [ "${cma_recorded_digest}" = "${cma_live_digest}" ] && \
    [ "${cma_live_digest}" = "${cma_expected_digest}" ] || return 1
  jq -se --arg predecessor "${cma_expected_predecessor_digest}" '
    length == 2 and
    .[0].schemaVersion == 1 and .[0].mode == "source" and
    .[0].lifecycle == {stage:"adopted",predecessorSha256:$predecessor} and
    .[1].schemaVersion == 1 and .[1].mode == "source" and
    .[1].lifecycle == .[0].lifecycle
  ' "${cma_projection_dir}/capture.json" "${cma_projection_file}" >/dev/null || return 1
  (
    set -e
    cma_projection_work_dir="$(mktemp -d)"
    trap 'rm -rf "${cma_projection_work_dir}"' EXIT HUP INT QUIT TERM
    cma_step10_projection_emit "${cma_projection_dir}/capture.json" \
      "${cma_projection_work_dir}/projection.json"
    cmp "${cma_projection_work_dir}/projection.json" "${cma_projection_file}" >/dev/null
  )
}
cma_step10_projection_digest() {
  local cma_projection_file="$1"
  local cma_projection_digest_file="${cma_projection_file%/*}/projection.sha256"
  local cma_recorded_digest cma_live_digest

  [ -f "${cma_projection_file}" ] && [ ! -L "${cma_projection_file}" ] && \
    [ -f "${cma_projection_digest_file}" ] && \
    [ ! -L "${cma_projection_digest_file}" ] || return 1
  cma_recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" {print $1}' \
    "${cma_projection_digest_file}")" || return 1
  [ "$(wc -l < "${cma_projection_digest_file}" | tr -d ' ')" = 1 ] || return 1
  cma_live_digest="$(cma_step10_sha256_file "${cma_projection_file}")" || return 1
  cma_live_digest="${cma_live_digest%% *}"
  cma_step10_sha256_valid "${cma_recorded_digest}" && \
    [ "${cma_recorded_digest}" = "${cma_live_digest}" ] || return 1
  printf '%s\n' "${cma_live_digest}"
}
cma_step10_source_projection_matches_active_receipt() {
  local cma_projection_file="$1"
  local cma_projection_digest

  [ -n "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT:-}" ] && \
    [ -f "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] && \
    [ ! -L "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] || return 1
  cma_projection_digest="$(cma_step10_sha256_file "${cma_projection_file}")" || return 1
  cma_projection_digest="${cma_projection_digest%% *}"
  jq -e --arg path "${cma_projection_file}" \
    --arg digest "${cma_projection_digest}" \
    --slurpfile source "${cma_projection_file}" '
      ($source | length) == 1 and
      .sourceProjection.path == $path and
      .sourceProjection.sha256 == $digest and
      .sourceProjection.lifecycle == $source[0].lifecycle and
      .sourceProjection.captureSha256 == $source[0].captureSha256 and
      .sourceProjection.behaviorPolicy == $source[0].behaviorPolicy and
      .sourceProjection.admissionReceipt == $source[0].admissionReceipt and
      .workload != null and $source[0].graph.apiWorkload != null and
      .workload == $source[0].graph.apiWorkload
    ' "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" >/dev/null
}

cma_step10_validate_fence_projection_receipt() {
  local cma_projection_dir="$1"
  local cma_expected_stage="$2"
  local cma_expected_predecessor_digest="$3"
  local cma_projection_digest_file="${cma_projection_dir}/projection.sha256"
  local cma_projection_file="${cma_projection_dir}/projection.json"
  local cma_recorded_digest cma_live_digest cma_projection_entry cma_projection_work_dir
  local cma_projection_stage

  [ -d "${cma_projection_dir}" ] && [ ! -L "${cma_projection_dir}" ] && \
    [ -O "${cma_projection_dir}" ] && \
    [ "$(find "${cma_projection_dir}" -mindepth 1 -maxdepth 1 -print | \
      wc -l | tr -d ' ')" = 3 ] || return 1
  for cma_projection_entry in capture.json projection.json projection.sha256; do
    [ -f "${cma_projection_dir}/${cma_projection_entry}" ] && \
      [ ! -L "${cma_projection_dir}/${cma_projection_entry}" ] && \
      [ -O "${cma_projection_dir}/${cma_projection_entry}" ] || return 1
  done
  cma_recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" {print $1}' \
    "${cma_projection_digest_file}")" || return 1
  [ "$(wc -l < "${cma_projection_digest_file}" | tr -d ' ')" = 1 ] || return 1
  cma_live_digest="$(cma_step10_sha256_file "${cma_projection_file}")" || return 1
  cma_live_digest="${cma_live_digest%% *}"
  cma_step10_sha256_valid "${cma_recorded_digest}" && \
    [ "${cma_recorded_digest}" = "${cma_live_digest}" ] || return 1
  (
    set -e
    cma_projection_work_dir="$(mktemp -d)"
    trap 'rm -rf "${cma_projection_work_dir}"' EXIT HUP INT QUIT TERM
    cma_step10_projection_emit "${cma_projection_dir}/capture.json" \
      "${cma_projection_work_dir}/projection.json"
    cmp "${cma_projection_work_dir}/projection.json" "${cma_projection_file}" >/dev/null
  ) || return 1
  [ -n "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT:-}" ] && \
    [ -f "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] && \
    [ ! -L "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] || return 1
  cma_step10_sha256_valid "${cma_expected_predecessor_digest}" || return 1
  case "${cma_expected_stage}" in
    fenced-pre-scale | fenced-post-scale | fenced-pre-reopen) ;;
    *) return 1 ;;
  esac
  jq -e --arg expected_stage "${cma_expected_stage}" \
    --arg expected_predecessor "${cma_expected_predecessor_digest}" \
    --slurpfile receipt "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" '
    ($receipt | length) == 1 and
    ($receipt[0].service) as $root |
    ($receipt[0].workload) as $workload |
    (.lifecycle.stage) as $stage |
    (.fullCapture.kubernetes.services.items |
      map(select(.metadata.namespace == "chrono-bot" and .metadata.name == "cma-api"))) as $services |
    ($services | length) == 1 and
    .schemaVersion == 1 and .mode == "fenced" and
    .lifecycle == {
      stage:$expected_stage,predecessorSha256:$expected_predecessor
    } and $stage == $expected_stage and
    .graph.services["cma-api"].uid == $root.uid and
    .graph.services["cma-api"].name == $root.name and
    .graph.services["cma-api"].zones == $root.zones and
    .graph.services["cma-api"].backendSelfLink == $root.backendSelfLink and
    .graph.services["cma-api"].backendFingerprint == $root.backendFingerprint and
    .graph.services["cma-api"].backendGroups == $root.backendGroups and
    .behaviorPolicy == $receipt[0].sourceProjection.behaviorPolicy and
    .admissionReceipt == $receipt[0].sourceProjection.admissionReceipt and
    .graph.fenceAdmission == $receipt[0].admission and
    .graph.apiWorkload.deployment.uid == $workload.deployment.uid and
    (if $stage == "fenced-pre-scale" then
       .lifecycle.predecessorSha256 == $receipt[0].sourceProjection.sha256 and
       .graph.apiWorkload == $workload
     elif $stage == "fenced-post-scale" then
       .graph.apiWorkload.deployment.replicas == 0 and
       .graph.apiWorkload.pods == [] and .graph.apiWorkload.verified == null
     else
       .graph.apiWorkload.deployment.replicas == 1 and
       (.graph.apiWorkload.pods | length) == 1 and
       .graph.apiWorkload.pods[0].uid != $workload.pods[0].uid and
       .graph.apiWorkload.verified != null
     end) and
    .graph.services["cma-api"].effectiveMembers == [] and
    .graph.services["cma-api"].effectiveHealth == [] and
    .graph.services["cma-api"].uid == $services[0].metadata.uid
  ' "${cma_projection_file}" >/dev/null || return 1
  cma_projection_stage="$(jq -er .lifecycle.stage "${cma_projection_file}")" || return 1
  if [ "${cma_projection_stage}" = fenced-pre-reopen ]; then
    cma_step10_projection_matches_final_workload "${cma_projection_file}"
  fi
}
cma_step10_validate_fence_projection() {
  local cma_projection_dir="$1"
  local cma_expected_stage="$2"
  local cma_expected_predecessor_digest="$3"
  local cma_initial_digest cma_semantic_digest cma_final_digest

  cma_initial_digest="$(cma_step10_projection_digest \
    "${cma_projection_dir}/projection.json")" || return 1
  cma_step10_validate_fence_projection_receipt \
    "${cma_projection_dir}" "${cma_expected_stage}" \
    "${cma_expected_predecessor_digest}" || return 1
  cma_semantic_digest="$(cma_step10_projection_digest \
    "${cma_projection_dir}/projection.json")" || return 1
  [ "${cma_semantic_digest}" = "${cma_initial_digest}" ] || return 1
  cma_step10_projection_check_live \
    "${cma_projection_dir}/projection.json" || return 1
  cma_final_digest="$(cma_step10_projection_digest \
    "${cma_projection_dir}/projection.json")" || return 1
  [ "${cma_final_digest}" = "${cma_initial_digest}" ] || return 1
  printf '%s\n' "${cma_initial_digest}"
}
cma_step10_validate_scaled_down_workload() {
  local cma_deployment_json cma_pods_json cma_old_pods cma_old_pod_name
  local cma_old_pod_uid cma_old_pod_json

  [ -n "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT:-}" ] && \
    [ -f "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] && \
    [ ! -L "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] || return 1
  cma_deployment_json="$(cma_step10_kubectl get deployment cma-api -o json)" || return 1
  printf '%s' "${cma_deployment_json}" | jq -e \
    --slurpfile receipt "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" '
      ($receipt | length) == 1 and
      .apiVersion == "apps/v1" and .kind == "Deployment" and
      .metadata.name == "cma-api" and .metadata.namespace == "chrono-bot" and
      .metadata.uid == $receipt[0].workload.deployment.uid and
      .metadata.deletionTimestamp == null and
      (.metadata.generation | type) == "number" and .metadata.generation > 0 and
      .spec.replicas == 0 and .status.observedGeneration == .metadata.generation and
      (.status.replicas // 0) == 0 and (.status.updatedReplicas // 0) == 0 and
      (.status.readyReplicas // 0) == 0 and (.status.availableReplicas // 0) == 0 and
      (.status.unavailableReplicas // 0) == 0
    ' >/dev/null || return 1
  cma_pods_json="$(cma_step10_kubectl get pod \
    -l app.kubernetes.io/name=cma-api -o json)" || return 1
  printf '%s' "${cma_pods_json}" | jq -e '
    .apiVersion == "v1" and .kind == "List" and .items == []
  ' >/dev/null || return 1
  cma_old_pods="$(jq -er '.workload.pods[] | [.name,.uid] | @tsv' \
    "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}")" || return 1
  [ -n "${cma_old_pods}" ] || return 1
  while IFS=$'\t' read -r cma_old_pod_name cma_old_pod_uid; do
    [ -n "${cma_old_pod_name}" ] && [ -n "${cma_old_pod_uid}" ] || return 1
    cma_old_pod_json="$(cma_step10_kubectl get pod "${cma_old_pod_name}" \
      --ignore-not-found -o json)" || return 1
    [ -z "${cma_old_pod_json}" ] || return 1
  done <<<"${cma_old_pods}"
}
cma_step10_fsync_paths() {
  timeout --signal=TERM --kill-after=2s 10s env -i PATH="${PATH}" LC_ALL=C \
    ruby --disable-gems -e 'ARGV.each { |path| File.open(path, File::RDONLY) { |file| file.fsync } }' "$@"
}
cma_step10_sync_published_fence_bundle() {
  local cma_bundle_dir="$1"
  local cma_parent_dir="$2"

  timeout --signal=TERM --kill-after=2s 10s env -i PATH="${PATH}" LC_ALL=C \
    ruby --disable-gems -e '
      bundle, parent = ARGV
      raise "bundle is not an exact directory" unless
        File.directory?(bundle) && !File.symlink?(bundle)
      raise "bundle entry set is not exact" unless Dir.children(bundle).sort ==
        ["receipt.json", "receipt.sha256"]
      Dir.children(bundle).sort.each do |entry|
        path = File.join(bundle, entry)
        stat = File.lstat(path)
        raise "bundle entry is not a regular file" unless stat.file? && !stat.symlink?
        File.open(path, File::RDONLY) { |file| file.fsync }
      end
      File.open(bundle, File::RDONLY) { |directory| directory.fsync }
      File.open(parent, File::RDONLY) { |directory| directory.fsync }
    ' "${cma_bundle_dir}" "${cma_parent_dir}"
}

cma_step10_write_fence_receipt() {
  local cma_source_projection="$1"
  local cma_root_record="$2"
  local cma_receipt_dir="${CMA_GATEWAY_FENCE_RECEIPT_DIR:-}"
  local cma_bundle_dir cma_work_dir cma_receipt_file cma_receipt_digest
  local cma_published_receipt_digest cma_published_live_digest
  local cma_projection_digest

  cma_step10_require_projection_file "${cma_source_projection}" \
    "${CMA_GATEWAY_SOURCE_PROJECTION_SHA256:-}" \
    "${CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256:-}" || {
    echo "the pre-patch source projection is not the exact adopted durable bundle" >&2
    return 1
  }

  [ "${CMA_GATEWAY_FENCE_RECEIPT_CONFIRM:-}" = durable-prepatch-root-v1 ] || {
    echo "CMA_GATEWAY_FENCE_RECEIPT_CONFIRM must authorize durable-prepatch-root-v1" >&2
    return 1
  }
  if [[ "${cma_receipt_dir}" != /* || "${cma_receipt_dir}" == / ]] || \
     [ ! -d "${cma_receipt_dir}" ] || [ -L "${cma_receipt_dir}" ] || \
     [ ! -O "${cma_receipt_dir}" ]; then
    echo "CMA_GATEWAY_FENCE_RECEIPT_DIR must be the preselected owned off-host custody directory" >&2
    return 1
  fi
  cma_bundle_dir="${cma_receipt_dir%/}/fence-root-${CMA_GATEWAY_RELEASE_REVISION}"
  if [ -d "${cma_bundle_dir}" ] && [ ! -L "${cma_bundle_dir}" ]; then
    cma_step10_sync_published_fence_bundle \
      "${cma_bundle_dir}" "${cma_receipt_dir}" || return 1
    cma_receipt_file="${cma_bundle_dir}/receipt.json"
    cma_step10_sha256_valid "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:-}" || {
      echo "an existing fence receipt requires its independently recorded SHA-256" >&2
      return 1
    }
    cma_step10_load_fence_receipt && \
      cma_step10_source_matches_receipt \
        "${cma_source_projection}" "${cma_root_record}" "${cma_receipt_file}" || return 1
    printf '%s|%s\n' "${cma_receipt_file}" "${CMA_GATEWAY_FENCE_RECEIPT_SHA256}"
    return 0
  fi
  [ ! -e "${cma_bundle_dir}" ] && [ ! -L "${cma_bundle_dir}" ] || return 1
  cma_work_dir="$(mktemp -d \
    "${cma_receipt_dir%/}/.fence-root-${CMA_GATEWAY_RELEASE_REVISION}.XXXXXXXX")" || return 1
  chmod 700 "${cma_work_dir}" || return 1
  cma_receipt_file="${cma_work_dir}/receipt.json"
  cma_projection_digest="$(cma_step10_sha256_file "${cma_source_projection}")" || return 1
  cma_projection_digest="${cma_projection_digest%% *}"
  cma_step10_sha256_valid "${cma_projection_digest}" || return 1
  jq -n --arg revision "${CMA_GATEWAY_RELEASE_REVISION}" \
    --arg projection "${cma_source_projection}" --arg projection_sha256 "${cma_projection_digest}" \
    --argjson service_root "${cma_root_record}" \
    --slurpfile sealed "${cma_source_projection}" '
      ($sealed[0].graph.services["cma-api"]) as $api |
      {
        schemaVersion:1,releaseRevision:$revision,
        sourceProjection:{path:$projection,sha256:$projection_sha256,
          lifecycle:$sealed[0].lifecycle,
          captureSha256:$sealed[0].captureSha256,
          behaviorPolicy:$sealed[0].behaviorPolicy,
          admissionReceipt:$sealed[0].admissionReceipt},
        service:{uid:$api.uid,name:$api.name,zones:$api.zones,
          backendSelfLink:$api.backendSelfLink,
          backendFingerprint:$api.backendFingerprint,
          backendGroups:$api.backendGroups},
        admission:$sealed[0].graph.fenceAdmission,
        workload:$sealed[0].graph.apiWorkload
      } |
      if .sourceProjection.lifecycle.stage == "adopted" and
         (.sourceProjection.lifecycle.predecessorSha256 | test("^[0-9a-f]{64}$")) and
         .service.uid == $service_root.uid and .service.name == $service_root.name and
         .service.zones == $service_root.zones and (.service.backendGroups | length) > 0 and
         .workload.deployment.name == "cma-api" and
         .workload.deployment.namespace == "chrono-bot" and
         .workload.deployment.replicas == 1 and (.workload.pods | length) == 1
      then . else error("source projection does not equal pre-patch Service/NEG/backend root") end
    ' > "${cma_receipt_file}" || {
      rm -rf "${cma_work_dir}"
      return 1
    }
  chmod 400 "${cma_receipt_file}" || return 1
  cma_receipt_digest="$(cma_step10_sha256_file "${cma_receipt_file}")" || return 1
  cma_receipt_digest="${cma_receipt_digest%% *}"
  cma_step10_sha256_valid "${cma_receipt_digest}" || return 1
  printf '%s\n' \
    "CMA fence receipt candidate before publication: ${cma_bundle_dir}/receipt.json; SHA-256 ${cma_receipt_digest}" >&2 || return 1
  printf '%s  receipt.json\n' "${cma_receipt_digest}" \
    > "${cma_work_dir}/receipt.sha256" || return 1
  chmod 400 "${cma_work_dir}/receipt.sha256" || return 1
  cma_step10_fsync_paths "${cma_receipt_file}" "${cma_work_dir}/receipt.sha256" \
    "${cma_work_dir}" "${cma_receipt_dir}" || return 1
  if ! env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
      source, destination = ARGV
      raise "destination exists" if File.exist?(destination) || File.symlink?(destination)
      File.rename(source, destination)
    ' "${cma_work_dir}" "${cma_bundle_dir}"; then
    rm -rf "${cma_work_dir}"
    [ -d "${cma_bundle_dir}" ] && [ ! -L "${cma_bundle_dir}" ] || return 1
    cma_step10_sync_published_fence_bundle \
      "${cma_bundle_dir}" "${cma_receipt_dir}" || return 1
    cma_published_receipt_digest="$(awk \
      'NF == 2 && $2 == "receipt.json" {print $1}' \
      "${cma_bundle_dir}/receipt.sha256")" || return 1
    cma_published_live_digest="$(cma_step10_sha256_file \
      "${cma_bundle_dir}/receipt.json")" || return 1
    cma_published_live_digest="${cma_published_live_digest%% *}"
    cma_step10_sha256_valid "${cma_published_receipt_digest}" && \
      [ "${cma_published_receipt_digest}" = "${cma_receipt_digest}" ] && \
      [ "${cma_published_live_digest}" = "${cma_receipt_digest}" ] || return 1
    CMA_GATEWAY_FENCE_RECEIPT_SHA256="${cma_receipt_digest}"
    cma_step10_load_fence_receipt && \
      cma_step10_source_matches_receipt "${cma_source_projection}" \
        "${cma_root_record}" "${cma_bundle_dir}/receipt.json" || return 1
  else
    cma_step10_sync_published_fence_bundle \
      "${cma_bundle_dir}" "${cma_receipt_dir}" || return 1
  fi
  printf '%s|%s\n' "${cma_bundle_dir}/receipt.json" "${cma_receipt_digest}"
}

cma_step10_source_matches_receipt() {
  local cma_source_projection="$1"
  local cma_root_record="$2"
  local cma_receipt_file="$3"
  local cma_projection_digest

  cma_projection_digest="$(cma_step10_sha256_file "${cma_source_projection}")" || return 1
  cma_projection_digest="${cma_projection_digest%% *}"
  jq -e --arg projection "${cma_source_projection}" \
    --arg projection_sha "${cma_projection_digest}" \
    --argjson root "${cma_root_record}" \
    --slurpfile source "${cma_source_projection}" '
      ($source | length) == 1 and $source[0].schemaVersion == 1 and
      $source[0].mode == "source" and
      $source[0].lifecycle.stage == "adopted" and
      .sourceProjection.path == $projection and
      .sourceProjection.sha256 == $projection_sha and
      .sourceProjection.lifecycle == $source[0].lifecycle and
      .sourceProjection.captureSha256 == $source[0].captureSha256 and
      .sourceProjection.behaviorPolicy == $source[0].behaviorPolicy and
      .sourceProjection.admissionReceipt == $source[0].admissionReceipt and
      .admission == $source[0].graph.fenceAdmission and
      .workload != null and $source[0].graph.apiWorkload != null and
      .workload == $source[0].graph.apiWorkload and
      .service.uid == $root.uid and .service.name == $root.name and
      .service.zones == $root.zones and
      .service.uid == $source[0].graph.services["cma-api"].uid and
      .service.name == $source[0].graph.services["cma-api"].name and
      .service.zones == $source[0].graph.services["cma-api"].zones and
      .service.backendSelfLink == $source[0].graph.services["cma-api"].backendSelfLink and
      .service.backendFingerprint == $source[0].graph.services["cma-api"].backendFingerprint and
      .service.backendGroups == $source[0].graph.services["cma-api"].backendGroups
    ' "${cma_receipt_file}" >/dev/null
}

cma_step10_load_fence_receipt() {
  local cma_receipt_dir="${CMA_GATEWAY_FENCE_RECEIPT_DIR:-}"
  local cma_bundle_dir="${cma_receipt_dir%/}/fence-root-${CMA_GATEWAY_RELEASE_REVISION}"
  local cma_receipt_file="${cma_bundle_dir}/receipt.json"
  local cma_receipt_digest

  [[ "${cma_receipt_dir}" == /* && "${cma_receipt_dir}" != / ]] && \
    [ -d "${cma_bundle_dir}" ] && [ ! -L "${cma_bundle_dir}" ] && \
    [ -O "${cma_bundle_dir}" ] && \
    [ "$(find "${cma_bundle_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" = 2 ] && \
    [ -f "${cma_receipt_file}" ] && [ ! -L "${cma_receipt_file}" ] && \
    [ -f "${cma_bundle_dir}/receipt.sha256" ] && \
    [ ! -L "${cma_bundle_dir}/receipt.sha256" ] && \
    [ -O "${cma_receipt_file}" ] || return 1
  cma_receipt_digest="$(cma_step10_sha256_file "${cma_receipt_file}")" || return 1
  cma_receipt_digest="${cma_receipt_digest%% *}"
  [ "${cma_receipt_digest}" = "${CMA_GATEWAY_FENCE_RECEIPT_SHA256:-}" ] && \
    [ "$(awk 'NF == 2 && $2 == "receipt.json" {print $1}' \
      "${cma_bundle_dir}/receipt.sha256")" = "${cma_receipt_digest}" ] && \
    [ "$(wc -l < "${cma_bundle_dir}/receipt.sha256" | tr -d ' ')" = 1 ] || return 1
  jq -e --arg revision "${CMA_GATEWAY_RELEASE_REVISION}" '
    .schemaVersion == 1 and .releaseRevision == $revision and
    (.sourceProjection | keys | sort) == [
      "admissionReceipt","behaviorPolicy","captureSha256","lifecycle","path","sha256"
    ] and
    (.sourceProjection.sha256 | test("^[0-9a-f]{64}$")) and
    .sourceProjection.lifecycle.stage == "adopted" and
    (.sourceProjection.lifecycle.predecessorSha256 | test("^[0-9a-f]{64}$")) and
    (.sourceProjection.behaviorPolicy.policySha256 | test("^[0-9a-f]{64}$")) and
    (.sourceProjection.behaviorPolicy.sourceEvidenceSha256 | test("^[0-9a-f]{64}$")) and
    (.sourceProjection.admissionReceipt.receiptSha256 | test("^[0-9a-f]{64}$")) and
    (.service | keys | sort) == [
      "backendFingerprint","backendGroups","backendSelfLink","name","uid","zones"
    ] and
    (.service.uid | type) == "string" and (.service.uid | length) > 0 and
    (.service.name | type) == "string" and (.service.name | length) > 0 and
    (.service.zones | type) == "array" and (.service.zones | length) > 0 and
    .service.zones == (.service.zones | unique | sort) and
    (.service.backendSelfLink | type) == "string" and
    (.service.backendSelfLink | length) > 0 and
    (.service.backendGroups | type) == "array" and (.service.backendGroups | length) > 0 and
    .service.backendGroups == (.service.backendGroups | unique | sort) and
    (.admission.policy.uid | type) == "string" and (.admission.binding.uid | type) == "string" and
    (.workload | keys | sort) == ["deployment","pods","verified"] and
    (.workload.deployment | keys | sort) == [
      "generation","name","namespace","replicas","resourceVersion","uid"
    ] and
    .workload.deployment.name == "cma-api" and
    .workload.deployment.namespace == "chrono-bot" and
    .workload.deployment.replicas == 1 and
    (.workload.deployment.generation | type) == "number" and
    .workload.deployment.generation > 0 and
    (.workload.deployment.resourceVersion | type) == "string" and
    (.workload.deployment.resourceVersion | length) > 0 and
    (.workload.deployment.uid | type) == "string" and
    (.workload.deployment.uid | length) > 0 and
    (.workload.pods | type) == "array" and (.workload.pods | length) == 1 and
    (.workload.pods[0] | keys | sort) == ["name","uid"] and
    (.workload.pods[0].name | type) == "string" and (.workload.pods[0].name | length) > 0 and
    (.workload.pods[0].uid | type) == "string" and (.workload.pods[0].uid | length) > 0 and
    .workload.verified == null
  ' "${cma_receipt_file}" >/dev/null || return 1
  cma_pinned_api_root_identity="$(jq -cer '{uid:.service.uid,name:.service.name,zones:.service.zones}' \
    "${cma_receipt_file}")" || return 1
  CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${cma_receipt_file}"
}

cma_step10_validate_source_projection() {
  local cma_projection_dir="$1"
  local cma_projection_digest_file="${cma_projection_dir}/projection.sha256"
  local cma_projection_file="${cma_projection_dir}/projection.json"
  local cma_recorded_digest cma_live_digest cma_projection_entry
  local cma_projection_work_dir cma_expected_predecessor
  local cma_semantic_digest cma_final_digest

  [ -d "${cma_projection_dir}" ] && [ ! -L "${cma_projection_dir}" ] && \
    [ -O "${cma_projection_dir}" ] && \
    [ "$(find "${cma_projection_dir}" -mindepth 1 -maxdepth 1 -print | \
      wc -l | tr -d ' ')" = 3 ] && \
    [ -n "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT:-}" ] && \
    [ -f "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] && \
    [ ! -L "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] || return 1
  for cma_projection_entry in capture.json projection.json projection.sha256; do
    [ -f "${cma_projection_dir}/${cma_projection_entry}" ] && \
      [ ! -L "${cma_projection_dir}/${cma_projection_entry}" ] && \
      [ -O "${cma_projection_dir}/${cma_projection_entry}" ] || return 1
  done
  cma_recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" {print $1}' \
    "${cma_projection_digest_file}")" || return 1
  [ "$(wc -l < "${cma_projection_digest_file}" | tr -d ' ')" = 1 ] || return 1
  cma_live_digest="$(cma_step10_sha256_file "${cma_projection_file}")" || return 1
  cma_live_digest="${cma_live_digest%% *}"
  cma_step10_sha256_valid "${cma_recorded_digest}" && \
    [ "${cma_recorded_digest}" = "${cma_live_digest}" ] || return 1
  (
    set -e
    cma_projection_work_dir="$(mktemp -d)"
    trap 'rm -rf "${cma_projection_work_dir}"' EXIT HUP INT QUIT TERM
    cma_step10_projection_emit "${cma_projection_dir}/capture.json" \
      "${cma_projection_work_dir}/projection.json"
    cmp "${cma_projection_work_dir}/projection.json" "${cma_projection_file}" >/dev/null
  ) || return 1
  cma_expected_predecessor="${cma_fenced_projection_sha256:-}"
  cma_step10_sha256_valid "${cma_expected_predecessor}" || return 1
  jq -e --arg predecessor "${cma_expected_predecessor}" \
    --slurpfile receipt "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" '
    ($receipt | length) == 1 and ($receipt[0].service) as $root |
    (.fullCapture.kubernetes.services.items |
      map(select(.metadata.namespace == "chrono-bot" and .metadata.name == "cma-api"))) as $services |
    ($services | length) == 1 and .schemaVersion == 1 and .mode == "source" and
    .lifecycle == {stage:"final-reopen",predecessorSha256:$predecessor} and
    .graph.services["cma-api"].uid == $root.uid and
    .graph.services["cma-api"].name == $root.name and
    .graph.services["cma-api"].zones == $root.zones and
    .graph.services["cma-api"].backendSelfLink == $root.backendSelfLink and
    .graph.services["cma-api"].backendFingerprint == $root.backendFingerprint and
    .graph.services["cma-api"].backendGroups == $root.backendGroups and
    .graph.services["cma-api"].uid == $services[0].metadata.uid and
    .behaviorPolicy == $receipt[0].sourceProjection.behaviorPolicy and
    .admissionReceipt == $receipt[0].sourceProjection.admissionReceipt and
    .graph.fenceAdmission == $receipt[0].admission and
    .graph.apiWorkload.deployment.uid == $receipt[0].workload.deployment.uid and
    .graph.apiWorkload.deployment.replicas == 1 and
    (.graph.apiWorkload.pods | length) == 1 and
    .graph.apiWorkload.pods[0].uid != $receipt[0].workload.pods[0].uid and
    .graph.apiWorkload.verified != null
  ' "${cma_projection_file}" >/dev/null || return 1
  cma_step10_projection_matches_final_workload "${cma_projection_file}" || return 1
  cma_semantic_digest="$(cma_step10_projection_digest \
    "${cma_projection_file}")" || return 1
  [ "${cma_semantic_digest}" = "${cma_live_digest}" ] || return 1
  cma_step10_projection_check_live "${cma_projection_file}" || return 1
  cma_final_digest="$(cma_step10_projection_digest \
    "${cma_projection_file}")" || return 1
  [ "${cma_final_digest}" = "${cma_live_digest}" ] || return 1
  printf '%s\n' "${cma_live_digest}"
}

cma_step10_validate_admission_reservation() {
  local cma_policy cma_binding
  [ "${CMA_GATEWAY_FENCE_LABEL_RESERVATION_CONFIRM:-}" = live-denial-drill-v1 ] || {
    echo "the cutover-fence label reservation lacks the reviewed live-denial drill acknowledgement" >&2
    return 1
  }
  .github/scripts/cma-trigger-gateway-admission-bootstrap.sh \
    --verify-receipt >/dev/null || {
    echo "the durable admission bootstrap receipt/live enforcement proof is invalid" >&2
    return 1
  }
  cma_policy="$(timeout --signal=TERM --kill-after=5s 25s kubectl --request-timeout=15s \
    --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke \
    get validatingadmissionpolicy cma-cutover-fence-label-reservation -o json)" || return 1
  cma_binding="$(timeout --signal=TERM --kill-after=5s 25s kubectl --request-timeout=15s \
    --context gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke \
    get validatingadmissionpolicybinding cma-cutover-fence-label-reservation -o json)" || return 1
  printf '%s\n%s\n' "${cma_policy}" "${cma_binding}" | jq -e -s '
    length == 2 and
    .[0] as $policy | .[1] as $binding |
    {"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"} as $labels |
    [{apiGroups:[""],apiVersions:["v1"],operations:["CREATE","UPDATE"],
      resources:["pods"],scope:"Namespaced"}] as $rules |
    {matchLabels:{"kubernetes.io/metadata.name":"chrono-bot"}} as $namespace |
    $policy.apiVersion == "admissionregistration.k8s.io/v1" and
    $policy.kind == "ValidatingAdmissionPolicy" and
    $policy.metadata.name == "cma-cutover-fence-label-reservation" and
    $policy.metadata.labels == $labels and
    $policy.metadata.deletionTimestamp == null and
    ($policy.metadata.uid | type) == "string" and ($policy.metadata.uid | length) > 0 and
    ($policy.metadata.generation | type) == "number" and $policy.metadata.generation > 0 and
    $policy.spec == {
      failurePolicy:"Fail",
      matchConstraints:{matchPolicy:"Equivalent",namespaceSelector:$namespace,resourceRules:$rules},
      matchConditions:[{name:"exact-chrono-bot-namespace",expression:"request.namespace == \u0027chrono-bot\u0027"}],
      validations:[{
        expression:"!has(object.metadata.labels) || !(\u0027release.chrono-ai.fun/cutover-fence\u0027 in object.metadata.labels) || object.metadata.labels[\u0027release.chrono-ai.fun/cutover-fence\u0027] != \u0027mongodb-715\u0027",
        message:"Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715",
        reason:"Forbidden"
      }]
    } and
    $policy.status.observedGeneration == $policy.metadata.generation and
    ($policy.status.typeChecking.expressionWarnings // []) == [] and
    $binding.apiVersion == "admissionregistration.k8s.io/v1" and
    $binding.kind == "ValidatingAdmissionPolicyBinding" and
    $binding.metadata.name == "cma-cutover-fence-label-reservation" and
    $binding.metadata.labels == $labels and
    $binding.metadata.deletionTimestamp == null and
    ($binding.metadata.uid | type) == "string" and ($binding.metadata.uid | length) > 0 and
    $binding.spec == {
      policyName:"cma-cutover-fence-label-reservation",
      validationActions:["Deny"],
      matchResources:{matchPolicy:"Equivalent",namespaceSelector:$namespace,resourceRules:$rules}
    }
  ' >/dev/null || return 1
  if [ -n "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT:-}" ]; then
    [ -f "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] && \
      [ ! -L "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" ] || return 1
    printf '%s\n%s\n' "${cma_policy}" "${cma_binding}" | jq -e -s \
      --slurpfile receipt "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}" '
        length == 2 and ($receipt | length) == 1 and
        $receipt[0].admission.policy.uid == .[0].metadata.uid and
        $receipt[0].admission.binding.uid == .[1].metadata.uid
      ' >/dev/null || return 1
  fi
}
cma_read_selector_state() {
  local cma_selector_record cma_selector_state

  cma_selector_json="$(cma_step10_kubectl get service cma-api -o json)" || return 1
  cma_selector_state="$(printf '%s' "${cma_selector_json}" | jq -er '
    if .spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"} then "fenced"
    elif .spec.selector == {"app.kubernetes.io/name":"cma-api"} then "source"
    else "unknown"
    end')" || return 1
  case "${cma_selector_state}" in
    fenced)
      cma_selector_record="$(cma_parse_fenced_api_neg "${cma_selector_json}")" || return 1
      ;;
    source)
      cma_selector_record="$(cma_parse_source_api_neg "${cma_selector_json}")" || return 1
      ;;
    unknown)
      printf '%s\n' unknown
      return 0
      ;;
    *) return 1 ;;
  esac
  cma_api_root_matches_pinned "${cma_selector_record}" || return 1
  printf '%s\n' "${cma_selector_state}"
}
cma_manifest_api_image() {
  timeout --signal=TERM --kill-after=2s 10s \
    ruby --disable-gems -ryaml -e '
      document = YAML.safe_load(
        File.binread(ARGV.fetch(0)), permitted_classes: [],
        permitted_symbols: [], aliases: false
      )
      containers = document.dig("spec", "template", "spec", "containers")
      unless document["apiVersion"] == "apps/v1" && document["kind"] == "Deployment" &&
             document.dig("metadata", "name") == "cma-api" &&
             document.dig("metadata", "namespace") == "chrono-bot" &&
             containers.is_a?(Array) && containers.length == 1 &&
             containers[0]["name"] == "api" && containers[0]["image"].is_a?(String) &&
             !containers[0]["image"].empty?
        exit 1
      end
      print containers[0]["image"]
    ' infra/api/deployment.yaml
}
cma_step10_validate_workload_contract() {
  timeout --signal=TERM --kill-after=2s 15s env -i PATH="${PATH}" LC_ALL=C \
    ruby --disable-gems .github/scripts/cma-trigger-gateway-projection.rb \
      --validate-workload infra/api/deployment.yaml "$1" "$2" "$3" >/dev/null
}
cma_step10_projection_matches_final_workload() {
  local cma_projection_file="$1"
  local cma_workload_contract_dir

  jq -e \
    --arg deployment_uid "${cma_final_deployment_uid}" \
    --arg replica_set "${cma_final_replica_set}" \
    --arg replica_set_uid "${cma_final_replica_set_uid}" \
    --arg pod "${cma_final_pod}" \
    --arg pod_uid "${cma_final_pod_uid}" \
    --arg pod_ip "${cma_final_pod_ip}" \
    --arg image "${cma_final_pod_image}" \
    --arg image_id "${cma_final_pod_image_id}" '
      (.fullCapture.kubernetes.apiDeploymentWorkload) as $deployment |
      (.fullCapture.kubernetes.apiReplicaSetWorkload) as $replica_set_live |
      (.fullCapture.kubernetes.apiPodWorkload) as $pod_live |
      (.graph.apiWorkload.verified) as $verified |
      $deployment.metadata.uid == $deployment_uid and
      $replica_set_live.metadata.name == $replica_set and
      $replica_set_live.metadata.uid == $replica_set_uid and
      $replica_set_live.metadata.ownerReferences == [{
        apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,
        kind:"Deployment",name:"cma-api",uid:$deployment_uid
      }] and
      $pod_live.metadata.name == $pod and $pod_live.metadata.uid == $pod_uid and
      $pod_live.metadata.ownerReferences == [{
        apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,
        kind:"ReplicaSet",name:$replica_set,uid:$replica_set_uid
      }] and
      $pod_live.status.podIP == $pod_ip and
      $verified.deployment.uid == $deployment_uid and
      $verified.replicaSet.name == $replica_set and
      $verified.replicaSet.uid == $replica_set_uid and
      $verified.pod.name == $pod and $verified.pod.uid == $pod_uid and
      $verified.pod.ip == $pod_ip and $verified.pod.image == $image and
      $verified.pod.imageID == $image_id
    ' "${cma_projection_file}" >/dev/null || return 1
  (
    set -e
    cma_workload_contract_dir="$(mktemp -d)"
    trap 'rm -rf "${cma_workload_contract_dir}"' EXIT HUP INT QUIT TERM
    chmod 700 "${cma_workload_contract_dir}"
    jq -e .fullCapture.kubernetes.apiDeploymentWorkload \
      "${cma_projection_file}" > "${cma_workload_contract_dir}/deployment.json"
    jq -e .fullCapture.kubernetes.apiReplicaSetWorkload \
      "${cma_projection_file}" > "${cma_workload_contract_dir}/replicaset.json"
    jq -e .fullCapture.kubernetes.apiPodWorkload \
      "${cma_projection_file}" > "${cma_workload_contract_dir}/pod.json"
    chmod 600 "${cma_workload_contract_dir}"/*.json
    cma_step10_validate_workload_contract \
      "${cma_workload_contract_dir}/deployment.json" \
      "${cma_workload_contract_dir}/replicaset.json" \
      "${cma_workload_contract_dir}/pod.json"
  )
}
cma_final_workload_is_exact() {
  local cma_pod_json cma_replica_set_json cma_deployment_json
  local cma_workload_contract_dir

  cma_pod_json="$(cma_step10_kubectl get pod "${cma_final_pod}" -o json)" || return 1
  printf '%s' "${cma_pod_json}" | jq -e \
    --arg pod "${cma_final_pod}" \
    --arg pod_uid "${cma_final_pod_uid}" \
    --arg pod_ip "${cma_final_pod_ip}" \
    --arg replica_set "${cma_final_replica_set}" \
    --arg replica_set_uid "${cma_final_replica_set_uid}" \
    --arg image "${cma_final_pod_image}" \
    --arg image_id "${cma_final_pod_image_id}" '
      .apiVersion == "v1" and .kind == "Pod" and
      .metadata.name == $pod and .metadata.namespace == "chrono-bot" and
      .metadata.uid == $pod_uid and
      .metadata.deletionTimestamp == null and
      .metadata.ownerReferences == [{
        apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,
        kind:"ReplicaSet",name:$replica_set,uid:$replica_set_uid
      }] and
      .status.phase == "Running" and .status.podIP == $pod_ip and
      (.spec.containers | length) == 1 and
      .spec.containers[0].name == "api" and .spec.containers[0].image == $image and
      (.status.containerStatuses | length) == 1 and
      .status.containerStatuses[0].name == "api" and
      .status.containerStatuses[0].image == $image and
      .status.containerStatuses[0].imageID == $image_id and
      .status.containerStatuses[0].ready == true and
      .status.containerStatuses[0].started == true and
      .status.containerStatuses[0].restartCount == 0 and
      (.status.containerStatuses[0].state.running.startedAt | type) == "string" and
      (.status.conditions // [] |
        map(select(.type == "Ready" and .status == "True")) | length) == 1
    ' >/dev/null || return 1

  cma_replica_set_json="$(cma_step10_kubectl get replicaset \
    "${cma_final_replica_set}" -o json)" || return 1
  printf '%s' "${cma_replica_set_json}" | jq -e \
    --arg replica_set "${cma_final_replica_set}" \
    --arg replica_set_uid "${cma_final_replica_set_uid}" \
    --arg deployment_uid "${cma_final_deployment_uid}" \
    --arg image "${cma_final_pod_image}" '
      .apiVersion == "apps/v1" and .kind == "ReplicaSet" and
      .metadata.name == $replica_set and .metadata.namespace == "chrono-bot" and
      .metadata.uid == $replica_set_uid and
      .metadata.deletionTimestamp == null and
      .metadata.ownerReferences == [{
        apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,
        kind:"Deployment",name:"cma-api",uid:$deployment_uid
      }] and
      (.metadata.labels["pod-template-hash"] | type) == "string" and
      .metadata.labels["app.kubernetes.io/name"] == "cma-api" and
      .metadata.labels["app.kubernetes.io/part-of"] == "cma" and
      (.spec.selector.matchExpressions // []) == [] and
      .spec.selector.matchLabels["app.kubernetes.io/name"] == "cma-api" and
      .spec.selector.matchLabels["pod-template-hash"] ==
        .metadata.labels["pod-template-hash"] and
      .spec.template.metadata.labels["app.kubernetes.io/name"] == "cma-api" and
      .spec.template.metadata.labels["app.kubernetes.io/part-of"] == "cma" and
      .spec.template.metadata.labels["pod-template-hash"] ==
        .metadata.labels["pod-template-hash"] and
      (.spec.template.spec.containers | length) == 1 and
      .spec.template.spec.containers[0].name == "api" and
      .spec.template.spec.containers[0].image == $image and
      .spec.replicas == 1 and .status.observedGeneration == .metadata.generation and
      .status.replicas == 1 and .status.fullyLabeledReplicas == 1 and
      .status.readyReplicas == 1 and .status.availableReplicas == 1
    ' >/dev/null || return 1

  cma_deployment_json="$(cma_step10_kubectl get deployment cma-api -o json)" || return 1
  printf '%s' "${cma_deployment_json}" | jq -e \
    --arg deployment_uid "${cma_final_deployment_uid}" \
    --arg image "${cma_final_pod_image}" '
      .apiVersion == "apps/v1" and .kind == "Deployment" and
      .metadata.name == "cma-api" and .metadata.namespace == "chrono-bot" and
      .metadata.uid == $deployment_uid and .metadata.deletionTimestamp == null and
      .spec.replicas == 1 and .spec.strategy == {type:"Recreate"} and
      .spec.selector == {matchLabels:{"app.kubernetes.io/name":"cma-api"}} and
      .spec.template.metadata.labels == {
        "app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"
      } and
      (.spec.template.spec.containers | length) == 1 and
      .spec.template.spec.containers[0].name == "api" and
      .spec.template.spec.containers[0].image == $image and
      .status.observedGeneration == .metadata.generation and
      .status.replicas == 1 and .status.updatedReplicas == 1 and
      .status.readyReplicas == 1 and .status.availableReplicas == 1 and
      (.status.unavailableReplicas // 0) == 0 and
      (.status.collisionCount // 0) == 0
    ' >/dev/null || return 1
  (
    set -e
    cma_workload_contract_dir="$(mktemp -d)"
    trap 'rm -rf "${cma_workload_contract_dir}"' EXIT HUP INT QUIT TERM
    chmod 700 "${cma_workload_contract_dir}"
    printf '%s\n' "${cma_deployment_json}" > \
      "${cma_workload_contract_dir}/deployment.json"
    printf '%s\n' "${cma_replica_set_json}" > \
      "${cma_workload_contract_dir}/replicaset.json"
    printf '%s\n' "${cma_pod_json}" > "${cma_workload_contract_dir}/pod.json"
    chmod 600 "${cma_workload_contract_dir}"/*.json
    cma_step10_validate_workload_contract \
      "${cma_workload_contract_dir}/deployment.json" \
      "${cma_workload_contract_dir}/replicaset.json" \
      "${cma_workload_contract_dir}/pod.json"
  )
}
cma_final_pod_topology() {
  local cma_pod_json cma_node_name cma_node_json

  cma_pod_json="$(cma_step10_kubectl get pod "${cma_final_pod}" -o json)" || return 1
  cma_node_name="$(printf '%s' "${cma_pod_json}" | jq -er \
    --arg uid "${cma_final_pod_uid}" '
      if .metadata.uid == $uid and (.spec.nodeName | type) == "string" and
         (.spec.nodeName | length) > 0
      then .spec.nodeName else error("pod topology is not exact") end
    ')" || return 1
  cma_node_json="$(cma_step10_kubectl get node "${cma_node_name}" -o json)" || return 1
  printf '%s' "${cma_node_json}" | jq -cer --arg node "${cma_node_name}" '
    (.spec.providerID // "") as $provider |
    if .apiVersion == "v1" and .kind == "Node" and .metadata.name == $node and
       .metadata.deletionTimestamp == null and
       ($provider | test("^gce://chronoai-501608/asia-northeast1-[a-z]/[^/]+$"))
    then {nodeName:$node,providerZone:($provider | split("/")[-2]),
      providerInstance:($provider | split("/")[-1])}
    else error("final Pod node/provider topology is not exact") end'
}
cma_route_and_policies_ready() {
  cma_step10_require_checkout || return 1
  cma_step10_validate_snapshot_seal || return 1
  timeout --signal=TERM --kill-after=5s 90s \
    /bin/bash --noprofile --norc -p \
      .github/scripts/cma-trigger-gateway-state.sh \
      --check-adopted-from-snapshot "${CMA_GATEWAY_ROLLBACK_DIR}" >/dev/null
}
cma_parse_fenced_api_neg() {
  printf '%s' "$1" | jq -cer '
    (.metadata.annotations["cloud.google.com/neg-status"] | fromjson) as $neg |
    ($neg.network_endpoint_groups["80"] // "") as $name |
    ($neg.zones // []) as $zones |
    if (keys | sort) == ["apiVersion","kind","metadata","spec","status"] and
       .apiVersion == "v1" and .kind == "Service" and
       (.metadata | keys | sort) == [
         "annotations","creationTimestamp","labels","name","namespace",
         "resourceVersion","uid"
       ] and
       .metadata.name == "cma-api" and .metadata.namespace == "chrono-bot" and
       .metadata.labels == {
         "app.kubernetes.io/name":"cma-api",
         "app.kubernetes.io/part-of":"cma"
       } and
       (.metadata.annotations | keys | sort) == [
         "cloud.google.com/neg","cloud.google.com/neg-status",
         "kubectl.kubernetes.io/last-applied-configuration"
       ] and
       (.metadata.annotations["cloud.google.com/neg"] | fromjson) ==
         {exposed_ports:{"80":{}}} and
       (.metadata.annotations["kubectl.kubernetes.io/last-applied-configuration"] |
         fromjson) == {
           apiVersion:"v1",kind:"Service",
           metadata:{
             annotations:{},
             labels:{
               "app.kubernetes.io/name":"cma-api",
               "app.kubernetes.io/part-of":"cma"
             },
             name:"cma-api",namespace:"chrono-bot"
           },
           spec:{
             ports:[{name:"http",port:80,protocol:"TCP",targetPort:"http"}],
             selector:{"app.kubernetes.io/name":"cma-api"},
             type:"ClusterIP"
           }
         } and
       (.metadata.creationTimestamp | type) == "string" and
       (.metadata.creationTimestamp | length) > 0 and
       (.metadata.uid | type) == "string" and (.metadata.uid | length) > 0 and
       (.metadata.resourceVersion | type) == "string" and
       (.metadata.resourceVersion | length) > 0 and
       .metadata.deletionTimestamp == null and
       (.metadata.finalizers // []) == [] and
       (.metadata.ownerReferences // []) == [] and
       (.spec | keys | sort) == [
         "clusterIP","clusterIPs","internalTrafficPolicy","ipFamilies",
         "ipFamilyPolicy","ports","selector","sessionAffinity","type"
       ] and
       .spec.type == "ClusterIP" and
       .spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"} and
       .spec.ports == [{name:"http",port:80,protocol:"TCP",targetPort:"http"}] and
       .spec.internalTrafficPolicy == "Cluster" and
       .spec.ipFamilies == ["IPv4"] and .spec.ipFamilyPolicy == "SingleStack" and
       .spec.sessionAffinity == "None" and .spec.sessionAffinityConfig == null and
       (.spec.clusterIP | type) == "string" and .spec.clusterIP != "None" and
       .spec.clusterIPs == [.spec.clusterIP] and
       (.spec.externalIPs // []) == [] and
       (.spec.loadBalancerSourceRanges // []) == [] and
       (.spec.publishNotReadyAddresses // false) == false and
       .spec.externalName == null and .spec.healthCheckNodePort == null and
       .status == {loadBalancer:{}} and
       (($neg | keys | sort) == ["network_endpoint_groups","zones"]) and
       (($neg.network_endpoint_groups | keys) == ["80"]) and
       ($name | test("^k8s1-[a-z0-9]+-chrono-bot-cma-api-80-[a-z0-9]+$")) and
       ($zones | type) == "array" and ($zones | length) > 0 and
       ($zones | length) == ($zones | unique | length) and
       ($zones | all(test("^asia-northeast1-[a-z]$")))
    then {
      uid:.metadata.uid,
      resourceVersion:.metadata.resourceVersion,
      name:$name,
      zones:($zones | sort)
    }
    else error("the fenced cma-api Service NEG identity is not exact")
    end'
}
cma_parse_source_api_neg() {
  printf '%s' "$1" | jq -cer '
    (.metadata.annotations["cloud.google.com/neg-status"] | fromjson) as $neg |
    ($neg.network_endpoint_groups["80"] // "") as $name |
    ($neg.zones // []) as $zones |
    if (keys | sort) == ["apiVersion","kind","metadata","spec","status"] and
       .apiVersion == "v1" and .kind == "Service" and
       (.metadata | keys | sort) == [
         "annotations","creationTimestamp","labels","name","namespace",
         "resourceVersion","uid"
       ] and
       .metadata.name == "cma-api" and .metadata.namespace == "chrono-bot" and
       .metadata.labels == {
         "app.kubernetes.io/name":"cma-api",
         "app.kubernetes.io/part-of":"cma"
       } and
       (.metadata.annotations | keys | sort) == [
         "cloud.google.com/neg","cloud.google.com/neg-status",
         "kubectl.kubernetes.io/last-applied-configuration"
       ] and
       (.metadata.annotations["cloud.google.com/neg"] | fromjson) ==
         {exposed_ports:{"80":{}}} and
       (.metadata.annotations["kubectl.kubernetes.io/last-applied-configuration"] |
         fromjson) == {
           apiVersion:"v1",kind:"Service",
           metadata:{
             annotations:{},
             labels:{
               "app.kubernetes.io/name":"cma-api",
               "app.kubernetes.io/part-of":"cma"
             },
             name:"cma-api",namespace:"chrono-bot"
           },
           spec:{
             ports:[{name:"http",port:80,protocol:"TCP",targetPort:"http"}],
             selector:{"app.kubernetes.io/name":"cma-api"},
             type:"ClusterIP"
           }
         } and
       (.metadata.creationTimestamp | type) == "string" and
       (.metadata.creationTimestamp | length) > 0 and
       (.metadata.uid | type) == "string" and (.metadata.uid | length) > 0 and
       (.metadata.resourceVersion | type) == "string" and
       (.metadata.resourceVersion | length) > 0 and
       .metadata.deletionTimestamp == null and
       (.metadata.finalizers // []) == [] and
       (.metadata.ownerReferences // []) == [] and
       (.spec | keys | sort) == [
         "clusterIP","clusterIPs","internalTrafficPolicy","ipFamilies",
         "ipFamilyPolicy","ports","selector","sessionAffinity","type"
       ] and
       .spec.type == "ClusterIP" and
       .spec.selector == {"app.kubernetes.io/name":"cma-api"} and
       .spec.ports == [{name:"http",port:80,protocol:"TCP",targetPort:"http"}] and
       .spec.internalTrafficPolicy == "Cluster" and
       .spec.ipFamilies == ["IPv4"] and .spec.ipFamilyPolicy == "SingleStack" and
       .spec.sessionAffinity == "None" and .spec.sessionAffinityConfig == null and
       (.spec.clusterIP | type) == "string" and .spec.clusterIP != "None" and
       .spec.clusterIPs == [.spec.clusterIP] and
       (.spec.externalIPs // []) == [] and
       (.spec.loadBalancerSourceRanges // []) == [] and
       (.spec.publishNotReadyAddresses // false) == false and
       .spec.externalName == null and .spec.healthCheckNodePort == null and
       .status == {loadBalancer:{}} and
       (($neg | keys | sort) == ["network_endpoint_groups","zones"]) and
       (($neg.network_endpoint_groups | keys) == ["80"]) and
       ($name | test("^k8s1-[a-z0-9]+-chrono-bot-cma-api-80-[a-z0-9]+$")) and
       ($zones | type) == "array" and ($zones | length) > 0 and
       ($zones | length) == ($zones | unique | length) and
       ($zones | all(test("^asia-northeast1-[a-z]$")))
    then {
      uid:.metadata.uid,
      resourceVersion:.metadata.resourceVersion,
      name:$name,
      zones:($zones | sort)
    }
    else error("the source cma-api Service NEG identity is not exact")
    end'
}
cma_parse_api_endpoint_slices() {
  printf '%s' "$1" | jq -cer --arg service_uid "$2" '
    if (keys | sort) == ["apiVersion","items","kind","metadata"] and
       .apiVersion == "v1" and .kind == "List" and
       (.metadata | keys) == ["resourceVersion"] and
       .metadata.resourceVersion == "" and (.items | type) == "array" and
       (.items | all(
         .apiVersion == "discovery.k8s.io/v1" and .kind == "EndpointSlice" and
         .addressType == "IPv4" and
         .ports == [{name:"http",port:8080,protocol:"TCP"}] and
         .metadata.namespace == "chrono-bot" and
         .metadata.labels == {
           "app.kubernetes.io/name":"cma-api",
           "app.kubernetes.io/part-of":"cma",
           "endpointslice.kubernetes.io/managed-by":"endpointslice-controller.k8s.io",
           "kubernetes.io/service-name":"cma-api"
         } and
         (.metadata.name | type) == "string" and (.metadata.name | length) > 0 and
         (.metadata.uid | type) == "string" and (.metadata.uid | length) > 0 and
         (.metadata.resourceVersion | type) == "string" and
         (.metadata.resourceVersion | length) > 0 and
         .metadata.deletionTimestamp == null and
         (.metadata.finalizers // []) == [] and
         .metadata.ownerReferences == [{
           apiVersion:"v1",
           blockOwnerDeletion:true,
           controller:true,
           kind:"Service",
           name:"cma-api",
           uid:$service_uid
         }] and
         (.endpoints | type) == "array" and
         (.endpoints | all(
           (keys | sort) == ["addresses","conditions","nodeName","targetRef","zone"] and
           (.addresses | type) == "array" and (.addresses | length) == 1 and
           (.addresses[0] | test("^(?:[0-9]{1,3}\\.){3}[0-9]{1,3}$")) and
           ((.conditions == {ready:true,serving:true,terminating:false}) or
            (.conditions == {ready:false,serving:false,terminating:false})) and
           (.nodeName | type) == "string" and (.nodeName | length) > 0 and
           (.zone | test("^asia-northeast1-[a-z]$")) and
           (.targetRef | keys | sort) == ["kind","name","namespace","uid"] and
           .targetRef.kind == "Pod" and .targetRef.namespace == "chrono-bot" and
           (.targetRef.name | type) == "string" and (.targetRef.name | length) > 0 and
           (.targetRef.uid | type) == "string" and (.targetRef.uid | length) > 0
         ))
       ))
    then {
      slices:([.items[] | {
        name:.metadata.name,
        uid:.metadata.uid,
        resourceVersion:.metadata.resourceVersion
      }] | sort_by(.name)),
      readyEndpoints:([.items[].endpoints[]? |
        select(.conditions == {ready:true,serving:true,terminating:false}) | {
          addresses:(.addresses | sort),
          nodeName:.nodeName,
          zone:.zone,
          targetRef:{
            kind:.targetRef.kind,
            name:.targetRef.name,
            namespace:.targetRef.namespace,
            uid:.targetRef.uid
          }
        }] | sort_by(.targetRef.uid, (.addresses | join(","))))
    }
    else error("the cma-api EndpointSlice list identity is not exact")
    end'
}
cma_api_root_from_neg_record() {
  printf '%s' "$1" | jq -cer '{uid:.uid,name:.name,zones:.zones}'
}
cma_api_root_matches_pinned() {
  local cma_observed_api_root

  if [ -z "${cma_pinned_api_root_identity}" ]; then
    return 0
  fi
  cma_observed_api_root="$(cma_api_root_from_neg_record "$1")" || return 1
  [ "${cma_observed_api_root}" = "${cma_pinned_api_root_identity}" ]
}
cma_pin_or_check_api_root() {
  local cma_observed_api_root

  cma_observed_api_root="$(cma_api_root_from_neg_record "$1")" || return 1
  if [ -z "${cma_pinned_api_root_identity}" ]; then
    cma_pinned_api_root_identity="${cma_observed_api_root}"
    return 0
  fi
  [ "${cma_observed_api_root}" = "${cma_pinned_api_root_identity}" ]
}
cma_fence_label_has_no_pods() {
  local cma_fence_pods_json

  cma_fence_pods_json="$(cma_step10_kubectl get pod \
    -l release.chrono-ai.fun/cutover-fence=mongodb-715 -o json)" || return 1
  printf '%s' "${cma_fence_pods_json}" | jq -e '
    .apiVersion == "v1" and .kind == "List" and
    (.items | type) == "array" and (.items | length) == 0
  ' >/dev/null
}
cma_wait_for_fenced_endpoints() {
  cma_fence_deadline=$((SECONDS + 300))
  cma_fence_previous_empty_observation=""
  while :; do
    if cma_step10_deadline_reached "${cma_fence_deadline}"; then
      echo "CRITICAL: the five-minute EndpointSlice/NEG drain deadline expired; impose a manual traffic fence" >&2
      return 1
    fi
    if ! cma_fence_label_has_no_pods; then
      echo "CRITICAL: the reserved cutover-fence selector matches a Pod or could not be checked" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_fence_deadline}"; then
      echo "CRITICAL: the cutover-fence Pod read crossed the five-minute drain deadline" >&2
      return 1
    fi
    if ! cma_fence_service_before="$(cma_step10_kubectl get service cma-api -o json)" || \
       ! cma_fence_neg_record_before="$(cma_parse_fenced_api_neg \
         "${cma_fence_service_before}")"; then
      echo "CRITICAL: the fenced cma-api Service does not advertise one exact zonal NEG set" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_fence_deadline}"; then
      echo "CRITICAL: the Service read crossed the five-minute drain deadline; impose a manual traffic fence" >&2
      return 1
    fi
    cma_fence_service_uid="$(printf '%s' "${cma_fence_neg_record_before}" | \
      jq -er '.uid')" || return 1
    if ! cma_fence_endpoints_json="$(cma_step10_kubectl get endpointslice \
      -l kubernetes.io/service-name=cma-api -o json)"; then
      echo "CRITICAL: cma-api is selected as fenced, but its EndpointSlices could not be read; keep or impose a manual traffic fence" >&2
      return 1
    fi
    if ! cma_fence_slice_record_before="$(cma_parse_api_endpoint_slices \
      "${cma_fence_endpoints_json}" "${cma_fence_service_uid}")" || \
       ! cma_fence_endpoint_count="$(printf '%s' \
         "${cma_fence_slice_record_before}" | jq -er '.readyEndpoints | length')"; then
      echo "CRITICAL: cma-api EndpointSlices could not be parsed after re-fencing; keep or impose a manual traffic fence" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_fence_deadline}"; then
      echo "CRITICAL: the EndpointSlice read crossed the five-minute drain deadline; impose a manual traffic fence" >&2
      return 1
    fi
    if [ "${cma_fence_endpoint_count}" -eq 0 ]; then
      cma_fence_neg_name="$(printf '%s' "${cma_fence_neg_record_before}" | \
        jq -er '.name')" || return 1
      cma_fence_neg_zones="$(printf '%s' "${cma_fence_neg_record_before}" | \
        jq -er '.zones[]')" || return 1
      cma_fence_negs_empty=1
      while IFS= read -r cma_fence_neg_zone; do
        if cma_step10_deadline_reached "${cma_fence_deadline}"; then
          echo "CRITICAL: the five-minute NEG drain deadline expired in ${cma_fence_neg_zone}; impose a manual traffic fence" >&2
          return 1
        fi
        if ! cma_fence_neg_members="$(cma_step10_gcloud \
          compute network-endpoint-groups list-network-endpoints \
          "${cma_fence_neg_name}" --project=chronoai-501608 \
          --zone="${cma_fence_neg_zone}" --format=json)"; then
          echo "CRITICAL: cma-api NEG membership could not be read in ${cma_fence_neg_zone}" >&2
          return 1
        fi
        if cma_step10_deadline_reached "${cma_fence_deadline}"; then
          echo "CRITICAL: the NEG membership read crossed the five-minute drain deadline in ${cma_fence_neg_zone}; impose a manual traffic fence" >&2
          return 1
        fi
        if ! cma_fence_neg_member_count="$(printf '%s' \
          "${cma_fence_neg_members}" | jq -er \
          'if type == "array" then length else error("NEG response is not an array") end')"; then
          echo "CRITICAL: cma-api NEG membership was not a JSON array in ${cma_fence_neg_zone}" >&2
          return 1
        fi
        if [ "${cma_fence_neg_member_count}" -ne 0 ]; then
          cma_fence_negs_empty=0
        fi
      done <<EOF
${cma_fence_neg_zones}
EOF
      if ! cma_fence_service_after="$(cma_step10_kubectl get service cma-api -o json)"; then
        echo "CRITICAL: the fenced cma-api Service could not be re-read after NEG inspection" >&2
        return 1
      fi
      if cma_step10_deadline_reached "${cma_fence_deadline}"; then
        echo "CRITICAL: the Service re-read crossed the five-minute drain deadline; impose a manual traffic fence" >&2
        return 1
      fi
      if ! cma_fence_neg_record_after="$(cma_parse_fenced_api_neg \
        "${cma_fence_service_after}")"; then
        echo "CRITICAL: the fenced cma-api Service could not be normalized after NEG inspection" >&2
        return 1
      fi
      cma_fence_service_uid_after="$(printf '%s' \
        "${cma_fence_neg_record_after}" | jq -er '.uid')" || return 1
      if ! cma_fence_endpoints_after="$(cma_step10_kubectl get endpointslice \
        -l kubernetes.io/service-name=cma-api -o json)" || \
         ! cma_fence_slice_record_after="$(cma_parse_api_endpoint_slices \
           "${cma_fence_endpoints_after}" \
           "${cma_fence_service_uid_after}")"; then
        echo "CRITICAL: cma-api EndpointSlices could not be re-read after NEG inspection" >&2
        return 1
      fi
      if cma_step10_deadline_reached "${cma_fence_deadline}"; then
        echo "CRITICAL: the EndpointSlice re-read crossed the five-minute drain deadline; impose a manual traffic fence" >&2
        return 1
      fi
      if [ "${cma_fence_neg_record_after}" != "${cma_fence_neg_record_before}" ] || \
         [ "${cma_fence_slice_record_after}" != "${cma_fence_slice_record_before}" ]; then
        echo "CRITICAL: cma-api Service or EndpointSlice identities changed during NEG inspection" >&2
        return 1
      fi
      if [ "${cma_fence_negs_empty}" -eq 1 ]; then
        cma_fence_empty_observation="${cma_fence_neg_record_before}
${cma_fence_slice_record_before}"
        if [ -n "${cma_fence_previous_empty_observation}" ] && \
           [ "${cma_fence_empty_observation}" = \
             "${cma_fence_previous_empty_observation}" ]; then
          if cma_step10_deadline_reached "${cma_fence_deadline}"; then
            echo "CRITICAL: the coherent empty NEG proof completed after the five-minute drain deadline; impose a manual traffic fence" >&2
            return 1
          fi
          if ! cma_pin_or_check_api_root "${cma_fence_neg_record_before}"; then
            echo "CRITICAL: the cma-api Service UID or NEG identity changed across the traffic-restoration transaction; impose a manual traffic fence" >&2
            return 1
          fi
          if ! cma_fence_label_has_no_pods; then
            echo "CRITICAL: a Pod matches the reserved cutover-fence selector after the coherent NEG drain proof" >&2
            return 1
          fi
          if cma_step10_deadline_reached "${cma_fence_deadline}"; then
            echo "CRITICAL: the final cutover-fence Pod read crossed the five-minute drain deadline" >&2
            return 1
          fi
          return 0
        fi
        cma_fence_previous_empty_observation="${cma_fence_empty_observation}"
      else
        cma_fence_previous_empty_observation=""
      fi
    else
      cma_fence_previous_empty_observation=""
    fi
    if cma_step10_deadline_reached "${cma_fence_deadline}"; then
      echo "CRITICAL: cma-api still has a routable EndpointSlice or zonal NEG member at the drain deadline; impose a manual traffic fence" >&2
      return 1
    fi
    if ! sleep 2; then
      echo "CRITICAL: the re-fence EndpointSlice wait was interrupted; verify or impose a manual traffic fence" >&2
      return 1
    fi
  done
}
cma_wait_for_reopened_endpoint() {
  cma_reopen_deadline=$((SECONDS + 300))
  while :; do
    if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
      echo "the five-minute Service/NEG reopen deadline expired" >&2
      return 1
    fi
    if ! cma_reopen_service_before="$(cma_step10_kubectl get service cma-api -o json)"; then
      echo "the reopened cma-api Service could not be read" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
      echo "the reopened Service read crossed the five-minute deadline" >&2
      return 1
    fi
    if ! cma_reopen_neg_record_before="$(cma_parse_source_api_neg \
      "${cma_reopen_service_before}")"; then
      echo "the reopened cma-api Service NEG identity could not be normalized" >&2
      return 1
    fi
    if ! cma_api_root_matches_pinned "${cma_reopen_neg_record_before}"; then
      echo "the reopened cma-api Service UID or NEG identity differs from the initial fenced proof" >&2
      return 1
    fi
    cma_reopen_service_uid="$(printf '%s' "${cma_reopen_neg_record_before}" | \
      jq -er '.uid')" || return 1
    if ! cma_reopen_endpoints_before="$(cma_step10_kubectl get endpointslice \
      -l kubernetes.io/service-name=cma-api -o json)"; then
      echo "the reopened cma-api EndpointSlices could not be read" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
      echo "the reopened EndpointSlice read crossed the five-minute deadline" >&2
      return 1
    fi
    if ! cma_reopen_slice_record_before="$(cma_parse_api_endpoint_slices \
      "${cma_reopen_endpoints_before}" "${cma_reopen_service_uid}")"; then
      echo "the reopened cma-api EndpointSlices could not be normalized" >&2
      return 1
    fi
    if ! cma_reopen_endpoint_state="$(printf '%s' \
      "${cma_reopen_slice_record_before}" | jq -er \
      --arg pod "${cma_final_pod}" --arg uid "${cma_final_pod_uid}" \
      --arg ip "${cma_final_pod_ip}" --arg node "${cma_final_pod_node_name}" \
      --arg zone "${cma_final_pod_provider_zone}" '
        if (.readyEndpoints | length) == 0 then "pending"
        elif .readyEndpoints == [{
          addresses:[$ip],
          nodeName:$node,
          zone:$zone,
          targetRef:{kind:"Pod",name:$pod,namespace:"chrono-bot",uid:$uid}
        }] then "exact"
        else "wrong"
        end')"; then
      echo "the reopened cma-api EndpointSlice projection could not be checked" >&2
      return 1
    fi

    cma_reopen_neg_name="$(printf '%s' "${cma_reopen_neg_record_before}" | \
      jq -er '.name')" || return 1
    cma_reopen_neg_zones="$(printf '%s' "${cma_reopen_neg_record_before}" | \
      jq -er '.zones[]')" || return 1
    cma_reopen_neg_endpoints='[]'
    while IFS= read -r cma_reopen_neg_zone; do
      if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
        echo "the five-minute NEG reopen deadline expired in ${cma_reopen_neg_zone}" >&2
        return 1
      fi
      if ! cma_reopen_zone_endpoints="$(cma_step10_gcloud \
        compute network-endpoint-groups list-network-endpoints \
        "${cma_reopen_neg_name}" --project=chronoai-501608 \
        --zone="${cma_reopen_neg_zone}" --format=json)"; then
        echo "the reopened cma-api NEG membership is unreadable in ${cma_reopen_neg_zone}" >&2
        return 1
      fi
      if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
        echo "the NEG membership read crossed the five-minute reopen deadline in ${cma_reopen_neg_zone}" >&2
        return 1
      fi
      if ! printf '%s' "${cma_reopen_zone_endpoints}" | jq -e '
        type == "array" and all(.[];
          type == "object" and (.ipAddress | type) == "string" and
          (.port | type) == "number")
        ' >/dev/null; then
        echo "the reopened cma-api NEG membership is malformed in ${cma_reopen_neg_zone}" >&2
        return 1
      fi
      if ! printf '%s' "${cma_reopen_zone_endpoints}" | jq -e \
        --arg ip "${cma_final_pod_ip}" '
          all(.[]; .ipAddress == $ip and .port == 8080)
        ' >/dev/null; then
        echo "a reopened zonal NEG member is not the exact verified pod address and port" >&2
        return 1
      fi
      if ! cma_reopen_neg_endpoints="$(jq -cn \
        --argjson accumulated "${cma_reopen_neg_endpoints}" \
        --argjson current "${cma_reopen_zone_endpoints}" \
        '$accumulated + $current')"; then
        echo "the reopened cma-api NEG membership could not be aggregated" >&2
        return 1
      fi
      if ! printf '%s' "${cma_reopen_neg_endpoints}" | \
        jq -e 'length <= 1' >/dev/null; then
        echo "the reopened zonal NEGs contain more than the one verified pod projection" >&2
        return 1
      fi
    done <<EOF
${cma_reopen_neg_zones}
EOF

    if ! cma_reopen_service_after="$(cma_step10_kubectl get service cma-api -o json)"; then
      echo "the reopened Service could not be re-read after NEG inspection" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
      echo "the reopened Service re-read crossed the five-minute deadline" >&2
      return 1
    fi
    if ! cma_reopen_neg_record_after="$(cma_parse_source_api_neg \
      "${cma_reopen_service_after}")"; then
      echo "the reopened Service could not be normalized after NEG inspection" >&2
      return 1
    fi
    cma_reopen_service_uid_after="$(printf '%s' \
      "${cma_reopen_neg_record_after}" | jq -er '.uid')" || return 1
    if ! cma_reopen_endpoints_after="$(cma_step10_kubectl get endpointslice \
      -l kubernetes.io/service-name=cma-api -o json)"; then
      echo "the reopened EndpointSlices could not be re-read after NEG inspection" >&2
      return 1
    fi
    if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
      echo "the reopened EndpointSlice re-read crossed the five-minute deadline" >&2
      return 1
    fi
    if ! cma_reopen_slice_record_after="$(cma_parse_api_endpoint_slices \
      "${cma_reopen_endpoints_after}" \
      "${cma_reopen_service_uid_after}")"; then
      echo "the reopened EndpointSlices could not be normalized after NEG inspection" >&2
      return 1
    fi
    if [ "${cma_reopen_neg_record_after}" != \
         "${cma_reopen_neg_record_before}" ] || \
       [ "${cma_reopen_slice_record_after}" != \
         "${cma_reopen_slice_record_before}" ]; then
      echo "the reopened Service or EndpointSlice identities changed during NEG inspection" >&2
      return 1
    fi

    case "${cma_reopen_endpoint_state}" in
      exact)
        if printf '%s' "${cma_reopen_neg_endpoints}" | jq -e \
          --arg ip "${cma_final_pod_ip}" '
            length == 1 and .[0].ipAddress == $ip and .[0].port == 8080
          ' >/dev/null; then
          if ! cma_reopen_projection="$(jq -cn \
            --argjson service "${cma_reopen_neg_record_before}" \
            --argjson slices "${cma_reopen_slice_record_before}" \
            --argjson endpoints "${cma_reopen_neg_endpoints}" '
              {
                service:$service,
                endpointSlices:$slices,
                negEndpoints:($endpoints | sort_by(tojson))
              }
            ')"; then
            echo "the reopened Service/EndpointSlice/NEG projection could not be normalized" >&2
            return 1
          fi
          if [ -n "${cma_reopen_committed_projection}" ] && \
             [ "${cma_reopen_projection}" != \
               "${cma_reopen_committed_projection}" ]; then
            echo "the reopened Service/EndpointSlice/NEG projection changed across public verification" >&2
            return 1
          fi
          cma_reopen_committed_projection="${cma_reopen_projection}"
          if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
            echo "the exact reopened projection completed after the five-minute deadline" >&2
            return 1
          fi
          return 0
        fi
        if ! printf '%s' "${cma_reopen_neg_endpoints}" | \
          jq -e 'length == 0' >/dev/null; then
          echo "a reopened zonal NEG member is not the exact verified pod address and port" >&2
          return 1
        fi
        ;;
      pending)
        if ! printf '%s' "${cma_reopen_neg_endpoints}" | \
          jq -e 'length == 0' >/dev/null; then
          echo "a zonal NEG became routable before the verified EndpointSlice projection" >&2
          return 1
        fi
        ;;
      wrong)
        echo "a routable EndpointSlice entry is not exactly the verified pod address and identity" >&2
        return 1
        ;;
      *)
        echo "the reopened EndpointSlice gate returned an unknown state" >&2
        return 1
        ;;
    esac
    if cma_step10_deadline_reached "${cma_reopen_deadline}"; then
      echo "the Service and zonal NEGs did not converge by the reopen deadline" >&2
      return 1
    fi
    if ! sleep 2; then
      echo "the reopened EndpointSlice/NEG wait was interrupted" >&2
      return 1
    fi
  done
}
cma_refence_api() {
  cma_refence_patch_status=0
  cma_step10_kubectl patch service cma-api --type=json \
    -p='[{"op":"test","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}},{"op":"replace","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}}]' || cma_refence_patch_status=$?
  if ! cma_refence_state="$(cma_read_selector_state)"; then
    echo "CRITICAL: could not read cma-api after the source-to-fence patch; impose a manual traffic fence" >&2
    return 1
  fi
  case "${cma_refence_state}" in
    fenced) ;;
    source)
      echo "CRITICAL: cma-api still has its source selector after the re-fence attempt; impose a manual traffic fence" >&2
      return 1
      ;;
    *)
      echo "CRITICAL: cma-api has an unknown selector after the re-fence attempt; impose a manual traffic fence" >&2
      return 1
      ;;
  esac
  if ! cma_wait_for_fenced_endpoints; then
    return 1
  fi
  if [ "${cma_refence_patch_status}" -ne 0 ]; then
    echo "the re-fence patch reported failure, but the exact selector, zero routable EndpointSlice entries, and empty zonal NEGs were verified" >&2
  fi
  return 0
}
cma_abort_after_reopen() {
  echo "$1; re-fencing cma-api before exit" >&2
  if cma_refence_api; then
    cma_traffic_open=0
    echo "cma-api is fenced again" >&2
    return 0
  fi
  echo "CRITICAL: automatic re-fencing could not be proved; impose and verify a manual traffic fence" >&2
  return 1
}
cma_refence_on_exit() {
  trap '' HUP INT QUIT TERM
  exec 9<&-
  cma_exit_status="$1"
  trap - EXIT
  if [ "${cma_traffic_open}" -eq 1 ]; then
    if [ "${cma_exit_status}" -eq 0 ]; then
      cma_exit_status=1
    fi
    cma_refence_api || echo "CRITICAL: exit-time re-fencing could not be proved; impose and verify a manual traffic fence" >&2
  fi
  exit "${cma_exit_status}"
}
cma_refence_on_signal() {
  trap '' HUP INT QUIT TERM
  exec 9<&-
  echo "traffic-restoration shell interrupted; re-fencing before exit" >&2
  exit 1
}
cma_step10_prove_fence_main() {
  local cma_fenced_projection_dir cma_predecessor_projection
  local cma_predecessor_projection_sha256 cma_fenced_projection_sha256
  local cma_source_projection_sha256
  if [ "$#" -ne 0 ]; then
    echo "usage: ${0##*/} --prove-fence" >&2
    return 2
  fi
  cma_step10_require_privileged_bash || return 1
  cma_step10_require_trusted_launcher || return 1
  cma_step10_reject_unexpected_token_fd || return 1
  : "${CMA_GATEWAY_CONFIRM:?set the dedicated read-only fence-proof confirmation}"
  : "${CMA_GATEWAY_RELEASE_REVISION:?export the reviewed full release commit}"
  : "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:?establish the checked-routing-graph freeze}"
  cma_step10_require_checkout || return 1
  cma_step10_require_confirmation prove fence || return 1
  cma_step10_load_fence_receipt || {
    echo "the durable pre-patch fence receipt could not be loaded" >&2
    return 1
  }
  cma_step10_validate_admission_reservation || {
    echo "the permanent cutover-fence label reservation drifted" >&2
    return 1
  }
  cma_source_projection_sha256="$(jq -er .sourceProjection.sha256 \
    "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}")" || return 1
  cma_step10_sha256_valid "${cma_source_projection_sha256}" || return 1
  cma_predecessor_projection="${CMA_GATEWAY_FENCED_PROJECTION_PARENT:-}/fenced-pre-scale-${CMA_GATEWAY_RELEASE_REVISION}/projection.json"
  cma_predecessor_projection_sha256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:-}"
  cma_step10_sha256_valid "${cma_predecessor_projection_sha256}" || {
    echo "the pre-scale fence projection requires its independently recorded SHA-256" >&2
    return 1
  }
  if [ "$(jq -er .lifecycle.stage "${cma_predecessor_projection}" 2>/dev/null || :)" != \
     fenced-pre-scale ] || \
     ! cma_step10_validate_fence_projection_receipt \
       "${cma_predecessor_projection%/projection.json}" \
       fenced-pre-scale "${cma_source_projection_sha256}"; then
    echo "the durable pre-scale fence projection is unavailable or differs from the pre-patch root" >&2
    return 1
  fi
  if ! cma_step10_validate_scaled_down_workload; then
    echo "cma-api is not stably scaled to zero or a predecessor API Pod still exists" >&2
    return 1
  fi
  if ! cma_wait_for_fenced_endpoints; then
    echo "the exact EndpointSlice and zonal NEG traffic fence is not proved" >&2
    return 1
  fi
  cma_fenced_projection_dir="$(cma_step10_projection_capture \
    "${CMA_GATEWAY_FENCED_PROJECTION_PARENT:-}" fenced-post-scale fenced \
    fenced-post-scale "${cma_predecessor_projection}" fenced-pre-scale \
    "${cma_predecessor_projection_sha256}" \
    "${cma_source_projection_sha256}")" || {
    echo "the stable fenced closed projection could not be captured" >&2
    return 1
  }
  cma_fenced_projection_sha256="$(cma_step10_validate_fence_projection \
    "${cma_fenced_projection_dir}" fenced-post-scale \
    "${cma_predecessor_projection_sha256}")" || {
    echo "the fenced closed projection does not equal the durable source root" >&2
    return 1
  }
  cma_step10_sha256_valid "${cma_fenced_projection_sha256}" || return 1
  printf '%s\n' \
    "CMA traffic fence is exact and externally drained; projection ${cma_fenced_projection_dir}; projection SHA-256 ${cma_fenced_projection_sha256}" || return 1
}
cma_step10_fence_main() {
  local cma_source_projection cma_fence_receipt_ack cma_fenced_projection_dir
  local cma_source_projection_sha256 cma_source_projection_predecessor_sha256
  local cma_fenced_projection_sha256
  if [ "$#" -ne 0 ]; then
    echo "usage: ${0##*/} --fence" >&2
    return 2
  fi
  cma_step10_require_production_mutation_enabled || return 1
  cma_step10_require_privileged_bash || return 1
  cma_step10_require_trusted_launcher || return 1
  cma_step10_reject_unexpected_token_fd || return 1
  : "${CMA_GATEWAY_CONFIRM:?set the dedicated traffic-fence confirmation}"
  : "${CMA_GATEWAY_RELEASE_REVISION:?export the reviewed full release commit}"
  : "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:?establish the checked-routing-graph freeze}"
  cma_step10_require_checkout || return 1
  cma_step10_require_confirmation fence traffic || return 1
  cma_step10_validate_admission_reservation || {
    echo "the permanent cutover-fence label reservation is not effective" >&2
    return 1
  }
  if ! cma_fence_label_has_no_pods; then
    echo "the cutover fence selector unexpectedly matches a pod" >&2
    return 1
  fi
  if ! cma_fence_initial_service="$(cma_step10_kubectl \
    get service cma-api -o json)" || \
     ! cma_fence_initial_state="$(printf '%s' "${cma_fence_initial_service}" | \
       jq -er '
         if .spec.selector == {"app.kubernetes.io/name":"cma-api"} then "source"
         elif .spec.selector == {"release.chrono-ai.fun/cutover-fence":"mongodb-715"} then "fenced"
         else "unknown"
         end')"; then
    echo "cma-api could not be read before fencing" >&2
    return 1
  fi
  case "${cma_fence_initial_state}" in
    source)
      cma_fence_initial_record="$(cma_parse_source_api_neg \
        "${cma_fence_initial_service}")" || return 1
      cma_source_projection="${CMA_GATEWAY_SOURCE_PROJECTION:-}"
      cma_source_projection_sha256="${CMA_GATEWAY_SOURCE_PROJECTION_SHA256:-}"
      cma_source_projection_predecessor_sha256="${CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256:-}"
      cma_step10_require_projection_file "${cma_source_projection}" \
        "${cma_source_projection_sha256}" \
        "${cma_source_projection_predecessor_sha256}" || {
        echo "CMA_GATEWAY_SOURCE_PROJECTION must name the stable adopted source projection" >&2
        return 1
      }
      cma_step10_projection_check_live "${cma_source_projection}" || {
        echo "the pre-patch live graph differs from the stable adopted source projection" >&2
        return 1
      }
      cma_fence_receipt_ack="$(cma_step10_write_fence_receipt \
        "${cma_source_projection}" "${cma_fence_initial_record}")" || {
        echo "the durable pre-patch fence receipt could not be committed" >&2
        return 1
      }
      CMA_GATEWAY_FENCE_RECEIPT_SHA256="${cma_fence_receipt_ack##*|}"
      export -n CMA_GATEWAY_FENCE_RECEIPT_SHA256
      cma_step10_load_fence_receipt || return 1
      ;;
    fenced)
      cma_step10_load_fence_receipt || {
        echo "an already-fenced Service requires its existing durable pre-patch root receipt" >&2
        return 1
      }
      cma_source_projection="$(jq -er .sourceProjection.path \
        "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}")" || {
        echo "the durable pre-patch root does not identify its source projection" >&2
        return 1
      }
      cma_source_projection_sha256="$(jq -er .sourceProjection.sha256 \
        "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}")" || return 1
      cma_source_projection_predecessor_sha256="$(jq -er \
        .sourceProjection.lifecycle.predecessorSha256 \
        "${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}")" || return 1
      cma_step10_require_projection_file "${cma_source_projection}" \
        "${cma_source_projection_sha256}" \
        "${cma_source_projection_predecessor_sha256}" || {
        echo "the durable pre-patch source projection is unavailable" >&2
        return 1
      }
      cma_step10_source_projection_matches_active_receipt \
        "${cma_source_projection}" || {
        echo "the durable pre-patch source projection differs from its fence receipt" >&2
        return 1
      }
      cma_fence_initial_record="$(cma_parse_fenced_api_neg \
        "${cma_fence_initial_service}")" || return 1
      ;;
    *)
      echo "cma-api has an unknown selector before fencing" >&2
      return 1
      ;;
  esac
  printf '%s\n' \
    "CMA durable fence root committed before Service mutation; receipt ${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}; SHA-256 ${CMA_GATEWAY_FENCE_RECEIPT_SHA256}" || return 1
  if ! cma_pin_or_check_api_root "${cma_fence_initial_record}"; then
    echo "cma-api is not the exact pinned Service/NEG root before fencing" >&2
    return 1
  fi
  cma_fence_patch_status=0
  if [ "${cma_fence_initial_state}" = source ]; then
    cma_step10_kubectl patch service cma-api --type=json \
      -p='[{"op":"test","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}},{"op":"replace","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}}]' || \
      cma_fence_patch_status=$?
  fi
  if ! cma_fence_observed_state="$(cma_read_selector_state)"; then
    echo "CRITICAL: the fence patch completed ambiguously and cma-api could not be read; impose and verify a manual traffic fence" >&2
    return 1
  fi
  case "${cma_fence_observed_state}" in
    fenced) ;;
    source)
      echo "the fence patch did not apply; cma-api remains at the exact source selector" >&2
      return 1
      ;;
    *)
      echo "CRITICAL: the fence patch left an unknown cma-api selector; impose and verify a manual traffic fence" >&2
      return 1
      ;;
  esac
  if ! cma_wait_for_fenced_endpoints; then
    echo "the exact EndpointSlice and zonal NEG traffic fence is not proved" >&2
    return 1
  fi
  cma_fenced_projection_dir="$(cma_step10_projection_capture \
    "${CMA_GATEWAY_FENCED_PROJECTION_PARENT:-}" fenced-pre-scale fenced \
    fenced-pre-scale "${cma_source_projection}" adopted \
    "${cma_source_projection_sha256}" \
    "${cma_source_projection_predecessor_sha256}")" || {
    echo "the stable fenced closed projection could not be captured" >&2
    return 1
  }
  cma_fenced_projection_sha256="$(cma_step10_validate_fence_projection \
    "${cma_fenced_projection_dir}" fenced-pre-scale \
    "${cma_source_projection_sha256}")" || {
    echo "the fenced projection did not prove every durable backend group empty" >&2
    return 1
  }
  cma_step10_sha256_valid "${cma_fenced_projection_sha256}" || return 1
  if [ "${cma_fence_patch_status}" -ne 0 ]; then
    echo "the fence patch reported failure, but the exact applied fence and external drain were independently proved" >&2
  fi
  printf '%s\n' "CMA traffic is fenced and externally drained; receipt ${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}; SHA-256 ${CMA_GATEWAY_FENCE_RECEIPT_SHA256}; projection ${cma_fenced_projection_dir}; projection SHA-256 ${cma_fenced_projection_sha256}" || return 1
}
cma_step10_main() {
  local cma_fenced_projection_dir cma_final_projection_dir cma_final_topology
  local cma_pre_scale_projection_sha256
  local cma_post_scale_projection cma_post_scale_projection_sha256
  local cma_fenced_projection_sha256 cma_final_projection_sha256
  if [ "$#" -ne 0 ]; then
    echo "usage: ${0##*/}" >&2
    return 2
  fi
  cma_step10_require_production_mutation_enabled || return 1
  cma_step10_require_privileged_bash || return 1
  cma_step10_require_trusted_launcher || return 1
  : "${CMA_FINAL_POD:?step 9 did not record the verified final pod}"
  : "${CMA_FINAL_POD_UID:?step 9 did not record the verified final pod UID}"
  : "${CMA_FINAL_POD_IP:?step 9 did not record the verified final pod IP}"
  : "${CMA_FINAL_DEPLOYMENT_UID:?step 9 did not record the verified Deployment UID}"
  : "${CMA_FINAL_REPLICA_SET:?step 9 did not record the verified ReplicaSet}"
  : "${CMA_FINAL_REPLICA_SET_UID:?step 9 did not record the verified ReplicaSet UID}"
  : "${CMA_FINAL_POD_IMAGE:?step 9 did not record the manifest-bound Pod image}"
  : "${CMA_FINAL_POD_IMAGE_ID:?step 9 did not record the verified immutable imageID}"
  if ! ulimit -c 0; then
    echo "core dumps must be disabled before reading the release bearer" >&2
    return 1
  fi
  unset CURL_CA_BUNDLE CURL_SSL_BACKEND SSL_CERT_DIR SSL_CERT_FILE SSLKEYLOGFILE
  : "${CMA_GATEWAY_TOKEN_FD:?trusted launcher did not provide the protected bearer descriptor}"
  if [ "${CMA_GATEWAY_TOKEN_FD}" != 8 ]; then
    echo "the protected bearer must arrive on reviewed descriptor 8" >&2
    return 1
  fi
  unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi
  unset cma_direct_sessions cma_app_sessions
  unset cma_reopen_committed_projection
  exec 9<&-
  if ! IFS= read -r cma_release_access_token <&8; then
    exec 8<&-
    echo "the protected bearer could not be read exactly once" >&2
    return 1
  fi
  exec 8<&-
  export -n cma_release_access_token
  unset CMA_GATEWAY_TOKEN_FD CHRONO_RELEASE_ACCESS_TOKEN
  case "${cma_release_access_token}" in
    *$'\r'*|*$'\n'*)
      echo "the protected release bearer must not contain CR or LF" >&2
      unset cma_release_access_token
      return 1
      ;;
  esac
  : "${CMA_GATEWAY_CONFIRM:?set the dedicated traffic-restoration confirmation while traffic is fenced}"
  : "${CMA_GATEWAY_RELEASE_REVISION:?export the reviewed full release commit while traffic is fenced}"
  : "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:?re-establish the checked-routing-graph freeze while traffic is fenced}"
  : "${CMA_GATEWAY_ROLLBACK_DIR:?export the retained sealed adoption snapshot path}"
  : "${CMA_GATEWAY_ROLLBACK_SHA256:?export the independently recorded adoption snapshot digest}"
  cma_final_pod="${CMA_FINAL_POD}"
  cma_final_pod_uid="${CMA_FINAL_POD_UID}"
  cma_final_pod_ip="${CMA_FINAL_POD_IP}"
  cma_final_deployment_uid="${CMA_FINAL_DEPLOYMENT_UID}"
  cma_final_replica_set="${CMA_FINAL_REPLICA_SET}"
  cma_final_replica_set_uid="${CMA_FINAL_REPLICA_SET_UID}"
  cma_final_pod_image="${CMA_FINAL_POD_IMAGE}"
  cma_final_pod_image_id="${CMA_FINAL_POD_IMAGE_ID}"
  cma_pinned_api_root_identity=""
  cma_reopen_committed_projection=""
  cma_step10_require_checkout || return 1
  cma_step10_require_confirmation restore traffic || return 1
  cma_manifest_image="$(cma_manifest_api_image)" || {
    echo "the reviewed API Deployment manifest image could not be resolved" >&2
    return 1
  }
  if [ "${cma_final_pod_image}" != "${cma_manifest_image}" ]; then
    echo "the retained Pod image differs from the reviewed API Deployment manifest" >&2
    return 1
  fi
  if ! [[ "${cma_final_pod_image_id}" =~ ^(docker-pullable://)?us-central1-docker\.pkg\.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:[0-9a-f]{64}$ ]]; then
    echo "the retained Pod imageID is not an immutable CMA API registry digest" >&2
    return 1
  fi
  cma_step10_load_fence_receipt || {
    echo "traffic restoration requires the durable pre-patch fence receipt" >&2
    return 1
  }
  cma_step10_validate_admission_reservation || {
    echo "the permanent cutover-fence label reservation drifted before reopen" >&2
    return 1
  }
  cma_step10_validate_snapshot_seal || return 1
trap '' HUP INT QUIT TERM
cma_traffic_open=1
trap 'cma_refence_on_exit "$?"' EXIT
trap cma_refence_on_signal HUP INT QUIT TERM
if ! cma_final_workload_is_exact; then
  echo "the final Deployment/ReplicaSet/Pod ownership, image, or readiness proof failed; re-fencing before exit" >&2
  exit 1
fi
cma_final_topology="$(cma_final_pod_topology)" || {
  echo "the final pod node/provider topology proof failed; re-fencing before exit" >&2
  exit 1
}
cma_final_pod_node_name="$(printf '%s' "${cma_final_topology}" | jq -er .nodeName)" || exit 1
cma_final_pod_provider_zone="$(printf '%s' "${cma_final_topology}" | jq -er .providerZone)" || exit 1
if ! cma_initial_selector_state="$(cma_read_selector_state)"; then
  echo "the initial cma-api selector could not be read; re-fencing before exit" >&2
  exit 1
fi
if [ "${cma_initial_selector_state}" != fenced ]; then
  echo "the initial cma-api selector is not the exact fence; re-fencing before exit" >&2
  exit 1
fi
if ! cma_wait_for_fenced_endpoints; then
  echo "the initial EndpointSlice and zonal NEG traffic fence is not proved" >&2
  exit 1
fi
cma_post_scale_projection="${CMA_GATEWAY_FENCED_PROJECTION_PARENT:-}/fenced-post-scale-${CMA_GATEWAY_RELEASE_REVISION}/projection.json"
cma_pre_scale_projection_sha256="${CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256:-}"
cma_post_scale_projection_sha256="${CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256:-}"
cma_step10_sha256_valid "${cma_pre_scale_projection_sha256}" || {
  echo "the post-scale projection ancestry requires the independently recorded pre-scale SHA-256" >&2
  exit 1
}
cma_step10_sha256_valid "${cma_post_scale_projection_sha256}" || {
  echo "the post-scale fence projection requires its independently recorded SHA-256" >&2
  exit 1
}
cma_fenced_projection_dir="$(cma_step10_projection_capture \
  "${CMA_GATEWAY_FENCED_PROJECTION_PARENT:-}" fenced-pre-reopen fenced \
  fenced-pre-reopen "${cma_post_scale_projection}" fenced-post-scale \
  "${cma_post_scale_projection_sha256}" \
  "${cma_pre_scale_projection_sha256}")" || {
  echo "the immediate pre-reopen fenced closed projection could not be captured" >&2
  exit 1
}
cma_fenced_projection_sha256="$(cma_step10_validate_fence_projection \
  "${cma_fenced_projection_dir}" fenced-pre-reopen \
  "${cma_post_scale_projection_sha256}")" || {
  echo "the immediate pre-reopen projection does not prove the durable backend root empty" >&2
  exit 1
}
cma_step10_sha256_valid "${cma_fenced_projection_sha256}" || exit 1
cma_traffic_open=0
if ! cma_route_and_policies_ready; then
  echo "the exact app-origin route and attached Gateway policies are not ready; traffic remains fenced" >&2
  exit 1
fi
if ! cma_final_workload_is_exact || \
   [ "$(cma_final_pod_topology)" != "${cma_final_topology}" ]; then
  echo "the retained workload drifted after the pre-reopen projection; traffic remains fenced" >&2
  exit 1
fi
cma_traffic_open=1
if ! cma_step10_kubectl patch service cma-api --type=json \
  -p='[{"op":"test","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}},{"op":"replace","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}}]'; then
  if ! cma_reopen_state="$(cma_read_selector_state)"; then
    echo "CRITICAL: the reopen patch failed and the live selector could not be read; impose and verify a manual traffic fence" >&2
    exit 1
  fi
  case "${cma_reopen_state}" in
    fenced)
      if cma_wait_for_fenced_endpoints; then
        cma_traffic_open=0
        echo "the reopen patch failed and cma-api remains fenced" >&2
      else
        echo "CRITICAL: the reopen patch failed and the EndpointSlice/NEG fence could not be proved" >&2
      fi
      ;;
    source)
      if cma_refence_api; then
        cma_traffic_open=0
        echo "the reopen patch reported failure after applying the source selector; cma-api is fenced again" >&2
      else
        echo "CRITICAL: the reopen patch reported failure after applying the source selector, and automatic re-fencing could not be proved" >&2
      fi
      ;;
    *)
      echo "CRITICAL: the reopen patch failed and cma-api has an unknown selector; impose and verify a manual traffic fence" >&2
      ;;
  esac
  exit 1
fi
if ! cma_reopen_state="$(cma_read_selector_state)"; then
  cma_abort_after_reopen "the post-patch selector could not be read" || :
  exit 1
fi
if [ "${cma_reopen_state}" != 'source' ]; then
  cma_abort_after_reopen "the post-patch selector is not the exact source selector" || :
  exit 1
fi
if ! cma_wait_for_reopened_endpoint; then
  cma_abort_after_reopen "the Service and zonal NEGs did not converge on the exact verified pod" || :
  exit 1
fi
if ! cma_direct_livez="$(curl --disable --noproxy '*' --fail --show-error \
  --silent --max-time 15 --write-out '|%{http_code}' \
  https://bot-api.chrono-ai.fun/livez)"; then
  cma_abort_after_reopen "the direct public liveness request failed" || :
  exit 1
fi
if [ "${cma_direct_livez}" != 'ok|200' ]; then
  cma_abort_after_reopen "the direct public liveness check failed" || :
  exit 1
fi
if ! cma_direct_ready="$(curl --disable --noproxy '*' --fail --show-error \
  --silent --max-time 15 --write-out '|%{http_code}' \
  https://bot-api.chrono-ai.fun/ready)"; then
  cma_abort_after_reopen "the direct public readiness request failed" || :
  exit 1
fi
if [ "${cma_direct_ready}" != '{"status":"ready"}|200' ]; then
  cma_abort_after_reopen "the direct public readiness check failed" || :
  exit 1
fi
if ! cma_app_openapi="$(curl --disable --noproxy '*' --fail --show-error --silent --max-time 15 \
  https://bot.chrono-ai.fun/openapi.json)"; then
  cma_abort_after_reopen "the app-origin discovery check failed" || :
  exit 1
fi
if ! printf '%s' "${cma_app_openapi}" | \
  jq -e '.openapi == "3.1.0" and
         (.paths["/api/v1/sessions"].get | type) == "object"' >/dev/null; then
  cma_abort_after_reopen "the app-origin discovery response is not the API OpenAPI document" || :
  exit 1
fi
if ! cma_direct_sessions="$(cma_step10_authenticated_curl \
  'https://bot-api.chrono-ai.fun/api/v1/sessions?limit=1')"; then
  cma_abort_after_reopen "the authenticated direct API read failed" || :
  exit 1
fi
if ! printf '%s' "${cma_direct_sessions}" | jq -e 'type == "array"' >/dev/null; then
  cma_abort_after_reopen "the authenticated direct API read returned a non-session response" || :
  exit 1
fi
if ! cma_app_sessions="$(cma_step10_authenticated_curl \
  'https://bot.chrono-ai.fun/api/v1/sessions?limit=1')"; then
  cma_abort_after_reopen "the authenticated app-origin API read failed" || :
  exit 1
fi
if ! printf '%s' "${cma_app_sessions}" | jq -e 'type == "array"' >/dev/null; then
  cma_abort_after_reopen "the authenticated app-origin API read returned a non-session response" || :
  exit 1
fi
if ! cma_route_and_policies_ready; then
  cma_abort_after_reopen "the app-origin route or Gateway policy drifted during traffic restoration" || :
  exit 1
fi
if ! cma_wait_for_reopened_endpoint; then
  cma_abort_after_reopen "the exact EndpointSlice/NEG population drifted during traffic restoration" || :
  exit 1
fi
if ! cma_final_workload_is_exact; then
  cma_abort_after_reopen "the final workload ownership, image, readiness, or restart proof drifted during traffic restoration" || :
  exit 1
fi
if [ "$(cma_final_pod_topology)" != "${cma_final_topology}" ]; then
  cma_abort_after_reopen "the final pod node/provider topology drifted during traffic restoration" || :
  exit 1
fi
cma_final_projection_dir="$(cma_step10_projection_capture \
  "${CMA_GATEWAY_FINAL_PROJECTION_PARENT:-}" final-reopen source \
  final-reopen "${cma_fenced_projection_dir}/projection.json" fenced-pre-reopen \
  "${cma_fenced_projection_sha256}" \
  "${cma_post_scale_projection_sha256}")" || {
  cma_abort_after_reopen "the stable final closed projection could not be captured" || :
  exit 1
}
cma_final_projection_sha256="$(cma_step10_validate_source_projection \
  "${cma_final_projection_dir}")" || {
  cma_abort_after_reopen "the final live graph differs from the durable pre-fence backend root" || :
  exit 1
}
cma_step10_sha256_valid "${cma_final_projection_sha256}" || {
  cma_abort_after_reopen "the final projection validator returned an invalid digest" || :
  exit 1
}
if ! cma_final_workload_is_exact || \
   [ "$(cma_final_pod_topology)" != "${cma_final_topology}" ]; then
  cma_abort_after_reopen "the retained workload drifted after the final projection" || :
  exit 1
fi
trap '' HUP INT QUIT TERM
unset cma_release_access_token cma_direct_livez cma_direct_ready cma_app_openapi
unset cma_direct_sessions cma_app_sessions
unset cma_reopen_committed_projection cma_reopen_projection
if ! printf '%s\n' "CMA traffic restoration is complete and verified; final projection ${cma_final_projection_dir}; SHA-256 ${cma_final_projection_sha256}"; then
  echo "the successful traffic restoration could not be acknowledged; re-fencing" >&2
  exit 1
fi
cma_traffic_open=0
trap - EXIT
trap - HUP INT QUIT TERM
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  case "${1:-}" in
    --fence)
      shift
      cma_step10_fence_main "$@"
      ;;
    --prove-fence)
      shift
      cma_step10_prove_fence_main "$@"
      ;;
    '')
      cma_step10_main "$@"
      ;;
    *)
      echo "usage: ${0##*/} [--fence | --prove-fence]" >&2
      exit 2
      ;;
  esac
fi
