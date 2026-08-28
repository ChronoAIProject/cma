#!/usr/bin/env bash
set -euo pipefail

# Read-only, fail-closed verification for the production Gateway resources used
# by CMA Trigger. Mutation and rollback stay in the release runbook; this
# script gives every stage one executable definition of "the route is ready".

readonly CMA_GATEWAY_CONTEXT="gke_example-project-02_asia-northeast1_example-cluster-02"
readonly CMA_GATEWAY_NAMESPACE="chrono-bot"
readonly CMA_GATEWAY_NAME="chronoai-gateway"
readonly CMA_GATEWAY_OWNER_NAMESPACE="gateway-infra"
readonly CMA_GATEWAY_PROJECT_ID="example-project-02"
readonly CMA_GATEWAY_PROJECT_NUMBER="191845086376"
readonly CMA_COMPUTE_SELF_LINK="https://www.googleapis.com/compute/v1"

cma_gateway_die() {
  printf 'cma-trigger Gateway state check: %s\n' "$*" >&2
  exit 1
}

cma_gateway_require_tools() {
  local command_name
  for command_name in jq kubectl gcloud mktemp timeout; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_gateway_die "required command is unavailable: ${command_name}"
  done
}

cma_gateway_kubectl() {
  local resource_namespace="$1"
  shift
  timeout --signal=TERM --kill-after=5s 25s \
    kubectl --request-timeout=15s --context "${CMA_GATEWAY_CONTEXT}" \
      --namespace "${resource_namespace}" "$@"
}

cma_gateway_gcloud() {
  # HOME intentionally selects the operator's default Cloud SDK configuration
  # and its active-config file. No ambient Cloud SDK override reaches gcloud.
  timeout --signal=TERM --kill-after=5s 25s \
    env -i \
      HOME="${HOME}" \
      PATH="${PATH}" \
      LC_ALL=C \
      CLOUDSDK_CORE_HTTP_TIMEOUT=15 \
      gcloud "$@"
}

cma_gateway_validate_owned_metadata() {
  local resource_file="$1"
  local expected_labels="$2"

  jq -e --argjson expected_labels "${expected_labels}" '
    (.metadata.annotations // {}) as $annotations |
    ((keys - ["apiVersion","kind","metadata","spec","status"]) | length) == 0 and
    (((.metadata | keys) - [
      "annotations","creationTimestamp","finalizers","generation","labels",
      "managedFields","name","namespace","ownerReferences","resourceVersion","uid"
    ]) | length) == 0 and
    (.metadata.uid | type) == "string" and
    (.metadata.uid | length) > 0 and
    (.metadata.resourceVersion | type) == "string" and
    (.metadata.resourceVersion | length) > 0 and
    .metadata.deletionTimestamp == null and
    (.metadata.creationTimestamp == null or
     (.metadata.creationTimestamp | type) == "string") and
    (.metadata.generation == null or (.metadata.generation | type) == "number") and
    (.metadata.managedFields == null or (.metadata.managedFields | type) == "array") and
    (.status == null or (.status | type) == "object") and
    (.metadata.labels // {}) == $expected_labels and
    (($annotations | keys) -
      ["kubectl.kubernetes.io/last-applied-configuration"] | length) == 0 and
    ($annotations["kubectl.kubernetes.io/last-applied-configuration"] == null or
     ($annotations["kubectl.kubernetes.io/last-applied-configuration"] | type) ==
       "string") and
    (.metadata.finalizers // []) == [] and
    (.metadata.ownerReferences // []) == []
  ' "${resource_file}" >/dev/null || return 1
}

cma_gateway_route_ready() {
  local route_file="$1"
  local route_name="$2"
  local section_name="$3"

  jq -e \
    --arg route_name "${route_name}" \
    --arg section_name "${section_name}" '
      def parent($section): {
        group:"gateway.networking.k8s.io",
        kind:"Gateway",
        name:"chronoai-gateway",
        namespace:"gateway-infra",
        sectionName:$section
      };
      .apiVersion == "gateway.networking.k8s.io/v1" and
      .kind == "HTTPRoute" and
      .metadata.name == $route_name and
      .metadata.namespace == "chrono-bot" and
      .metadata.generation as $generation |
      ([.status.parents[]? |
        select(.controllerName == "networking.gke.io/gateway" and
               .parentRef == parent($section_name))] | length) == 1 and
      ([.status.parents[]? |
        select(.controllerName == "networking.gke.io/gateway" and
               .parentRef == parent($section_name)) |
        .conditions[]? |
        select((.type == "Accepted" or
                .type == "ResolvedRefs" or
                .type == "Reconciled") and
               .status == "True" and
               .observedGeneration == $generation) |
        .type] | unique | sort) ==
          ["Accepted", "Reconciled", "ResolvedRefs"]
    ' "${route_file}" >/dev/null || return 1
}

cma_gateway_validate_api_route() {
  local route_file="$1"

  cma_gateway_validate_owned_metadata \
    "${route_file}" \
    '{"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"}' || \
    return 1
  jq -e '
    def backend: [{group:"",kind:"Service",name:"cma-api",port:80,weight:1}];
    .spec == {
      parentRefs:[{
        group:"gateway.networking.k8s.io",
        kind:"Gateway",
        name:"chronoai-gateway",
        namespace:"gateway-infra",
        sectionName:"https"
      }],
      hostnames:["bot-api.chrono-ai.fun"],
      rules:[{
        matches:[{path:{type:"PathPrefix",value:"/"}}],
        backendRefs:backend
      }]
    }
  ' "${route_file}" >/dev/null || return 1
  cma_gateway_route_ready "${route_file}" cma-api https
}

cma_gateway_validate_web_route() {
  local route_file="$1"
  local expected_mode="$2"

  cma_gateway_validate_owned_metadata \
    "${route_file}" \
    '{"app.kubernetes.io/name":"cma-web","app.kubernetes.io/part-of":"cma"}' || \
    return 1
  jq -e --arg expected_mode "${expected_mode}" '
    def api_backend: [{group:"",kind:"Service",name:"cma-api",port:80,weight:1}];
    def web_backend: [{group:"",kind:"Service",name:"cma-web",port:80,weight:1}];
    def parent: [{
      group:"gateway.networking.k8s.io",
      kind:"Gateway",
      name:"chronoai-gateway",
      namespace:"gateway-infra",
      sectionName:"https"
    }];
    def legacy_rules: [{
      matches:[{path:{type:"PathPrefix",value:"/"}}],
      backendRefs:web_backend
    }];
    def adopted_rules: [
      {
        matches:[
          {path:{type:"PathPrefix",value:"/api"}},
          {path:{type:"PathPrefix",value:"/auth"}},
          {path:{type:"Exact",value:"/openapi.json"}},
          {path:{type:"Exact",value:"/.well-known/agent-card.json"}}
        ],
        backendRefs:api_backend
      },
      {
        matches:[{path:{type:"PathPrefix",value:"/"}}],
        backendRefs:web_backend
      }
    ];
    .spec == {
      parentRefs:parent,
      hostnames:["bot.chrono-ai.fun"],
      rules:(if $expected_mode == "legacy" then legacy_rules
             elif $expected_mode == "adopted" then adopted_rules
             else null end)
    }
  ' "${route_file}" >/dev/null || return 1
  cma_gateway_route_ready "${route_file}" cma-web https
}

cma_gateway_validate_redirect_route() {
  local route_file="$1"

  cma_gateway_validate_owned_metadata \
    "${route_file}" '{"app.kubernetes.io/part-of":"cma"}' || return 1
  jq -e '
    .spec == {
      parentRefs:[{
        group:"gateway.networking.k8s.io",
        kind:"Gateway",
        name:"chronoai-gateway",
        namespace:"gateway-infra",
        sectionName:"http"
      }],
      hostnames:["bot.chrono-ai.fun", "bot-api.chrono-ai.fun"],
      rules:[{
        matches:[{path:{type:"PathPrefix",value:"/"}}],
        filters:[{
          type:"RequestRedirect",
          requestRedirect:{scheme:"https",statusCode:301}
        }]
      }]
    }
  ' "${route_file}" >/dev/null || return 1
  cma_gateway_route_ready "${route_file}" cma-http-redirect http
}

cma_gateway_validate_health_policy() {
  local policy_file="$1"
  local expected_path="$2"

  cma_gateway_validate_owned_metadata \
    "${policy_file}" \
    '{"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"}' || \
    return 1
  jq -e --arg expected_path "${expected_path}" '
    .apiVersion == "networking.gke.io/v1" and
    .kind == "HealthCheckPolicy" and
    .metadata.name == "cma-api" and
    .metadata.namespace == "chrono-bot" and
    .spec == {
      targetRef:{group:"",kind:"Service",name:"cma-api"},
      default:{config:{
        type:"HTTP",
        httpHealthCheck:{port:8080,requestPath:$expected_path}
      }}
    } and
    ([.status.conditions[]? |
      select(.type == "Attached" and .status == "True" and
             .reason == "Attached")] | length) == 1
  ' "${policy_file}" >/dev/null || return 1
}

cma_gateway_validate_backend_policy() {
  local policy_file="$1"
  local service_name="$2"

  jq -e --arg service_name "${service_name}" '
    .apiVersion == "networking.gke.io/v1" and
    .kind == "GCPBackendPolicy" and
    .metadata.name == $service_name and
    .metadata.namespace == "chrono-bot" and
    .spec == {
      targetRef:{group:"",kind:"Service",name:$service_name},
      default:{timeoutSec:660}
    } and
    ([.status.conditions[]? |
      select(.type == "Attached" and .status == "True" and
             .reason == "Attached")] | length) == 1
  ' "${policy_file}" >/dev/null || return 1
}

cma_gateway_resource_reference() {
  local gateway_file="$1"
  local annotation_name="$2"
  local resource_kind="$3"
  local service_name="$4"

  jq -er \
    --arg annotation_name "${annotation_name}" \
    --arg project_number "${CMA_GATEWAY_PROJECT_NUMBER}" \
    --arg resource_kind "${resource_kind}" \
    --arg service_marker "-chrono-bot-${service_name}-80-" '
      (.metadata.annotations[$annotation_name] // "" |
       split(",") |
       map(gsub("^ +| +$"; "")) |
       map(select(
         startswith("/projects/" + $project_number + "/global/" +
                    $resource_kind + "/gkegw1-") and
         contains($service_marker)
       ))) as $matches |
      if ($matches | length) == 1 then $matches[0]
      else error("expected exactly one owned Gateway resource reference")
      end
    ' "${gateway_file}"
}

cma_gateway_neg_name() {
  local service_file="$1"
  local service_name="$2"

  jq -er --arg service_name "${service_name}" '
    if .metadata.name == $service_name and
       .metadata.namespace == "chrono-bot" and
       .spec.ports == [{name:"http",port:80,protocol:"TCP",targetPort:"http"}]
    then
      (.metadata.annotations["cloud.google.com/neg-status"] | fromjson) as $neg |
      ($neg.network_endpoint_groups["80"] // "") as $name |
      ($neg.zones // []) as $zones |
      if ($name | test("^k8s1-[a-z0-9]+-chrono-bot-" + $service_name +
                       "-80-[a-z0-9]+$")) and
         ($zones | type) == "array" and ($zones | length) > 0 and
         ($zones | all(test("^asia-northeast1-[a-z]$")))
      then $name
      else error("Service NEG identity is not exact")
      end
    else error("Service port or identity is not exact")
    end
  ' "${service_file}"
}

cma_gateway_validate_effective_state() {
  local state_dir="$1"
  local expected_health_path="$2"
  local api_health_ref api_backend_ref web_backend_ref
  local api_neg_name web_neg_name

  api_health_ref="$(cma_gateway_resource_reference \
    "${state_dir}/gateway.json" "networking.gke.io/health-checks" \
    healthChecks cma-api)" || return 1
  api_backend_ref="$(cma_gateway_resource_reference \
    "${state_dir}/gateway.json" "networking.gke.io/backend-services" \
    backendServices cma-api)" || return 1
  web_backend_ref="$(cma_gateway_resource_reference \
    "${state_dir}/gateway.json" "networking.gke.io/backend-services" \
    backendServices cma-web)" || return 1
  api_neg_name="$(cma_gateway_neg_name \
    "${state_dir}/cma-api-service.json" cma-api)" || return 1
  web_neg_name="$(cma_gateway_neg_name \
    "${state_dir}/cma-web-service.json" cma-web)" || return 1

  jq -e \
    --arg self_link "${CMA_COMPUTE_SELF_LINK}${api_health_ref}" \
    --arg resource_name "${api_health_ref##*/}" \
    --arg expected_path "${expected_health_path}" '
      .selfLink == $self_link and
      .name == $resource_name and
      .type == "HTTP" and
      .httpHealthCheck.port == 8080 and
      .httpHealthCheck.portSpecification == "USE_FIXED_PORT" and
      .httpHealthCheck.requestPath == $expected_path
    ' "${state_dir}/cma-api-effective-health.json" >/dev/null || return 1

  jq -e \
    --arg self_link "${CMA_COMPUTE_SELF_LINK}${api_backend_ref}" \
    --arg health_link "${CMA_COMPUTE_SELF_LINK}${api_health_ref}" \
    --arg project_number "${CMA_GATEWAY_PROJECT_NUMBER}" \
    --arg neg_name "${api_neg_name}" \
    --slurpfile service "${state_dir}/cma-api-service.json" '
      ($service[0].metadata.annotations["cloud.google.com/neg-status"] |
       fromjson | .zones | sort) as $zones |
      [$zones[] |
       "https://www.googleapis.com/compute/v1/projects/" + $project_number +
       "/zones/" + . + "/networkEndpointGroups/" + $neg_name] as $groups |
      .selfLink == $self_link and
      .timeoutSec == 660 and
      .protocol == "HTTP" and
      .loadBalancingScheme == "EXTERNAL_MANAGED" and
      .healthChecks == [$health_link] and
      ([.backends[]?.group] | sort) == ($groups | sort)
    ' "${state_dir}/cma-api-effective-backend.json" >/dev/null || return 1

  jq -e \
    --arg self_link "${CMA_COMPUTE_SELF_LINK}${web_backend_ref}" \
    --arg project_number "${CMA_GATEWAY_PROJECT_NUMBER}" \
    --arg neg_name "${web_neg_name}" \
    --slurpfile service "${state_dir}/cma-web-service.json" '
      ($service[0].metadata.annotations["cloud.google.com/neg-status"] |
       fromjson | .zones | sort) as $zones |
      [$zones[] |
       "https://www.googleapis.com/compute/v1/projects/" + $project_number +
       "/zones/" + . + "/networkEndpointGroups/" + $neg_name] as $groups |
      .selfLink == $self_link and
      .timeoutSec == 660 and
      .protocol == "HTTP" and
      .loadBalancingScheme == "EXTERNAL_MANAGED" and
      ([.backends[]?.group] | sort) == ($groups | sort)
    ' "${state_dir}/cma-web-effective-backend.json" >/dev/null || return 1
}

cma_gateway_validate_local_state() {
  local state_dir="$1"
  local expected_mode="$2"
  local expected_health_path="$3"

  cma_gateway_validate_api_route "${state_dir}/cma-api-route.json" || return 1
  cma_gateway_validate_web_route \
    "${state_dir}/cma-web-route.json" "${expected_mode}" || return 1
  cma_gateway_validate_redirect_route \
    "${state_dir}/cma-http-redirect-route.json" || return 1
  cma_gateway_validate_health_policy \
    "${state_dir}/cma-api-health.json" "${expected_health_path}" || return 1
  cma_gateway_validate_backend_policy \
    "${state_dir}/cma-api-backend-policy.json" cma-api || return 1
  cma_gateway_validate_backend_policy \
    "${state_dir}/cma-web-backend-policy.json" cma-web || return 1
  cma_gateway_validate_effective_state \
    "${state_dir}" "${expected_health_path}" || return 1
}

cma_gateway_capture_live_state() {
  local state_dir="$1"
  local gateway_file="${state_dir}/gateway.json"
  local api_health_ref api_backend_ref web_backend_ref

  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get httproute cma-api -o json > "${state_dir}/cma-api-route.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get httproute cma-web -o json > "${state_dir}/cma-web-route.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get httproute cma-http-redirect -o json > "${state_dir}/cma-http-redirect-route.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get healthcheckpolicy cma-api -o json > "${state_dir}/cma-api-health.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get gcpbackendpolicy cma-api -o json > "${state_dir}/cma-api-backend-policy.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get gcpbackendpolicy cma-web -o json > "${state_dir}/cma-web-backend-policy.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get service cma-api -o json > "${state_dir}/cma-api-service.json"
  cma_gateway_kubectl "${CMA_GATEWAY_NAMESPACE}" \
    get service cma-web -o json > "${state_dir}/cma-web-service.json"
  cma_gateway_kubectl "${CMA_GATEWAY_OWNER_NAMESPACE}" \
    get gateway "${CMA_GATEWAY_NAME}" -o json > "${gateway_file}"

  api_health_ref="$(cma_gateway_resource_reference \
    "${gateway_file}" "networking.gke.io/health-checks" healthChecks cma-api)"
  api_backend_ref="$(cma_gateway_resource_reference \
    "${gateway_file}" "networking.gke.io/backend-services" backendServices cma-api)"
  web_backend_ref="$(cma_gateway_resource_reference \
    "${gateway_file}" "networking.gke.io/backend-services" backendServices cma-web)"

  cma_gateway_gcloud compute health-checks describe "${api_health_ref##*/}" \
    --project="${CMA_GATEWAY_PROJECT_ID}" --global --format=json \
    > "${state_dir}/cma-api-effective-health.json"
  cma_gateway_gcloud compute backend-services describe "${api_backend_ref##*/}" \
    --project="${CMA_GATEWAY_PROJECT_ID}" --global --format=json \
    > "${state_dir}/cma-api-effective-backend.json"
  cma_gateway_gcloud compute backend-services describe "${web_backend_ref##*/}" \
    --project="${CMA_GATEWAY_PROJECT_ID}" --global --format=json \
    > "${state_dir}/cma-web-effective-backend.json"

  # Detect a controller or operator change while the cloud-side resources were
  # read. The next invocation can retry from one coherent snapshot.
  cma_gateway_assert_resource_unchanged httproute cma-api \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-api-route.json"
  cma_gateway_assert_resource_unchanged httproute cma-web \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-web-route.json"
  cma_gateway_assert_resource_unchanged httproute cma-http-redirect \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-http-redirect-route.json"
  cma_gateway_assert_resource_unchanged healthcheckpolicy cma-api \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-api-health.json"
  cma_gateway_assert_resource_unchanged gcpbackendpolicy cma-api \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-api-backend-policy.json"
  cma_gateway_assert_resource_unchanged gcpbackendpolicy cma-web \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-web-backend-policy.json"
  cma_gateway_assert_resource_unchanged service cma-api \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-api-service.json"
  cma_gateway_assert_resource_unchanged service cma-web \
    "${CMA_GATEWAY_NAMESPACE}" "${state_dir}/cma-web-service.json"
  cma_gateway_assert_resource_unchanged gateway "${CMA_GATEWAY_NAME}" \
    "${CMA_GATEWAY_OWNER_NAMESPACE}" "${gateway_file}"
}

cma_gateway_assert_resource_unchanged() {
  local resource_kind="$1"
  local resource_name="$2"
  local resource_namespace="$3"
  local before_file="$4"

  cma_gateway_kubectl "${resource_namespace}" \
    get "${resource_kind}" "${resource_name}" -o json | \
    jq -e --slurpfile before "${before_file}" '
      .metadata.uid == $before[0].metadata.uid and
      .metadata.resourceVersion == $before[0].metadata.resourceVersion
    ' >/dev/null
}

cma_gateway_snapshot_mode() {
  local snapshot_dir="$1"

  if cma_gateway_validate_web_route \
      "${snapshot_dir}/cma-web-route.json" legacy 2>/dev/null && \
     cma_gateway_validate_health_policy \
      "${snapshot_dir}/cma-api-health.json" /livez 2>/dev/null; then
    printf '%s\n' legacy
    return 0
  fi
  if cma_gateway_validate_web_route \
      "${snapshot_dir}/cma-web-route.json" adopted 2>/dev/null && \
     cma_gateway_validate_health_policy \
      "${snapshot_dir}/cma-api-health.json" /ready 2>/dev/null; then
    printf '%s\n' adopted
    return 0
  fi
  return 1
}

cma_gateway_validate_snapshot() {
  local snapshot_dir="$1"
  local snapshot_mode

  snapshot_mode="$(cma_gateway_snapshot_mode "${snapshot_dir}")" || \
    cma_gateway_die "rollback snapshot is neither the exact legacy nor adopted state"
  cma_gateway_validate_api_route \
    "${snapshot_dir}/cma-api-route.json" || return 1
  cma_gateway_validate_redirect_route \
    "${snapshot_dir}/cma-http-redirect-route.json" || return 1
  printf '%s\n' "${snapshot_mode}"
}

cma_gateway_compare_snapshot_identity() {
  local live_file="$1"
  local snapshot_file="$2"

  jq -e --slurpfile snapshot "${snapshot_file}" '
    .metadata.uid == $snapshot[0].metadata.uid and
    (.metadata.labels // {}) == ($snapshot[0].metadata.labels // {}) and
    (.metadata.annotations // {}) == ($snapshot[0].metadata.annotations // {}) and
    (.metadata.finalizers // []) == ($snapshot[0].metadata.finalizers // []) and
    (.metadata.ownerReferences // []) ==
      ($snapshot[0].metadata.ownerReferences // [])
    ' "${live_file}" >/dev/null || return 1
}

cma_gateway_compare_snapshot_metadata() {
  local live_file="$1"
  local snapshot_file="$2"

  cma_gateway_compare_snapshot_identity "${live_file}" "${snapshot_file}" || return 1
  jq -e --slurpfile snapshot "${snapshot_file}" \
    '.spec == $snapshot[0].spec' "${live_file}" >/dev/null || return 1
}

cma_gateway_run_live_check() {
  local expected_mode="$1"
  local expected_health_path="$2"
  local state_dir

  cma_gateway_require_tools
  state_dir="$(mktemp -d)"
  chmod 700 "${state_dir}"
  trap 'rm -rf "${state_dir}"' EXIT
  trap 'exit 1' HUP INT TERM
  cma_gateway_capture_live_state "${state_dir}"
  cma_gateway_validate_local_state \
    "${state_dir}" "${expected_mode}" "${expected_health_path}"
  trap - EXIT HUP INT TERM
  rm -rf "${state_dir}"
}

cma_gateway_check_snapshot_live() {
  local snapshot_dir="$1"
  local snapshot_mode expected_health_path state_dir resource_name

  cma_gateway_require_tools
  snapshot_mode="$(cma_gateway_validate_snapshot "${snapshot_dir}")"
  if [[ "${snapshot_mode}" == legacy ]]; then
    expected_health_path=/livez
  else
    expected_health_path=/ready
  fi

  state_dir="$(mktemp -d)"
  chmod 700 "${state_dir}"
  trap 'rm -rf "${state_dir}"' EXIT
  trap 'exit 1' HUP INT TERM
  cma_gateway_capture_live_state "${state_dir}"
  cma_gateway_validate_local_state \
    "${state_dir}" "${snapshot_mode}" "${expected_health_path}"
  for resource_name in \
    cma-api-route cma-web-route cma-http-redirect-route cma-api-health; do
    cma_gateway_compare_snapshot_metadata \
      "${state_dir}/${resource_name}.json" \
      "${snapshot_dir}/${resource_name}.json"
  done
  trap - EXIT HUP INT TERM
  rm -rf "${state_dir}"
}

cma_gateway_check_ready_transition() {
  local snapshot_dir="$1"
  local snapshot_mode state_dir resource_name

  cma_gateway_require_tools
  snapshot_mode="$(cma_gateway_validate_snapshot "${snapshot_dir}")"
  state_dir="$(mktemp -d)"
  chmod 700 "${state_dir}"
  trap 'rm -rf "${state_dir}"' EXIT
  trap 'exit 1' HUP INT TERM
  cma_gateway_capture_live_state "${state_dir}"
  cma_gateway_validate_local_state "${state_dir}" "${snapshot_mode}" /ready
  for resource_name in cma-api-route cma-web-route cma-http-redirect-route; do
    cma_gateway_compare_snapshot_metadata \
      "${state_dir}/${resource_name}.json" \
      "${snapshot_dir}/${resource_name}.json"
  done
  cma_gateway_compare_snapshot_identity \
    "${state_dir}/cma-api-health.json" \
    "${snapshot_dir}/cma-api-health.json"
  trap - EXIT HUP INT TERM
  rm -rf "${state_dir}"
}

cma_gateway_validate_rollback_local_state() {
  local state_dir="$1"
  local snapshot_mode="$2"
  local policy_path effective_path

  policy_path="$(jq -er \
    '.spec.default.config.httpHealthCheck.requestPath' \
    "${state_dir}/cma-api-health.json")" || return 1
  effective_path="$(jq -er '.httpHealthCheck.requestPath' \
    "${state_dir}/cma-api-effective-health.json")" || return 1
  if [[ "${snapshot_mode}" == legacy ]]; then
    [[ "${policy_path}" == /livez || "${policy_path}" == /ready ]] || return 1
    [[ "${effective_path}" == /livez || "${effective_path}" == /ready ]] || return 1
  else
    [[ "${policy_path}" == /ready && "${effective_path}" == /ready ]] || return 1
  fi

  cma_gateway_validate_api_route "${state_dir}/cma-api-route.json" || return 1
  cma_gateway_validate_web_route \
    "${state_dir}/cma-web-route.json" "${snapshot_mode}" || return 1
  cma_gateway_validate_redirect_route \
    "${state_dir}/cma-http-redirect-route.json" || return 1
  cma_gateway_validate_health_policy \
    "${state_dir}/cma-api-health.json" "${policy_path}" || return 1
  cma_gateway_validate_backend_policy \
    "${state_dir}/cma-api-backend-policy.json" cma-api || return 1
  cma_gateway_validate_backend_policy \
    "${state_dir}/cma-web-backend-policy.json" cma-web || return 1
  cma_gateway_validate_effective_state \
    "${state_dir}" "${effective_path}" || return 1
}

cma_gateway_check_rollback_routes() {
  local snapshot_dir="$1"
  local snapshot_mode state_dir resource_name

  cma_gateway_require_tools
  snapshot_mode="$(cma_gateway_validate_snapshot "${snapshot_dir}")"
  state_dir="$(mktemp -d)"
  chmod 700 "${state_dir}"
  trap 'rm -rf "${state_dir}"' EXIT
  trap 'exit 1' HUP INT TERM
  cma_gateway_capture_live_state "${state_dir}"
  cma_gateway_validate_rollback_local_state "${state_dir}" "${snapshot_mode}"
  for resource_name in cma-api-route cma-web-route cma-http-redirect-route; do
    cma_gateway_compare_snapshot_metadata \
      "${state_dir}/${resource_name}.json" \
      "${snapshot_dir}/${resource_name}.json"
  done
  cma_gateway_compare_snapshot_identity \
    "${state_dir}/cma-api-health.json" \
    "${snapshot_dir}/cma-api-health.json"
  trap - EXIT HUP INT TERM
  rm -rf "${state_dir}"
}

cma_gateway_check_adopted_from_snapshot() {
  local snapshot_dir="$1"
  local state_dir resource_name

  cma_gateway_require_tools
  cma_gateway_validate_snapshot "${snapshot_dir}" >/dev/null
  state_dir="$(mktemp -d)"
  chmod 700 "${state_dir}"
  trap 'rm -rf "${state_dir}"' EXIT
  trap 'exit 1' HUP INT TERM
  cma_gateway_capture_live_state "${state_dir}"
  cma_gateway_validate_local_state "${state_dir}" adopted /ready
  for resource_name in \
    cma-api-route cma-web-route cma-http-redirect-route cma-api-health; do
    cma_gateway_compare_snapshot_identity \
      "${state_dir}/${resource_name}.json" \
      "${snapshot_dir}/${resource_name}.json"
  done
  trap - EXIT HUP INT TERM
  rm -rf "${state_dir}"
}

cma_gateway_self_test() {
  local fixture_dir gcloud_bin_dir gcloud_home gcloud_path
  local observed_gcloud_environment

  command -v jq >/dev/null 2>&1 || cma_gateway_die "jq is required for self-test"
  fixture_dir="$(mktemp -d)"
  chmod 700 "${fixture_dir}"
  trap 'rm -rf "${fixture_dir}"' EXIT
  trap 'exit 1' HUP INT TERM

  gcloud_bin_dir="${fixture_dir}/gcloud-bin"
  gcloud_home="${fixture_dir}/gcloud-home"
  mkdir "${gcloud_bin_dir}" "${gcloud_home}"
  printf '%s\n' '#!/bin/sh' 'env' >"${gcloud_bin_dir}/gcloud"
  chmod 700 "${gcloud_bin_dir}/gcloud"
  gcloud_path="${gcloud_bin_dir}:${PATH}"
  if ! observed_gcloud_environment="$(
    HOME="${gcloud_home}" \
    PATH="${gcloud_path}" \
    CLOUDSDK_CONFIG="${fixture_dir}/hostile-config" \
    CLOUDSDK_ACTIVE_CONFIG_NAME=hostile \
    CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE=http://127.0.0.1:1 \
    CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE="${fixture_dir}/hostile-auth.json" \
    CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE="${fixture_dir}/hostile-ca.pem" \
    CLOUDSDK_CORE_HTTP_TIMEOUT=999 \
    CLOUDSDK_PYTHON="${fixture_dir}/hostile-python-bin" \
    GOOGLE_APPLICATION_CREDENTIALS="${fixture_dir}/hostile-adc.json" \
    GOOGLE_API_USE_MTLS_ENDPOINT=always \
    GOOGLE_API_USE_CLIENT_CERTIFICATE=true \
    GOOGLE_AUTH_TOKEN_ENDPOINT=http://127.0.0.1:1 \
    GCE_METADATA_HOST=127.0.0.1 \
    GRPC_DEFAULT_SSL_ROOTS_FILE_PATH="${fixture_dir}/hostile-grpc-ca.pem" \
    CURL_CA_BUNDLE="${fixture_dir}/hostile-curl-ca.pem" \
    REQUESTS_CA_BUNDLE="${fixture_dir}/hostile-requests-ca.pem" \
    SSL_CERT_DIR="${fixture_dir}/hostile-ssl-ca-dir" \
    SSL_CERT_FILE="${fixture_dir}/hostile-ssl-ca.pem" \
    PYTHONPATH="${fixture_dir}/hostile-python" \
    HTTP_PROXY=http://127.0.0.1:1 \
    HTTPS_PROXY=http://127.0.0.1:1 \
    ALL_PROXY=http://127.0.0.1:1 \
    NO_PROXY='' \
    http_proxy=http://127.0.0.1:1 \
    https_proxy=http://127.0.0.1:1 \
    all_proxy=http://127.0.0.1:1 \
    no_proxy='' \
      cma_gateway_gcloud compute health-checks describe fixture
  )"; then
    cma_gateway_die "self-test could not execute the isolated gcloud child"
  fi
  if ! grep -qx "HOME=${gcloud_home}" <<<"${observed_gcloud_environment}" || \
     ! grep -qx "PATH=${gcloud_path}" <<<"${observed_gcloud_environment}" || \
     ! grep -qx 'LC_ALL=C' <<<"${observed_gcloud_environment}" || \
     ! grep -qx 'CLOUDSDK_CORE_HTTP_TIMEOUT=15' \
       <<<"${observed_gcloud_environment}" || \
     [ "$(grep -Ec '^CLOUDSDK_' <<<"${observed_gcloud_environment}")" -ne 1 ] || \
     grep -Eq '^(GOOGLE_.*|GCE_.*|GRPC_.*|CURL_.*|REQUESTS_CA_BUNDLE|SSL_CERT_.*|PYTHON.*|HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|NO_PROXY|http_proxy|https_proxy|all_proxy|no_proxy)=' \
       <<<"${observed_gcloud_environment}"; then
    cma_gateway_die "self-test observed a poisoned gcloud child environment"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  cma_gateway_validate_local_state "${fixture_dir}" adopted /ready

  cma_gateway_write_test_fixture "${fixture_dir}" legacy /livez
  cma_gateway_validate_local_state "${fixture_dir}" legacy /livez

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  jq '.spec.rules[0].backendRefs[0].name = "cma-web"' \
    "${fixture_dir}/cma-api-route.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-api-route.json"
  if cma_gateway_validate_local_state "${fixture_dir}" adopted /ready 2>/dev/null; then
    cma_gateway_die "self-test accepted a direct route with the wrong backend"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  jq '(.status.parents[].conditions[].observedGeneration) = 6' \
    "${fixture_dir}/cma-web-route.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-web-route.json"
  if cma_gateway_validate_local_state "${fixture_dir}" adopted /ready 2>/dev/null; then
    cma_gateway_die "self-test accepted stale HTTPRoute conditions"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  jq '.httpHealthCheck.requestPath = "/livez"' \
    "${fixture_dir}/cma-api-effective-health.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-api-effective-health.json"
  if cma_gateway_validate_local_state "${fixture_dir}" adopted /ready 2>/dev/null; then
    cma_gateway_die "self-test accepted a stale effective health-check path"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  jq '.timeoutSec = 30' \
    "${fixture_dir}/cma-api-effective-backend.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-api-effective-backend.json"
  if cma_gateway_validate_local_state "${fixture_dir}" adopted /ready 2>/dev/null; then
    cma_gateway_die "self-test accepted a stale effective backend timeout"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  jq '.backends[0].group |= sub("cma-api"; "cma-web")' \
    "${fixture_dir}/cma-api-effective-backend.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-api-effective-backend.json"
  if cma_gateway_validate_local_state "${fixture_dir}" adopted /ready 2>/dev/null; then
    cma_gateway_die "self-test accepted a backend detached from the Service NEG"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" adopted /ready
  jq '.metadata.ownerReferences = [{
        "apiVersion":"v1","kind":"Secret","name":"other","uid":"foreign"
      }]' \
    "${fixture_dir}/cma-web-route.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-web-route.json"
  if cma_gateway_validate_local_state "${fixture_dir}" adopted /ready 2>/dev/null; then
    cma_gateway_die "self-test accepted unowned route metadata"
  fi

  cma_gateway_write_test_fixture "${fixture_dir}" legacy /ready
  jq '.spec.default.config.httpHealthCheck.requestPath = "/livez"' \
    "${fixture_dir}/cma-api-health.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-api-health.json"
  cma_gateway_validate_rollback_local_state "${fixture_dir}" legacy

  cma_gateway_write_test_fixture "${fixture_dir}" legacy /ready
  jq '.httpHealthCheck.requestPath = "/startup"' \
    "${fixture_dir}/cma-api-effective-health.json" > "${fixture_dir}/mutated.json"
  mv "${fixture_dir}/mutated.json" "${fixture_dir}/cma-api-effective-health.json"
  if cma_gateway_validate_rollback_local_state \
      "${fixture_dir}" legacy 2>/dev/null; then
    cma_gateway_die "self-test accepted an open rollback health transition"
  fi

  trap - EXIT HUP INT TERM
  rm -rf "${fixture_dir}"
  printf '%s\n' "CMA Trigger Gateway state checker self-test passed"
}

cma_gateway_write_test_fixture() {
  local fixture_dir="$1"
  local fixture_mode="$2"
  local health_path="$3"
  local generation=7
  local api_health_ref="/projects/${CMA_GATEWAY_PROJECT_NUMBER}/global/healthChecks/gkegw1-ungf-chrono-bot-cma-api-80-api123"
  local api_backend_ref="/projects/${CMA_GATEWAY_PROJECT_NUMBER}/global/backendServices/gkegw1-ungf-chrono-bot-cma-api-80-api123"
  local web_backend_ref="/projects/${CMA_GATEWAY_PROJECT_NUMBER}/global/backendServices/gkegw1-ungf-chrono-bot-cma-web-80-web123"

  jq -n --argjson generation "${generation}" '
    def parent($section): {
      group:"gateway.networking.k8s.io",kind:"Gateway",name:"chronoai-gateway",
      namespace:"gateway-infra",sectionName:$section
    };
    def conditions: [
      {type:"ResolvedRefs",status:"True",observedGeneration:$generation},
      {type:"Accepted",status:"True",observedGeneration:$generation},
      {type:"Reconciled",status:"True",observedGeneration:$generation}
    ];
    {
      apiVersion:"gateway.networking.k8s.io/v1",kind:"HTTPRoute",
      metadata:{
        name:"cma-api",namespace:"chrono-bot",generation:$generation,
        uid:"cma-api-uid",resourceVersion:"101",
        labels:{"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"}
      },
      spec:{
        parentRefs:[parent("https")],hostnames:["bot-api.chrono-ai.fun"],
        rules:[{
          matches:[{path:{type:"PathPrefix",value:"/"}}],
          backendRefs:[{group:"",kind:"Service",name:"cma-api",port:80,weight:1}]
        }]
      },
      status:{parents:[{
        controllerName:"networking.gke.io/gateway",parentRef:parent("https"),
        conditions:conditions
      }]}
    }
  ' > "${fixture_dir}/cma-api-route.json"

  jq -n --arg mode "${fixture_mode}" --argjson generation "${generation}" '
    def parent: {
      group:"gateway.networking.k8s.io",kind:"Gateway",name:"chronoai-gateway",
      namespace:"gateway-infra",sectionName:"https"
    };
    def api_backend: [{group:"",kind:"Service",name:"cma-api",port:80,weight:1}];
    def web_backend: [{group:"",kind:"Service",name:"cma-web",port:80,weight:1}];
    def conditions: [
      {type:"ResolvedRefs",status:"True",observedGeneration:$generation},
      {type:"Accepted",status:"True",observedGeneration:$generation},
      {type:"Reconciled",status:"True",observedGeneration:$generation}
    ];
    def legacy_rules: [{
      matches:[{path:{type:"PathPrefix",value:"/"}}],backendRefs:web_backend
    }];
    def adopted_rules: [
      {matches:[
        {path:{type:"PathPrefix",value:"/api"}},
        {path:{type:"PathPrefix",value:"/auth"}},
        {path:{type:"Exact",value:"/openapi.json"}},
        {path:{type:"Exact",value:"/.well-known/agent-card.json"}}
      ],backendRefs:api_backend},
      {matches:[{path:{type:"PathPrefix",value:"/"}}],backendRefs:web_backend}
    ];
    {
      apiVersion:"gateway.networking.k8s.io/v1",kind:"HTTPRoute",
      metadata:{
        name:"cma-web",namespace:"chrono-bot",generation:$generation,
        uid:"cma-web-uid",resourceVersion:"102",
        labels:{"app.kubernetes.io/name":"cma-web","app.kubernetes.io/part-of":"cma"}
      },
      spec:{parentRefs:[parent],hostnames:["bot.chrono-ai.fun"],
            rules:(if $mode == "legacy" then legacy_rules else adopted_rules end)},
      status:{parents:[{
        controllerName:"networking.gke.io/gateway",parentRef:parent,
        conditions:conditions
      }]}
    }
  ' > "${fixture_dir}/cma-web-route.json"

  jq -n --argjson generation "${generation}" '
    def parent: {
      group:"gateway.networking.k8s.io",kind:"Gateway",name:"chronoai-gateway",
      namespace:"gateway-infra",sectionName:"http"
    };
    {
      apiVersion:"gateway.networking.k8s.io/v1",kind:"HTTPRoute",
      metadata:{
        name:"cma-http-redirect",namespace:"chrono-bot",generation:$generation,
        uid:"cma-http-redirect-uid",resourceVersion:"103",
        labels:{"app.kubernetes.io/part-of":"cma"}
      },
      spec:{
        parentRefs:[parent],hostnames:["bot.chrono-ai.fun","bot-api.chrono-ai.fun"],
        rules:[{
          matches:[{path:{type:"PathPrefix",value:"/"}}],
          filters:[{type:"RequestRedirect",requestRedirect:{scheme:"https",statusCode:301}}]
        }]
      },
      status:{parents:[{
        controllerName:"networking.gke.io/gateway",parentRef:parent,
        conditions:[
          {type:"ResolvedRefs",status:"True",observedGeneration:$generation},
          {type:"Accepted",status:"True",observedGeneration:$generation},
          {type:"Reconciled",status:"True",observedGeneration:$generation}
        ]
      }]}
    }
  ' > "${fixture_dir}/cma-http-redirect-route.json"

  jq -n --arg health_path "${health_path}" '
    {
      apiVersion:"networking.gke.io/v1",kind:"HealthCheckPolicy",
      metadata:{
        name:"cma-api",namespace:"chrono-bot",generation:4,
        uid:"cma-api-health-uid",resourceVersion:"104",
        labels:{"app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma"}
      },
      spec:{targetRef:{group:"",kind:"Service",name:"cma-api"},
            default:{config:{type:"HTTP",httpHealthCheck:{port:8080,requestPath:$health_path}}}},
      status:{conditions:[{type:"Attached",status:"True",reason:"Attached"}]}
    }
  ' > "${fixture_dir}/cma-api-health.json"

  for service_name in cma-api cma-web; do
    jq -n --arg service_name "${service_name}" '
      {
        apiVersion:"networking.gke.io/v1",kind:"GCPBackendPolicy",
        metadata:{name:$service_name,namespace:"chrono-bot",generation:3},
        spec:{targetRef:{group:"",kind:"Service",name:$service_name},
              default:{timeoutSec:660}},
        status:{conditions:[{type:"Attached",status:"True",reason:"Attached"}]}
      }
    ' > "${fixture_dir}/${service_name}-backend-policy.json"
  done

  for service_name in cma-api cma-web; do
    jq -n --arg service_name "${service_name}" \
      --arg neg_name "k8s1-002d2683-chrono-bot-${service_name}-80-test123" '
      {
        metadata:{
          name:$service_name,namespace:"chrono-bot",
          annotations:{
            "cloud.google.com/neg-status":({
              network_endpoint_groups:{"80":$neg_name},
              zones:["asia-northeast1-a","asia-northeast1-b","asia-northeast1-c"]
            } | tojson)
          }
        },
        spec:{ports:[{name:"http",port:80,protocol:"TCP",targetPort:"http"}]}
      }
    ' > "${fixture_dir}/${service_name}-service.json"
  done

  jq -n \
    --arg api_health_ref "${api_health_ref}" \
    --arg api_backend_ref "${api_backend_ref}" \
    --arg web_backend_ref "${web_backend_ref}" '
    {metadata:{annotations:{
      "networking.gke.io/health-checks":$api_health_ref,
      "networking.gke.io/backend-services":($api_backend_ref + ", " + $web_backend_ref)
    }}}
  ' > "${fixture_dir}/gateway.json"

  jq -n \
    --arg self_link "${CMA_COMPUTE_SELF_LINK}${api_health_ref}" \
    --arg name "${api_health_ref##*/}" \
    --arg health_path "${health_path}" '
    {selfLink:$self_link,name:$name,type:"HTTP",
     httpHealthCheck:{port:8080,portSpecification:"USE_FIXED_PORT",requestPath:$health_path}}
  ' > "${fixture_dir}/cma-api-effective-health.json"

  for service_name in cma-api cma-web; do
    if [[ "${service_name}" == cma-api ]]; then
      backend_ref="${api_backend_ref}"
      health_checks="[\"${CMA_COMPUTE_SELF_LINK}${api_health_ref}\"]"
    else
      backend_ref="${web_backend_ref}"
      health_checks='[]'
    fi
    jq -n \
      --arg self_link "${CMA_COMPUTE_SELF_LINK}${backend_ref}" \
      --arg project_number "${CMA_GATEWAY_PROJECT_NUMBER}" \
      --arg neg_name "k8s1-002d2683-chrono-bot-${service_name}-80-test123" \
      --argjson health_checks "${health_checks}" '
      {
        selfLink:$self_link,timeoutSec:660,protocol:"HTTP",
        loadBalancingScheme:"EXTERNAL_MANAGED",healthChecks:$health_checks,
        backends:["a","b","c" | {
          group:("https://www.googleapis.com/compute/v1/projects/" +
                 $project_number + "/zones/asia-northeast1-" + . +
                 "/networkEndpointGroups/" + $neg_name)
        }]
      }
    ' > "${fixture_dir}/${service_name}-effective-backend.json"
  done
}

cma_gateway_usage() {
  cat >&2 <<'USAGE'
usage:
  cma-trigger-gateway-state.sh --check-legacy
  cma-trigger-gateway-state.sh --check-adopted
  cma-trigger-gateway-state.sh --validate-snapshot DIR
  cma-trigger-gateway-state.sh --check-ready-transition DIR
  cma-trigger-gateway-state.sh --check-rollback-routes DIR
  cma-trigger-gateway-state.sh --check-adopted-from-snapshot DIR
  cma-trigger-gateway-state.sh --check-snapshot DIR
  cma-trigger-gateway-state.sh --self-test

All check modes are read-only. Effective GCP verification requires
compute.healthChecks.get and compute.backendServices.get on project
example-project-02.
USAGE
}

case "${1:-}" in
  --check-legacy)
    [[ "$#" -eq 1 ]] || cma_gateway_die "--check-legacy takes no arguments"
    cma_gateway_run_live_check legacy /livez
    printf '%s\n' "legacy CMA Gateway state is exact and effective"
    ;;
  --check-adopted)
    [[ "$#" -eq 1 ]] || cma_gateway_die "--check-adopted takes no arguments"
    cma_gateway_run_live_check adopted /ready
    printf '%s\n' "adopted CMA Gateway state is exact and effective"
    ;;
  --validate-snapshot)
    [[ "$#" -eq 2 ]] || cma_gateway_die "--validate-snapshot requires one directory"
    cma_gateway_validate_snapshot "$2" >/dev/null
    printf '%s\n' "CMA Gateway rollback snapshot is exact"
    ;;
  --check-ready-transition)
    [[ "$#" -eq 2 ]] || \
      cma_gateway_die "--check-ready-transition requires one directory"
    cma_gateway_check_ready_transition "$2"
    printf '%s\n' "CMA Gateway /ready transition is effective with snapshot routes"
    ;;
  --check-rollback-routes)
    [[ "$#" -eq 2 ]] || \
      cma_gateway_die "--check-rollback-routes requires one directory"
    cma_gateway_check_rollback_routes "$2"
    printf '%s\n' "CMA Gateway snapshot routes are reconciled before health rollback"
    ;;
  --check-adopted-from-snapshot)
    [[ "$#" -eq 2 ]] || \
      cma_gateway_die "--check-adopted-from-snapshot requires one directory"
    cma_gateway_check_adopted_from_snapshot "$2"
    printf '%s\n' "adopted CMA Gateway state retains every snapshot identity"
    ;;
  --check-snapshot)
    [[ "$#" -eq 2 ]] || cma_gateway_die "--check-snapshot requires one directory"
    cma_gateway_check_snapshot_live "$2"
    printf '%s\n' "live CMA Gateway state exactly matches the effective rollback snapshot"
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
