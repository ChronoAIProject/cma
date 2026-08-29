#!/usr/bin/env bash
# shellcheck disable=SC2317

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=.github/scripts/cma-trigger-gateway-cloudflare-capture.sh
source "${SCRIPT_DIR}/cma-trigger-gateway-cloudflare-capture.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT
readonly TEST_TOKEN='cloudflare-test-token-must-never-reach-evidence'
readonly TEST_ZONE_ID=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
readonly TEST_ACCOUNT_ID=dddddddddddddddddddddddddddddddd
readonly TEST_RULESET_ID=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
readonly TEST_SECOND_RULESET_ID=cccccccccccccccccccccccccccccccc

cma_cf_api_get() {
  local token="$1"
  local request_path="$2"
  local output_file="$3"
  local page total_pages total_count result cursor_after cursor_mode=0
  local bad_mode="${CMA_CF_TEST_BAD_PAGINATION:-}"

  [[ "${token}" == "${TEST_TOKEN}" ]] || return 1
  printf '%s\n' "${request_path}" >>"${TEST_ROOT}/requests.log"
  case "${request_path}" in
    /user/tokens/verify)
      result='{"id":"token-id","status":"active","not_before":"2026-08-29T00:55:00Z","expires_on":"2026-08-29T01:30:00Z"}'
      ;;
    /ips)
      jq -n --arg ipv4 "${CMA_CF_IPV4}" --arg ipv6 "${CMA_CF_IPV6}" '
        {result:{ipv4_cidrs:($ipv4|split(" ")),ipv6_cidrs:($ipv6|split(" ")),etag:"fixture"},
         success:true,errors:[],messages:[]}
      ' >"${output_file}"
      return 0
      ;;
    "/zones/${TEST_ZONE_ID}/settings/pseudo_ipv4")
      result='{"id":"pseudo_ipv4","value":"off","editable":true}'
      ;;
    "/zones/${TEST_ZONE_ID}/settings")
      result='[{"id":"pseudo_ipv4","value":"off","editable":true},{"id":"ssl","value":"full","editable":true},{"id":"cache_level","value":"aggressive","editable":true}]'
      ;;
    "/zones/${TEST_ZONE_ID}/url_normalization")
      result='{"type":"cloudflare","scope":"incoming"}'
      ;;
    "/zones/${TEST_ZONE_ID}/cache/variants")
      result='{"value":{}}'
      ;;
    "/zones/${TEST_ZONE_ID}/argo/tiered_caching")
      result='{"value":"off"}'
      ;;
    "/zones/${TEST_ZONE_ID}/managed_headers")
      result='{"managed_request_headers":[{"id":"remove_visitor_ip_headers","enabled":false,"has_conflict":false},{"id":"add_true_client_ip_headers","enabled":false,"has_conflict":false}],"managed_response_headers":[]}'
      ;;
    "/zones/${TEST_ZONE_ID}/workers/routes")
      result='[]'
      ;;
    "/accounts/${TEST_ACCOUNT_ID}/workers/domains?hostname=bot.chrono-ai.fun"|\
    "/accounts/${TEST_ACCOUNT_ID}/workers/domains?hostname=bot-api.chrono-ai.fun")
      result='[]'
      ;;
    "/accounts/${TEST_ACCOUNT_ID}/rulesets?per_page=50")
      result='[]'
      cursor_after=''
      cursor_mode=1
      ;;
    "/zones/${TEST_ZONE_ID}/access/apps")
      result='[]'
      ;;
    "/zones/${TEST_ZONE_ID}/firewall/rules?page=1&per_page=100")
      jq -n '{result:[],success:true,errors:[],messages:[],
        result_info:{count:0,page:1,per_page:100,total_count:0}}' >"${output_file}"
      return 0
      ;;
    "/zones/${TEST_ZONE_ID}/rate_limits?page=1&per_page=1000")
      jq -n '{result:[],success:true,errors:[],messages:[],
        result_info:{count:0,page:1,per_page:1000,total_count:0}}' >"${output_file}"
      return 0
      ;;
    "/zones/${TEST_ZONE_ID}/rulesets/${TEST_RULESET_ID}")
      result="{\"id\":\"${TEST_RULESET_ID}\",\"name\":\"request transforms\",\"kind\":\"zone\",\"phase\":\"http_request_late_transform\",\"version\":\"1\",\"last_updated\":\"2026-08-29T00:00:00Z\",\"rules\":[]}"
      ;;
    "/zones/${TEST_ZONE_ID}/rulesets/${TEST_SECOND_RULESET_ID}")
      result="{\"id\":\"${TEST_SECOND_RULESET_ID}\",\"name\":\"cache\",\"kind\":\"zone\",\"phase\":\"http_request_cache_settings\",\"version\":\"1\",\"last_updated\":\"2026-08-29T00:00:00Z\",\"rules\":[]}"
      ;;
    "/zones/${TEST_ZONE_ID}/origin_tls_client_auth/settings")
      result='{"enabled":false}'
      ;;
    "/zones/${TEST_ZONE_ID}/load_balancers")
      result='[]'
      ;;
    "/zones/${TEST_ZONE_ID}/pagerules?status=active"|\
    "/zones/${TEST_ZONE_ID}/pagerules?status=disabled")
      result='[]'
      ;;
    "/zones/${TEST_ZONE_ID}/snippets/snippet_rules")
      result='[]'
      ;;
    /zones\?name=chrono-ai.fun\&status=active\&match=all\&page=1\&per_page=50)
      result="[{\"id\":\"${TEST_ZONE_ID}\",\"name\":\"chrono-ai.fun\",\"status\":\"active\",\"type\":\"full\",\"account\":{\"id\":\"${TEST_ACCOUNT_ID}\",\"name\":\"ChronoAI\"}}]"
      page=1
      total_pages=1
      total_count=1
      if [[ "${bad_mode}" == duplicate-page-results ]]; then
        result="$(jq -cn --argjson rows "${result}" '$rows + $rows')"
        total_count=2
      fi
      ;;
    "/zones/${TEST_ZONE_ID}/dns_records?name=bot.chrono-ai.fun&page=1&per_page=50")
      result='[{"id":"11111111111111111111111111111111","type":"A","name":"bot.chrono-ai.fun","content":"203.0.113.10","proxied":true,"proxiable":true,"ttl":1}]'
      page=1
      total_pages=1
      total_count=1
      ;;
    "/zones/${TEST_ZONE_ID}/dns_records?name=bot-api.chrono-ai.fun&page=1&per_page=50")
      result='[{"id":"22222222222222222222222222222222","type":"A","name":"bot-api.chrono-ai.fun","content":"203.0.113.11","proxied":true,"proxiable":true,"ttl":1}]'
      page=1
      total_pages=1
      total_count=1
      ;;
    "/zones/${TEST_ZONE_ID}/rulesets?per_page=50")
      result="[{\"id\":\"${TEST_RULESET_ID}\",\"name\":\"request transforms\",\"kind\":\"zone\",\"phase\":\"http_request_late_transform\",\"version\":\"1\",\"last_updated\":\"2026-08-29T00:00:00Z\"}]"
      if [[ "${bad_mode}" == missing-terminal-cursor ]]; then
        cursor_after=missing-1
      else
        cursor_after=next-cursor
      fi
      cursor_mode=1
      ;;
    "/zones/${TEST_ZONE_ID}/rulesets?cursor=next-cursor&per_page=50")
      result="[{\"id\":\"${TEST_SECOND_RULESET_ID}\",\"name\":\"cache\",\"kind\":\"zone\",\"phase\":\"http_request_cache_settings\",\"version\":\"1\",\"last_updated\":\"2026-08-29T00:00:00Z\"}]"
      if [[ "${bad_mode}" == repeated-cursor ]]; then
        cursor_after=next-cursor
      else
        cursor_after=''
      fi
      cursor_mode=1
      ;;
    "/zones/${TEST_ZONE_ID}/rulesets?cursor=missing-"*"&per_page=50")
      result='[]'
      cursor_after="${request_path#*cursor=missing-}"
      cursor_after="${cursor_after%&per_page=50}"
      cursor_after="missing-$((10#${cursor_after} + 1))"
      cursor_mode=1
      ;;
    "/zones/${TEST_ZONE_ID}/snippets?page=1&per_page=50")
      result='[]'
      page=1
      total_pages=1
      total_count=0
      ;;
    "/accounts/${TEST_ACCOUNT_ID}/access/apps?page=1&per_page=50"|\
    "/zones/${TEST_ZONE_ID}/waiting_rooms?page=1&per_page=50")
      result='[]'
      page=1
      total_pages=1
      total_count=0
      ;;
    "/zones/${TEST_ZONE_ID}/origin_tls_client_auth/hostnames?status=all&page=1&per_page=50")
      result='[]'
      if [[ "${request_path}" == *origin_tls_client_auth* ]]; then
        result='[{"cert_id":"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee","created_at":"2026-08-29T00:00:00Z","enabled":true,"hostname":"unrelated.chrono-ai.fun","status":"active","updated_at":"2026-08-29T00:00:00Z"}]'
      fi
      page=1
      total_pages=1
      total_count="$(jq -nr --argjson result "${result}" '$result | length')"
      ;;
    *) return 1 ;;
  esac
  if [[ -n "${page:-}" ]]; then
    jq -n --argjson result "${result}" --argjson page "${page}" \
      --argjson total_pages "${total_pages}" --argjson total_count "${total_count}" '
      {result:$result,success:true,errors:[],messages:[],
       result_info:{page:$page,per_page:50,count:($result|length),
                    total_count:$total_count,total_pages:$total_pages}}
    ' >"${output_file}"
    case "${bad_mode}" in
      wrong-count|wrong-per-page|wrong-total-count|wrong-total-pages)
        if [[ "${request_path}" == /zones\?name=chrono-ai.fun* ]]; then
          case "${bad_mode}" in
            wrong-count) jq '.result_info.count += 1' "${output_file}" >"${output_file}.next" ;;
            wrong-per-page) jq '.result_info.per_page = 49' "${output_file}" >"${output_file}.next" ;;
            wrong-total-count) jq '.result_info.total_count += 1' "${output_file}" >"${output_file}.next" ;;
            wrong-total-pages) jq '.result_info.total_pages += 1' "${output_file}" >"${output_file}.next" ;;
          esac
          mv "${output_file}.next" "${output_file}"
        fi
        ;;
    esac
  elif [[ "${cursor_mode}" == 1 ]]; then
    jq -n --argjson result "${result}" --arg after "${cursor_after}" '
      {result:$result,success:true,errors:[],messages:[],
       result_info:{cursors:(if $after == "" then {} else {after:$after} end)}}
    ' >"${output_file}"
  else
    jq -n --argjson result "${result}" \
      '{result:$result,success:true,errors:[],messages:[]}' >"${output_file}"
  fi
}

cma_test_capture() {
  local destination="$1"
  printf '%s\n' "${TEST_TOKEN}" | (
    exec 7<&0
    CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7
    CMA_CF_TEST_CAPTURED_AT=2026-08-29T01:00:00Z
    CMA_CF_TEST_COMPLETED_AT=2026-08-29T01:00:05Z
    export CMA_GATEWAY_CLOUDFLARE_TOKEN_FD CMA_CF_TEST_CAPTURED_AT CMA_CF_TEST_COMPLETED_AT
    cma_cf_capture "${destination}" >/dev/null
    [[ ! -e /dev/fd/7 ]] || {
      echo "Cloudflare token descriptor remained open after capture" >&2
      exit 1
    }
  )
}

success_dir="${TEST_ROOT}/success"
cma_test_capture "${success_dir}"
success_sha="$(cma_cf_sha256 "${success_dir}/receipt.json")"
CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256="${success_sha}"
export CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256
cma_cf_verify_receipt "${success_dir}/receipt.json"
candidate_policy="${TEST_ROOT}/candidate-policy.json"
cma_cf_derive_policy "${success_dir}/receipt.json" >"${candidate_policy}"
jq -e --arg source_sha "${success_sha}" --arg zone_id "${TEST_ZONE_ID}" \
  --arg account_id "${TEST_ACCOUNT_ID}" '
    .schemaVersion == 1 and .status == "candidate-pending-review" and
    .sourceEvidenceSha256 == $source_sha and .zoneId == $zone_id and
    .accountId == $account_id and
    .expected == {behaviorProjection:.expected.behaviorProjection,
                  settings:.expected.settings}
  ' "${candidate_policy}" >/dev/null
jq -e '
  .kind == "CmaGatewayCloudflareCapture" and .status == "captured-read-only" and
  .capturedAt == "2026-08-29T01:00:00Z" and
  .completedAt == "2026-08-29T01:00:05Z" and
  .transport == {tokenDescriptor:7,requestMethod:"GET",apiRoot:"https://api.cloudflare.com/client/v4"} and
  .settings.pseudoIPv4 == "Disabled" and
  .settings.removeVisitorIpHeaders == false and
  .settings.identityHeaderWorkers == [] and
  .settings.identityHeaderTransforms == [] and
  (.settings.dnsProxyRecords["bot.chrono-ai.fun"] | length) == 1 and
  (.behaviorProjection.zoneSettings | length) == 3 and
  .behaviorProjection.urlNormalization == {type:"cloudflare",scope:"incoming"} and
  .behaviorProjection.cacheVariants == {value:{}} and
  .behaviorProjection.tieredCaching == {value:"off"} and
  .behaviorProjection.accountRulesets == [] and
  .behaviorProjection.workerDomains == [] and
  .behaviorProjection.accessApplications == {account:[],zone:[]} and
  .behaviorProjection.waitingRooms == [] and
  .behaviorProjection.legacyFirewallRules == [] and
  .behaviorProjection.legacyRateLimits == [] and
  (.behaviorProjection.rulesets | length) == 2 and
  .behaviorProjection.pageRules == [] and
  .behaviorProjection.loadBalancers == [] and
  (.behaviorProjection.originTlsClientAuthHostnames | length) == 1 and
  .behaviorProjection.snippetRules == [] and
  (.evidence | length) == 29
' "${success_dir}/receipt.json" >/dev/null
[[ "$(grep -Fxc "/zones/${TEST_ZONE_ID}/rulesets?cursor=next-cursor&per_page=50" \
  "${TEST_ROOT}/requests.log")" == 1 ]]

tampered_dir="${TEST_ROOT}/tampered"
cp -R "${success_dir}" "${tampered_dir}"
chmod 600 "${tampered_dir}/receipt.json"
jq '.settings.pseudoIPv4 = "Overwrite"' "${tampered_dir}/receipt.json" \
  >"${tampered_dir}/next"
mv "${tampered_dir}/next" "${tampered_dir}/receipt.json"
if cma_cf_verify_receipt "${tampered_dir}/receipt.json" >/dev/null 2>&1; then
  echo "tampered Cloudflare receipt unexpectedly verified" >&2
  exit 1
fi

partial_dir="${TEST_ROOT}/partial"
cp -R "${success_dir}" "${partial_dir}"
rm "${partial_dir}/evidence/managed-headers.json"
if cma_cf_verify_receipt "${partial_dir}/receipt.json" >/dev/null 2>&1; then
  echo "partial Cloudflare evidence unexpectedly verified" >&2
  exit 1
fi

forged_dir="${TEST_ROOT}/forged-evidence"
cp -R "${success_dir}" "${forged_dir}"
chmod 600 "${forged_dir}/evidence/pseudo-ipv4.json"
jq '.result.value = "overwrite_header"' "${forged_dir}/evidence/pseudo-ipv4.json" \
  >"${forged_dir}/evidence/next"
mv "${forged_dir}/evidence/next" "${forged_dir}/evidence/pseudo-ipv4.json"
if cma_cf_verify_receipt "${forged_dir}/receipt.json" >/dev/null 2>&1; then
  echo "forged Cloudflare evidence unexpectedly verified" >&2
  exit 1
fi

for bad_mode in \
    wrong-count wrong-per-page wrong-total-count wrong-total-pages \
    duplicate-page-results repeated-cursor missing-terminal-cursor; do
  bad_pagination_dir="${TEST_ROOT}/bad-pagination-${bad_mode}"
  if (
      CMA_CF_TEST_BAD_PAGINATION="${bad_mode}"
      export CMA_CF_TEST_BAD_PAGINATION
      cma_test_capture "${bad_pagination_dir}"
    ) >/dev/null 2>&1; then
    echo "inconsistent Cloudflare pagination unexpectedly captured: ${bad_mode}" >&2
    exit 1
  fi
  [[ ! -e "${bad_pagination_dir}" ]]
done

if (
    exec 7<&-
    CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7
    export CMA_GATEWAY_CLOUDFLARE_TOKEN_FD
    cma_cf_capture "${TEST_ROOT}/missing-token"
  ) >/dev/null 2>&1; then
  echo "Cloudflare capture accepted a missing protected token" >&2
  exit 1
fi

if find "${TEST_ROOT}" -type f -exec grep -F -l -- "${TEST_TOKEN}" {} + | grep -q .; then
  echo "Cloudflare bearer reached an evidence file or log" >&2
  exit 1
fi

printf '%s\n' "CMA Trigger Cloudflare capture tests passed (14 cases)"
