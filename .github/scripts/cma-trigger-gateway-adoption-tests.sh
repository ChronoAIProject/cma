#!/usr/bin/env bash
set -euo pipefail

# Behavioral failure-injection tests for the production Gateway adoption
# transaction. Every external boundary is replaced by a private, file-backed
# fake; the production adoption, snapshot, CAS, rollback, and signal handlers
# execute unchanged unless a case explicitly injects a failure at that seam.

CMA_TEST_SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CMA_TEST_SCRIPT_PATH="${CMA_TEST_SCRIPT_DIR}/$(basename "${BASH_SOURCE[0]}")"
CMA_TEST_REPO_ROOT="$(cd "${CMA_TEST_SCRIPT_DIR}/../.." && pwd)"
CMA_TEST_ADOPTION_SCRIPT="${CMA_TEST_SCRIPT_DIR}/cma-trigger-gateway-adoption.sh"
CMA_TEST_SUITE_DIR=""
CMA_TEST_SUITE_PARENT=""

readonly CMA_TEST_SCRIPT_DIR
readonly CMA_TEST_SCRIPT_PATH
readonly CMA_TEST_REPO_ROOT
readonly CMA_TEST_ADOPTION_SCRIPT

cma_test_fail() {
  printf 'not ok - %s\n' "$*" >&2
  return 1
}

cma_test_assert_empty_file() {
  local source_file="$1"
  local description="$2"

  [[ -f "${source_file}" ]] || cma_test_fail "missing ${description}: ${source_file}"
  [[ ! -s "${source_file}" ]] || cma_test_fail "${description} was not empty"
}

cma_test_assert_line_count() {
  local source_file="$1"
  local expected_count="$2"
  local description="$3"
  local actual_count

  [[ -f "${source_file}" ]] || cma_test_fail "missing ${description}: ${source_file}"
  actual_count="$(wc -l < "${source_file}" | tr -d ' ')"
  [[ "${actual_count}" == "${expected_count}" ]] || \
    cma_test_fail "${description}: expected ${expected_count} lines, got ${actual_count}"
}

cma_test_assert_cluster_matches() {
  local cluster_dir="$1"
  local target_dir="$2"
  local scratch_dir="$3"
  local identity_dir object_name

  identity_dir="${cluster_dir%/cluster}/legacy-fixture"
  mkdir -p "${scratch_dir}"
  for object_name in \
      cma-api-route cma-web-route cma-http-redirect-route cma-api-health; do
    jq -S '{
      apiVersion: .apiVersion,
      kind: .kind,
      metadata: {
        name: .metadata.name,
        namespace: .metadata.namespace,
        labels: (.metadata.labels // {}),
        uid: .metadata.uid,
        annotations: (.metadata.annotations // {}),
        finalizers: (.metadata.finalizers // []),
        ownerReferences: (.metadata.ownerReferences // [])
      },
      spec: .spec
    }' "${cluster_dir}/${object_name}.json" \
      > "${scratch_dir}/${object_name}.cluster.json"
    jq -S --slurpfile identity "${identity_dir}/${object_name}.json" '{
      apiVersion: .apiVersion,
      kind: .kind,
      metadata: {
        name: .metadata.name,
        namespace: .metadata.namespace,
        labels: (.metadata.labels // {}),
        uid: $identity[0].metadata.uid,
        annotations: ($identity[0].metadata.annotations // {}),
        finalizers: ($identity[0].metadata.finalizers // []),
        ownerReferences: ($identity[0].metadata.ownerReferences // [])
      },
      spec: .spec
    }' "${target_dir}/${object_name}.json" \
      > "${scratch_dir}/${object_name}.target.json"
    cmp "${scratch_dir}/${object_name}.cluster.json" \
      "${scratch_dir}/${object_name}.target.json" >/dev/null || \
      cma_test_fail "${object_name} did not match the expected state"
  done
}

cma_test_assert_rollback_order() {
  local source_file="$1"
  local actual_order

  cma_test_assert_line_count "${source_file}" 4 "rollback object attempts"
  actual_order="$(tr '\n' ' ' < "${source_file}")"
  [[ "${actual_order}" == \
    'cma-api-route cma-web-route cma-http-redirect-route cma-api-health ' ]] || \
    cma_test_fail "rollback did not attempt all four objects in production order"
}

cma_test_assert_route_rollback_order() {
  local source_file="$1"
  local actual_order

  cma_test_assert_line_count "${source_file}" 3 "rollback route attempts"
  actual_order="$(tr '\n' ' ' < "${source_file}")"
  [[ "${actual_order}" == \
    'cma-api-route cma-web-route cma-http-redirect-route ' ]] || \
    cma_test_fail "rollback did not attempt all three routes in production order"
}

cma_test_assert_temp_paths_scoped() {
  local case_dir="$1"
  local created_path

  while IFS= read -r created_path; do
    [[ -n "${created_path}" ]] || continue
    [[ "${created_path}" != / && "${created_path}" == "${case_dir}/"* ]] || \
      cma_test_fail "temporary path escaped the private case directory: ${created_path}"
  done < "${case_dir}/tmpdirs.log"
}

cma_test_key_from_identity() {
  local resource_kind="$1"
  local resource_name="$2"

  case "${resource_kind}:${resource_name}" in
    httproute:cma-api) printf '%s\n' cma-api-route ;;
    httproute:cma-web) printf '%s\n' cma-web-route ;;
    httproute:cma-http-redirect) printf '%s\n' cma-http-redirect-route ;;
    healthcheckpolicy:cma-api) printf '%s\n' cma-api-health ;;
    *) return 1 ;;
  esac
}

cma_test_key_from_file() {
  local source_file="$1"
  local object_kind object_name

  object_kind="$(jq -er '.kind | ascii_downcase' "${source_file}")" || return 1
  object_name="$(jq -er '.metadata.name' "${source_file}")" || return 1
  cma_test_key_from_identity "${object_kind}" "${object_name}"
}

cma_test_setup_fake_cluster() {
  local object_name cluster_file temporary_file

  mkdir -p \
    "${CMA_TEST_CASE_DIR}/cluster" \
    "${CMA_TEST_CASE_DIR}/custody" \
    "${CMA_TEST_CASE_DIR}/desired-fixture" \
    "${CMA_TEST_CASE_DIR}/legacy-fixture"
  chmod 700 \
    "${CMA_TEST_CASE_DIR}/cluster" \
    "${CMA_TEST_CASE_DIR}/custody" \
    "${CMA_TEST_CASE_DIR}/desired-fixture" \
    "${CMA_TEST_CASE_DIR}/legacy-fixture"

  cma_gateway_source_contract --emit \
    "${CMA_TEST_CASE_DIR}/desired-fixture" >/dev/null

  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    cluster_file="${CMA_TEST_CASE_DIR}/cluster/${object_name}.json"
    temporary_file="${cluster_file}.new"
    jq --arg uid "uid-${object_name}" '
      .metadata.uid = $uid |
      .metadata.resourceVersion = "1" |
      .metadata.annotations = {} |
      .metadata.finalizers = [] |
      .metadata.ownerReferences = []
    ' "${CMA_TEST_CASE_DIR}/desired-fixture/${object_name}.json" \
      > "${temporary_file}"
    mv "${temporary_file}" "${cluster_file}"
    chmod 600 "${cluster_file}"
  done

  cluster_file="${CMA_TEST_CASE_DIR}/cluster/cma-web-route.json"
  temporary_file="${cluster_file}.new"
  jq '
    .spec.rules = [{
      matches: [{path: {type: "PathPrefix", value: "/"}}],
      backendRefs: [{
        group: "",
        kind: "Service",
        name: "cma-web",
        port: 80,
        weight: 1
      }]
    }]
  ' "${cluster_file}" > "${temporary_file}"
  mv "${temporary_file}" "${cluster_file}"
  chmod 600 "${cluster_file}"

  cluster_file="${CMA_TEST_CASE_DIR}/cluster/cma-api-health.json"
  temporary_file="${cluster_file}.new"
  jq '.spec.default.config.httpHealthCheck.requestPath = "/livez"' \
    "${cluster_file}" > "${temporary_file}"
  mv "${temporary_file}" "${cluster_file}"
  chmod 600 "${cluster_file}"

  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    cp "${CMA_TEST_CASE_DIR}/cluster/${object_name}.json" \
      "${CMA_TEST_CASE_DIR}/legacy-fixture/${object_name}.json"
    chmod 600 "${CMA_TEST_CASE_DIR}/legacy-fixture/${object_name}.json"
  done

  : > "${CMA_TEST_CASE_DIR}/adopted-checks.log"
  : > "${CMA_TEST_CASE_DIR}/check.log"
  : > "${CMA_TEST_CASE_DIR}/dry-run.log"
  printf '%s\n' /livez > "${CMA_TEST_CASE_DIR}/effective-health-path"
  : > "${CMA_TEST_CASE_DIR}/forward-attempts.log"
  : > "${CMA_TEST_CASE_DIR}/forward-failures.log"
  : > "${CMA_TEST_CASE_DIR}/probe.log"
  : > "${CMA_TEST_CASE_DIR}/raw-boundary.log"
  : > "${CMA_TEST_CASE_DIR}/rollback-attempts.log"
  : > "${CMA_TEST_CASE_DIR}/tmpdirs.log"
  : > "${CMA_TEST_CASE_DIR}/writes.log"
}

cma_test_set_cluster_desired() {
  local object_name cluster_file temporary_file

  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    cluster_file="${CMA_TEST_CASE_DIR}/cluster/${object_name}.json"
    temporary_file="${cluster_file}.desired"
    jq --slurpfile target \
      "${CMA_TEST_CASE_DIR}/desired-fixture/${object_name}.json" '
        .metadata.labels = ($target[0].metadata.labels // {}) |
        .metadata.resourceVersion = "10" |
        .spec = $target[0].spec
      ' "${cluster_file}" > "${temporary_file}"
    mv "${temporary_file}" "${cluster_file}"
    chmod 600 "${cluster_file}"
  done
  printf '%s\n' /ready > "${CMA_TEST_CASE_DIR}/effective-health-path"
}

cma_test_make_snapshot() {
  local snapshot_dir="$1"
  local object_name

  mkdir "${snapshot_dir}"
  chmod 700 "${snapshot_dir}"
  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    cp "${CMA_TEST_CASE_DIR}/legacy-fixture/${object_name}.json" \
      "${snapshot_dir}/${object_name}.json"
    chmod 400 "${snapshot_dir}/${object_name}.json"
  done
  CMA_TEST_SNAPSHOT_DIGEST="$(cma_gateway_named_files_digest "${snapshot_dir}")"
}

# These guards turn an accidental escape from the fake boundary into a hard,
# locally visible test failure. They must never invoke the real commands.
kubectl() {
  printf 'kubectl %s\n' "$*" >> "${CMA_TEST_CASE_DIR}/raw-boundary.log"
  return 97
}

gcloud() {
  printf 'gcloud %s\n' "$*" >> "${CMA_TEST_CASE_DIR}/raw-boundary.log"
  return 97
}

curl() {
  printf 'curl %s\n' "$*" >> "${CMA_TEST_CASE_DIR}/raw-boundary.log"
  return 97
}

mktemp() {
  local created_path

  if [[ -z "${CMA_TEST_CASE_DIR:-}" ]]; then
    command mktemp "$@"
    return
  fi
  if [[ -e "${CMA_TEST_CASE_DIR}/fail-next-mktemp.armed" ]]; then
    command rm -f "${CMA_TEST_CASE_DIR}/fail-next-mktemp.armed"
    : > "${CMA_TEST_CASE_DIR}/mktemp-failure.injected"
    return 1
  fi
  if [[ "$#" -eq 1 && "$1" == -d ]]; then
    created_path="$(command mktemp -d \
      "${CMA_TEST_CASE_DIR}/runtime/cma-gateway.XXXXXXXXXX")" || return 1
  else
    created_path="$(command mktemp "$@")" || return 1
  fi
  printf '%s\n' "${created_path}" >> "${CMA_TEST_CASE_DIR}/tmpdirs.log"
  printf '%s\n' "${created_path}"
}

cma_gateway_require_checkout() {
  [[ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:-}" == sole-spec-writer ]] || \
    cma_gateway_die \
      "set CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer after fencing other spec writers"
  CMA_GATEWAY_SOURCE_SHA256="$(cma_gateway_source_digest)" || \
    cma_gateway_die "the test source bundle could not be hashed"
  cma_gateway_sha256_valid "${CMA_GATEWAY_SOURCE_SHA256}" || \
    cma_gateway_die "the test source bundle hash is malformed"
  export CMA_GATEWAY_SOURCE_SHA256
}

cma_gateway_require_production_mutation_enabled() {
  return 0
}

cma_gateway_validate_projection_receipt() {
  return 0
}

cma_gateway_verify_admission_receipt() {
  return 0
}

cma_test_projection_capture() {
  local output_dir="$2"
  [[ "${CMA_GATEWAY_PROJECTION_STAGE:-}" == adopted && \
    "${CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256:-}" == \
      "${CMA_GATEWAY_PROJECTION_SHA256:-}" ]] || return 1
  mkdir -p "${output_dir}"
  printf '%s\n' '{}' >"${output_dir}/capture.json"
  printf '%s\n' '{}' >"${output_dir}/projection.json"
  printf '%s  projection.json\n' \
    0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef \
    >"${output_dir}/projection.sha256"
}

cma_gateway_capture_projection() {
  cma_test_projection_capture "$@"
}

cma_gateway_source_contract() {
  if [[ "${CMA_TEST_CASE}" == seal_bundle_failure && \
        "${1:-}" == --seal-bundle ]]; then
    : > "${CMA_TEST_CASE_DIR}/seal-bundle-failure.injected"
    return 1
  fi
  cma_test_real_source_contract "$@"
}

cma_gateway_kubectl() {
  local operation="${1:-}"
  local resource_kind resource_name object_name source_file
  local dry_run=0 payload_file="" stored_file="" temporary_file
  local payload_uid payload_version stored_uid stored_version next_version

  case "${operation}" in
    get)
      [[ "$#" -eq 5 && "${4:-}" == -o && "${5:-}" == json ]] || return 1
      resource_kind="$2"
      resource_name="$3"
      object_name="$(cma_test_key_from_identity \
        "${resource_kind}" "${resource_name}")" || return 1
      source_file="${CMA_TEST_CASE_DIR}/cluster/${object_name}.json"
      [[ -f "${source_file}" ]] || return 1
      command cat "${source_file}"
      ;;
    replace)
      shift
      while [[ "$#" -gt 0 ]]; do
        case "$1" in
          --dry-run=server)
            dry_run=1
            shift
            ;;
          -f)
            [[ "$#" -ge 2 ]] || return 1
            payload_file="$2"
            shift 2
            ;;
          -o)
            [[ "$#" -ge 2 && "$2" == json ]] || return 1
            shift 2
            ;;
          *) return 1 ;;
        esac
      done
      [[ -n "${payload_file}" && -f "${payload_file}" ]] || return 1
      object_name="$(cma_test_key_from_file "${payload_file}")" || return 1
      stored_file="${CMA_TEST_CASE_DIR}/cluster/${object_name}.json"
      [[ -f "${stored_file}" ]] || return 1

      if [[ "${dry_run}" -eq 1 ]]; then
        printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/dry-run.log"
        if [[ ("${CMA_TEST_CASE}" == preflight_failure || \
              "${CMA_TEST_CASE}" == conditional_prewrite_failure) && \
              "${object_name}" == cma-web-route ]]; then
          return 1
        fi
        if [[ "${CMA_TEST_CASE}" == dry_run_mutated_success && \
              "${object_name}" == cma-web-route ]]; then
          : > "${CMA_TEST_CASE_DIR}/dry-run-mutated.injected"
          command cat "${stored_file}"
          return 0
        fi
      elif [[ "${CMA_TEST_CASE}" == resource_version_conflict && \
              "${object_name}" == cma-api-health && \
              ! -e "${CMA_TEST_CASE_DIR}/resource-version-conflict.injected" ]]; then
        temporary_file="${stored_file}.server-advance"
        jq '.metadata.resourceVersion = "2"' \
          "${stored_file}" > "${temporary_file}"
        mv "${temporary_file}" "${stored_file}"
        chmod 600 "${stored_file}"
        : > "${CMA_TEST_CASE_DIR}/resource-version-conflict.injected"
      fi

      payload_uid="$(jq -er '.metadata.uid' "${payload_file}")" || return 1
      payload_version="$(jq -er '.metadata.resourceVersion' "${payload_file}")" || \
        return 1
      stored_uid="$(jq -er '.metadata.uid' "${stored_file}")" || return 1
      stored_version="$(jq -er '.metadata.resourceVersion' "${stored_file}")" || \
        return 1
      if [[ "${payload_uid}" != "${stored_uid}" || \
            "${payload_version}" != "${stored_version}" ]]; then
        printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/cas-conflicts.log"
        return 1
      fi

      if [[ "${dry_run}" -eq 1 ]]; then
        command cat "${payload_file}"
        return 0
      fi
      [[ "${stored_version}" =~ ^[0-9]+$ ]] || return 1
      next_version="$((stored_version + 1))"
      temporary_file="${stored_file}.replace"
      jq --arg resource_version "${next_version}" \
        '.metadata.resourceVersion = $resource_version' \
        "${payload_file}" > "${temporary_file}"
      mv "${temporary_file}" "${stored_file}"
      chmod 600 "${stored_file}"
      printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/writes.log"
      if [[ "${object_name}" == cma-api-health ]]; then
        jq -r '.spec.default.config.httpHealthCheck.requestPath' \
          "${stored_file}" > "${CMA_TEST_CASE_DIR}/effective-health-path"
      fi
      if [[ "${CMA_TEST_CASE}" == replace_mutated_success && \
            "${object_name}" == cma-api-health ]]; then
        : > "${CMA_TEST_CASE_DIR}/replace-mutated.injected"
        jq '.spec.default.config.httpHealthCheck.requestPath = "/mutated"' \
          "${stored_file}"
        return 0
      fi
      if [[ "${CMA_TEST_CASE}" == post_write_uid_recreation && \
            "${object_name}" == cma-api-health ]]; then
        cp "${stored_file}" "${CMA_TEST_CASE_DIR}/post-write-valid-response.json"
        temporary_file="${stored_file}.recreated"
        jq '.metadata.uid = "uid-cma-api-health-recreated" |
            .metadata.resourceVersion = "1"' \
          "${stored_file}" > "${temporary_file}"
        mv "${temporary_file}" "${stored_file}"
        chmod 600 "${stored_file}"
        : > "${CMA_TEST_CASE_DIR}/post-write-uid-recreation.injected"
        command cat "${CMA_TEST_CASE_DIR}/post-write-valid-response.json"
        return 0
      fi
      command cat "${stored_file}"
      ;;
    *) return 1 ;;
  esac
}

cma_test_state_matches() {
  local target_dir="$1"
  local expected_effective_path object_name

  for object_name in ${CMA_GATEWAY_OBJECTS}; do
    jq -e \
      --slurpfile target "${target_dir}/${object_name}.json" \
      --slurpfile identity \
        "${CMA_TEST_CASE_DIR}/legacy-fixture/${object_name}.json" '
      .apiVersion == $target[0].apiVersion and
      .kind == $target[0].kind and
      .metadata.name == $target[0].metadata.name and
      .metadata.namespace == $target[0].metadata.namespace and
      .metadata.uid == $identity[0].metadata.uid and
      (.metadata.annotations // {}) ==
        ($identity[0].metadata.annotations // {}) and
      (.metadata.finalizers // []) ==
        ($identity[0].metadata.finalizers // []) and
      (.metadata.ownerReferences // []) ==
        ($identity[0].metadata.ownerReferences // []) and
      (.metadata.labels // {}) == ($target[0].metadata.labels // {}) and
      .spec == $target[0].spec
    ' "${CMA_TEST_CASE_DIR}/cluster/${object_name}.json" >/dev/null || return 1
  done
  expected_effective_path="$(jq -er \
    '.spec.default.config.httpHealthCheck.requestPath' \
    "${target_dir}/cma-api-health.json")" || return 1
  [[ "$(command cat "${CMA_TEST_CASE_DIR}/effective-health-path")" == \
    "${expected_effective_path}" ]]
}

cma_test_ready_transition_matches() {
  local snapshot_dir="$1"
  local object_name

  for object_name in cma-api-route cma-web-route cma-http-redirect-route; do
    jq -e \
      --slurpfile target "${snapshot_dir}/${object_name}.json" \
      --slurpfile identity \
        "${CMA_TEST_CASE_DIR}/legacy-fixture/${object_name}.json" '
      .spec == $target[0].spec and
      .metadata.uid == $identity[0].metadata.uid and
      (.metadata.annotations // {}) ==
        ($identity[0].metadata.annotations // {}) and
      (.metadata.finalizers // []) ==
        ($identity[0].metadata.finalizers // []) and
      (.metadata.ownerReferences // []) ==
        ($identity[0].metadata.ownerReferences // []) and
      (.metadata.labels // {}) == ($target[0].metadata.labels // {})
    ' "${CMA_TEST_CASE_DIR}/cluster/${object_name}.json" >/dev/null || return 1
  done
  jq -e \
    --slurpfile target "${CMA_GATEWAY_DESIRED_DIR}/cma-api-health.json" \
    --slurpfile identity \
      "${CMA_TEST_CASE_DIR}/legacy-fixture/cma-api-health.json" '
      .spec == $target[0].spec and
      .metadata.uid == $identity[0].metadata.uid and
      (.metadata.annotations // {}) ==
        ($identity[0].metadata.annotations // {}) and
      (.metadata.finalizers // []) ==
        ($identity[0].metadata.finalizers // []) and
      (.metadata.ownerReferences // []) ==
        ($identity[0].metadata.ownerReferences // []) and
      (.metadata.labels // {}) == ($target[0].metadata.labels // {})
    ' "${CMA_TEST_CASE_DIR}/cluster/cma-api-health.json" >/dev/null || return 1
  [[ "$(command cat "${CMA_TEST_CASE_DIR}/effective-health-path")" == /ready ]]
}

cma_test_rollback_routes_match() {
  local snapshot_dir="$1"
  local effective_path object_name

  for object_name in cma-api-route cma-web-route cma-http-redirect-route; do
    jq -e \
      --slurpfile target "${snapshot_dir}/${object_name}.json" \
      --slurpfile identity \
        "${CMA_TEST_CASE_DIR}/legacy-fixture/${object_name}.json" '
      .spec == $target[0].spec and
      .metadata.uid == $identity[0].metadata.uid and
      (.metadata.annotations // {}) ==
        ($identity[0].metadata.annotations // {}) and
      (.metadata.finalizers // []) ==
        ($identity[0].metadata.finalizers // []) and
      (.metadata.ownerReferences // []) ==
        ($identity[0].metadata.ownerReferences // []) and
      (.metadata.labels // {}) == ($target[0].metadata.labels // {})
    ' "${CMA_TEST_CASE_DIR}/cluster/${object_name}.json" >/dev/null || return 1
  done
  jq -e \
    --slurpfile snapshot "${snapshot_dir}/cma-api-health.json" \
    --slurpfile desired "${CMA_GATEWAY_DESIRED_DIR}/cma-api-health.json" \
    --slurpfile identity \
      "${CMA_TEST_CASE_DIR}/legacy-fixture/cma-api-health.json" '
      .metadata.uid == $identity[0].metadata.uid and
      (.metadata.annotations // {}) ==
        ($identity[0].metadata.annotations // {}) and
      (.metadata.finalizers // []) ==
        ($identity[0].metadata.finalizers // []) and
      (.metadata.ownerReferences // []) ==
        ($identity[0].metadata.ownerReferences // []) and
      ((.spec == $snapshot[0].spec and
        (.metadata.labels // {}) == ($snapshot[0].metadata.labels // {})) or
       (.spec == $desired[0].spec and
        (.metadata.labels // {}) == ($desired[0].metadata.labels // {}))) and
      (.spec.default.config.httpHealthCheck.requestPath == "/livez" or
       .spec.default.config.httpHealthCheck.requestPath == "/ready")
    ' "${CMA_TEST_CASE_DIR}/cluster/cma-api-health.json" >/dev/null || return 1
  effective_path="$(command cat \
    "${CMA_TEST_CASE_DIR}/effective-health-path")" || return 1
  [[ "${effective_path}" == /livez || "${effective_path}" == /ready ]]
}

cma_gateway_check() {
  local check_mode="${1:-}"
  local check_count

  printf '%s\n' "$*" >> "${CMA_TEST_CASE_DIR}/check.log"
  case "${check_mode}" in
    --validate-snapshot)
      [[ "$#" -eq 2 ]]
      ;;
    --check-snapshot)
      [[ "$#" -eq 2 ]] || return 1
      if [[ "${CMA_TEST_CASE}" == \
              rollback_cross_policy_livez_effective_ready ]]; then
        printf '%s\n' /livez > "${CMA_TEST_CASE_DIR}/effective-health-path"
      fi
      cma_test_state_matches "$2"
      ;;
    --check-adopted-from-snapshot)
      [[ "$#" -eq 2 && -d "$2" ]] || return 1
      printf '%s\n' adopted >> "${CMA_TEST_CASE_DIR}/adopted-checks.log"
      check_count="$(wc -l < "${CMA_TEST_CASE_DIR}/adopted-checks.log" | tr -d ' ')"
      if [[ "${CMA_TEST_CASE}" == final_check_failure && \
            "${check_count}" -ge 2 ]]; then
        return 1
      fi
      cma_test_state_matches "${CMA_GATEWAY_DESIRED_DIR}"
      ;;
    --check-ready-transition)
      [[ "$#" -eq 2 ]] || return 1
      cma_test_ready_transition_matches "$2"
      ;;
    --check-rollback-routes)
      [[ "$#" -eq 2 ]] || return 1
      cma_test_rollback_routes_match "$2"
      ;;
    *) return 1 ;;
  esac
}

cma_gateway_probe_old_images() {
  local expected_status="$1"

  printf '%s\n' "${expected_status}" >> "${CMA_TEST_CASE_DIR}/probe.log"
  if [[ "${CMA_TEST_CASE}" == probe_mktemp_failure ]]; then
    : > "${CMA_TEST_CASE_DIR}/fail-next-mktemp.armed"
    cma_test_real_probe_old_images "${expected_status}"
    return
  fi
  if [[ "${CMA_TEST_CASE}" == uid_conflict && \
        "${expected_status}" == 200 && \
        ! -e "${CMA_TEST_CASE_DIR}/uid-conflict.injected" ]]; then
    jq '.metadata.uid = "uid-cma-web-route-recreated"' \
      "${CMA_TEST_CASE_DIR}/cluster/cma-web-route.json" \
      > "${CMA_TEST_CASE_DIR}/cluster/cma-web-route.json.new"
    mv "${CMA_TEST_CASE_DIR}/cluster/cma-web-route.json.new" \
      "${CMA_TEST_CASE_DIR}/cluster/cma-web-route.json"
    chmod 600 "${CMA_TEST_CASE_DIR}/cluster/cma-web-route.json"
    : > "${CMA_TEST_CASE_DIR}/uid-conflict.injected"
    return 0
  fi
  if [[ "${CMA_TEST_CASE}" == post_probe_failure && \
        "${expected_status}" == 404 ]]; then
    return 1
  fi
  case "${expected_status}" in
    200) cma_test_rollback_routes_match "${CMA_GATEWAY_ROLLBACK_DIR}" ;;
    404) cma_test_state_matches "${CMA_GATEWAY_DESIRED_DIR}" ;;
    *) return 1 ;;
  esac
}

cma_gateway_replace_cas() {
  local target_file="$2"
  local object_name="${target_file##*/}"
  local failed_object=""

  object_name="${object_name%.json}"
  if [[ "${target_file}" == "${CMA_GATEWAY_DESIRED_DIR}/"* ]]; then
    printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/forward-attempts.log"
    if [[ "${CMA_TEST_CASE}" == forward_cma_api_health_ambiguous_applied && \
          "${object_name}" == cma-api-health ]]; then
      cma_test_real_replace_cas "$@" || return 1
      printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/forward-failures.log"
      : > "${CMA_TEST_CASE_DIR}/ambiguous-health-write.injected"
      return 1
    fi
    if [[ ("${CMA_TEST_CASE}" == signal_term_forward_after_apply || \
           "${CMA_TEST_CASE}" == signal_quit_forward_after_apply) && \
          "${object_name}" == cma-api-health ]]; then
      cma_test_real_replace_cas "$@" || return 1
      : > "${CMA_TEST_CASE_DIR}/forward-signal.injected"
      if [[ "${CMA_TEST_CASE}" == signal_quit_forward_after_apply ]]; then
        kill -s QUIT "$$"
      else
        kill -s TERM "$$"
      fi
      return 1
    fi
    case "${CMA_TEST_CASE}" in
      forward_cma_api_health) failed_object=cma-api-health ;;
      forward_cma_api_route | conditional_postwrite_failure)
        failed_object=cma-api-route
        ;;
      forward_cma_web_route) failed_object=cma-web-route ;;
      forward_cma_http_redirect_route) failed_object=cma-http-redirect-route ;;
      rollback_one_failure | rollback_reported_failure_all_attempted | \
        restore_mktemp_failure | signal_int_rollback | signal_term_rollback | \
        signal_quit_rollback)
        failed_object=cma-http-redirect-route
        ;;
    esac
    if [[ "${object_name}" == "${failed_object}" ]]; then
      printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/forward-failures.log"
      if [[ "${CMA_TEST_CASE}" == restore_mktemp_failure ]]; then
        : > "${CMA_TEST_CASE_DIR}/fail-next-mktemp.armed"
      fi
      return 1
    fi
  fi
  cma_test_real_replace_cas "$@"
}

cma_gateway_restore_one() {
  local object_name="$1"

  printf '%s\n' "${object_name}" >> "${CMA_TEST_CASE_DIR}/rollback-attempts.log"
  if [[ "${CMA_TEST_CASE}" == rollback_one_failure && \
        "${object_name}" == cma-web-route && \
        ! -e "${CMA_TEST_CASE_DIR}/rollback-failure.injected" ]]; then
    : > "${CMA_TEST_CASE_DIR}/rollback-failure.injected"
    return 1
  fi
  if [[ "${CMA_TEST_CASE}" == rollback_reported_failure_all_attempted && \
        "${object_name}" == cma-api-route && \
        ! -e "${CMA_TEST_CASE_DIR}/rollback-failure.injected" ]]; then
    : > "${CMA_TEST_CASE_DIR}/rollback-failure.injected"
    return 1
  fi
  if [[ "${CMA_TEST_CASE}" == signal_int_rollback && \
        ! -e "${CMA_TEST_CASE_DIR}/signal.injected" ]]; then
    : > "${CMA_TEST_CASE_DIR}/signal.injected"
    kill -s INT "$$"
    : > "${CMA_TEST_CASE_DIR}/signal.survived"
  elif [[ "${CMA_TEST_CASE}" == signal_term_rollback && \
          ! -e "${CMA_TEST_CASE_DIR}/signal.injected" ]]; then
    : > "${CMA_TEST_CASE_DIR}/signal.injected"
    kill -s TERM "$$"
    : > "${CMA_TEST_CASE_DIR}/signal.survived"
  elif [[ "${CMA_TEST_CASE}" == signal_quit_rollback && \
          ! -e "${CMA_TEST_CASE_DIR}/signal.injected" ]]; then
    : > "${CMA_TEST_CASE_DIR}/signal.injected"
    kill -s QUIT "$$"
    : > "${CMA_TEST_CASE_DIR}/signal.survived"
  elif [[ "${CMA_TEST_CASE}" == manual_restore_quit && \
          ! -e "${CMA_TEST_CASE_DIR}/manual-quit.injected" ]]; then
    : > "${CMA_TEST_CASE_DIR}/manual-quit.injected"
    kill -s QUIT "$$"
    : > "${CMA_TEST_CASE_DIR}/manual-quit.survived"
  fi
  cma_test_real_restore_one "$@"
}

cma_gateway_wait_for_state() {
  if [[ "${CMA_TEST_CASE}" == adopted_convergence_failure && \
        "$1" == adopted ]]; then
    return 1
  fi
  case "$1" in
    adopted) cma_gateway_check --check-adopted-from-snapshot "$2" >/dev/null ;;
    snapshot) cma_gateway_check --check-snapshot "$2" >/dev/null ;;
    ready-transition) cma_gateway_check --check-ready-transition "$2" >/dev/null ;;
    rollback-routes) cma_gateway_check --check-rollback-routes "$2" >/dev/null ;;
    *) return 1 ;;
  esac
}

cma_test_run_direct_seal_case() {
  local case_name="$1"
  local snapshot_dir="${CMA_TEST_CASE_DIR}/snapshot"
  local symlink_target

  case "${case_name}" in
    seal_baseline)
      cma_test_make_snapshot "${snapshot_dir}"
      cma_gateway_validate_snapshot_seal \
        "${snapshot_dir}" "${CMA_TEST_SNAPSHOT_DIGEST}"
      ;;
    tampered_snapshot)
      cma_test_make_snapshot "${snapshot_dir}"
      chmod 600 "${snapshot_dir}/cma-web-route.json"
      jq '.spec.hostnames = ["tampered.invalid"]' \
        "${snapshot_dir}/cma-web-route.json" \
        > "${CMA_TEST_CASE_DIR}/tampered.json"
      mv "${CMA_TEST_CASE_DIR}/tampered.json" \
        "${snapshot_dir}/cma-web-route.json"
      chmod 400 "${snapshot_dir}/cma-web-route.json"
      if cma_gateway_validate_snapshot_seal \
          "${snapshot_dir}" "${CMA_TEST_SNAPSHOT_DIGEST}"; then
        cma_test_fail "tampered snapshot was accepted"
      fi
      ;;
    extra_snapshot_entry)
      cma_test_make_snapshot "${snapshot_dir}"
      printf '{}\n' > "${snapshot_dir}/unexpected.json"
      chmod 400 "${snapshot_dir}/unexpected.json"
      if cma_gateway_validate_snapshot_seal \
          "${snapshot_dir}" "${CMA_TEST_SNAPSHOT_DIGEST}"; then
        cma_test_fail "snapshot with an extra entry was accepted"
      fi
      ;;
    symlinked_snapshot_file)
      cma_test_make_snapshot "${snapshot_dir}"
      symlink_target="${CMA_TEST_CASE_DIR}/symlink-target.json"
      mv "${snapshot_dir}/cma-api-route.json" "${symlink_target}"
      ln -s "${symlink_target}" "${snapshot_dir}/cma-api-route.json"
      if cma_gateway_validate_snapshot_seal \
          "${snapshot_dir}" "${CMA_TEST_SNAPSHOT_DIGEST}"; then
        cma_test_fail "snapshot containing a symlinked object was accepted"
      fi
      ;;
    symlinked_snapshot_directory)
      cma_test_make_snapshot "${CMA_TEST_CASE_DIR}/real-snapshot"
      ln -s "${CMA_TEST_CASE_DIR}/real-snapshot" "${snapshot_dir}"
      if cma_gateway_validate_snapshot_seal \
          "${snapshot_dir}" "${CMA_TEST_SNAPSHOT_DIGEST}"; then
        cma_test_fail "symlinked snapshot directory was accepted"
      fi
      ;;
    *) return 1 ;;
  esac
}

cma_test_install_hash_failure() {
  CMA_TEST_HASH_FAILURE_SUFFIX="$1"

  cma_gateway_sha256_file() {
    if [[ "$1" == *"${CMA_TEST_HASH_FAILURE_SUFFIX}" ]]; then
      : >"${CMA_TEST_CASE_DIR}/hash-failure.injected"
      printf '0%.0s' {1..64}
      printf '\n'
      return 1
    fi
    cma_test_real_sha256_file "$@"
  }
}

cma_test_assert_hash_failure_seal() {
  local snapshot_dir="${CMA_TEST_CASE_DIR}/hash-failure-snapshot"
  local observed_digest=""

  cma_test_make_snapshot "${snapshot_dir}"
  cma_test_install_hash_failure cma-web-route.json
  if observed_digest="$(cma_gateway_named_files_digest "${snapshot_dir}")"; then
    cma_test_fail "a per-file hash failure produced an accepted bundle digest"
  fi
  [[ -z "${observed_digest}" ]] || \
    cma_test_fail "a failed bundle digest leaked a plausible digest"
  if cma_gateway_validate_snapshot_seal \
      "${snapshot_dir}" "${CMA_TEST_SNAPSHOT_DIGEST}"; then
    cma_test_fail "snapshot validation ignored a per-file hash failure"
  fi
  [[ -e "${CMA_TEST_CASE_DIR}/hash-failure.injected" ]] || \
    cma_test_fail "the per-file hash failure was not injected"
}

cma_test_install_snapshot_output_failure() {
  CMA_TEST_PRINTF_FAILURE_TARGET="$1"
  export CMA_TEST_PRINTF_FAILURE_TARGET

  printf() {
    if [[ "${CMA_TEST_PRINTF_FAILURE_TARGET}" == adoption && \
          "${1:-}" == '%s\n' && \
          "${2:-}" == 'CMA Gateway route and /ready health check are adopted and effective;'* ]]; then
      : >"${CMA_TEST_CASE_DIR}/success-output-failure.injected"
      return 1
    fi
    if [[ "${CMA_TEST_PRINTF_FAILURE_TARGET}" == manual && \
          "${1:-}" == '%s\n' && \
          "${2:-}" == 'CMA Gateway rollback is exact and effective' ]]; then
      : >"${CMA_TEST_CASE_DIR}/success-output-failure.injected"
      return 1
    fi
    case "${CMA_TEST_PRINTF_FAILURE_TARGET}:${1:-}" in
      path:'Gateway rollback material: %s\n' | \
        digest:'Gateway rollback SHA-256: %s\n' | \
        acknowledgement:'Gateway rollback custody acknowledged: %s\n')
        : >"${CMA_TEST_CASE_DIR}/snapshot-output-failure.injected"
      return 1
        ;;
    esac
    # shellcheck disable=SC2059 # Deliberately proxy the caller's printf format.
    builtin printf "$@"
  }
}

cma_test_assert_signal_disposition_order() {
  ruby - "${CMA_TEST_ADOPTION_SCRIPT}" <<'RUBY'
source = File.read(ARGV.fetch(0))

def function_body(source, name)
  match = source.match(/^#{Regexp.escape(name)}\(\) \{\n(.*?)^\}\n/m)
  raise "missing production function: #{name}" unless match

  match[1].gsub(/\\\n\s*/, " ").gsub(/\s+/, " ")
end

def assert_order(body, function_name, markers)
  cursor = -1
  markers.each do |marker|
    position = body.index(marker, cursor + 1)
    raise "#{function_name}: missing or misordered #{marker.inspect}" unless position

    cursor = position
  end
end

on_exit = function_body(source, "cma_gateway_on_exit")
assert_order(
  on_exit,
  "cma_gateway_on_exit",
  [
    "trap '' HUP INT QUIT TERM",
    'local original_status="$1"',
    "local rollback_status=0",
    "trap - EXIT",
    "set +e",
    "cma_gateway_restore",
    "trap - HUP INT QUIT TERM",
    'exit "${original_status}"',
  ],
)

unless source.include?('trap \'cma_gateway_on_exit "$?"\' EXIT')
  raise "cma_gateway_adopt: EXIT trap does not pass the captured status"
end

adopt = function_body(source, "cma_gateway_adopt")
assert_order(
  adopt,
  "cma_gateway_adopt",
  [
    "trap '' HUP INT QUIT TERM",
    "CMA Gateway route and /ready health check are adopted and effective",
    "CMA_GATEWAY_MUTATED=0",
    "trap - EXIT",
    'rm -rf "${CMA_GATEWAY_WORK_DIR}" "${CMA_GATEWAY_DESIRED_DIR}"',
    'CMA_GATEWAY_WORK_DIR=""',
    'CMA_GATEWAY_DESIRED_DIR=""',
    "trap - HUP INT QUIT TERM",
  ],
)

manual_restore = function_body(source, "cma_gateway_manual_restore")
mask_position = manual_restore.index("trap '' HUP INT QUIT TERM")
cleanup_position = manual_restore.rindex('rm -rf "${CMA_GATEWAY_DESIRED_DIR}"')
empty_position = manual_restore.rindex('CMA_GATEWAY_DESIRED_DIR=""')
output_position = manual_restore.index("CMA Gateway rollback is exact and effective")
unmask_position = manual_restore.rindex("trap - HUP INT QUIT TERM")
positions = [mask_position, cleanup_position, empty_position, output_position, unmask_position]
unless positions.none?(&:nil?) && positions.each_cons(2).all? { |left, right| left < right }
  raise "cma_gateway_manual_restore: success disposition order is not mask, cleanup, output, unmask"
end
RUBY
}

cma_test_write_success_state() {
  local trap_state seal_valid=0

  trap_state="$(trap -p EXIT HUP INT QUIT TERM)"
  if cma_gateway_validate_snapshot_seal \
      "${CMA_GATEWAY_ROLLBACK_DIR}" "${CMA_GATEWAY_ROLLBACK_SHA256}"; then
    seal_valid=1
  fi
  {
    printf 'mutated=%s\n' "${CMA_GATEWAY_MUTATED}"
    printf 'trap_state=%s\n' "${trap_state}"
    printf 'work_dir=%s\n' "${CMA_GATEWAY_WORK_DIR}"
    printf 'rollback_dir=%s\n' "${CMA_GATEWAY_ROLLBACK_DIR}"
    printf 'rollback_sealed=%s\n' "${CMA_GATEWAY_ROLLBACK_SEALED}"
    printf 'rollback_sha256=%s\n' "${CMA_GATEWAY_ROLLBACK_SHA256}"
    printf 'seal_valid=%s\n' "${seal_valid}"
    printf 'desired_dir=%s\n' "${CMA_GATEWAY_DESIRED_DIR}"
  } > "${CMA_TEST_CASE_DIR}/success-state"
}

cma_test_write_manual_restore_state() {
  local trap_state

  trap_state="$(trap -p EXIT HUP INT QUIT TERM)"
  {
    printf 'trap_state=%s\n' "${trap_state}"
    printf 'desired_dir=%s\n' "${CMA_GATEWAY_DESIRED_DIR}"
    printf 'rollback_dir=%s\n' "${CMA_GATEWAY_ROLLBACK_DIR}"
    printf 'rollback_sha256=%s\n' "${CMA_GATEWAY_ROLLBACK_SHA256}"
  } > "${CMA_TEST_CASE_DIR}/manual-restore-state"
}

cma_test_assert_late_wait_modes() {
  local state_mode marker="${CMA_TEST_CASE_DIR}/late-wait.marker"

  cma_gateway_check() {
    : > "${marker}"
    return 0
  }
  cma_gateway_deadline_reached() {
    [[ -e "${marker}" ]]
  }
  for state_mode in adopted snapshot ready-transition rollback-routes; do
    rm -f "${marker}"
    if cma_test_real_wait_for_state \
        "${state_mode}" "${CMA_TEST_CASE_DIR}/late-wait-snapshot"; then
      cma_test_fail "${state_mode} accepted a checker result after its wall-clock deadline"
      return 1
    fi
  done
}

cma_test_assert_privileged_startup_is_closed() {
  local startup_file="${CMA_TEST_CASE_DIR}/bash-env"
  local startup_marker="${CMA_TEST_CASE_DIR}/bash-env-ran"
  local function_marker="${CMA_TEST_CASE_DIR}/imported-function-ran"
  local actual_status

  # shellcheck disable=SC2016 # Write literal code for the hostile startup file.
  printf '%s\n' \
    'printf "%s\n" "${CMA_GATEWAY_STARTUP_SENTINEL:-missing}" > "${CMA_STARTUP_MARKER}"' \
    >"${startup_file}"
  set +e
  # shellcheck disable=SC2016 # Pass a literal exported-function definition.
  env \
    BASH_ENV="${startup_file}" \
    ENV="${startup_file}" \
    CMA_GATEWAY_STARTUP_SENTINEL=privileged-adoption-startup-sentinel \
    CMA_STARTUP_MARKER="${startup_marker}" \
    CMA_FUNCTION_MARKER="${function_marker}" \
    CMA_GATEWAY_CONFIRM=adopt-v0.10-route \
    'BASH_FUNC_umask%%=() { : > "${CMA_FUNCTION_MARKER}"; builtin umask "$@"; }' \
    "${CMA_TEST_ADOPTION_SCRIPT}" --adopt \
      >"${CMA_TEST_CASE_DIR}/startup.stdout" \
      2>"${CMA_TEST_CASE_DIR}/startup.stderr"
  actual_status=$?
  set -e

  if [[ "${actual_status}" -eq 0 || -e "${startup_marker}" || \
        -e "${function_marker}" ]] || \
     grep -R -q 'privileged-adoption-startup-sentinel' \
       "${CMA_TEST_CASE_DIR}" || \
     ! grep -q 'production Gateway mutation is disabled' \
       "${CMA_TEST_CASE_DIR}/startup.stderr"; then
    cma_test_fail "Bash startup input or imported function executed"
  fi
}

cma_test_run_manual_restore_case() {
  local case_name="$1"
  local snapshot_dir="${CMA_TEST_CASE_DIR}/manual-snapshot"
  local cluster_health="${CMA_TEST_CASE_DIR}/cluster/cma-api-health.json"
  local temporary_file="${cluster_health}.policy"

  cma_test_set_cluster_desired
  cma_test_make_snapshot "${snapshot_dir}"
  case "${case_name}" in
    rollback_cross_policy_ready_effective_livez)
      printf '%s\n' /livez > "${CMA_TEST_CASE_DIR}/effective-health-path"
      ;;
    rollback_cross_policy_livez_effective_ready)
      jq --slurpfile target \
        "${CMA_TEST_CASE_DIR}/legacy-fixture/cma-api-health.json" '
          .metadata.labels = ($target[0].metadata.labels // {}) |
          .spec = $target[0].spec
        ' "${cluster_health}" > "${temporary_file}"
      mv "${temporary_file}" "${cluster_health}"
      chmod 600 "${cluster_health}"
      ;;
    rollback_cross_invalid_path | conditional_manual_postwrite_failure)
      printf '%s\n' /startup > "${CMA_TEST_CASE_DIR}/effective-health-path"
      ;;
    manual_restore_success | manual_restore_quit | manual_success_output_failure)
      if [[ "${case_name}" == manual_success_output_failure ]]; then
        cma_test_install_snapshot_output_failure manual
      fi
      ;;
    *) return 1 ;;
  esac

  CMA_GATEWAY_CONFIRM=restore-v0.10-route
  CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
  CMA_GATEWAY_ROLLBACK_SHA256="${CMA_TEST_SNAPSHOT_DIGEST}"
  export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
  export CMA_GATEWAY_ROLLBACK_SHA256
  if [[ "${case_name}" == conditional_manual_postwrite_failure ]]; then
    if cma_gateway_manual_restore "${snapshot_dir}"; then
      : >"${CMA_TEST_CASE_DIR}/conditional-call-returned"
      cma_test_fail "conditional manual restore unexpectedly succeeded"
    else
      : >"${CMA_TEST_CASE_DIR}/conditional-call-returned"
      cma_test_fail "conditional manual restore returned instead of exiting"
    fi
  else
    cma_gateway_manual_restore "${snapshot_dir}"
  fi
  cma_test_write_manual_restore_state
}

cma_test_run_scenario() {
  local case_name="$1"
  local case_dir="$2"
  local snapshot_dir
  local test_check test_kubectl test_mutation_gate test_probe test_replace test_require
  local test_source_contract test_projection test_admission
  local test_capture_projection
  local test_restore_one test_wait

  CMA_TEST_CASE="${case_name}"
  CMA_TEST_CASE_DIR="${case_dir}"
  CMA_TEST_SNAPSHOT_DIGEST=""
  export CMA_TEST_CASE CMA_TEST_CASE_DIR

  cd "${CMA_TEST_REPO_ROOT}"
  test_check="$(declare -f cma_gateway_check)"
  test_kubectl="$(declare -f cma_gateway_kubectl)"
  test_mutation_gate="$(declare -f cma_gateway_require_production_mutation_enabled)"
  test_probe="$(declare -f cma_gateway_probe_old_images)"
  test_replace="$(declare -f cma_gateway_replace_cas)"
  test_require="$(declare -f cma_gateway_require_checkout)"
  test_source_contract="$(declare -f cma_gateway_source_contract)"
  test_projection="$(declare -f cma_gateway_validate_projection_receipt)"
  test_admission="$(declare -f cma_gateway_verify_admission_receipt)"
  test_capture_projection="$(declare -f cma_gateway_capture_projection)"
  test_restore_one="$(declare -f cma_gateway_restore_one)"
  test_wait="$(declare -f cma_gateway_wait_for_state)"
  # shellcheck disable=SC1090
  source "${CMA_TEST_ADOPTION_SCRIPT}"
  eval "$(declare -f cma_gateway_sha256_file |
    sed '1s/^cma_gateway_sha256_file /cma_test_real_sha256_file /')"
  eval "$(declare -f cma_gateway_replace_cas |
    sed '1s/^cma_gateway_replace_cas /cma_test_real_replace_cas /')"
  eval "$(declare -f cma_gateway_probe_old_images |
    sed '1s/^cma_gateway_probe_old_images /cma_test_real_probe_old_images /')"
  eval "$(declare -f cma_gateway_restore_one |
    sed '1s/^cma_gateway_restore_one /cma_test_real_restore_one /')"
  eval "$(declare -f cma_gateway_wait_for_state |
    sed '1s/^cma_gateway_wait_for_state /cma_test_real_wait_for_state /')"
  eval "$(declare -f cma_gateway_source_contract |
    sed '1s/^cma_gateway_source_contract /cma_test_real_source_contract /')"
  eval "${test_check}"
  eval "${test_kubectl}"
  if [[ "${case_name}" != production_gate_adopt && \
        "${case_name}" != production_gate_restore ]]; then
    eval "${test_mutation_gate}"
  fi
  eval "${test_probe}"
  eval "${test_replace}"
  eval "${test_require}"
  eval "${test_source_contract}"
  eval "${test_projection}"
  eval "${test_admission}"
  eval "${test_capture_projection}"
  eval "${test_restore_one}"
  eval "${test_wait}"
  umask 077
  cma_test_setup_fake_cluster
  mkdir -p "${CMA_TEST_CASE_DIR}/adopted-projections"
  CMA_GATEWAY_ADOPTED_PROJECTION_PARENT="${CMA_TEST_CASE_DIR}/adopted-projections"
  CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
  CMA_GATEWAY_PROJECTION_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  export CMA_GATEWAY_ADOPTED_PROJECTION_PARENT CMA_GATEWAY_RELEASE_REVISION
  export CMA_GATEWAY_PROJECTION_SHA256

  case "${case_name}" in
    seal_baseline | tampered_snapshot | extra_snapshot_entry | \
      symlinked_snapshot_file | symlinked_snapshot_directory)
      cma_test_run_direct_seal_case "${case_name}"
      ;;
    hash_failure_seal)
      cma_test_assert_hash_failure_seal
      ;;
    signal_disposition_order)
      cma_test_assert_signal_disposition_order
      ;;
    late_wait_modes)
      cma_test_assert_late_wait_modes
      ;;
    privileged_startup)
      cma_test_assert_privileged_startup_is_closed
      ;;
    exit_handler_zero)
      CMA_GATEWAY_MUTATED=1
      CMA_GATEWAY_ROLLBACK_DIR="${CMA_TEST_CASE_DIR}/handler-snapshot"
      CMA_GATEWAY_ROLLBACK_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
      # shellcheck disable=SC2034 # Consumed dynamically by the sourced exit handler.
      CMA_GATEWAY_PROBE_DIR=""
      CMA_GATEWAY_WORK_DIR=""
      CMA_GATEWAY_DESIRED_DIR=""
      cma_gateway_restore() {
        printf '%s\n' restore >>"${CMA_TEST_CASE_DIR}/handler-zero.log"
        return 0
      }
      cma_gateway_on_exit 0
      ;;
    hash_failure_adopt)
      cma_test_install_hash_failure infra/api/httproute.yaml
      CMA_GATEWAY_CONFIRM=adopt-v0.10-route
      CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host
      CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT="${CMA_TEST_CASE_DIR}/custody"
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT
      cma_gateway_adopt
      ;;
    snapshot_output_failure_path | snapshot_output_failure_digest | \
      snapshot_output_failure_acknowledgement)
      cma_test_install_snapshot_output_failure \
        "${case_name#snapshot_output_failure_}"
      CMA_GATEWAY_CONFIRM=adopt-v0.10-route
      CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host
      CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT="${CMA_TEST_CASE_DIR}/custody"
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT
      cma_gateway_adopt
      ;;
    adoption_success_output_failure)
      cma_test_install_snapshot_output_failure adoption
      CMA_GATEWAY_CONFIRM=adopt-v0.10-route
      CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host
      CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT="${CMA_TEST_CASE_DIR}/custody"
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT
      cma_gateway_adopt
      ;;
    production_gate_adopt)
      CMA_GATEWAY_CONFIRM=adopt-v0.10-route
      export CMA_GATEWAY_CONFIRM
      cma_gateway_adopt
      ;;
    production_gate_restore)
      CMA_GATEWAY_CONFIRM=restore-v0.10-route
      CMA_GATEWAY_ROLLBACK_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_ROLLBACK_SHA256
      cma_gateway_manual_restore "${CMA_TEST_CASE_DIR}/never-read-snapshot"
      ;;
    wrong_external_snapshot_digest)
      snapshot_dir="${CMA_TEST_CASE_DIR}/manual-snapshot"
      cma_test_make_snapshot "${snapshot_dir}"
      CMA_GATEWAY_ROLLBACK_SHA256="$(printf '0%.0s' {1..64})"
      [[ "${CMA_GATEWAY_ROLLBACK_SHA256}" != "${CMA_TEST_SNAPSHOT_DIGEST}" ]]
      CMA_GATEWAY_CONFIRM=restore-v0.10-route
      CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
      export CMA_GATEWAY_ROLLBACK_SHA256
      cma_gateway_manual_restore "${snapshot_dir}"
      ;;
    manual_restore_success | manual_restore_quit | \
      rollback_cross_policy_ready_effective_livez | \
      rollback_cross_policy_livez_effective_ready | rollback_cross_invalid_path | \
      conditional_manual_postwrite_failure | manual_success_output_failure)
      cma_test_run_manual_restore_case "${case_name}"
      ;;
    success)
      CMA_GATEWAY_CONFIRM=adopt-v0.10-route
      CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host
      CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT="${CMA_TEST_CASE_DIR}/custody"
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT
      cma_gateway_adopt
      cma_test_write_success_state
      ;;
    *)
      CMA_GATEWAY_CONFIRM=adopt-v0.10-route
      if [[ "${case_name}" == exclusive_confirmation_failure ]]; then
        CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=""
      else
        CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      fi
      CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM=durable-off-host
      CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT="${CMA_TEST_CASE_DIR}/custody"
      export CMA_GATEWAY_CONFIRM CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_CONFIRM
      export CMA_GATEWAY_ROLLBACK_CUSTODY_ROOT
      if [[ "${case_name}" == conditional_prewrite_failure || \
            "${case_name}" == conditional_postwrite_failure ]]; then
        if cma_gateway_adopt; then
          : >"${CMA_TEST_CASE_DIR}/conditional-call-returned"
          cma_test_fail "conditional adoption unexpectedly succeeded"
        else
          : >"${CMA_TEST_CASE_DIR}/conditional-call-returned"
          cma_test_fail "conditional adoption returned instead of exiting"
        fi
      else
        cma_gateway_adopt
      fi
      ;;
  esac
}

cma_test_assert_failure_case() {
  local case_name="$1"
  local case_dir="$2"
  local scenario_status="$3"
  local mixed_target_dir retained_count retained_dir

  [[ "${scenario_status}" -ne 0 ]] || \
    cma_test_fail "${case_name} unexpectedly succeeded"
  cma_test_assert_empty_file \
    "${case_dir}/raw-boundary.log" "raw external-boundary calls"

  case "${case_name}" in
    hash_failure_adopt)
      [[ -e "${case_dir}/hash-failure.injected" ]] || \
        cma_test_fail "the source hash failure was not injected"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "source-hash-failure forward writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "source-hash-failure rollback attempts"
      ;;
    snapshot_output_failure_path | snapshot_output_failure_digest | \
      snapshot_output_failure_acknowledgement)
      [[ -e "${case_dir}/snapshot-output-failure.injected" ]] || \
        cma_test_fail "the required snapshot output failure was not injected"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "snapshot-output-failure forward writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "snapshot-output-failure rollback attempts"
      retained_count="$(find "${case_dir}/custody" -mindepth 1 -maxdepth 1 \
        -type d | wc -l | tr -d ' ')"
      if [[ "${case_name}" == snapshot_output_failure_acknowledgement ]]; then
        [[ "${retained_count}" == 1 ]] || \
          cma_test_fail "acknowledged sealed custody was not retained"
        retained_dir="$(find "${case_dir}/custody" -mindepth 1 -maxdepth 1 \
          -type d -print -quit)"
        [[ "$(find "${retained_dir}" -mindepth 1 -maxdepth 1 \
          -type f | wc -l | tr -d ' ')" == 4 ]] || \
          cma_test_fail "retained custody is incomplete"
      else
        [[ "${retained_count}" == 0 ]] || \
          cma_test_fail "unacknowledged rollback custody was retained"
      fi
      ;;
    exit_handler_zero)
      cma_test_assert_line_count \
        "${case_dir}/handler-zero.log" 1 "status-zero handler rollback attempts"
      ;;
    preflight_failure | conditional_prewrite_failure)
      [[ ! -e "${case_dir}/conditional-call-returned" ]] || \
        cma_test_fail "conditional pre-write adoption returned past a fatal failure"
      cma_test_assert_empty_file "${case_dir}/writes.log" "preflight writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "preflight rollback attempts"
      cma_test_assert_line_count \
        "${case_dir}/dry-run.log" 2 "preflight dry-run attempts"
      ;;
    dry_run_mutated_success)
      [[ -e "${case_dir}/dry-run-mutated.injected" ]] || \
        cma_test_fail "mutated successful dry-run response was not injected"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "mutated dry-run writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "mutated dry-run rollback attempts"
      ;;
    production_gate_adopt | production_gate_restore | exclusive_confirmation_failure)
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "unattested adoption writes"
      cma_test_assert_empty_file \
        "${case_dir}/dry-run.log" "unattested adoption dry-run attempts"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "unattested adoption rollback attempts"
      ;;
    seal_bundle_failure)
      [[ -e "${case_dir}/seal-bundle-failure.injected" ]] || \
        cma_test_fail "the rollback seal failure was not injected"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "unsealed-custody forward writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "unsealed-custody rollback attempts"
      if find "${case_dir}/custody" -mindepth 1 -maxdepth 1 -print -quit | \
          grep -q .; then
        cma_test_fail "unsealed rollback custody was retained"
      fi
      ;;
    probe_mktemp_failure)
      [[ -e "${case_dir}/mktemp-failure.injected" ]] || \
        cma_test_fail "probe mktemp failure was not injected"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "probe-mktemp failure writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "probe-mktemp rollback attempts"
      cma_test_assert_temp_paths_scoped "${case_dir}"
      ;;
    uid_conflict)
      [[ -e "${case_dir}/uid-conflict.injected" ]] || \
        cma_test_fail "UID conflict was not injected"
      cma_test_assert_empty_file "${case_dir}/writes.log" "UID-conflict writes"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "UID-conflict rollback attempts"
      ;;
    resource_version_conflict)
      [[ -e "${case_dir}/resource-version-conflict.injected" ]] || \
        cma_test_fail "resourceVersion conflict was not injected"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "resourceVersion-conflict successful writes"
      cma_test_assert_route_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    post_write_uid_recreation)
      [[ -e "${case_dir}/post-write-uid-recreation.injected" ]] || \
        cma_test_fail "post-write UID recreation was not injected"
      cma_test_assert_line_count \
        "${case_dir}/writes.log" 1 "writes before UID recreation"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "unsafe UID-recreation rollback attempts"
      ;;
    wrong_external_snapshot_digest)
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "wrong-digest restore writes"
      ;;
    rollback_cross_invalid_path | conditional_manual_postwrite_failure)
      [[ ! -e "${case_dir}/conditional-call-returned" ]] || \
        cma_test_fail "conditional manual restore returned past a fatal failure"
      cma_test_assert_route_rollback_order "${case_dir}/rollback-attempts.log"
      cma_test_assert_line_count \
        "${case_dir}/writes.log" 1 "writes before invalid rollback barrier"
      [[ "$(command cat "${case_dir}/effective-health-path")" == /startup ]] || \
        cma_test_fail "invalid effective health path was not retained fail-closed"
      ;;
    rollback_one_failure)
      [[ -e "${case_dir}/rollback-failure.injected" ]] || \
        cma_test_fail "rollback failure was not injected"
      cma_test_assert_route_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    rollback_reported_failure_all_attempted)
      [[ -e "${case_dir}/rollback-failure.injected" ]] || \
        cma_test_fail "reported rollback failure was not injected"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    restore_mktemp_failure)
      [[ -e "${case_dir}/mktemp-failure.injected" ]] || \
        cma_test_fail "restore mktemp failure was not injected"
      cma_test_assert_line_count \
        "${case_dir}/forward-failures.log" 1 "restore-mktemp forward failures"
      cma_test_assert_empty_file \
        "${case_dir}/rollback-attempts.log" "restore attempts after mktemp failure"
      cma_test_assert_temp_paths_scoped "${case_dir}"
      ;;
    signal_int_rollback | signal_term_rollback | signal_quit_rollback)
      [[ -e "${case_dir}/signal.injected" && \
         -e "${case_dir}/signal.survived" ]] || \
        cma_test_fail "rollback did not survive ${case_name}"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    forward_cma_api_health)
      cma_test_assert_line_count \
        "${case_dir}/forward-failures.log" 1 "injected forward CAS failures"
      cma_test_assert_empty_file \
        "${case_dir}/writes.log" "failed health CAS writes"
      cma_test_assert_route_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    forward_cma_api_health_ambiguous_applied)
      [[ -e "${case_dir}/ambiguous-health-write.injected" ]] || \
        cma_test_fail "ambiguous applied health failure was not injected"
      cma_test_assert_line_count \
        "${case_dir}/forward-failures.log" 1 "ambiguous forward CAS failures"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    replace_mutated_success)
      [[ -e "${case_dir}/replace-mutated.injected" ]] || \
        cma_test_fail "mutated successful replace response was not injected"
      cma_test_assert_line_count \
        "${case_dir}/writes.log" 2 "mutated replace and rollback writes"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    forward_cma_api_route | conditional_postwrite_failure | forward_cma_web_route | \
      forward_cma_http_redirect_route)
      [[ "${case_name}" != conditional_postwrite_failure || \
         ! -e "${case_dir}/conditional-call-returned" ]] || \
        cma_test_fail "conditional post-write adoption returned past a fatal failure"
      cma_test_assert_line_count \
        "${case_dir}/forward-failures.log" 1 "injected forward CAS failures"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    signal_term_forward_after_apply | signal_quit_forward_after_apply)
      [[ -e "${case_dir}/forward-signal.injected" ]] || \
        cma_test_fail "the signal was not injected during the forward health CAS"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    adopted_convergence_failure | post_probe_failure | final_check_failure)
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    adoption_success_output_failure)
      [[ -e "${case_dir}/success-output-failure.injected" ]] || \
        cma_test_fail "the adoption success-output failure was not injected"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    manual_success_output_failure)
      [[ -e "${case_dir}/success-output-failure.injected" ]] || \
        cma_test_fail "the manual-restore success-output failure was not injected"
      cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      ;;
    *) cma_test_fail "unrecognized expected-failure case: ${case_name}" ;;
  esac

  case "${case_name}" in
    uid_conflict | post_write_uid_recreation)
      # The UID mutation represents a recreated object, so only the zero-write
      # fence is asserted; its identity must intentionally differ afterward.
      ;;
    rollback_one_failure)
      mixed_target_dir="${case_dir}/safe-partial-target"
      mkdir "${mixed_target_dir}"
      cp "${case_dir}/legacy-fixture/cma-api-route.json" \
        "${mixed_target_dir}/cma-api-route.json"
      cp "${case_dir}/desired-fixture/cma-web-route.json" \
        "${mixed_target_dir}/cma-web-route.json"
      cp "${case_dir}/legacy-fixture/cma-http-redirect-route.json" \
        "${mixed_target_dir}/cma-http-redirect-route.json"
      cp "${case_dir}/desired-fixture/cma-api-health.json" \
        "${mixed_target_dir}/cma-api-health.json"
      cma_test_assert_cluster_matches \
        "${case_dir}/cluster" "${mixed_target_dir}" \
        "${case_dir}/assert-safe-partial-rollback"
      ;;
    restore_mktemp_failure)
      cma_test_assert_cluster_matches \
        "${case_dir}/cluster" "${case_dir}/desired-fixture" \
        "${case_dir}/assert-restore-mktemp-failure"
      ;;
    rollback_cross_invalid_path | conditional_manual_postwrite_failure)
      mixed_target_dir="${case_dir}/invalid-cross-target"
      mkdir "${mixed_target_dir}"
      cp "${case_dir}/legacy-fixture/cma-api-route.json" \
        "${mixed_target_dir}/cma-api-route.json"
      cp "${case_dir}/legacy-fixture/cma-web-route.json" \
        "${mixed_target_dir}/cma-web-route.json"
      cp "${case_dir}/legacy-fixture/cma-http-redirect-route.json" \
        "${mixed_target_dir}/cma-http-redirect-route.json"
      cp "${case_dir}/desired-fixture/cma-api-health.json" \
        "${mixed_target_dir}/cma-api-health.json"
      cma_test_assert_cluster_matches \
        "${case_dir}/cluster" "${mixed_target_dir}" \
        "${case_dir}/assert-invalid-cross-product"
      ;;
    wrong_external_snapshot_digest)
      cma_test_assert_cluster_matches \
        "${case_dir}/cluster" "${case_dir}/legacy-fixture" \
        "${case_dir}/assert-wrong-digest"
      ;;
    *)
      cma_test_assert_cluster_matches \
        "${case_dir}/cluster" "${case_dir}/legacy-fixture" \
        "${case_dir}/assert-rollback"
      ;;
  esac
}

cma_test_assert_success_case() {
  local case_name="$1"
  local case_dir="$2"
  local scenario_status="$3"
  local created_dir retained_rollback_dir

  [[ "${scenario_status}" -eq 0 ]] || \
    cma_test_fail "${case_name} failed with status ${scenario_status}"
  cma_test_assert_empty_file \
    "${case_dir}/raw-boundary.log" "raw external-boundary calls"

  case "${case_name}" in
    manual_restore_success | manual_restore_quit | \
      rollback_cross_policy_ready_effective_livez | \
      rollback_cross_policy_livez_effective_ready)
      cma_test_assert_cluster_matches \
        "${case_dir}/cluster" "${case_dir}/legacy-fixture" \
        "${case_dir}/assert-manual-restore"
      [[ "$(command cat "${case_dir}/effective-health-path")" == /livez ]] || \
        cma_test_fail "manual restore did not converge effective health to /livez"
      grep -Fx 'trap_state=' "${case_dir}/manual-restore-state" >/dev/null || \
        cma_test_fail "manual restore left signal traps armed"
      grep -Fx 'desired_dir=' "${case_dir}/manual-restore-state" >/dev/null || \
        cma_test_fail "manual restore retained its desired directory variable"
      grep -E '^rollback_sha256=[0-9a-f]{64}$' \
        "${case_dir}/manual-restore-state" >/dev/null || \
        cma_test_fail "manual restore lost its external snapshot digest"
      if [[ "${case_name}" == rollback_cross_policy_livez_effective_ready ]]; then
        cma_test_assert_route_rollback_order "${case_dir}/rollback-attempts.log"
      else
        cma_test_assert_rollback_order "${case_dir}/rollback-attempts.log"
      fi
      cma_test_assert_line_count \
        "${case_dir}/dry-run.log" 4 "manual-restore dry-run attempts"
      if [[ "${case_name}" == manual_restore_quit ]]; then
        [[ -e "${case_dir}/manual-quit.injected" && \
           -e "${case_dir}/manual-quit.survived" ]] || \
          cma_test_fail "manual restore did not remain masked across QUIT"
      fi
      return 0
      ;;
  esac

  if [[ "${case_name}" != success ]]; then
    return 0
  fi
  cma_test_assert_cluster_matches \
    "${case_dir}/cluster" "${case_dir}/desired-fixture" \
    "${case_dir}/assert-success"
  cma_test_assert_empty_file \
    "${case_dir}/rollback-attempts.log" "successful-adoption rollback attempts"
  cma_test_assert_line_count \
    "${case_dir}/dry-run.log" 8 "successful-adoption dry-run attempts"
  grep -Fx 'mutated=0' "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption left the mutation flag armed"
  grep -Fx 'trap_state=' "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption left a rollback or signal trap armed"
  grep -Fx 'work_dir=' "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption retained its work directory variable"
  retained_rollback_dir="$(sed -n 's/^rollback_dir=//p' \
    "${case_dir}/success-state")"
  [[ -n "${retained_rollback_dir}" && -d "${retained_rollback_dir}" && \
     ! -L "${retained_rollback_dir}" ]] || \
    cma_test_fail "successful adoption did not retain sealed rollback custody"
  grep -Fx 'rollback_sealed=1' "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption did not mark rollback custody sealed"
  grep -Fx 'seal_valid=1' "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption retained an invalid rollback seal"
  grep -E '^rollback_sha256=[0-9a-f]{64}$' \
    "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption did not retain a valid rollback digest"
  grep -Fx 'desired_dir=' "${case_dir}/success-state" >/dev/null || \
    cma_test_fail "successful adoption retained its desired directory variable"
  while IFS= read -r created_dir; do
    [[ -n "${created_dir}" ]] || continue
    if [[ "${created_dir}" == "${retained_rollback_dir}" ]]; then
      [[ -d "${created_dir}" ]] || \
        cma_test_fail "successful adoption removed sealed custody ${created_dir}"
    else
      [[ ! -e "${created_dir}" ]] || \
        cma_test_fail "successful adoption did not remove ${created_dir}"
    fi
  done < "${case_dir}/tmpdirs.log"
}

cma_test_run_internal() {
  local case_name="$1"
  local case_dir="$2"
  local scenario_status

  mkdir "${case_dir}/runtime"
  chmod 700 "${case_dir}/runtime"
  set +e
  TMPDIR="${case_dir}/runtime" \
    CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM='' \
    CMA_GATEWAY_ROLLBACK_SHA256='' \
    /bin/bash --noprofile --norc -p \
      "${CMA_TEST_SCRIPT_PATH}" --scenario "${case_name}" "${case_dir}" \
      > "${case_dir}/stdout.log" 2> "${case_dir}/stderr.log"
  scenario_status=$?
  set -e
  printf '%s\n' "${scenario_status}" > "${case_dir}/status"

  case "${case_name}" in
    seal_baseline | tampered_snapshot | extra_snapshot_entry | \
      symlinked_snapshot_file | symlinked_snapshot_directory | \
      hash_failure_seal | success | \
      signal_disposition_order | late_wait_modes | privileged_startup | \
      manual_restore_success | manual_restore_quit | \
      rollback_cross_policy_ready_effective_livez | \
      rollback_cross_policy_livez_effective_ready)
      cma_test_assert_success_case "${case_name}" "${case_dir}" "${scenario_status}"
      ;;
    *)
      cma_test_assert_failure_case "${case_name}" "${case_dir}" "${scenario_status}"
      ;;
  esac
}

cma_test_cleanup() {
  if [[ -n "${CMA_TEST_SUITE_DIR}" && \
        -n "${CMA_TEST_SUITE_PARENT}" && \
        -d "${CMA_TEST_SUITE_DIR}" && \
        "${CMA_TEST_SUITE_DIR}" == \
          "${CMA_TEST_SUITE_PARENT}/cma-trigger-gateway-adoption-tests."* ]]; then
    rm -rf -- "${CMA_TEST_SUITE_DIR}"
  fi
}

cma_test_run_suite() {
  local case_name case_dir
  local passed_count=0 failed_count=0
  local test_cases='
    seal_baseline
    hash_failure_seal
    signal_disposition_order
    late_wait_modes
    privileged_startup
    exit_handler_zero
    hash_failure_adopt
    conditional_prewrite_failure
    conditional_postwrite_failure
    conditional_manual_postwrite_failure
    snapshot_output_failure_path
    snapshot_output_failure_digest
    snapshot_output_failure_acknowledgement
    adoption_success_output_failure
    manual_success_output_failure
    production_gate_adopt
    production_gate_restore
    exclusive_confirmation_failure
    seal_bundle_failure
    probe_mktemp_failure
    preflight_failure
    dry_run_mutated_success
    forward_cma_api_health
    forward_cma_api_health_ambiguous_applied
    forward_cma_api_route
    forward_cma_web_route
    forward_cma_http_redirect_route
    adopted_convergence_failure
    post_probe_failure
    final_check_failure
    rollback_one_failure
    rollback_reported_failure_all_attempted
    restore_mktemp_failure
    uid_conflict
    resource_version_conflict
    post_write_uid_recreation
    replace_mutated_success
    tampered_snapshot
    extra_snapshot_entry
    symlinked_snapshot_file
    symlinked_snapshot_directory
    wrong_external_snapshot_digest
    manual_restore_success
    manual_restore_quit
    rollback_cross_policy_ready_effective_livez
    rollback_cross_policy_livez_effective_ready
    rollback_cross_invalid_path
    signal_int_rollback
    signal_term_rollback
    signal_quit_rollback
    signal_term_forward_after_apply
    signal_quit_forward_after_apply
    success
  '

  CMA_TEST_SUITE_PARENT="${TMPDIR:-/tmp}"
  CMA_TEST_SUITE_PARENT="${CMA_TEST_SUITE_PARENT%/}"
  CMA_TEST_SUITE_DIR="$(mktemp -d \
    "${CMA_TEST_SUITE_PARENT}/cma-trigger-gateway-adoption-tests.XXXXXX")"
  chmod 700 "${CMA_TEST_SUITE_DIR}"
  trap cma_test_cleanup EXIT HUP INT QUIT TERM

  for case_name in ${test_cases}; do
    case_dir="${CMA_TEST_SUITE_DIR}/${case_name}"
    mkdir "${case_dir}"
    chmod 700 "${case_dir}"
    if bash "${CMA_TEST_SCRIPT_PATH}" --internal "${case_name}" "${case_dir}"; then
      printf 'ok - %s\n' "${case_name}"
      passed_count=$((passed_count + 1))
    else
      printf 'not ok - %s\n' "${case_name}" >&2
      if [[ -f "${case_dir}/stdout.log" ]]; then
        sed 's/^/  stdout: /' "${case_dir}/stdout.log" >&2
      fi
      if [[ -f "${case_dir}/stderr.log" ]]; then
        sed 's/^/  stderr: /' "${case_dir}/stderr.log" >&2
      fi
      failed_count=$((failed_count + 1))
    fi
  done

  if [[ "${failed_count}" -ne 0 ]]; then
    cma_test_fail "${failed_count} of $((passed_count + failed_count)) cases failed"
  fi
  printf 'CMA Trigger Gateway adoption behavioral tests passed (%s cases)\n' \
    "${passed_count}"
}

case "${1:-}" in
  '')
    [[ "$#" -eq 0 ]] || exit 2
    cma_test_run_suite
    ;;
  --internal)
    [[ "$#" -eq 3 ]] || exit 2
    cma_test_run_internal "$2" "$3"
    ;;
  --scenario)
    [[ "$#" -eq 3 ]] || exit 2
    cma_test_run_scenario "$2" "$3"
    ;;
  *)
    printf 'usage: %s\n' "$(basename "$0")" >&2
    exit 2
    ;;
esac
