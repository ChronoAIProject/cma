#!/bin/bash -p

{ set +x; } 2>/dev/null
set -euo pipefail
umask 077

unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH
for cma_admission_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_admission_name}"
done
unset cma_admission_name

readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0
readonly CMA_ADMISSION_CONTEXT="gke_example-project-02_asia-northeast1_example-cluster-02"
readonly CMA_ADMISSION_NAMESPACE="chrono-bot"
readonly CMA_ADMISSION_NAME="cma-cutover-fence-label-reservation"
readonly CMA_ADMISSION_MANIFEST="infra/api/cutover-fence-policy.yaml"
readonly CMA_ADMISSION_FIELD_MANAGER="cma-trigger-gateway-admission-bootstrap"
readonly CMA_ADMISSION_RESERVED_SELECTOR="release.chrono-ai.fun/cutover-fence=mongodb-715"
readonly CMA_ADMISSION_SOURCE_PATHS="infra/api/cutover-fence-policy.yaml .github/scripts/cma-trigger-gateway-admission-bootstrap.sh .github/scripts/cma-trigger-gateway-launch.sh .github/scripts/cma-trigger-gateway-projection.rb .github/scripts/cma-trigger-gateway-projection.sh .github/scripts/cma-trigger-gateway-tls-sanitize.rb .github/scripts/cma-trigger-gateway-live-policy.json .github/scripts/cma-trigger-gateway-cloudflare-capture.sh .github/scripts/cma-trigger-gateway-cloudflare-policy.json .github/scripts/cma-trigger-gateway-origin-evidence.sh .github/scripts/cma-trigger-gateway-state.sh .github/scripts/cma-trigger-gateway-adoption.sh .github/scripts/cma-trigger-gateway-restore-traffic.sh"

cma_admission_die() {
  printf 'cma-trigger Gateway admission bootstrap: %s\n' "$*" >&2
  exit 1
}

cma_admission_require_launcher() {
  [[ "${CMA_GATEWAY_TRUSTED_LAUNCH:-}" == env-i-v1 ]] || \
    cma_admission_die "the reviewed env-i launcher is required"
  [[ "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" == reviewed-host-toolchain ]] || \
    cma_admission_die "the reviewed host-toolchain attestation is required"
  [[ -n "${CMA_GATEWAY_TRUSTED_PATH:-}" && "${PATH}" == "${CMA_GATEWAY_TRUSTED_PATH}" ]] || \
    cma_admission_die "PATH differs from the reviewed launcher PATH"
}

cma_admission_require_production_mutation_enabled() {
  [[ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" == 1 ]] || {
    echo "production admission bootstrap is disabled in the checked-in script" >&2
    return 1
  }
}

cma_admission_require_tools() {
  local command_name
  for command_name in chmod cmp find git jq kubectl mktemp mv ruby timeout wc; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_admission_die "required command is unavailable: ${command_name}"
  done
  if ! command -v sha256sum >/dev/null 2>&1 && \
     ! command -v shasum >/dev/null 2>&1; then
    cma_admission_die "a SHA-256 command is required"
  fi
}

cma_admission_sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cma_admission_sha256_stream() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum | awk '{print $1}'
  else
    shasum -a 256 | awk '{print $1}'
  fi
}

cma_admission_sha256_valid() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]]
}

cma_admission_source_digest() {
  local source_path source_sha
  for source_path in ${CMA_ADMISSION_SOURCE_PATHS}; do
    [[ -f "${source_path}" && ! -L "${source_path}" ]] || return 1
    source_sha="$(cma_admission_sha256_file "${source_path}")" || return 1
    cma_admission_sha256_valid "${source_sha}" || return 1
    printf '%s\0%s\0' "${source_path}" "${source_sha}"
  done | cma_admission_sha256_stream
}

cma_admission_require_checkout() {
  local revision="${CMA_GATEWAY_RELEASE_REVISION:-}"
  local source_path

  [[ "${revision}" =~ ^[0-9a-f]{40}$ ]] || \
    cma_admission_die "CMA_GATEWAY_RELEASE_REVISION must be the reviewed full commit"
  [[ "$(git rev-parse HEAD)" == "${revision}" ]] || \
    cma_admission_die "checkout HEAD differs from the reviewed release revision"
  for source_path in ${CMA_ADMISSION_SOURCE_PATHS}; do
    git diff --quiet -- "${source_path}" || \
      cma_admission_die "reviewed admission source is modified: ${source_path}"
    git show "${revision}:${source_path}" | cmp -s - "${source_path}" || \
      cma_admission_die "reviewed admission source bytes differ: ${source_path}"
  done
}

cma_admission_kubectl_cluster() {
  timeout --signal=TERM --kill-after=5s 30s \
    kubectl --request-timeout=20s --context "${CMA_ADMISSION_CONTEXT}" "$@"
}

cma_admission_kubectl_namespace() {
  timeout --signal=TERM --kill-after=5s 30s \
    kubectl --request-timeout=20s --context "${CMA_ADMISSION_CONTEXT}" \
      --namespace "${CMA_ADMISSION_NAMESPACE}" "$@"
}

cma_admission_validate_policy() {
  local require_status="$1"
  jq -e --argjson require_status "${require_status}" '
    {"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"} as $labels |
    [{apiGroups:[""],apiVersions:["v1"],operations:["CREATE","UPDATE"],
      resources:["pods"],scope:"Namespaced"}] as $rules |
    {matchLabels:{"kubernetes.io/metadata.name":"chrono-bot"}} as $namespace |
    .apiVersion == "admissionregistration.k8s.io/v1" and
    .kind == "ValidatingAdmissionPolicy" and
    .metadata.name == "cma-cutover-fence-label-reservation" and
    .metadata.labels == $labels and .metadata.deletionTimestamp == null and
    (.metadata.annotations // {}) == {} and (.metadata.finalizers // []) == [] and
    (.metadata.ownerReferences // []) == [] and
    .spec == {
      failurePolicy:"Fail",
      matchConstraints:{matchPolicy:"Equivalent",namespaceSelector:$namespace,resourceRules:$rules},
      matchConditions:[{name:"exact-chrono-bot-namespace",expression:"request.namespace == \u0027chrono-bot\u0027"}],
      validations:[{
        expression:"!has(object.metadata.labels) || !(\u0027release.chrono-ai.fun/cutover-fence\u0027 in object.metadata.labels) || object.metadata.labels[\u0027release.chrono-ai.fun/cutover-fence\u0027] != \u0027mongodb-715\u0027",
        message:"Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715",
        reason:"Forbidden"
      }]
    } and
    (if $require_status then
      (.metadata.uid | type) == "string" and (.metadata.uid | length) > 0 and
      (.metadata.resourceVersion | type) == "string" and (.metadata.resourceVersion | length) > 0 and
      (.metadata.generation | type) == "number" and .metadata.generation > 0 and
      .status.observedGeneration == .metadata.generation and
      (.status.typeChecking.expressionWarnings // []) == []
    else true end)
  ' >/dev/null
}

cma_admission_validate_binding() {
  local require_identity="$1"
  jq -e --argjson require_identity "${require_identity}" '
    {"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"} as $labels |
    [{apiGroups:[""],apiVersions:["v1"],operations:["CREATE","UPDATE"],
      resources:["pods"],scope:"Namespaced"}] as $rules |
    {matchLabels:{"kubernetes.io/metadata.name":"chrono-bot"}} as $namespace |
    .apiVersion == "admissionregistration.k8s.io/v1" and
    .kind == "ValidatingAdmissionPolicyBinding" and
    .metadata.name == "cma-cutover-fence-label-reservation" and
    .metadata.labels == $labels and .metadata.deletionTimestamp == null and
    (.metadata.annotations // {}) == {} and (.metadata.finalizers // []) == [] and
    (.metadata.ownerReferences // []) == [] and
    .spec == {
      policyName:"cma-cutover-fence-label-reservation",validationActions:["Deny"],
      matchResources:{matchPolicy:"Equivalent",namespaceSelector:$namespace,resourceRules:$rules}
    } and
    (if $require_identity then
      (.metadata.uid | type) == "string" and (.metadata.uid | length) > 0 and
      (.metadata.resourceVersion | type) == "string" and (.metadata.resourceVersion | length) > 0 and
      (.metadata.generation | type) == "number" and .metadata.generation > 0
    else true end)
  ' >/dev/null
}

cma_admission_read_pair() {
  CMA_ADMISSION_LIVE_POLICY="$(cma_admission_kubectl_cluster get validatingadmissionpolicy \
    "${CMA_ADMISSION_NAME}" --ignore-not-found -o json)" || return 1
  CMA_ADMISSION_LIVE_BINDING="$(cma_admission_kubectl_cluster get validatingadmissionpolicybinding \
    "${CMA_ADMISSION_NAME}" --ignore-not-found -o json)" || return 1
  if [[ -z "${CMA_ADMISSION_LIVE_POLICY}" && -n "${CMA_ADMISSION_LIVE_BINDING}" ]]; then
    echo "binding-before-policy state is forbidden" >&2
    return 1
  fi
  if [[ -n "${CMA_ADMISSION_LIVE_POLICY}" ]]; then
    printf '%s' "${CMA_ADMISSION_LIVE_POLICY}" | cma_admission_validate_policy false || {
      echo "same-name admission policy is not byte-equivalent to the reviewed desired object" >&2
      return 1
    }
  fi
  if [[ -n "${CMA_ADMISSION_LIVE_BINDING}" ]]; then
    printf '%s' "${CMA_ADMISSION_LIVE_BINDING}" | cma_admission_validate_binding false || {
      echo "same-name admission binding is not byte-equivalent to the reviewed desired object" >&2
      return 1
    }
  fi
}

cma_admission_wait_policy() {
  local deadline=$((SECONDS + 90))
  while (( SECONDS < deadline )); do
    cma_admission_read_pair || return 1
    if [[ -n "${CMA_ADMISSION_LIVE_POLICY}" ]] && \
       printf '%s' "${CMA_ADMISSION_LIVE_POLICY}" | cma_admission_validate_policy true; then
      return 0
    fi
    sleep 2
  done
  return 1
}

cma_admission_wait_pair() {
  local deadline=$((SECONDS + 90))
  while (( SECONDS < deadline )); do
    cma_admission_read_pair || return 1
    if [[ -n "${CMA_ADMISSION_LIVE_POLICY}" && -n "${CMA_ADMISSION_LIVE_BINDING}" ]] && \
       printf '%s' "${CMA_ADMISSION_LIVE_POLICY}" | cma_admission_validate_policy true && \
       printf '%s' "${CMA_ADMISSION_LIVE_BINDING}" | cma_admission_validate_binding true; then
      return 0
    fi
    sleep 2
  done
  return 1
}

cma_admission_capture_empty_reserved_pods() {
  local output_file="$1"
  cma_admission_kubectl_namespace get pod -l "${CMA_ADMISSION_RESERVED_SELECTOR}" -o json \
    > "${output_file}" || return 1
  jq -e '
    .apiVersion == "v1" and .kind == "List" and
    (.metadata.resourceVersion | type) == "string" and .items == []
  ' "${output_file}" >/dev/null
}

cma_admission_prepare_desired() {
  local work_dir="$1"
  cma_admission_kubectl_cluster create --dry-run=client \
    -f "${CMA_ADMISSION_MANIFEST}" -o json | jq -s -e '
      ([.[] | select(.kind == "ValidatingAdmissionPolicy")]) as $policy |
      ([.[] | select(.kind == "ValidatingAdmissionPolicyBinding")]) as $binding |
      if ($policy | length) == 1 and ($binding | length) == 1
      then {policy:$policy[0],binding:$binding[0]}
      else error("admission manifest does not contain the exact object pair") end
    ' > "${work_dir}/desired.json" || return 1
  jq -e '.policy' "${work_dir}/desired.json" > "${work_dir}/policy.json" || return 1
  jq -e '.binding' "${work_dir}/desired.json" > "${work_dir}/binding.json" || return 1
  cma_admission_validate_policy false < "${work_dir}/policy.json" && \
    cma_admission_validate_binding false < "${work_dir}/binding.json"
}

cma_admission_server_dry_run() {
  local object_kind="$1"
  local object_file="$2"
  local response
  response="$(cma_admission_kubectl_cluster apply --server-side \
    --field-manager="${CMA_ADMISSION_FIELD_MANAGER}" --dry-run=server \
    -f "${object_file}" -o json)" || return 1
  if [[ "${object_kind}" == policy ]]; then
    printf '%s' "${response}" | cma_admission_validate_policy false
  else
    printf '%s' "${response}" | cma_admission_validate_binding false
  fi
}

cma_admission_create_absent() {
  local object_kind="$1"
  local object_file="$2"
  local create_status=0

  cma_admission_server_dry_run "${object_kind}" "${object_file}" || return 1
  cma_admission_read_pair || return 1
  if [[ "${object_kind}" == policy ]]; then
    [[ -z "${CMA_ADMISSION_LIVE_POLICY}" && -z "${CMA_ADMISSION_LIVE_BINDING}" ]] || return 1
  else
    [[ -n "${CMA_ADMISSION_LIVE_POLICY}" && -z "${CMA_ADMISSION_LIVE_BINDING}" ]] || return 1
    printf '%s' "${CMA_ADMISSION_LIVE_POLICY}" | cma_admission_validate_policy true || return 1
  fi
  cma_admission_kubectl_cluster create --field-manager="${CMA_ADMISSION_FIELD_MANAGER}" \
    -f "${object_file}" -o json >/dev/null || create_status=$?
  cma_admission_read_pair || return 1
  if [[ "${object_kind}" == policy ]]; then
    [[ -n "${CMA_ADMISSION_LIVE_POLICY}" ]] && \
      printf '%s' "${CMA_ADMISSION_LIVE_POLICY}" | cma_admission_validate_policy false || return 1
  else
    [[ -n "${CMA_ADMISSION_LIVE_BINDING}" ]] && \
      printf '%s' "${CMA_ADMISSION_LIVE_BINDING}" | cma_admission_validate_binding true || return 1
  fi
  if [[ "${create_status}" -ne 0 ]]; then
    echo "create returned failure, but the exact same-name object was independently re-read" >&2
  fi
}

cma_admission_run_drills() {
  local output_dir="$1"
  local denied_status=0
  jq -n '{
    apiVersion:"v1",kind:"Pod",
    metadata:{name:"cma-cutover-fence-denial-drill",namespace:"chrono-bot",
      labels:{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}},
    spec:{restartPolicy:"Never",containers:[{name:"drill",image:"registry.k8s.io/pause:3.10"}]}
  }' > "${output_dir}/denied-pod.json"
  jq -n '{
    apiVersion:"v1",kind:"Pod",
    metadata:{name:"cma-cutover-fence-admit-drill",namespace:"chrono-bot"},
    spec:{restartPolicy:"Never",containers:[{name:"drill",image:"registry.k8s.io/pause:3.10"}]}
  }' > "${output_dir}/admitted-pod.json"
  cma_admission_kubectl_namespace create --dry-run=server \
    -f "${output_dir}/denied-pod.json" -o json \
    > "${output_dir}/denied.stdout" 2> "${output_dir}/denied.stderr" || denied_status=$?
  [[ "${denied_status}" -ne 0 && ! -s "${output_dir}/denied.stdout" ]] || return 1
  grep -Fq "${CMA_ADMISSION_NAME}" "${output_dir}/denied.stderr" && \
    grep -Fq 'Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715' \
      "${output_dir}/denied.stderr" || return 1
  cma_admission_kubectl_namespace create --dry-run=server \
    -f "${output_dir}/admitted-pod.json" -o json \
    > "${output_dir}/admitted.json" 2> "${output_dir}/admitted.stderr" || return 1
  [[ ! -s "${output_dir}/admitted.stderr" ]] || return 1
  jq -e '
    .apiVersion == "v1" and .kind == "Pod" and
    .metadata.name == "cma-cutover-fence-admit-drill" and
    .metadata.namespace == "chrono-bot" and
    ((.metadata.labels // {})["release.chrono-ai.fun/cutover-fence"] == null)
  ' "${output_dir}/admitted.json" >/dev/null || return 1
  cma_admission_capture_pod_absence cma-cutover-fence-denial-drill \
    "${output_dir}/denied-absence.json" && \
    cma_admission_capture_pod_absence cma-cutover-fence-admit-drill \
      "${output_dir}/admitted-absence.json"
}

cma_admission_capture_pod_absence() {
  local pod_name="$1"
  local output_file="$2"
  local response

  response="$(cma_admission_kubectl_namespace get pod "${pod_name}" \
    --ignore-not-found -o json)" || return 1
  [[ -z "${response}" ]] || return 1
  jq -n --arg name "${pod_name}" \
    '{apiVersion:"evidence.cma.chrono-ai.fun/v1",kind:"PodAbsence",name:$name,absent:true}' \
    > "${output_file}"
}

cma_admission_fsync() {
  timeout --signal=TERM --kill-after=2s 10s env -i PATH="${PATH}" LC_ALL=C \
    ruby --disable-gems -e 'ARGV.each { |path| File.open(path, File::RDONLY) { |file| file.fsync } }' "$@"
}

cma_admission_publish_directory() {
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    source, destination = ARGV
    raise "destination exists" if File.exist?(destination) || File.symlink?(destination)
    File.rename(source, destination)
    File.open(File.dirname(destination), File::RDONLY) { |directory| directory.fsync }
  ' "$1" "$2"
}

cma_admission_validate_bundle() {
  local receipt_file="${CMA_GATEWAY_ADMISSION_RECEIPT:-}"
  local bundle_dir entry_count receipt_sha recorded_sha source_sha evidence_name evidence_sha

  [[ "${receipt_file}" == /* && -f "${receipt_file}" && ! -L "${receipt_file}" ]] || return 1
  bundle_dir="${receipt_file%/*}"
  [[ "${receipt_file}" == "${bundle_dir}/receipt.json" && -d "${bundle_dir}" && \
    ! -L "${bundle_dir}" && -O "${bundle_dir}" ]] || return 1
  entry_count="$(find "${bundle_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" || return 1
  [[ "${entry_count}" == 8 ]] || return 1
  for evidence_name in pre-pods.json post-pods.json denied.stderr admitted.json \
      denied-absence.json admitted-absence.json receipt.json receipt.sha256; do
    [[ -f "${bundle_dir}/${evidence_name}" && ! -L "${bundle_dir}/${evidence_name}" ]] || return 1
  done
  receipt_sha="$(cma_admission_sha256_file "${receipt_file}")" || return 1
  recorded_sha="$(awk 'NF == 2 && $2 == "receipt.json" {print $1}' "${bundle_dir}/receipt.sha256")" || return 1
  cma_admission_sha256_valid "${receipt_sha}" && \
    [[ "${receipt_sha}" == "${recorded_sha}" && \
      "${receipt_sha}" == "${CMA_GATEWAY_ADMISSION_RECEIPT_SHA256:-}" ]] || return 1
  source_sha="$(cma_admission_source_digest)" || return 1
  jq -e --arg revision "${CMA_GATEWAY_RELEASE_REVISION}" --arg source_sha "${source_sha}" '
    .schemaVersion == 1 and .releaseRevision == $revision and
    .permanentRetention == true and .source.sourceSetSha256 == $source_sha and
    (.source.manifestSha256 | test("^[0-9a-f]{64}$")) and
    (.admission.policy.uid | type) == "string" and
    (.admission.policy.generation | type) == "number" and
    (.admission.policy.resourceVersion | type) == "string" and
    (.admission.binding.uid | type) == "string" and
    (.admission.binding.generation | type) == "number" and
    (.admission.binding.resourceVersion | type) == "string" and
    (.evidence | keys | sort) == [
      "admitted","admittedAbsence","denied","deniedAbsence","postPods","prePods"
    ]
  ' "${receipt_file}" >/dev/null || return 1
  [[ "$(jq -er .source.manifestSha256 "${receipt_file}")" == \
    "$(cma_admission_sha256_file "${CMA_ADMISSION_MANIFEST}")" ]] || return 1
  for evidence_name in prePods postPods denied admitted deniedAbsence admittedAbsence; do
    case "${evidence_name}" in
      prePods) local evidence_file=pre-pods.json ;;
      postPods) local evidence_file=post-pods.json ;;
      denied) local evidence_file=denied.stderr ;;
      admitted) local evidence_file=admitted.json ;;
      deniedAbsence) local evidence_file=denied-absence.json ;;
      admittedAbsence) local evidence_file=admitted-absence.json ;;
    esac
    evidence_sha="$(cma_admission_sha256_file "${bundle_dir}/${evidence_file}")" || return 1
    [[ "${evidence_sha}" == "$(jq -er --arg key "${evidence_name}" '.evidence[$key].sha256' \
      "${receipt_file}")" ]] || return 1
  done
  jq -e '.items == []' "${bundle_dir}/pre-pods.json" >/dev/null && \
    jq -e '.items == []' "${bundle_dir}/post-pods.json" >/dev/null && \
    grep -Fq "${CMA_ADMISSION_NAME}" "${bundle_dir}/denied.stderr" && \
    grep -Fq 'Pods in chrono-bot must not use the reserved CMA cutover-fence label value mongodb-715' \
      "${bundle_dir}/denied.stderr" && \
    jq -e '.kind == "Pod" and .metadata.name == "cma-cutover-fence-admit-drill"' \
      "${bundle_dir}/admitted.json" >/dev/null && \
    jq -e '.kind == "PodAbsence" and .name == "cma-cutover-fence-denial-drill" and .absent == true' \
      "${bundle_dir}/denied-absence.json" >/dev/null && \
    jq -e '.kind == "PodAbsence" and .name == "cma-cutover-fence-admit-drill" and .absent == true' \
      "${bundle_dir}/admitted-absence.json" >/dev/null
}

cma_admission_verify_live_receipt() {
  local work_dir current_identity receipt_identity

  cma_admission_validate_bundle || return 1
  cma_admission_read_pair || return 1
  [[ -n "${CMA_ADMISSION_LIVE_POLICY}" && -n "${CMA_ADMISSION_LIVE_BINDING}" ]] || return 1
  printf '%s' "${CMA_ADMISSION_LIVE_POLICY}" | cma_admission_validate_policy true || return 1
  printf '%s' "${CMA_ADMISSION_LIVE_BINDING}" | cma_admission_validate_binding true || return 1
  current_identity="$(printf '%s\n%s\n' "${CMA_ADMISSION_LIVE_POLICY}" \
    "${CMA_ADMISSION_LIVE_BINDING}" | jq -sc '{
      policy:{uid:.[0].metadata.uid,generation:.[0].metadata.generation,
        resourceVersion:.[0].metadata.resourceVersion},
      binding:{uid:.[1].metadata.uid,generation:.[1].metadata.generation,
        resourceVersion:.[1].metadata.resourceVersion}
    }')" || return 1
  receipt_identity="$(jq -c .admission "${CMA_GATEWAY_ADMISSION_RECEIPT}")" || return 1
  [[ "${current_identity}" == "${receipt_identity}" ]] || return 1
  work_dir="$(mktemp -d)" || return 1
  chmod 700 "${work_dir}"
  cma_admission_capture_empty_reserved_pods "${work_dir}/pods.json" && \
    cma_admission_run_drills "${work_dir}"
  local status=$?
  rm -rf "${work_dir}"
  return "${status}"
}

cma_admission_write_bundle() {
  local work_dir="$1"
  local destination_dir="$2"
  local source_sha manifest_sha receipt_sha

  source_sha="$(cma_admission_source_digest)" || return 1
  manifest_sha="$(cma_admission_sha256_file "${CMA_ADMISSION_MANIFEST}")" || return 1
  jq -n --arg revision "${CMA_GATEWAY_RELEASE_REVISION}" \
    --arg source_sha "${source_sha}" --arg manifest_sha "${manifest_sha}" \
    --arg pre_sha "$(cma_admission_sha256_file "${work_dir}/pre-pods.json")" \
    --arg post_sha "$(cma_admission_sha256_file "${work_dir}/post-pods.json")" \
    --arg denied_sha "$(cma_admission_sha256_file "${work_dir}/denied.stderr")" \
    --arg admitted_sha "$(cma_admission_sha256_file "${work_dir}/admitted.json")" \
    --arg denied_absence_sha "$(cma_admission_sha256_file "${work_dir}/denied-absence.json")" \
    --arg admitted_absence_sha "$(cma_admission_sha256_file "${work_dir}/admitted-absence.json")" \
    --argjson policy "${CMA_ADMISSION_LIVE_POLICY}" \
    --argjson binding "${CMA_ADMISSION_LIVE_BINDING}" '{
      schemaVersion:1,releaseRevision:$revision,permanentRetention:true,
      source:{sourceSetSha256:$source_sha,manifestSha256:$manifest_sha},
      admission:{
        policy:{uid:$policy.metadata.uid,generation:$policy.metadata.generation,
          resourceVersion:$policy.metadata.resourceVersion},
        binding:{uid:$binding.metadata.uid,generation:$binding.metadata.generation,
          resourceVersion:$binding.metadata.resourceVersion}
      },
      evidence:{
        prePods:{sha256:$pre_sha},postPods:{sha256:$post_sha},
        denied:{sha256:$denied_sha,policy:"cma-cutover-fence-label-reservation"},
        admitted:{sha256:$admitted_sha},
        deniedAbsence:{sha256:$denied_absence_sha},
        admittedAbsence:{sha256:$admitted_absence_sha}
      }
    }' > "${work_dir}/receipt.json" || return 1
  receipt_sha="$(cma_admission_sha256_file "${work_dir}/receipt.json")" || return 1
  printf '%s  receipt.json\n' "${receipt_sha}" > "${work_dir}/receipt.sha256" || return 1
  rm -f "${work_dir}/denied-pod.json" "${work_dir}/denied.stdout" \
    "${work_dir}/admitted-pod.json" "${work_dir}/admitted.stderr" \
    "${work_dir}/desired.json" "${work_dir}/policy.json" "${work_dir}/binding.json"
  chmod 400 "${work_dir}"/* || return 1
  cma_admission_fsync "${work_dir}"/* "${work_dir}" "${work_dir%/*}" || return 1
  cma_admission_publish_directory "${work_dir}" "${destination_dir}" || return 1
  printf '%s|%s\n' "${destination_dir}/receipt.json" "${receipt_sha}"
}

cma_admission_bootstrap() {
  local custody_root destination_dir work_dir acknowledgement

  cma_admission_require_production_mutation_enabled || return 1
  [[ "${CMA_GATEWAY_CONFIRM:-}" == bootstrap-v0.10-admission ]] || \
    cma_admission_die "set CMA_GATEWAY_CONFIRM=bootstrap-v0.10-admission"
  [[ "${CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM:-}" == sole-spec-writer ]] || \
    cma_admission_die "the exclusive admission-writer attestation is absent"
  [[ "${CMA_GATEWAY_ADMISSION_CUSTODY_CONFIRM:-}" == durable-off-host ]] || \
    cma_admission_die "durable admission evidence custody is not acknowledged"
  cma_admission_require_checkout
  custody_root="${CMA_GATEWAY_ADMISSION_CUSTODY_ROOT:-}"
  [[ "${custody_root}" == /* && "${custody_root}" != / && -d "${custody_root}" && \
    ! -L "${custody_root}" && -O "${custody_root}" ]] || \
    cma_admission_die "admission custody root must be an exact owned absolute directory"
  destination_dir="${custody_root%/}/admission-bootstrap-${CMA_GATEWAY_RELEASE_REVISION}"
  if [[ -d "${destination_dir}" && ! -L "${destination_dir}" ]]; then
    CMA_GATEWAY_ADMISSION_RECEIPT="${destination_dir}/receipt.json"
    CMA_GATEWAY_ADMISSION_RECEIPT_SHA256="$(awk 'NF == 2 && $2 == "receipt.json" {print $1}' \
      "${destination_dir}/receipt.sha256")"
    cma_admission_verify_live_receipt || cma_admission_die "existing admission receipt is not reusable"
    printf '%s|%s\n' "${CMA_GATEWAY_ADMISSION_RECEIPT}" \
      "${CMA_GATEWAY_ADMISSION_RECEIPT_SHA256}"
    return 0
  fi
  [[ ! -e "${destination_dir}" && ! -L "${destination_dir}" ]] || \
    cma_admission_die "admission receipt destination is occupied"
  work_dir="$(mktemp -d "${custody_root%/}/.admission-bootstrap.XXXXXXXX")" || \
    cma_admission_die "admission evidence work directory could not be created"
  chmod 700 "${work_dir}"
  trap 'rm -rf "${work_dir}"' EXIT HUP INT QUIT TERM
  cma_admission_prepare_desired "${work_dir}" || cma_admission_die "reviewed admission manifest is invalid"
  cma_admission_capture_empty_reserved_pods "${work_dir}/pre-pods.json" || \
    cma_admission_die "a Pod already carries the reserved fence label"
  cma_admission_read_pair || cma_admission_die "admission pair has a forbidden initial state"
  if [[ -z "${CMA_ADMISSION_LIVE_POLICY}" ]]; then
    cma_admission_create_absent policy "${work_dir}/policy.json" || \
      cma_admission_die "admission policy creation could not be reconciled"
  fi
  cma_admission_wait_policy || cma_admission_die "admission policy did not become type-safe"
  cma_admission_read_pair || cma_admission_die "admission policy reread failed"
  if [[ -z "${CMA_ADMISSION_LIVE_BINDING}" ]]; then
    cma_admission_create_absent binding "${work_dir}/binding.json" || \
      cma_admission_die "admission binding creation could not be reconciled"
  fi
  cma_admission_wait_pair || cma_admission_die "admission pair did not become exact"
  cma_admission_capture_empty_reserved_pods "${work_dir}/post-pods.json" || \
    cma_admission_die "a Pod acquired the reserved fence label during bootstrap"
  cma_admission_run_drills "${work_dir}" || \
    cma_admission_die "live denial/admission drills did not prove the intended policy"
  cma_admission_read_pair || cma_admission_die "admission pair drifted after drills"
  acknowledgement="$(cma_admission_write_bundle "${work_dir}" "${destination_dir}")" || \
    cma_admission_die "admission evidence could not be atomically committed"
  trap - EXIT HUP INT QUIT TERM
  CMA_GATEWAY_ADMISSION_RECEIPT="${acknowledgement%%|*}"
  CMA_GATEWAY_ADMISSION_RECEIPT_SHA256="${acknowledgement##*|}"
  cma_admission_validate_bundle || \
    cma_admission_die "published admission evidence failed closed self-validation"
  printf '%s\n' "${acknowledgement}"
}

cma_admission_usage() {
  cat >&2 <<'USAGE'
usage:
  cma-trigger-gateway-admission-bootstrap.sh --bootstrap
  cma-trigger-gateway-admission-bootstrap.sh --verify-receipt

The checked-in production mutation gate remains hard-disabled. The bootstrap
creates the policy before the binding and never overwrites a same-name object.
USAGE
}

cma_admission_main() {
  case "${1:-}" in
    --bootstrap)
      [[ "$#" -eq 1 ]] || cma_admission_die "--bootstrap takes no arguments"
      cma_admission_require_launcher
      cma_admission_require_tools
      cma_admission_bootstrap
      ;;
    --verify-receipt)
      [[ "$#" -eq 1 ]] || cma_admission_die "--verify-receipt takes no arguments"
      cma_admission_require_launcher
      cma_admission_require_tools
      cma_admission_require_checkout
      cma_admission_verify_live_receipt || cma_admission_die "admission receipt/live proof is invalid"
      printf '%s\n' "CMA Gateway admission receipt and live enforcement are exact"
      ;;
    *) cma_admission_usage; return 2 ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  cma_admission_main "$@"
fi
