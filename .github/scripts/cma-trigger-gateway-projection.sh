#!/bin/bash -p
set -euo pipefail
umask 077

unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB
for cma_projection_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_projection_name}"
done
unset cma_projection_name

readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0
readonly CMA_GATEWAY_CONTEXT="gke_chronoai-501608_asia-northeast1_chronoai-prod-asia-northeast1-gke"
readonly CMA_GATEWAY_NAMESPACE="chrono-bot"
readonly CMA_GATEWAY_OWNER_NAMESPACE="gateway-infra"
readonly CMA_GATEWAY_PROJECT_ID="chronoai-501608"
readonly CMA_GATEWAY_PROJECT_NUMBER="191845086376"
readonly CMA_GATEWAY_PROJECTION_CONTRACT=".github/scripts/cma-trigger-gateway-projection.rb"
readonly CMA_GATEWAY_TLS_SANITIZER=".github/scripts/cma-trigger-gateway-tls-sanitize.rb"
readonly CMA_GATEWAY_ADMISSION_BOOTSTRAP=".github/scripts/cma-trigger-gateway-admission-bootstrap.sh"
readonly CMA_GATEWAY_API_ROOT="https://www.googleapis.com/compute/v1"

cma_projection_die() {
  printf 'cma-trigger closed Gateway projection: %s\n' "$*" >&2
  exit 1
}

cma_projection_require_trusted_launcher() {
  [[ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" == 0 ]] || \
    cma_projection_die "the checked-in production mutation gate is unexpectedly open"
  [[ "${CMA_GATEWAY_TRUSTED_LAUNCH:-}" == env-i-v1 ]] || \
    cma_projection_die "capture requires the reviewed env-i launcher"
  [[ "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" == reviewed-host-toolchain ]] || \
    cma_projection_die "capture requires the reviewed host-toolchain attestation"
  [[ -n "${CMA_GATEWAY_TRUSTED_PATH:-}" && "${PATH}" == "${CMA_GATEWAY_TRUSTED_PATH}" ]] || \
    cma_projection_die "capture PATH differs from the reviewed launcher PATH"
}

cma_projection_require_tools() {
  local command_name
  for command_name in chmod cmp gcloud jq kubectl mkdir mktemp mv rm ruby timeout; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_projection_die "required command is unavailable: ${command_name}"
  done
  if ! command -v sha256sum >/dev/null 2>&1 && \
     ! command -v shasum >/dev/null 2>&1; then
    cma_projection_die "a SHA-256 command is required"
  fi
}

cma_projection_kubectl() {
  local namespace="$1"
  shift
  timeout --signal=TERM --kill-after=5s 30s \
    kubectl --request-timeout=20s --context "${CMA_GATEWAY_CONTEXT}" \
      --namespace "${namespace}" "$@"
}

cma_projection_kubectl_cluster() {
  timeout --signal=TERM --kill-after=5s 30s \
    kubectl --request-timeout=20s --context "${CMA_GATEWAY_CONTEXT}" "$@"
}

cma_projection_gcloud() {
  timeout --signal=TERM --kill-after=5s 30s \
    env CLOUDSDK_CORE_HTTP_TIMEOUT=20 gcloud --quiet "$@"
}

cma_projection_sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cma_projection_fsync() {
  timeout --signal=TERM --kill-after=2s 10s \
    env -i PATH="${PATH}" LC_ALL=C \
      ruby --disable-gems -e '
        ARGV.each do |path|
          File.open(path, File::RDONLY) { |file| file.fsync }
        end
      ' "$@"
}

cma_projection_emit() {
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems \
    "${CMA_GATEWAY_PROJECTION_CONTRACT}" --emit "$1" "$2" >/dev/null
}

cma_projection_admission_receipt() {
  local receipt="${CMA_GATEWAY_ADMISSION_RECEIPT:-}"
  local receipt_sha="${CMA_GATEWAY_ADMISSION_RECEIPT_SHA256:-}"

  [[ "${receipt}" == /* && -f "${receipt}" && ! -L "${receipt}" && \
    "${receipt_sha}" =~ ^[0-9a-f]{64}$ ]] || \
    cma_projection_die "the durable admission bootstrap receipt identity is absent"
  "${CMA_GATEWAY_ADMISSION_BOOTSTRAP}" --verify-receipt >/dev/null || \
    cma_projection_die "the durable admission bootstrap receipt/live proof is invalid"
  jq -e --arg receipt_sha "${receipt_sha}" '{
    receiptSha256:$receipt_sha,releaseRevision,sourceSetSha256:.source.sourceSetSha256,
    manifestSha256:.source.manifestSha256,
    policy:.admission.policy,binding:.admission.binding
  }' "${receipt}"
}

cma_projection_lifecycle() {
  local mode="$1"
  local stage="${CMA_GATEWAY_PROJECTION_STAGE:-}"
  local predecessor="${CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256:-}"

  case "${stage}:${mode}" in
    source:source)
      [[ "${predecessor}" == genesis ]] || return 1
      ;;
    adopted:source|final-reopen:source|rollback-source:source|fenced-pre-scale:fenced|fenced-post-scale:fenced|fenced-pre-reopen:fenced)
      [[ "${predecessor}" =~ ^[0-9a-f]{64}$ ]] || return 1
      ;;
    *) return 1 ;;
  esac
  jq -cn --arg stage "${stage}" --arg predecessor "${predecessor}" \
    '{stage:$stage,predecessorSha256:$predecessor}'
}

cma_projection_refs() {
  local gateway_file="$1"
  local annotation="$2"

  jq -er --arg annotation "${annotation}" '
    (.metadata.annotations[$annotation] // "") |
    if . == "" then empty
    else split(",")[] | gsub("^ +| +$"; "") |
      if test("^/projects/191845086376/(global|zones/asia-northeast1-[a-z])/[A-Za-z]+/[A-Za-z0-9._-]+$")
      then . else error("Gateway annotation contains an invalid Compute reference") end
    end
  ' "${gateway_file}"
}

cma_projection_ref_name() {
  local ref="$1"
  printf '%s\n' "${ref##*/}"
}

cma_projection_capture_ref_array() {
  local gateway_file="$1"
  local annotation="$2"
  local command_group="$3"
  local output_file="$4"
  local scratch_dir="$5"
  local ref index=0

  mkdir -m 700 "${scratch_dir}"
  while IFS= read -r ref; do
    [[ -n "${ref}" ]] || continue
    case "${command_group}" in
      forwarding-rules)
        cma_projection_gcloud compute forwarding-rules describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      target-http-proxies)
        cma_projection_gcloud compute target-http-proxies describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      target-https-proxies)
        cma_projection_gcloud compute target-https-proxies describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      url-maps)
        cma_projection_gcloud compute url-maps describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      ssl-certificates)
        cma_projection_gcloud compute ssl-certificates describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      backend-services)
        cma_projection_gcloud compute backend-services describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      health-checks)
        cma_projection_gcloud compute health-checks describe \
          "$(cma_projection_ref_name "${ref}")" --global \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      firewalls)
        cma_projection_gcloud compute firewall-rules describe \
          "$(cma_projection_ref_name "${ref}")" \
          --project="${CMA_GATEWAY_PROJECT_ID}" --format=json
        ;;
      *) cma_projection_die "unknown Compute capture group ${command_group}" ;;
    esac > "${scratch_dir}/$(printf '%04d' "${index}").json" || \
      cma_projection_die "could not capture ${command_group} reference ${ref}"
    index=$((index + 1))
  done < <(cma_projection_refs "${gateway_file}" "${annotation}")
  if [[ "${index}" -eq 0 ]]; then
    printf '%s\n' '[]' > "${output_file}"
  else
    jq -s '.' "${scratch_dir}"/*.json > "${output_file}" || \
      cma_projection_die "could not aggregate ${command_group} capture"
  fi
}

cma_projection_capture_forwarding_rules_for_address() {
  local address_file="$1"
  local output_file="$2"
  local scratch_dir="$3"
  local address list_file name index=0

  mkdir -m 700 "${scratch_dir}"
  address="$(jq -er '.address | select(type == "string" and length > 0)' "${address_file}")" || \
    cma_projection_die "global address value could not be read"
  list_file="${scratch_dir}/list.json"
  cma_projection_gcloud compute forwarding-rules list --global \
    --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
    > "${list_file}" || cma_projection_die "could not enumerate global forwarding rules"
  while IFS= read -r name; do
    [[ -n "${name}" ]] || continue
    cma_projection_gcloud compute forwarding-rules describe "${name}" --global \
      --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
      > "${scratch_dir}/$(printf '%04d' "${index}").json" || \
      cma_projection_die "could not capture forwarding rule ${name} on the Gateway address"
    index=$((index + 1))
  done < <(jq -er --arg address "${address}" '
    [.[] | select(.IPAddress == $address) | .name] | unique | sort | .[] |
    if test("^[A-Za-z0-9._-]+$") then .
    else error("forwarding rule name is not exact") end
  ' "${list_file}")
  rm "${list_file}"
  [[ "${index}" -gt 0 ]] || cma_projection_die "Gateway address has no global forwarding rule"
  jq -s '.' "${scratch_dir}"/*.json > "${output_file}" || \
    cma_projection_die "could not aggregate forwarding rules on the Gateway address"
}

cma_projection_capture_backend_health() {
  local backend_file="$1"
  local output_file="$2"
  local scratch_dir="$3"
  local backend name raw_file index=0

  mkdir -m 700 "${scratch_dir}"
  while IFS= read -r backend; do
    [[ -n "${backend}" ]] || continue
    name="${backend##*/}"
    raw_file="${scratch_dir}/$(printf '%04d' "${index}").raw.json"
    cma_projection_gcloud compute backend-services get-health "${name}" --global \
      --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
      > "${raw_file}" || cma_projection_die "could not capture backend health for ${name}"
    jq -e --arg backend "${backend}" '
      if type == "array" then .
      elif type == "object" and has("healthStatus") then [
        {backend:(.backend // error("backend health response lacks its group root")),status:.}
      ]
      else error("backend get-health response has an unsupported shape") end |
      {backendService:$backend,groups:.}
    ' "${raw_file}" > "${scratch_dir}/$(printf '%04d' "${index}").json" || \
      cma_projection_die "could not normalize backend health for ${name}"
    rm "${raw_file}"
    index=$((index + 1))
  done < <(jq -er '.[].selfLink' "${backend_file}")
  [[ "${index}" -gt 0 ]] || cma_projection_die "backend capture did not expose health roots"
  jq -s '.' "${scratch_dir}"/*.json > "${output_file}" || \
    cma_projection_die "could not aggregate backend health"
}

cma_projection_capture_ssl_policies() {
  local https_proxies="$1"
  local output_file="$2"
  local scratch_dir="$3"
  local ref index=0

  mkdir -m 700 "${scratch_dir}"
  while IFS= read -r ref; do
    [[ -n "${ref}" ]] || continue
    [[ "${ref}" =~ ^${CMA_GATEWAY_API_ROOT}/projects/${CMA_GATEWAY_PROJECT_NUMBER}/global/sslPolicies/[A-Za-z0-9._-]+$ ]] || \
      cma_projection_die "HTTPS proxy has an invalid SSL-policy reference"
    cma_projection_gcloud compute ssl-policies describe "${ref##*/}" --global \
      --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
      > "${scratch_dir}/$(printf '%04d' "${index}").json" || \
      cma_projection_die "could not capture SSL policy ${ref}"
    index=$((index + 1))
  done < <(jq -er '.[] | .sslPolicy // empty' "${https_proxies}")
  if [[ "${index}" -eq 0 ]]; then
    printf '%s\n' '[]' > "${output_file}"
  else
    jq -s '.' "${scratch_dir}"/*.json > "${output_file}" || \
      cma_projection_die "could not aggregate SSL-policy capture"
  fi
}

cma_projection_capture_negs() {
  local backend_file="$1"
  local output_file="$2"
  local scratch_dir="$3"
  local ref zone name index=0 resource_file endpoints_file

  mkdir -m 700 "${scratch_dir}"
  while IFS= read -r ref; do
    [[ -n "${ref}" ]] || continue
    if [[ ! "${ref}" =~ ^${CMA_GATEWAY_API_ROOT}/projects/${CMA_GATEWAY_PROJECT_NUMBER}/zones/(asia-northeast1-[a-z])/networkEndpointGroups/([A-Za-z0-9._-]+)$ ]]; then
      cma_projection_die "backend group is not an exact zonal NEG reference: ${ref}"
    fi
    zone="${BASH_REMATCH[1]}"
    name="${BASH_REMATCH[2]}"
    resource_file="${scratch_dir}/$(printf '%04d' "${index}").resource.json"
    endpoints_file="${scratch_dir}/$(printf '%04d' "${index}").endpoints.json"
    cma_projection_gcloud compute network-endpoint-groups describe "${name}" \
      --zone="${zone}" --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
      > "${resource_file}" || cma_projection_die "could not capture NEG ${zone}/${name}"
    cma_projection_gcloud compute network-endpoint-groups list-network-endpoints "${name}" \
      --zone="${zone}" --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
      > "${endpoints_file}" || cma_projection_die "could not capture NEG membership ${zone}/${name}"
    jq -e --slurpfile resource "${resource_file}" \
      --slurpfile endpoints "${endpoints_file}" \
      '$resource | length == 1 and $endpoints | length == 1 |
       {resource:$resource[0],endpoints:$endpoints[0]}' \
      > "${scratch_dir}/$(printf '%04d' "${index}").json" || \
      cma_projection_die "could not normalize NEG ${zone}/${name}"
    rm "${resource_file}" "${endpoints_file}"
    index=$((index + 1))
  done < <(jq -er '
    [.[].backends[]?.group] | unique | sort | .[] |
    if test("^https://www.googleapis.com/compute/v1/projects/191845086376/zones/asia-northeast1-[a-z]/networkEndpointGroups/[A-Za-z0-9._-]+$")
    then . else error("backend group is not a closed NEG self-link") end
  ' "${backend_file}")
  [[ "${index}" -gt 0 ]] || cma_projection_die "backend capture did not expose any NEG"
  jq -s '.' "${scratch_dir}"/*.json > "${output_file}" || \
    cma_projection_die "could not aggregate NEG capture"
}

cma_projection_capture_state() {
  local state_dir="$1"
  local capture_purpose="${2:-release}"
  local gateway_file="${state_dir}/gateway.json"
  local route_mode lifecycle_stage="${CMA_GATEWAY_PROJECTION_STAGE:-}"

  mkdir -m 700 "${state_dir}"
  cma_projection_kubectl "${CMA_GATEWAY_OWNER_NAMESPACE}" get gateway \
    chronoai-gateway -o json > "${gateway_file}" || \
    cma_projection_die "could not capture the shared Gateway"
  { set +x; } 2>/dev/null
  cma_projection_kubectl "${CMA_GATEWAY_OWNER_NAMESPACE}" get secret \
    chrono-ai-fun-cert -o json | \
    env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems \
      "${CMA_GATEWAY_TLS_SANITIZER}" "${state_dir}/certificate-secret.json" || \
    cma_projection_die "could not prove and sanitize the Gateway TLS private/public key binding"
  if [[ "${capture_purpose}" == review ]]; then
    cma_projection_kubectl_cluster get validatingadmissionpolicy \
      cma-cutover-fence-label-reservation --ignore-not-found -o json \
      > "${state_dir}/admission-policy.json" || \
      cma_projection_die "could not inspect the optional cutover-fence admission policy"
    cma_projection_kubectl_cluster get validatingadmissionpolicybinding \
      cma-cutover-fence-label-reservation --ignore-not-found -o json \
      > "${state_dir}/admission-binding.json" || \
      cma_projection_die "could not inspect the optional cutover-fence admission binding"
    [[ -s "${state_dir}/admission-policy.json" ]] || printf '%s\n' null \
      > "${state_dir}/admission-policy.json"
    [[ -s "${state_dir}/admission-binding.json" ]] || printf '%s\n' null \
      > "${state_dir}/admission-binding.json"
  else
    cma_projection_kubectl_cluster get validatingadmissionpolicy \
      cma-cutover-fence-label-reservation -o json \
      > "${state_dir}/admission-policy.json" || \
      cma_projection_die "could not capture the cutover-fence admission policy"
    cma_projection_kubectl_cluster get validatingadmissionpolicybinding \
      cma-cutover-fence-label-reservation -o json \
      > "${state_dir}/admission-binding.json" || \
      cma_projection_die "could not capture the cutover-fence admission binding"
  fi
  cma_projection_kubectl_cluster get httproutes.gateway.networking.k8s.io \
    --all-namespaces -o json > "${state_dir}/routes.json" || \
    cma_projection_die "could not capture all attachable HTTPRoutes"
  cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get healthcheckpolicy cma-api \
    -o json > "${state_dir}/health-policy.json" || \
    cma_projection_die "could not capture cma-api HealthCheckPolicy"
  cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get gcpbackendpolicy \
    cma-api cma-web -o json > "${state_dir}/backend-policies.json" || \
    cma_projection_die "could not capture CMA backend policies"
  cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get service cma-api cma-web \
    -o json > "${state_dir}/services.json" || \
    cma_projection_die "could not capture CMA Services"
  cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get deployment cma-api -o json | \
    jq '{
      apiVersion,kind,
      metadata:{name:.metadata.name,namespace:.metadata.namespace,uid:.metadata.uid,
                resourceVersion:.metadata.resourceVersion,generation:.metadata.generation,
                deletionTimestamp:.metadata.deletionTimestamp},
      spec,
      status:{observedGeneration:.status.observedGeneration,
              replicas:(.status.replicas // 0),updatedReplicas:(.status.updatedReplicas // 0),
              readyReplicas:(.status.readyReplicas // 0),
              availableReplicas:(.status.availableReplicas // 0),
              unavailableReplicas:(.status.unavailableReplicas // 0),
              collisionCount:(.status.collisionCount // 0)}
    }' > "${state_dir}/api-deployment-workload.json" || \
    cma_projection_die "could not capture the full cma-api Deployment workload proof"
  jq '{
      apiVersion,kind,
      metadata:{name:.metadata.name,namespace:.metadata.namespace,uid:.metadata.uid,
                resourceVersion:.metadata.resourceVersion,generation:.metadata.generation,
                deletionTimestamp:.metadata.deletionTimestamp},
      spec:{replicas:.spec.replicas},
      status:{observedGeneration:.status.observedGeneration,
              replicas:(.status.replicas // 0),updatedReplicas:(.status.updatedReplicas // 0),
              readyReplicas:(.status.readyReplicas // 0),
              availableReplicas:(.status.availableReplicas // 0),
              unavailableReplicas:(.status.unavailableReplicas // 0)}
    }' "${state_dir}/api-deployment-workload.json" \
    > "${state_dir}/api-deployment-scale.json" || \
    cma_projection_die "could not capture the cma-api Deployment scale root"
  if [[ "${lifecycle_stage}" == fenced-pre-reopen || \
        "${lifecycle_stage}" == final-reopen ]]; then
    [[ -n "${CMA_FINAL_REPLICA_SET:-}" && -n "${CMA_FINAL_POD:-}" ]] || \
      cma_projection_die "the retained ReplicaSet and Pod identities are required for reopen capture"
    cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get replicaset \
      "${CMA_FINAL_REPLICA_SET}" -o json | jq '{
        apiVersion,kind,
        metadata:{name:.metadata.name,namespace:.metadata.namespace,uid:.metadata.uid,
                  resourceVersion:.metadata.resourceVersion,generation:.metadata.generation,
                  deletionTimestamp:.metadata.deletionTimestamp,labels:.metadata.labels,
                  ownerReferences:.metadata.ownerReferences},
        spec,
        status:{observedGeneration:.status.observedGeneration,
                replicas:(.status.replicas // 0),fullyLabeledReplicas:(.status.fullyLabeledReplicas // 0),
                readyReplicas:(.status.readyReplicas // 0),
                availableReplicas:(.status.availableReplicas // 0)}
      }' > "${state_dir}/api-replicaset-workload.json" || \
      cma_projection_die "could not capture the retained cma-api ReplicaSet workload proof"
    cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get pod \
      "${CMA_FINAL_POD}" -o json | jq '{
        apiVersion,kind,
        metadata:{name:.metadata.name,namespace:.metadata.namespace,uid:.metadata.uid,
                  resourceVersion:.metadata.resourceVersion,
                  deletionTimestamp:.metadata.deletionTimestamp,labels:.metadata.labels,
                  ownerReferences:.metadata.ownerReferences},
        spec,
        status:{podIP:.status.podIP,phase:.status.phase,conditions:.status.conditions,
                containerStatuses:.status.containerStatuses}
      }' > "${state_dir}/api-pod-workload.json" || \
      cma_projection_die "could not capture the retained cma-api Pod workload proof"
  else
    printf '%s\n' null > "${state_dir}/api-replicaset-workload.json"
    printf '%s\n' null > "${state_dir}/api-pod-workload.json"
    printf '%s\n' null > "${state_dir}/api-deployment-workload.json"
  fi
  for service_name in cma-api cma-web; do
    cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get endpointslice \
      -l "kubernetes.io/service-name=${service_name}" -o json \
      > "${state_dir}/${service_name}-endpointslices.json" || \
      cma_projection_die "could not capture ${service_name} EndpointSlices"
    cma_projection_kubectl "${CMA_GATEWAY_NAMESPACE}" get pod \
      -l "app.kubernetes.io/name=${service_name}" -o json | \
      jq '{apiVersion,kind,metadata,items:[.items[] | {
        apiVersion,kind,
        metadata:{name:.metadata.name,namespace:.metadata.namespace,
                  uid:.metadata.uid,resourceVersion:.metadata.resourceVersion,
                  deletionTimestamp:.metadata.deletionTimestamp,labels:.metadata.labels},
        spec:{nodeName:.spec.nodeName},
        status:{podIP:.status.podIP,phase:.status.phase,conditions:.status.conditions,
                containerStatuses:.status.containerStatuses}
      }]}' > "${state_dir}/${service_name}-pods.json" || \
      cma_projection_die "could not capture ${service_name} Pod identities"
  done
  cma_projection_kubectl_cluster get nodes -o json | \
    jq '{apiVersion,kind,metadata,items:[.items[] | {
      apiVersion,kind,
      metadata:{name:.metadata.name,uid:.metadata.uid,
                resourceVersion:.metadata.resourceVersion},
      spec:{providerID:.spec.providerID},status:{addresses:.status.addresses}
    }]}' > "${state_dir}/nodes.json" || cma_projection_die "could not capture node provider identities"

  cma_projection_gcloud compute addresses describe chronoai-gateway-external-ip \
    --global --project="${CMA_GATEWAY_PROJECT_ID}" --format=json \
    > "${state_dir}/address.json" || cma_projection_die "could not capture the Gateway global address"
  cma_projection_capture_forwarding_rules_for_address "${state_dir}/address.json" \
    "${state_dir}/forwarding-rules.json" "${state_dir}/forwarding-rules"
  cma_projection_capture_ref_array "${gateway_file}" \
    networking.gke.io/target-http-proxies target-http-proxies \
    "${state_dir}/target-http-proxies.json" "${state_dir}/target-http-proxies"
  cma_projection_capture_ref_array "${gateway_file}" \
    networking.gke.io/target-https-proxies target-https-proxies \
    "${state_dir}/target-https-proxies.json" "${state_dir}/target-https-proxies"
  cma_projection_capture_ref_array "${gateway_file}" networking.gke.io/url-maps \
    url-maps "${state_dir}/url-maps.json" "${state_dir}/url-maps"
  cma_projection_capture_ref_array "${gateway_file}" networking.gke.io/ssl-certificates \
    ssl-certificates "${state_dir}/ssl-certificates.json" "${state_dir}/ssl-certificates"
  cma_projection_capture_ssl_policies "${state_dir}/target-https-proxies.json" \
    "${state_dir}/ssl-policies.json" "${state_dir}/ssl-policies"
  cma_projection_capture_ref_array "${gateway_file}" networking.gke.io/backend-services \
    backend-services "${state_dir}/backend-services.json" "${state_dir}/backend-services"
  cma_projection_capture_ref_array "${gateway_file}" networking.gke.io/health-checks \
    health-checks "${state_dir}/health-checks.json" "${state_dir}/health-checks"
  cma_projection_capture_ref_array "${gateway_file}" networking.gke.io/firewalls \
    firewalls "${state_dir}/firewalls.json" "${state_dir}/firewalls"
  cma_projection_capture_negs "${state_dir}/backend-services.json" \
    "${state_dir}/negs.json" "${state_dir}/negs"
  cma_projection_capture_backend_health "${state_dir}/backend-services.json" \
    "${state_dir}/backend-health.json" "${state_dir}/backend-health"

  route_mode="$(jq -er --slurpfile health "${state_dir}/health-policy.json" '
    ([.items[] | select(.kind == "HTTPRoute" and
      .metadata.namespace == "chrono-bot" and .metadata.name == "cma-web")]) as $web |
    ($health[0].spec.default.config.httpHealthCheck.requestPath // "") as $path |
    if ($web | length) != 1 then error("cma-web route is ambiguous")
    elif $path == "/livez" and
      ([$web[0].spec.rules[].backendRefs[]?.name] | unique) == ["cma-web"] then "legacy"
    elif $path == "/ready" and
      ([$web[0].spec.rules[].backendRefs[]?.name] | unique | sort) == ["cma-api","cma-web"] then "adopted"
    else error("route and health modes are not a stable legacy/adopted pair") end
  ' "${state_dir}/routes.json")" || \
    cma_projection_die "could not derive the exact route/health mode"

  jq -n --arg route_mode "${route_mode}" \
    --slurpfile gateway "${state_dir}/gateway.json" \
    --slurpfile certificate_secret "${state_dir}/certificate-secret.json" \
    --slurpfile admission_policy "${state_dir}/admission-policy.json" \
    --slurpfile admission_binding "${state_dir}/admission-binding.json" \
    --slurpfile routes "${state_dir}/routes.json" \
    --slurpfile health "${state_dir}/health-policy.json" \
    --slurpfile policies "${state_dir}/backend-policies.json" \
    --slurpfile services "${state_dir}/services.json" \
    --slurpfile api_deployment_scale "${state_dir}/api-deployment-scale.json" \
    --slurpfile api_deployment_workload "${state_dir}/api-deployment-workload.json" \
    --slurpfile api_replicaset_workload "${state_dir}/api-replicaset-workload.json" \
    --slurpfile api_pod_workload "${state_dir}/api-pod-workload.json" \
    --slurpfile api_slices "${state_dir}/cma-api-endpointslices.json" \
    --slurpfile web_slices "${state_dir}/cma-web-endpointslices.json" \
    --slurpfile api_pods "${state_dir}/cma-api-pods.json" \
    --slurpfile web_pods "${state_dir}/cma-web-pods.json" \
    --slurpfile nodes "${state_dir}/nodes.json" \
    --slurpfile address "${state_dir}/address.json" \
    --slurpfile forwarding "${state_dir}/forwarding-rules.json" \
    --slurpfile http_proxy "${state_dir}/target-http-proxies.json" \
    --slurpfile https_proxy "${state_dir}/target-https-proxies.json" \
    --slurpfile maps "${state_dir}/url-maps.json" \
    --slurpfile certificates "${state_dir}/ssl-certificates.json" \
    --slurpfile ssl_policies "${state_dir}/ssl-policies.json" \
    --slurpfile backends "${state_dir}/backend-services.json" \
    --slurpfile backend_health "${state_dir}/backend-health.json" \
    --slurpfile health_checks "${state_dir}/health-checks.json" \
    --slurpfile firewalls "${state_dir}/firewalls.json" \
    --slurpfile negs "${state_dir}/negs.json" '
      {
        routeMode:$route_mode,
        kubernetes:{
          gateway:$gateway[0],certificateSecret:$certificate_secret[0],
          admissionPolicy:$admission_policy[0],admissionBinding:$admission_binding[0],
          routes:$routes[0],healthPolicy:$health[0],
          backendPolicies:$policies[0],services:$services[0],
          apiDeploymentScale:$api_deployment_scale[0],
          apiDeploymentWorkload:$api_deployment_workload[0],
          apiReplicaSetWorkload:$api_replicaset_workload[0],
          apiPodWorkload:$api_pod_workload[0],
          endpointSlices:{"cma-api":$api_slices[0],"cma-web":$web_slices[0]},
          pods:{"cma-api":$api_pods[0],"cma-web":$web_pods[0]},nodes:$nodes[0]
        },
        compute:{
          address:$address[0],forwardingRules:$forwarding[0],
          targetHttpProxies:$http_proxy[0],targetHttpsProxies:$https_proxy[0],
          urlMaps:$maps[0],sslCertificates:$certificates[0],sslPolicies:$ssl_policies[0],
          backendServices:$backends[0],backendHealth:$backend_health[0],
          healthChecks:$health_checks[0],firewalls:$firewalls[0],
          networkEndpointGroups:$negs[0]
        }
      }
    ' > "${state_dir}/state.json" || cma_projection_die "could not assemble closed state capture"
}

cma_projection_capture() {
  local output_dir="$1"
  local mode="$2"
  local parent_dir work_dir projection_sha256 admission_receipt lifecycle

  [[ "${output_dir}" == /* && "${output_dir}" != / ]] || \
    cma_projection_die "capture directory must be an absolute non-root path"
  [[ "${mode}" == source || "${mode}" == fenced ]] || \
    cma_projection_die "capture mode must be source or fenced"
  parent_dir="${output_dir%/*}"
  [[ -d "${parent_dir}" && ! -L "${parent_dir}" && -O "${parent_dir}" ]] || \
    cma_projection_die "capture parent must be an owned existing non-symlink directory"
  lifecycle="$(cma_projection_lifecycle "${mode}")" || \
    cma_projection_die "projection lifecycle stage/predecessor binding is invalid"
  if [[ -d "${output_dir}" && ! -L "${output_dir}" ]]; then
    cma_projection_validate_published_capture \
      "${output_dir}" "${mode}" "${lifecycle}" || \
      cma_projection_die "existing capture directory is partial, tampered, or not reusable"
    cma_projection_check_live "${output_dir}/projection.json" >/dev/null || \
      cma_projection_die "existing capture no longer equals live state"
    cma_projection_sync_published_directory "${output_dir}" "${parent_dir}" || \
      cma_projection_die "existing projection publication could not be durably re-synced"
    projection_sha256="$(cma_projection_sha256_file "${output_dir}/projection.json")"
    printf 'CMA Gateway projection directory: %s\n' "${output_dir}"
    printf 'CMA Gateway projection SHA-256: %s\n' "${projection_sha256}"
    return 0
  fi
  [[ ! -e "${output_dir}" && ! -L "${output_dir}" ]] || \
    cma_projection_die "capture destination is occupied by a non-directory or symlink"
  admission_receipt="$(cma_projection_admission_receipt)" || \
    cma_projection_die "admission bootstrap receipt binding could not be read"
  work_dir="$(mktemp -d "${parent_dir}/.cma-gateway-projection.XXXXXXXX")" || \
    cma_projection_die "capture work directory could not be created"
  chmod 700 "${work_dir}"
  trap 'rm -rf "${work_dir}"' EXIT HUP INT QUIT TERM

  cma_projection_capture_state "${work_dir}/first"
  cma_projection_capture_state "${work_dir}/second"
  jq -n --arg mode "${mode}" --argjson admission "${admission_receipt}" \
    --argjson lifecycle "${lifecycle}" \
    --slurpfile first "${work_dir}/first/state.json" \
    --slurpfile second "${work_dir}/second/state.json" \
    '{schemaVersion:1,mode:$mode,lifecycle:$lifecycle,
      admissionReceipt:$admission,first:$first[0],second:$second[0]}' \
    > "${work_dir}/capture.json" || cma_projection_die "capture receipt could not be assembled"
  cma_projection_emit "${work_dir}/capture.json" "${work_dir}/projection.json" || \
    cma_projection_die "live routing graph is incomplete, unstable, or outside the closed contract"
  rm -rf "${work_dir}/first" "${work_dir}/second"
  projection_sha256="$(cma_projection_sha256_file "${work_dir}/projection.json")" || \
    cma_projection_die "projection receipt could not be hashed"
  [[ "${projection_sha256}" =~ ^[0-9a-f]{64}$ ]] || \
    cma_projection_die "projection receipt digest is malformed"
  printf '%s  projection.json\n' "${projection_sha256}" > "${work_dir}/projection.sha256" || \
    cma_projection_die "projection digest receipt could not be written"
  chmod 400 "${work_dir}/capture.json" "${work_dir}/projection.json" \
    "${work_dir}/projection.sha256" || cma_projection_die "projection receipt permissions failed"
  cma_projection_fsync "${work_dir}/capture.json" "${work_dir}/projection.json" \
    "${work_dir}/projection.sha256" "${work_dir}" "${parent_dir}" || \
    cma_projection_die "projection receipt could not be durably fsynced"
  cma_projection_validate_published_capture \
    "${work_dir}" "${mode}" "${lifecycle}" || \
    cma_projection_die "projection receipt failed its closed pre-publication validation"
  if cma_projection_publish_directory "${work_dir}" "${output_dir}"; then
    cma_projection_sync_published_directory "${output_dir}" "${parent_dir}" || \
      cma_projection_die "projection publication directory entry could not be durably synced"
  else
    cma_projection_validate_published_capture \
      "${output_dir}" "${mode}" "${lifecycle}" || \
      cma_projection_die "projection publication raced with a non-equivalent destination"
    rm -rf "${work_dir}"
    cma_projection_sync_published_directory "${output_dir}" "${parent_dir}" || \
      cma_projection_die "concurrent projection publication could not be durably re-synced"
    cma_projection_check_live "${output_dir}/projection.json" >/dev/null || \
      cma_projection_die "the concurrently published projection does not equal live state"
    projection_sha256="$(cma_projection_sha256_file "${output_dir}/projection.json")" || \
      cma_projection_die "the concurrently published projection could not be hashed"
  fi
  trap - EXIT HUP INT QUIT TERM
  printf 'CMA Gateway projection directory: %s\n' "${output_dir}" || \
    cma_projection_die "projection path acknowledgement failed"
  printf 'CMA Gateway projection SHA-256: %s\n' "${projection_sha256}" || \
    cma_projection_die "projection digest acknowledgement failed"
}

cma_projection_validate_published_capture() {
  local capture_dir="$1"
  local expected_mode="$2"
  local expected_lifecycle="$3"
  local entry_count recorded_digest live_digest work_dir

  [[ -d "${capture_dir}" && ! -L "${capture_dir}" ]] || return 1
  entry_count="$(find "${capture_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" || return 1
  [[ "${entry_count}" == 3 ]] || return 1
  for entry in capture.json projection.json projection.sha256; do
    [[ -f "${capture_dir}/${entry}" && ! -L "${capture_dir}/${entry}" ]] || return 1
  done
  [[ "$(jq -er .mode "${capture_dir}/projection.json")" == "${expected_mode}" ]] || return 1
  jq -se --argjson expected "${expected_lifecycle}" '
    length == 2 and all(.[]; .lifecycle == $expected)
  ' "${capture_dir}/capture.json" "${capture_dir}/projection.json" >/dev/null || return 1
  recorded_digest="$(awk 'NF == 2 && $2 == "projection.json" {print $1}' \
    "${capture_dir}/projection.sha256")" || return 1
  [[ "$(wc -l < "${capture_dir}/projection.sha256" | tr -d ' ')" == 1 ]] || return 1
  live_digest="$(cma_projection_sha256_file "${capture_dir}/projection.json")" || return 1
  [[ "${recorded_digest}" =~ ^[0-9a-f]{64}$ && "${recorded_digest}" == "${live_digest}" ]] || return 1
  work_dir="$(mktemp -d)" || return 1
  cma_projection_emit "${capture_dir}/capture.json" "${work_dir}/projection.json" && \
    cmp "${work_dir}/projection.json" "${capture_dir}/projection.json" >/dev/null
  local status=$?
  rm -rf "${work_dir}"
  return "${status}"
}

cma_projection_derive_policy() {
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems \
    "${CMA_GATEWAY_PROJECTION_CONTRACT}" --derive-policy "$1" "$2" >/dev/null
}

cma_projection_publish_directory() {
  local source_dir="$1"
  local destination_dir="$2"

  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    source, destination = ARGV
    raise "destination exists" if File.exist?(destination) || File.symlink?(destination)
    File.rename(source, destination)
  ' "${source_dir}" "${destination_dir}"
}

cma_projection_sync_published_directory() {
  local published_dir="$1"
  local parent_dir="$2"

  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    published, parent = ARGV
    raise "published path is not an exact directory" unless
      File.directory?(published) && !File.symlink?(published)
    Dir.children(published).sort.each do |entry|
      path = File.join(published, entry)
      stat = File.lstat(path)
      raise "published entry is not a regular file" unless stat.file? && !stat.symlink?
      File.open(path, File::RDONLY) { |file| file.fsync }
    end
    File.open(published, File::RDONLY) { |directory| directory.fsync }
    File.open(parent, File::RDONLY) { |directory| directory.fsync }
  ' "${published_dir}" "${parent_dir}"
}

cma_projection_validate_review_capture() {
  local review_dir="$1"
  local capture_file="${review_dir}/review-capture.json"
  local candidate_file="${review_dir}/candidate-policy.json"
  local digest_file="${review_dir}/review.sha256"
  local work_dir reconstructed candidate digest expected_digest entry_count

  [[ -d "${review_dir}" && ! -L "${review_dir}" ]] || return 1
  entry_count="$(find "${review_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" || return 1
  [[ "${entry_count}" == 3 ]] || return 1
  for candidate in "${capture_file}" "${candidate_file}" "${digest_file}"; do
    [[ -f "${candidate}" && ! -L "${candidate}" ]] || return 1
  done
  jq -e --slurpfile policy "${candidate_file}" '
    .schemaVersion == 1 and .kind == "CmaGatewayBehaviorPolicyReviewCapture" and
    .nonConsumable == true and .mode == "source" and
    (.first | type) == "object" and (.second | type) == "object" and
    .first == .second and ($policy | length) == 1 and
    $policy[0].status == "candidate-requires-maintainer-review" and
    .rawEvidenceSha256 == $policy[0].sourceEvidenceSha256 and
    (.rawEvidenceSha256 | test("^[0-9a-f]{64}$")) and
    (.candidatePolicySha256 | test("^[0-9a-f]{64}$"))
  ' "${capture_file}" >/dev/null || return 1
  digest="$(cma_projection_sha256_file "${candidate_file}")" || return 1
  expected_digest="$(jq -er .candidatePolicySha256 "${capture_file}")" || return 1
  [[ "${digest}" == "${expected_digest}" ]] || return 1
  while read -r digest candidate; do
    [[ "${digest}" =~ ^[0-9a-f]{64}$ && \
      ( "${candidate}" == review-capture.json || "${candidate}" == candidate-policy.json ) ]] || return 1
    [[ "$(cma_projection_sha256_file "${review_dir}/${candidate}")" == "${digest}" ]] || return 1
  done < "${digest_file}"
  [[ "$(wc -l < "${digest_file}" | tr -d ' ')" == 2 ]] || return 1
  work_dir="$(mktemp -d)" || return 1
  reconstructed="${work_dir}/capture.json"
  candidate="${work_dir}/candidate.json"
  jq '{schemaVersion:1,mode:.mode,first,second}' "${capture_file}" > "${reconstructed}" || {
    rm -rf "${work_dir}"
    return 1
  }
  cma_projection_derive_policy "${reconstructed}" "${candidate}" && \
    cmp "${candidate}" "${candidate_file}" >/dev/null
  local status=$?
  rm -rf "${work_dir}"
  return "${status}"
}

cma_projection_capture_review() {
  local output_dir="$1"
  local parent_dir work_dir raw_capture candidate_policy review_capture
  local candidate_digest review_digest

  [[ "${output_dir}" == /* && "${output_dir}" != / ]] || \
    cma_projection_die "review capture directory must be an absolute non-root path"
  parent_dir="${output_dir%/*}"
  [[ -d "${parent_dir}" && ! -L "${parent_dir}" && -O "${parent_dir}" ]] || \
    cma_projection_die "review capture parent must be an owned existing directory"
  [[ ! -e "${output_dir}" && ! -L "${output_dir}" ]] || \
    cma_projection_die "review capture directory already exists"
  work_dir="$(mktemp -d "${parent_dir}/.cma-gateway-review.XXXXXXXX")" || \
    cma_projection_die "review capture work directory could not be created"
  chmod 700 "${work_dir}"
  trap 'rm -rf "${work_dir}"' EXIT HUP INT QUIT TERM
  cma_projection_capture_state "${work_dir}/first" review
  cma_projection_capture_state "${work_dir}/second" review
  raw_capture="${work_dir}/capture.json"
  candidate_policy="${work_dir}/candidate-policy.json"
  review_capture="${work_dir}/review-capture.json"
  jq -n --slurpfile first "${work_dir}/first/state.json" \
    --slurpfile second "${work_dir}/second/state.json" \
    '{schemaVersion:1,mode:"source",first:$first[0],second:$second[0]}' \
    > "${raw_capture}" || cma_projection_die "review raw capture could not be assembled"
  cma_projection_derive_policy "${raw_capture}" "${candidate_policy}" || \
    cma_projection_die "two stable reads could not derive a candidate behavior policy"
  candidate_digest="$(cma_projection_sha256_file "${candidate_policy}")" || \
    cma_projection_die "candidate behavior policy could not be hashed"
  review_digest="$(jq -er .sourceEvidenceSha256 "${candidate_policy}")" || \
    cma_projection_die "candidate behavior evidence digest could not be read"
  jq -n --arg evidence "${review_digest}" --arg candidate "${candidate_digest}" \
    --slurpfile first "${work_dir}/first/state.json" \
    --slurpfile second "${work_dir}/second/state.json" '{
      schemaVersion:1,kind:"CmaGatewayBehaviorPolicyReviewCapture",nonConsumable:true,
      mode:"source",rawEvidenceSha256:$evidence,candidatePolicySha256:$candidate,
      first:$first[0],second:$second[0]
    }' > "${review_capture}" || cma_projection_die "review capture receipt could not be assembled"
  rm -rf "${work_dir}/first" "${work_dir}/second" "${raw_capture}"
  printf '%s  review-capture.json\n' "$(cma_projection_sha256_file "${review_capture}")" \
    > "${work_dir}/review.sha256"
  printf '%s  candidate-policy.json\n' "${candidate_digest}" >> "${work_dir}/review.sha256"
  chmod 400 "${review_capture}" "${candidate_policy}" "${work_dir}/review.sha256"
  cma_projection_fsync "${review_capture}" "${candidate_policy}" \
    "${work_dir}/review.sha256" "${work_dir}" "${parent_dir}" || \
    cma_projection_die "review evidence could not be durably fsynced"
  cma_projection_validate_review_capture "${work_dir}" || \
    cma_projection_die "review evidence failed its closed self-validation"
  cma_projection_publish_directory "${work_dir}" "${output_dir}" || \
    cma_projection_die "review evidence could not be atomically published"
  cma_projection_sync_published_directory "${output_dir}" "${parent_dir}" || \
    cma_projection_die "review evidence publication could not be durably synced"
  trap - EXIT HUP INT QUIT TERM
  printf 'CMA Gateway non-consumable review capture: %s\n' "${output_dir}"
  printf 'CMA Gateway candidate behavior policy SHA-256: %s\n' "${candidate_digest}"
}

cma_projection_check_live() {
  local receipt_file="$1"
  local mode work_dir candidate_capture candidate_projection admission_receipt lifecycle

  [[ -f "${receipt_file}" && ! -L "${receipt_file}" ]] || \
    cma_projection_die "reviewed projection receipt is not a regular non-symlink file"
  mode="$(jq -er 'if .schemaVersion == 1 and (.mode == "source" or .mode == "fenced")
                   then .mode else error("projection receipt mode is invalid") end' \
    "${receipt_file}")" || cma_projection_die "projection receipt mode could not be read"
  admission_receipt="$(cma_projection_admission_receipt)" || \
    cma_projection_die "admission bootstrap receipt binding could not be read"
  lifecycle="$(jq -cer '
    if .lifecycle.stage == "source" and .lifecycle.predecessorSha256 == "genesis" then .lifecycle
    elif (.lifecycle.stage == "adopted" or .lifecycle.stage == "final-reopen" or
          .lifecycle.stage == "rollback-source" or
          .lifecycle.stage == "fenced-pre-scale" or .lifecycle.stage == "fenced-post-scale" or
          .lifecycle.stage == "fenced-pre-reopen") and
         (.lifecycle.predecessorSha256 | test("^[0-9a-f]{64}$")) then .lifecycle
    else error("projection lifecycle binding is invalid") end
  ' "${receipt_file}")" || cma_projection_die "projection lifecycle binding could not be read"
  CMA_GATEWAY_PROJECTION_STAGE="$(printf '%s' "${lifecycle}" | jq -er .stage)" || \
    cma_projection_die "projection lifecycle stage could not be selected for live comparison"
  export CMA_GATEWAY_PROJECTION_STAGE
  work_dir="$(mktemp -d)" || cma_projection_die "live comparison work directory could not be created"
  [[ -n "${work_dir}" && -d "${work_dir}" && ! -L "${work_dir}" ]] || \
    cma_projection_die "live comparison work directory is not exact"
  chmod 700 "${work_dir}"
  trap 'rm -rf "${work_dir}"' EXIT HUP INT QUIT TERM
  cma_projection_capture_state "${work_dir}/first"
  cma_projection_capture_state "${work_dir}/second"
  candidate_capture="${work_dir}/capture.json"
  candidate_projection="${work_dir}/projection.json"
  jq -n --arg mode "${mode}" --argjson admission "${admission_receipt}" \
    --argjson lifecycle "${lifecycle}" \
    --slurpfile first "${work_dir}/first/state.json" \
    --slurpfile second "${work_dir}/second/state.json" \
    '{schemaVersion:1,mode:$mode,lifecycle:$lifecycle,
      admissionReceipt:$admission,first:$first[0],second:$second[0]}' \
    > "${candidate_capture}" || cma_projection_die "live comparison capture could not be assembled"
  cma_projection_emit "${candidate_capture}" "${candidate_projection}" || \
    cma_projection_die "current routing graph is incomplete or unstable"
  jq -e --slurpfile candidate "${candidate_projection}" '
    .schemaVersion == 1 and .mode == $candidate[0].mode and
    .lifecycle == $candidate[0].lifecycle and
    .captureSha256 == $candidate[0].captureSha256 and
    .behaviorPolicy == $candidate[0].behaviorPolicy and
    .admissionReceipt == $candidate[0].admissionReceipt and
    .graph == $candidate[0].graph and .fullCapture == $candidate[0].fullCapture
  ' "${receipt_file}" >/dev/null || \
    cma_projection_die "current routing graph differs from the reviewed closed projection"
  rm -rf "${work_dir}"
  trap - EXIT HUP INT QUIT TERM
  printf '%s\n' "live Gateway graph exactly matches the reviewed closed projection"
}

cma_projection_usage() {
  cat >&2 <<'USAGE'
usage:
  cma-trigger-gateway-projection.sh --capture ABSOLUTE_DIR source|fenced
  cma-trigger-gateway-projection.sh --capture-review ABSOLUTE_DIR
  cma-trigger-gateway-projection.sh --validate-review ABSOLUTE_DIR
  cma-trigger-gateway-projection.sh --check-live PROJECTION.json
  cma-trigger-gateway-projection.sh --validate CAPTURE.json
  cma-trigger-gateway-projection.sh --self-test

Capture is read-only but requires the reviewed env-i launcher and the complete
temporary Compute viewer role. The production mutation constant remains 0.
USAGE
}

cma_projection_main() {
case "${1:-}" in
  --capture-review)
    [[ "$#" -eq 2 ]] || cma_projection_die "--capture-review requires one absolute directory"
    cma_projection_require_trusted_launcher
    cma_projection_require_tools
    cma_projection_capture_review "$2"
    ;;
  --validate-review)
    [[ "$#" -eq 2 ]] || cma_projection_die "--validate-review requires one review directory"
    cma_projection_require_tools
    cma_projection_validate_review_capture "$2" || \
      cma_projection_die "non-consumable review capture is invalid"
    printf '%s\n' "CMA Gateway non-consumable review capture is exact"
    ;;
  --capture)
    [[ "$#" -eq 3 ]] || cma_projection_die "--capture requires an absolute directory and mode"
    cma_projection_require_trusted_launcher
    cma_projection_require_tools
    cma_projection_capture "$2" "$3"
    ;;
  --validate)
    [[ "$#" -eq 2 ]] || cma_projection_die "--validate requires one capture file"
    env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems \
      "${CMA_GATEWAY_PROJECTION_CONTRACT}" --validate "$2"
    ;;
  --check-live)
    [[ "$#" -eq 2 ]] || cma_projection_die "--check-live requires one projection receipt"
    cma_projection_require_trusted_launcher
    cma_projection_require_tools
    cma_projection_check_live "$2"
    ;;
  --self-test)
    [[ "$#" -eq 1 ]] || cma_projection_die "--self-test takes no arguments"
    env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems \
      "${CMA_GATEWAY_PROJECTION_CONTRACT}" --self-test
    ;;
  *)
    cma_projection_usage
    exit 2
    ;;
esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  cma_projection_main "$@"
fi
