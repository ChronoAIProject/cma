#!/usr/bin/env bash

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=.github/scripts/cma-trigger-gateway-origin-evidence.sh
source "${SCRIPT_DIR}/cma-trigger-gateway-origin-evidence.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

public_receipt="${TEST_ROOT}/public-receipt.json"
jq -n --arg ipv4 "${CMA_CLOUDFLARE_IPV4}" --arg ipv6 "${CMA_CLOUDFLARE_IPV6}" '
  {settings:{cloudflareIpv4Cidrs:($ipv4|split(" ")),
             cloudflareIpv6Cidrs:($ipv6|split(" "))}}
' >"${public_receipt}"

cma_test_meta() {
  printf '%s|%s|%s\n' "$1" "$2" "$3" >"$4"
}

cma_test_headers() {
  local ray="$1" server="$2" cache_status="$3" age="$4" output="$5"
  {
    printf 'HTTP/2 200\r\nserver: %s\r\ncf-ray: %s\r\n' "${server}" "${ray}"
    [[ "${cache_status}" == - ]] || printf 'cf-cache-status: %s\r\n' "${cache_status}"
    [[ "${age}" == - ]] || printf 'age: %s\r\n' "${age}"
    printf 'content-type: application/json\r\n\r\n'
  } >"${output}"
}

valid4_meta="${TEST_ROOT}/valid4.meta"
valid4_headers="${TEST_ROOT}/valid4.headers"
cma_test_meta 200 104.16.0.1 0 "${valid4_meta}"
cma_test_headers 0123456789abcdef-NRT cloudflare DYNAMIC - "${valid4_headers}"
cma_origin_validate_public_chain "${valid4_meta}" "${valid4_headers}" \
  136.69.78.214 "${public_receipt}" | jq -e '
    .remoteIp == "104.16.0.1" and .addressFamily == "IPv4" and
    .cfRay == "0123456789abcdef-NRT" and .edgeColo == "NRT" and
    .server == "cloudflare" and .cacheStatus == "DYNAMIC" and
    .ageHeader == false
' >/dev/null

valid6_meta="${TEST_ROOT}/valid6.meta"
valid6_headers="${TEST_ROOT}/valid6.headers"
cma_test_meta 200 2606:4700::1 0 "${valid6_meta}"
cma_test_headers fedcba9876543210-SIN cloudflare dynamic - "${valid6_headers}"
cma_origin_validate_public_chain "${valid6_meta}" "${valid6_headers}" \
  136.69.78.214 "${public_receipt}" | jq -e '
    .remoteIp == "2606:4700::1" and .addressFamily == "IPv6" and
    .edgeColo == "SIN" and .cacheStatus == "DYNAMIC"
' >/dev/null

cma_test_reject_public() {
  local label="$1" status="$2" remote_ip="$3" tls="$4" origin_ip="$5"
  local ray="$6" server="$7" cache_status="$8" age="$9"
  local meta_file="${TEST_ROOT}/${label}.meta"
  local headers_file="${TEST_ROOT}/${label}.headers"
  cma_test_meta "${status}" "${remote_ip}" "${tls}" "${meta_file}"
  cma_test_headers "${ray}" "${server}" "${cache_status}" "${age}" "${headers_file}"
  if cma_origin_validate_public_chain "${meta_file}" "${headers_file}" \
      "${origin_ip}" "${public_receipt}" >/dev/null 2>&1; then
    echo "invalid public chain was accepted: ${label}" >&2
    exit 1
  fi
}

cma_test_reject_public direct-origin 200 104.16.0.1 0 104.16.0.1 0123456789abcdef-NRT cloudflare DYNAMIC -
cma_test_reject_public outside-cidr 200 34.102.1.2 0 136.69.78.214 0123456789abcdef-NRT cloudflare DYNAMIC -
cma_test_reject_public malformed-ip 200 not-an-ip 0 136.69.78.214 0123456789abcdef-NRT cloudflare DYNAMIC -
cma_test_reject_public noncanonical-ipv6 200 2606:4700:0:0:0:0:0:1 0 136.69.78.214 0123456789abcdef-NRT cloudflare DYNAMIC -
cma_test_reject_public bad-ray 200 104.16.0.1 0 136.69.78.214 malformed cloudflare DYNAMIC -
cma_test_reject_public wrong-server 200 104.16.0.1 0 136.69.78.214 0123456789abcdef-NRT origin DYNAMIC -
cma_test_reject_public bad-tls 200 104.16.0.1 1 136.69.78.214 0123456789abcdef-NRT cloudflare DYNAMIC -
cma_test_reject_public cache-hit 200 104.16.0.1 0 136.69.78.214 0123456789abcdef-NRT cloudflare HIT 120
cma_test_reject_public cache-miss 200 104.16.0.1 0 136.69.78.214 0123456789abcdef-NRT cloudflare MISS -
cma_test_reject_public missing-cache 200 104.16.0.1 0 136.69.78.214 0123456789abcdef-NRT cloudflare - -
cma_test_reject_public dynamic-with-age 200 104.16.0.1 0 136.69.78.214 0123456789abcdef-NRT cloudflare DYNAMIC 0

contract_receipt="${TEST_ROOT}/contract-receipt.json"
jq -n --arg ipv4 "${CMA_CLOUDFLARE_IPV4}" --arg ipv6 "${CMA_CLOUDFLARE_IPV6}" '
  def record($id; $host):
    {id:$id,type:"A",name:$host,content:"203.0.113.10",proxied:true,
     proxiable:true,ttl:1};
  (record("11111111111111111111111111111111"; "bot.chrono-ai.fun")) as $bot |
  (record("22222222222222222222222222222222"; "bot-api.chrono-ai.fun")) as $api |
  {
    schemaVersion:1,kind:"CmaGatewayCloudflareCapture",status:"captured-read-only",
    zone:{id:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",name:"chrono-ai.fun"},
    behaviorProjection:{
      account:{id:"dddddddddddddddddddddddddddddddd",name:"ChronoAI"},
      dnsProxyRecords:{"bot.chrono-ai.fun":[$bot],"bot-api.chrono-ai.fun":[$api]},
      workerRoutes:[],workerDomains:[],accessApplications:{account:[],zone:[]},
      waitingRooms:[],legacyFirewallRules:[],legacyRateLimits:[],accountRulesets:[],
      loadBalancers:[],snippetRules:[],originTlsClientAuthHostnames:[],rulesets:[],
      cacheVariants:{value:{}},tieredCaching:{value:"off"},urlNormalization:{type:"cloudflare"}
    },
    settings:{pseudoIPv4:"Disabled",removeVisitorIpHeaders:false,
      addHeaderCompatibilityEvidence:"not-required",identityHeaderWorkers:[],
      identityHeaderTransforms:[],
      dnsProxyRecords:{"bot.chrono-ai.fun":[$bot],"bot-api.chrono-ai.fun":[$api]},
      cloudflareIpv4Cidrs:($ipv4|split(" ")|sort),
      cloudflareIpv6Cidrs:($ipv6|split(" ")|sort)}
  }
' >"${contract_receipt}"

cma_write_policy() {
  local receipt="$1" policy="$2"
  jq -n --slurpfile receipt "${receipt}" '
    {schemaVersion:1,status:"approved",sourceEvidenceSha256:("a" * 64),
     zoneId:$receipt[0].zone.id,accountId:$receipt[0].behaviorProjection.account.id,
     expected:{behaviorProjection:$receipt[0].behaviorProjection,
               settings:$receipt[0].settings}}
  ' >"${policy}"
}

contract_policy="${TEST_ROOT}/contract-policy.json"
cma_write_policy "${contract_receipt}" "${contract_policy}"
cma_origin_validate_cloudflare_contract "${contract_receipt}" 203.0.113.10 \
  "${contract_policy}"

cma_test_reject_contract() {
  local label="$1" filter="$2"
  local receipt="${TEST_ROOT}/${label}.json" policy="${TEST_ROOT}/${label}-policy.json"
  jq "${filter}" "${contract_receipt}" >"${receipt}"
  cma_write_policy "${receipt}" "${policy}"
  if cma_origin_validate_cloudflare_contract "${receipt}" 203.0.113.10 \
      "${policy}" >/dev/null 2>&1; then
    echo "unsupported Cloudflare contract was accepted: ${label}" >&2
    exit 1
  fi
}

cma_test_reject_contract dns-origin-mismatch \
  '.settings.dnsProxyRecords["bot.chrono-ai.fun"][0].content = "203.0.113.11" |
   .behaviorProjection.dnsProxyRecords["bot.chrono-ai.fun"][0].content = "203.0.113.11"'
cma_test_reject_contract worker-route \
  '.behaviorProjection.workerRoutes = [{id:"1",pattern:"bot.chrono-ai.fun/*"}] |
   .settings.identityHeaderWorkers = [{id:"1",pattern:"bot.chrono-ai.fun/*"}]'
cma_test_reject_contract worker-domain \
  '.behaviorProjection.workerDomains = [{id:"1",hostname:"bot.chrono-ai.fun"}]'
cma_test_reject_contract access-application \
  '.behaviorProjection.accessApplications.account = [{id:"1",domain:"bot.chrono-ai.fun"}]'
cma_test_reject_contract waiting-room \
  '.behaviorProjection.waitingRooms = [{id:"1",host:"bot.chrono-ai.fun"}]'
cma_test_reject_contract account-ruleset \
  '.behaviorProjection.accountRulesets = [{id:"1"}]'
cma_test_reject_contract load-balancer \
  '.behaviorProjection.loadBalancers = [{id:"1",name:"bot.chrono-ai.fun"}]'
cma_test_reject_contract snippet-rule \
  '.behaviorProjection.snippetRules = [{id:"1",snippet_name:"rewrite"}]'
cma_test_reject_contract hostname-aop \
  '.behaviorProjection.originTlsClientAuthHostnames = [{hostname:"bot.chrono-ai.fun",status:"active",enabled:true}]'
cma_test_reject_contract external-ruleset \
  '.behaviorProjection.rulesets = [{rules:[{action:"execute",action_parameters:{id:"managed"}}]}]'
# shellcheck disable=SC2016 # The dollar-prefixed list name is literal Cloudflare syntax.
cma_test_reject_contract list-expression \
  '.behaviorProjection.rulesets = [{rules:[{action:"block",expression:"ip.src in $blocked"}]}]'
cma_test_reject_contract pseudo-add-header \
  '.settings.pseudoIPv4 = "Add Header" |
   .settings.addHeaderCompatibilityEvidence = "requires-live-origin-evidence"'

drifted_receipt="${TEST_ROOT}/policy-drift.json"
jq '.behaviorProjection.cacheVariants = {value:{jpeg:["image/avif"]}}' \
  "${contract_receipt}" >"${drifted_receipt}"
if cma_origin_validate_cloudflare_contract "${drifted_receipt}" 203.0.113.10 \
    "${contract_policy}" >/dev/null 2>&1; then
  echo "Cloudflare behavior drift from the approved baseline was accepted" >&2
  exit 1
fi

pre_receipt="${TEST_ROOT}/pre.json"
post_receipt="${TEST_ROOT}/post.json"
jq -n '{capturedAt:"2026-08-29T01:00:00Z",completedAt:"2026-08-29T01:00:05Z",
  zone:{id:"zone",name:"chrono-ai.fun"},behaviorProjection:{version:"1"},settings:{mode:"off"}}' \
  >"${pre_receipt}"
jq -n '{capturedAt:"2026-08-29T01:00:20Z",completedAt:"2026-08-29T01:00:25Z",
  zone:{id:"zone",name:"chrono-ai.fun"},behaviorProjection:{version:"1"},settings:{mode:"off"}}' \
  >"${post_receipt}"
cma_origin_compare_cloudflare_window "${pre_receipt}" "${post_receipt}" \
  2026-08-29T01:00:10Z 2026-08-29T01:00:15Z

post_drift="${TEST_ROOT}/post-drift.json"
jq '.behaviorProjection.version = "2"' "${post_receipt}" >"${post_drift}"
if cma_origin_compare_cloudflare_window "${pre_receipt}" "${post_drift}" \
    2026-08-29T01:00:10Z 2026-08-29T01:00:15Z >/dev/null 2>&1; then
  echo "Cloudflare behavior drift across probes was accepted" >&2
  exit 1
fi
if cma_origin_compare_cloudflare_window "${pre_receipt}" "${post_receipt}" \
    2026-08-29T01:00:10Z 2026-08-29T01:00:30Z >/dev/null 2>&1; then
  echo "post-capture preceding probe completion was accepted" >&2
  exit 1
fi

(
  CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM=writers-disabled-through-origin-probes
  CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID=change/CMA-123
  export CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID
  cma_origin_require_writer_freeze
)
if (
    CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM=writers-disabled-through-origin-probes
    CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID='bad value'
    export CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID
    cma_origin_require_writer_freeze
  ) >/dev/null 2>&1; then
  echo "malformed Cloudflare writer-freeze attestation was accepted" >&2
  exit 1
fi

printf '%s\n' "CMA Trigger origin evidence tests passed (32 cases)"
