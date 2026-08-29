#!/usr/bin/env bash
# Mocked production boundaries are intentionally defined for each isolated case.
# shellcheck disable=SC2030,SC2031,SC2317,SC2329

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=.github/scripts/cma-trigger-gateway-admission-bootstrap.sh
source "${SCRIPT_DIR}/cma-trigger-gateway-admission-bootstrap.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

cma_admission_test_policy() {
  local live="$1"
  jq -n --argjson live "${live}" '{
    apiVersion:"admissionregistration.k8s.io/v1",kind:"ValidatingAdmissionPolicy",
    metadata:({name:"cma-cutover-fence-label-reservation",
      labels:{"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"}} +
      (if $live then {uid:"policy-uid",resourceVersion:"11",generation:1} else {} end)),
    spec:{
      failurePolicy:"Fail",
      matchConstraints:{matchPolicy:"Equivalent",
        namespaceSelector:{matchLabels:{"kubernetes.io/metadata.name":"chrono-bot"}},
        resourceRules:[{apiGroups:[""],apiVersions:["v1"],operations:["CREATE","UPDATE"],
          resources:["pods"],scope:"Namespaced"}]},
      matchConditions:[{name:"exact-chrono-bot-namespace",expression:"request.namespace == \u0027chrono-bot\u0027"}],
      validations:[{
        expression:"!has(object.metadata.labels) || !(\u0027release.chrono-ai.fun/cutover-fence\u0027 in object.metadata.labels) || object.metadata.labels[\u0027release.chrono-ai.fun/cutover-fence\u0027] != \u0027mongodb-715\u0027",
        message:"Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715",
        reason:"Forbidden"}]}
  } + (if $live then {status:{observedGeneration:1,typeChecking:{expressionWarnings:[]}}} else {} end)'
}

cma_admission_test_binding() {
  local live="$1"
  jq -n --argjson live "${live}" '{
    apiVersion:"admissionregistration.k8s.io/v1",kind:"ValidatingAdmissionPolicyBinding",
    metadata:({name:"cma-cutover-fence-label-reservation",
      labels:{"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"}} +
      (if $live then {uid:"binding-uid",resourceVersion:"12",generation:1} else {} end)),
    spec:{policyName:"cma-cutover-fence-label-reservation",validationActions:["Deny"],
      matchResources:{matchPolicy:"Equivalent",
        namespaceSelector:{matchLabels:{"kubernetes.io/metadata.name":"chrono-bot"}},
        resourceRules:[{apiGroups:[""],apiVersions:["v1"],operations:["CREATE","UPDATE"],
          resources:["pods"],scope:"Namespaced"}]}}
  }'
}

cma_admission_test_configure() {
  local case_name="$1"
  local case_dir="$2"
  local initial_state="$3"

  printf '%s\n' "${initial_state}" > "${case_dir}/state"
  : > "${case_dir}/writes"
  cma_admission_require_production_mutation_enabled() { return 0; }
  cma_admission_require_checkout() { CMA_GATEWAY_RELEASE_TAG=v1.2.3; }
  cma_admission_source_digest() {
    printf '%s\n' aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
  }
  cma_admission_prepare_desired() {
    local work_dir="$1"
    cma_admission_test_policy false > "${work_dir}/policy.json"
    cma_admission_test_binding false > "${work_dir}/binding.json"
    printf '%s\n' '{}' > "${work_dir}/desired.json"
  }
  cma_admission_kubectl_cluster() {
    local state object_file
    state="$(<"${case_dir}/state")"
    if [[ "$*" == "get validatingadmissionpolicy ${CMA_ADMISSION_NAME} --ignore-not-found -o json" ]]; then
      case "${state}" in
        absent|binding-only) return 0 ;;
        drift-policy)
          cma_admission_test_policy true | jq '.spec.failurePolicy = "Ignore"'
          ;;
        *) cma_admission_test_policy true ;;
      esac
      return 0
    fi
    if [[ "$*" == "get validatingadmissionpolicybinding ${CMA_ADMISSION_NAME} --ignore-not-found -o json" ]]; then
      case "${state}" in
        both|binding-only) cma_admission_test_binding true ;;
        *) return 0 ;;
      esac
      return 0
    fi
    if [[ "$1" == apply ]]; then
      # The exact -f argument is immediately before -o json.
      for (( cma_i=1; cma_i<=$#; cma_i++ )); do
        if [[ "${!cma_i}" == -f ]]; then
          cma_j=$((cma_i + 1))
          object_file="${!cma_j}"
          break
        fi
      done
      cat "${object_file}"
      return 0
    fi
    if [[ "$1" == create && "$2" == --field-manager=* ]]; then
      for (( cma_i=1; cma_i<=$#; cma_i++ )); do
        if [[ "${!cma_i}" == -f ]]; then
          cma_j=$((cma_i + 1))
          object_file="${!cma_j}"
          break
        fi
      done
      if jq -e '.kind == "ValidatingAdmissionPolicy"' "${object_file}" >/dev/null; then
        printf '%s\n' policy >> "${case_dir}/writes"
        if [[ "${case_name}" != policy-write-lost ]]; then
          printf '%s\n' policy-only > "${case_dir}/state"
        fi
        [[ "${case_name}" != policy-create-ambiguous ]] || return 42
        [[ "${case_name}" != policy-write-lost ]] || return 42
        cma_admission_test_policy true
      else
        printf '%s\n' binding >> "${case_dir}/writes"
        printf '%s\n' both > "${case_dir}/state"
        cma_admission_test_binding true
      fi
      return 0
    fi
    return 97
  }
  cma_admission_kubectl_namespace() {
    if [[ "$1" == get && "$2" == pod && "$3" == -l ]]; then
      if [[ "${case_name}" == reserved-pod ]]; then
        printf '%s\n' '{"apiVersion":"v1","kind":"List","metadata":{"resourceVersion":"1"},"items":[{}]}'
      else
        printf '%s\n' '{"apiVersion":"v1","kind":"List","metadata":{"resourceVersion":"1"},"items":[]}'
      fi
      return 0
    fi
    if [[ "$1" == get && "$2" == pod ]]; then
      if [[ "${case_name}" == persisted-admitted && \
        "$3" == cma-cutover-fence-admit-drill ]]; then
        printf '%s\n' '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"cma-cutover-fence-admit-drill","namespace":"chrono-bot"}}'
      fi
      return 0
    fi
    if [[ "$1" == create && "$2" == --dry-run=server ]]; then
      if [[ "$*" == *denied-pod.json* ]]; then
        if [[ "${case_name}" == wrong-denial ]]; then
          echo 'denied by unrelated policy' >&2
        else
          echo "${CMA_ADMISSION_NAME}: Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715" >&2
        fi
        return 1
      fi
      printf '%s\n' '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"cma-cutover-fence-admit-drill","namespace":"chrono-bot"}}'
      return 0
    fi
    return 96
  }
}

cma_admission_test_case() {
  local case_name="$1"
  local initial_state="$2"
  local expected_status="$3"
  local expected_writes="$4"
  local confirmation="${5:-bootstrap-v1.2.3-admission}"
  local case_dir="${TEST_ROOT}/${case_name}"
  local actual_status actual_writes

  mkdir -m 700 "${case_dir}" "${case_dir}/custody"
  set +e
  (
    set -euo pipefail
    export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
    export CMA_GATEWAY_CONFIRM="${confirmation}"
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_ADMISSION_CUSTODY_CONFIRM=durable-off-host
    export CMA_GATEWAY_ADMISSION_CUSTODY_ROOT="${case_dir}/custody"
    cma_admission_test_configure "${case_name}" "${case_dir}" "${initial_state}"
    cma_admission_bootstrap
  ) > "${case_dir}/stdout" 2> "${case_dir}/stderr"
  actual_status=$?
  set -e
  actual_writes="$(tr '\n' ',' < "${case_dir}/writes" | sed 's/,$//')"
  if [[ "${actual_status}" -ne "${expected_status}" || \
    "${actual_writes}" != "${expected_writes}" ]]; then
    echo "${case_name}: expected status/writes ${expected_status}/${expected_writes}, got ${actual_status}/${actual_writes}" >&2
    sed -n '1,160p' "${case_dir}/stderr" >&2
    return 1
  fi
  if [[ "${expected_status}" -eq 0 ]]; then
    local receipt_dir="${case_dir}/custody/admission-bootstrap-0123456789abcdef0123456789abcdef01234567"
    [[ -f "${receipt_dir}/receipt.json" && -f "${receipt_dir}/receipt.sha256" ]] || return 1
  elif find "${case_dir}/custody" -name receipt.json -print | grep -q .; then
    echo "${case_name}: failed bootstrap published a receipt" >&2
    return 1
  fi
}

cma_admission_test_production_gate() {
  local fake_bin="${TEST_ROOT}/gate-bin"
  local marker="${TEST_ROOT}/production-boundary"
  mkdir "${fake_bin}"
  for command_name in chmod cmp find git jq kubectl mktemp mv ruby timeout wc sha256sum awk; do
    ln -s "$(command -v "${command_name}")" "${fake_bin}/${command_name}"
  done
  rm "${fake_bin}/kubectl"
  printf '#!/bin/sh\n: > %s\nexit 99\n' "${marker}" > "${fake_bin}/kubectl"
  chmod +x "${fake_bin}/kubectl"
  set +e
  env -i PATH="${fake_bin}" LC_ALL=C \
    CMA_GATEWAY_TRUSTED_PATH="${fake_bin}" CMA_GATEWAY_TRUSTED_LAUNCH=env-i-v1 \
    CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
    /bin/bash --noprofile --norc -p \
      "${SCRIPT_DIR}/cma-trigger-gateway-admission-bootstrap.sh" --bootstrap \
      > "${TEST_ROOT}/gate.stdout" 2> "${TEST_ROOT}/gate.stderr"
  local status=$?
  set -e
  [[ "${status}" -ne 0 && ! -e "${marker}" ]] || {
    echo "disabled admission bootstrap reached kubectl" >&2
    return 1
  }
  sed 's/^readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0$/readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=1/' \
    "${SCRIPT_DIR}/cma-trigger-gateway-admission-bootstrap.sh" \
    > "${TEST_ROOT}/admission-gate-open.sh"
  # shellcheck disable=SC2016 # Expand positional parameters in the child Bash.
  env -i PATH="${PATH}" LC_ALL=C \
    CMA_GATEWAY_TRUSTED_PATH="${PATH}" CMA_GATEWAY_TRUSTED_LAUNCH=env-i-v1 \
    CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM=reviewed-host-toolchain \
    /bin/bash --noprofile --norc -p -c '
      source "$1"
      cma_admission_require_launcher
      cma_admission_require_production_mutation_enabled
    ' cma-admission-gate "${TEST_ROOT}/admission-gate-open.sh"
}

cma_admission_test_publication_fsync_recovery() {
  local case_name=publication-fsync-recovery
  local case_dir="${TEST_ROOT}/${case_name}"
  local revision=0123456789abcdef0123456789abcdef01234567
  local destination_dir first_status

  mkdir -m 700 "${case_dir}" "${case_dir}/custody"
  destination_dir="${case_dir}/custody/admission-bootstrap-${revision}"
  set +e
  (
    set -euo pipefail
    export CMA_GATEWAY_RELEASE_REVISION="${revision}"
    export CMA_GATEWAY_CONFIRM=bootstrap-v1.2.3-admission
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_ADMISSION_CUSTODY_CONFIRM=durable-off-host
    export CMA_GATEWAY_ADMISSION_CUSTODY_ROOT="${case_dir}/custody"
    cma_admission_test_configure "${case_name}" "${case_dir}" absent
    eval "$(declare -f cma_admission_sync_published_directory |
      sed '1s/^cma_admission_sync_published_directory /cma_admission_test_real_sync /')"
    cma_admission_sync_published_directory() {
      if [[ ! -e "${case_dir}/sync-failure.injected" ]]; then
        : > "${case_dir}/sync-failure.injected"
        return 1
      fi
      cma_admission_test_real_sync "$@"
    }

    cma_admission_bootstrap
  ) >"${case_dir}/first.stdout" 2>"${case_dir}/first.stderr"
  first_status=$?
  set -e
  if [[ "${first_status}" -eq 0 || ! -d "${destination_dir}" || \
        ! -e "${case_dir}/sync-failure.injected" || -s "${case_dir}/first.stdout" ]]; then
    echo "admission bootstrap accepted rename success followed by fsync failure" >&2
    return 1
  fi

  (
    set -euo pipefail
    export CMA_GATEWAY_RELEASE_REVISION="${revision}"
    export CMA_GATEWAY_CONFIRM=bootstrap-v1.2.3-admission
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_ADMISSION_CUSTODY_CONFIRM=durable-off-host
    export CMA_GATEWAY_ADMISSION_CUSTODY_ROOT="${case_dir}/custody"
    cma_admission_test_configure "${case_name}" "${case_dir}" both
    cma_admission_bootstrap >"${case_dir}/retry.stdout"
    grep -Eq '/receipt\.json\|[0-9a-f]{64}$' "${case_dir}/retry.stdout"
  )
}

cma_admission_test_case absent-success absent 0 policy,binding
cma_admission_test_case policy-partial-retry policy-only 0 binding
cma_admission_test_case policy-create-ambiguous absent 0 policy,binding
cma_admission_test_case policy-write-lost absent 1 policy
cma_admission_test_case binding-before-policy binding-only 1 ''
cma_admission_test_case non-equivalent-policy drift-policy 1 ''
cma_admission_test_case reserved-pod absent 1 ''
cma_admission_test_case wrong-denial absent 1 policy,binding
cma_admission_test_case persisted-admitted absent 1 policy,binding
cma_admission_test_case confirmation-mismatch absent 1 '' bootstrap-v9.9.9-admission
cma_admission_test_publication_fsync_recovery
cma_admission_test_production_gate

printf '%s\n' "CMA Trigger Gateway admission-bootstrap tests passed (12 cases)"
