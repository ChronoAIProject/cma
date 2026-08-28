#!/bin/bash -p
set -euo pipefail
umask 077

{ set +x; } 2>/dev/null
unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH
unset ALL_PROXY FTP_PROXY HTTPS_PROXY HTTP_PROXY NO_PROXY
unset all_proxy ftp_proxy https_proxy http_proxy no_proxy
unset CURL_CA_BUNDLE CURL_HOME CURL_SSL_BACKEND SSL_CERT_DIR SSL_CERT_FILE SSLKEYLOGFILE
for cma_cf_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_cf_name}"
done
unset cma_cf_name

readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0
readonly CMA_CF_API_ROOT=https://api.cloudflare.com/client/v4
readonly CMA_CF_ZONE_NAME=chrono-ai.fun
readonly CMA_CF_IPV4='173.245.48.0/20 103.21.244.0/22 103.22.200.0/22 103.31.4.0/22 141.101.64.0/18 108.162.192.0/18 190.93.240.0/20 188.114.96.0/20 197.234.240.0/22 198.41.128.0/17 162.158.0.0/15 104.16.0.0/13 104.24.0.0/14 172.64.0.0/13 131.0.72.0/22'
readonly CMA_CF_IPV6='2400:cb00::/32 2606:4700::/32 2803:f800::/32 2405:b500::/32 2405:8100::/32 2a06:98c0::/29 2c0f:f248::/32'
readonly CMA_CF_EVIDENCE_FILES='token.json ips.json zones.json zone-settings.json url-normalization.json cache-variants.json tiered-caching.json pseudo-ipv4.json managed-headers.json workers-routes.json worker-domain-bot.json worker-domain-api.json access-apps-account.json access-apps-zone.json waiting-rooms.json firewall-rules.json rate-limits.json dns-bot.json dns-api.json account-rulesets.json rulesets.json ruleset-details.json page-rules-active.json page-rules-disabled.json load-balancers.json origin-tls-settings.json origin-tls-hostnames.json snippet-rules.json snippets.json'
CMA_CF_CAPTURE_TOKEN=""

cma_cf_die() {
  printf 'cma-trigger Cloudflare capture: %s\n' "$*" >&2
  exit 1
}

cma_cf_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cma_cf_fsync() {
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    ARGV.each { |path| File.open(path, File::RDONLY) { |file| file.fsync } }
  ' "$@"
}

cma_cf_require_launcher() {
  [[ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" == 0 ]] || \
    cma_cf_die "the checked-in production mutation gate is unexpectedly open"
  [[ "${CMA_GATEWAY_TRUSTED_LAUNCH:-}" == env-i-v1 && \
    "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" == reviewed-host-toolchain && \
    -n "${CMA_GATEWAY_TRUSTED_PATH:-}" && "${PATH}" == "${CMA_GATEWAY_TRUSTED_PATH}" ]] || \
    cma_cf_die "capture requires the reviewed env-i host-toolchain launcher"
}

cma_cf_require_tools() {
  local command_name
  for command_name in awk chmod cmp curl date find grep jq mkdir mktemp mv rm ruby wc; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_cf_die "required command is unavailable: ${command_name}"
  done
}

cma_cf_read_token() {
  local token_fd="${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}"

  [[ "${token_fd}" == 7 && -r /dev/fd/7 ]] || \
    cma_cf_die "the short-lived Cloudflare bearer must arrive on protected descriptor 7"
  if ! IFS= read -r CMA_CF_CAPTURE_TOKEN <&7; then
    exec 7<&-
    cma_cf_die "the protected Cloudflare bearer could not be read exactly once"
  fi
  exec 7<&-
  unset CMA_GATEWAY_CLOUDFLARE_TOKEN_FD CLOUDFLARE_API_TOKEN
  [[ -n "${CMA_CF_CAPTURE_TOKEN}" ]] || cma_cf_die "the protected Cloudflare bearer is empty"
  case "${CMA_CF_CAPTURE_TOKEN}" in
    *$'\r'*|*$'\n'*)
      CMA_CF_CAPTURE_TOKEN=""
      cma_cf_die "the protected Cloudflare bearer must not contain CR or LF"
      ;;
  esac
}

cma_cf_api_get() {
  local token="$1"
  local request_path="$2"
  local output_file="$3"
  local status

  [[ "${request_path}" == /* && "${request_path}" != *$'\r'* && \
    "${request_path}" != *$'\n'* ]] || return 1
  if ! status="$(printf 'Authorization: Bearer %s\n' "${token}" | \
      curl --disable --noproxy '*' --fail --show-error --silent \
      --max-time 30 --request GET --header @- \
      --output "${output_file}" --write-out '%{http_code}' \
      "${CMA_CF_API_ROOT}${request_path}")"; then
    return 1
  fi
  [[ "${status}" == 200 ]] || return 1
  jq -e '.success == true and (.errors // []) == [] and (.messages // []) == []' \
    "${output_file}" >/dev/null
}

cma_cf_capture_single() {
  local token="$1"
  local request_path="$2"
  local output_file="$3"

  cma_cf_api_get "${token}" "${request_path}" "${output_file}" || \
    cma_cf_die "Cloudflare GET failed or returned a non-success envelope: ${request_path}"
}

cma_cf_capture_paginated() {
  local token="$1"
  local request_base="$2"
  local output_file="$3"
  local scratch_dir="$4"
  local page=1 total_pages=1 page_file separator

  mkdir -m 700 "${scratch_dir}"
  [[ "${request_base}" == *\?* ]] && separator='&' || separator='?'
  while (( page <= total_pages )); do
    (( page <= 50 )) || cma_cf_die "Cloudflare pagination exceeded the closed 50-page bound"
    page_file="${scratch_dir}/$(printf '%03d' "${page}").json"
    cma_cf_capture_single "${token}" \
      "${request_base}${separator}page=${page}&per_page=50" "${page_file}"
    jq -e --argjson page "${page}" '
      (.result | type) == "array" and
      (.result_info | type) == "object" and
      .result_info.page == $page and
      .result_info.per_page == 50 and
      .result_info.count == (.result | length) and
      (.result_info.count | type) == "number" and .result_info.count >= 0 and
      (.result_info.total_count | type) == "number" and .result_info.total_count >= 0 and
      (.result_info.total_pages | type) == "number" and
      (.result_info.total_pages | floor) == .result_info.total_pages and
      .result_info.total_pages >= 1 and .result_info.total_pages <= 50 and
      .result_info.total_pages == (if .result_info.total_count == 0 then 1
        else ((.result_info.total_count + 49) / 50 | floor) end) and
      (if $page < .result_info.total_pages then .result_info.count == 50
       else .result_info.count <= 50 end)
    ' "${page_file}" >/dev/null || cma_cf_die "Cloudflare pagination metadata is not closed"
    if (( page == 1 )); then
      total_pages="$(jq -er .result_info.total_pages "${page_file}")"
    else
      [[ "$(jq -er .result_info.total_pages "${page_file}")" == "${total_pages}" ]] || \
        cma_cf_die "Cloudflare total_pages changed during capture"
    fi
    page=$((page + 1))
  done
  jq -s --arg request_base "${request_base}" '
    (.[0].result_info.total_count) as $total_count |
    if all(.[]; .result_info.total_count == $total_count) and
       (map(.result_info.count) | add) == $total_count and
       ((map(.result) | add | map(.id // .snippet_name // .hostname) |
         all(type == "string"))) and
       ((map(.result) | add | map(.id // .snippet_name // .hostname) |
         unique | length) == $total_count)
    then {
      schemaVersion:1,requestBase:$request_base,
      pagination:"page-v1",totalCount:$total_count,
      pages:(map({page:.result_info.page,perPage:.result_info.per_page,
        count:.result_info.count,totalCount:.result_info.total_count,
        totalPages:.result_info.total_pages,response:.})),
      result:(map(.result) | add)
    } else error("page pagination changed or contained duplicate/incomplete results") end
  ' "${scratch_dir}"/*.json >"${output_file}" || \
    cma_cf_die "Cloudflare paginated evidence could not be assembled"
}

cma_cf_urlencode() {
  ruby --disable-gems -ruri -e 'print URI.encode_www_form_component(ARGV.fetch(0))' "$1"
}

cma_cf_capture_cursor_paginated() {
  local token="$1"
  local request_base="$2"
  local output_file="$3"
  local scratch_dir="$4"
  local page=1 page_file cursor='' encoded_cursor next_cursor

  mkdir -m 700 "${scratch_dir}"
  : >"${scratch_dir}/seen-cursors"
  while :; do
    (( page <= 50 )) || cma_cf_die "Cloudflare cursor pagination exceeded the closed 50-page bound"
    page_file="${scratch_dir}/$(printf '%03d' "${page}").json"
    if [[ -z "${cursor}" ]]; then
      cma_cf_capture_single "${token}" "${request_base}?per_page=50" "${page_file}"
    else
      encoded_cursor="$(cma_cf_urlencode "${cursor}")" || \
        cma_cf_die "Cloudflare ruleset cursor could not be encoded"
      cma_cf_capture_single "${token}" \
        "${request_base}?cursor=${encoded_cursor}&per_page=50" "${page_file}"
    fi
    jq -e '
      (.result | type) == "array" and (.result | length) <= 50 and
      ((.result_info // {}) | type) == "object" and
      ((.result_info.cursors // {}) | type) == "object" and
      ((.result_info.cursors.after // null) == null or
       ((.result_info.cursors.after | type) == "string" and
        (.result_info.cursors.after | length) > 0)) and
      all(.result[]; (.id | type) == "string" and (.id | length) > 0)
    ' "${page_file}" >/dev/null || \
      cma_cf_die "Cloudflare cursor pagination metadata is not closed"
    next_cursor="$(jq -er '.result_info.cursors.after // ""' "${page_file}")" || \
      cma_cf_die "Cloudflare next cursor could not be read"
    [[ -n "${next_cursor}" ]] || break
    if grep -Fqx -- "${next_cursor}" "${scratch_dir}/seen-cursors"; then
      cma_cf_die "Cloudflare cursor pagination repeated a cursor"
    fi
    printf '%s\n' "${next_cursor}" >>"${scratch_dir}/seen-cursors"
    cursor="${next_cursor}"
    page=$((page + 1))
  done
  rm "${scratch_dir}/seen-cursors"
  jq -s --arg request_base "${request_base}" '
    (map(.result) | add) as $results |
    if ($results | map(.id) | unique | length) == ($results | length) and
       (.[-1].result_info.cursors.after // null) == null
    then {
      schemaVersion:1,requestBase:$request_base,pagination:"cursor-v1",
      pages:(to_entries | map({sequence:(.key + 1),
        after:(.value.result_info.cursors.after // null),response:.value})),
      result:$results
    } else error("cursor pagination contained duplicate results or lacked a terminal page") end
  ' "${scratch_dir}"/*.json >"${output_file}" || \
    cma_cf_die "Cloudflare cursor evidence could not be assembled"
}

cma_cf_capture_ruleset_details() {
  local token="$1"
  local zone_id="$2"
  local rulesets_file="$3"
  local output_file="$4"
  local scratch_dir="$5"
  local ruleset_id index=0 detail_file

  mkdir -m 700 "${scratch_dir}"
  while IFS= read -r ruleset_id; do
    [[ -n "${ruleset_id}" ]] || continue
    [[ "${ruleset_id}" =~ ^[0-9a-f]{32}$ ]] || cma_cf_die "Cloudflare ruleset ID is malformed"
    detail_file="${scratch_dir}/$(printf '%03d' "${index}").json"
    cma_cf_capture_single "${token}" "/zones/${zone_id}/rulesets/${ruleset_id}" \
      "${detail_file}"
    jq -e --arg id "${ruleset_id}" '.result.id == $id and (.result.rules | type) == "array"' \
      "${detail_file}" >/dev/null || cma_cf_die "Cloudflare ruleset detail is incomplete"
    index=$((index + 1))
  done < <(jq -er '[.result[].id] | unique | sort | .[]' "${rulesets_file}")
  if (( index == 0 )); then
    jq -n '{schemaVersion:1,requests:[]}' >"${output_file}"
  else
    jq -s '{schemaVersion:1,requests:map({id:.result.id,response:.})}' \
      "${scratch_dir}"/*.json >"${output_file}"
  fi
}

cma_cf_validate_token_window() {
  local token_file="$1"
  local captured_at="$2"
  local completed_at="${3:-${captured_at}}"
  ruby --disable-gems -rjson -rtime -e '
    body = JSON.parse(File.read(ARGV.fetch(0)))
    captured = Time.iso8601(ARGV.fetch(1))
    completed = Time.iso8601(ARGV.fetch(2))
    raise unless completed >= captured
    result = body.fetch("result")
    raise unless result.fetch("status") == "active"
    expires = Time.iso8601(result.fetch("expires_on"))
    starts = result["not_before"]
    raise unless starts.nil? || Time.iso8601(starts) <= captured
    raise unless expires > completed && expires <= captured + 3600
  ' "${token_file}" "${captured_at}" "${completed_at}"
}

cma_cf_evidence_manifest() {
  local evidence_dir="$1"
  local output_file="$2"
  local file digest

  : >"${output_file}"
  for file in ${CMA_CF_EVIDENCE_FILES}; do
    [[ -f "${evidence_dir}/${file}" && ! -L "${evidence_dir}/${file}" ]] || return 1
    digest="$(cma_cf_sha256 "${evidence_dir}/${file}")" || return 1
    jq -cn --arg file "${file}" --arg sha256 "${digest}" \
      '{file:$file,sha256:$sha256}' >>"${output_file}" || return 1
  done
}

cma_cf_emit_receipt() {
  local evidence_dir="$1"
  local captured_at="$2"
  local completed_at="$3"
  local tool_sha="$4"
  local output_file="$5"
  local manifest_file="$6"
  local ipv4_json ipv6_json

  ipv4_json="$(jq -cn --arg cidrs "${CMA_CF_IPV4}" '$cidrs | split(" ") | sort')"
  ipv6_json="$(jq -cn --arg cidrs "${CMA_CF_IPV6}" '$cidrs | split(" ") | sort')"
  jq -n --arg captured_at "${captured_at}" --arg completed_at "${completed_at}" \
    --arg tool_sha "${tool_sha}" \
    --argjson official4 "${ipv4_json}" --argjson official6 "${ipv6_json}" \
    --slurpfile token "${evidence_dir}/token.json" \
    --slurpfile ips "${evidence_dir}/ips.json" \
    --slurpfile zones "${evidence_dir}/zones.json" \
    --slurpfile zone_settings "${evidence_dir}/zone-settings.json" \
    --slurpfile url_normalization "${evidence_dir}/url-normalization.json" \
    --slurpfile cache_variants "${evidence_dir}/cache-variants.json" \
    --slurpfile tiered_caching "${evidence_dir}/tiered-caching.json" \
    --slurpfile pseudo "${evidence_dir}/pseudo-ipv4.json" \
    --slurpfile managed "${evidence_dir}/managed-headers.json" \
    --slurpfile workers "${evidence_dir}/workers-routes.json" \
    --slurpfile worker_domain_bot "${evidence_dir}/worker-domain-bot.json" \
    --slurpfile worker_domain_api "${evidence_dir}/worker-domain-api.json" \
    --slurpfile access_account "${evidence_dir}/access-apps-account.json" \
    --slurpfile access_zone "${evidence_dir}/access-apps-zone.json" \
    --slurpfile waiting_rooms "${evidence_dir}/waiting-rooms.json" \
    --slurpfile firewall_rules "${evidence_dir}/firewall-rules.json" \
    --slurpfile rate_limits "${evidence_dir}/rate-limits.json" \
    --slurpfile dns_bot "${evidence_dir}/dns-bot.json" \
    --slurpfile dns_api "${evidence_dir}/dns-api.json" \
    --slurpfile account_rulesets "${evidence_dir}/account-rulesets.json" \
    --slurpfile rulesets "${evidence_dir}/rulesets.json" \
    --slurpfile details "${evidence_dir}/ruleset-details.json" \
    --slurpfile page_rules_active "${evidence_dir}/page-rules-active.json" \
    --slurpfile page_rules_disabled "${evidence_dir}/page-rules-disabled.json" \
    --slurpfile load_balancers "${evidence_dir}/load-balancers.json" \
    --slurpfile origin_tls "${evidence_dir}/origin-tls-settings.json" \
    --slurpfile origin_tls_hostnames "${evidence_dir}/origin-tls-hostnames.json" \
    --slurpfile snippet_rules "${evidence_dir}/snippet-rules.json" \
    --slurpfile snippets "${evidence_dir}/snippets.json" \
    --slurpfile evidence "${manifest_file}" '
    def common:
      .success == true and (.errors // []) == [] and (.messages // []) == [];
    def paginated($document; $base):
      $document.schemaVersion == 1 and $document.requestBase == $base and
      $document.pagination == "page-v1" and
      ($document.pages | type) == "array" and ($document.pages | length) >= 1 and
      ($document.pages[0].totalPages == ($document.pages | length)) and
      $document.totalCount == ($document.result | length) and
      ([range(1; ($document.pages | length) + 1)] == [$document.pages[].page]) and
      all($document.pages[];
        .perPage == 50 and .response.result_info.per_page == 50 and
        .count == (.response.result | length) and .response.result_info.count == .count and
        .totalCount == $document.totalCount and .response.result_info.total_count == .totalCount and
        .totalPages == ($document.pages | length) and .response.result_info.page == .page and
        .response.result_info.total_pages == .totalPages and (.response | common)) and
      ([ $document.pages[].count ] | add) == $document.totalCount and
      $document.result == ([$document.pages[].response.result] | add) and
      ([$document.result[] | (.id // .snippet_name // .hostname)] | unique | length) ==
        $document.totalCount;
    def cursor_paginated($document; $base):
      $document.schemaVersion == 1 and $document.requestBase == $base and
      $document.pagination == "cursor-v1" and
      ($document.pages | type) == "array" and ($document.pages | length) >= 1 and
      ([range(1; ($document.pages | length) + 1)] == [$document.pages[].sequence]) and
      all($document.pages[]; (.response | common) and
        (.response.result | type) == "array" and (.response.result | length) <= 50 and
        .after == (.response.result_info.cursors.after // null)) and
      $document.pages[-1].after == null and
      ([ $document.pages[] | select(.after != null) | .after ] | unique | length) ==
        ([ $document.pages[] | select(.after != null) ] | length) and
      $document.result == ([$document.pages[].response.result] | add) and
      ([$document.result[].id] | unique | length) == ($document.result | length);
    def dns_records($document; $name):
      if paginated($document; "/zones/" + $zones[0].result[0].id + "/dns_records?name=" + $name) and
         ($document.result | length) == 1 and
         all($document.result[];
           .name == $name and .proxied == true and .proxiable == true and
           .type == "A" and (.content | test("^([0-9]{1,3}\\.){3}[0-9]{1,3}$")) and
           (.id | test("^[0-9a-f]{32}$")))
      then [$document.result[] | {id,type,name,content,proxied,proxiable,ttl}] | sort_by(.id)
      else error("DNS proxy evidence is incomplete") end;
    def worker_domain_filter($document; $host):
      ($document | common) and ($document.result | type) == "array" and
      all($document.result[];
        (.id | type) == "string" and .hostname == $host);
    def closed_empty_legacy_collection($document; $per_page):
      ($document | common) and $document.result == [] and
      ($document.result_info | type) == "object" and
      $document.result_info.page == 1 and
      $document.result_info.per_page == $per_page and
      $document.result_info.count == 0 and
      $document.result_info.total_count == 0;
    ($zones[0]) as $zone_pages |
    if ($token | length) == 1 and ($token[0] | common) and
       $token[0].result.status == "active" and
       ($token[0].result.id | type) == "string" and
       ($token[0].result.expires_on | type) == "string" and
       ($ips | length) == 1 and ($ips[0] | common) and
       ($ips[0].result.ipv4_cidrs | sort) == $official4 and
       ($ips[0].result.ipv6_cidrs | sort) == $official6 and
       ($zones | length) == 1 and
       paginated($zone_pages; "/zones?name=chrono-ai.fun&status=active&match=all") and
       ($zone_pages.result | length) == 1 and
       $zone_pages.result[0].name == "chrono-ai.fun" and
       $zone_pages.result[0].status == "active" and
       ($zone_pages.result[0].id | test("^[0-9a-f]{32}$")) and
       ($zone_pages.result[0].account.id | test("^[0-9a-f]{32}$")) and
       ($zone_settings | length) == 1 and ($zone_settings[0] | common) and
       ($zone_settings[0].result | type) == "array" and
       ([$zone_settings[0].result[].id] | all(type == "string")) and
       ([$zone_settings[0].result[].id] | unique | length) ==
         ($zone_settings[0].result | length) and
       ($url_normalization | length) == 1 and ($url_normalization[0] | common) and
       ($url_normalization[0].result | type) == "object" and
       ($cache_variants | length) == 1 and ($cache_variants[0] | common) and
       ($cache_variants[0].result | type) == "object" and
       ($tiered_caching | length) == 1 and ($tiered_caching[0] | common) and
       ($tiered_caching[0].result | type) == "object" and
       ($pseudo | length) == 1 and ($pseudo[0] | common) and
       $pseudo[0].result.id == "pseudo_ipv4" and
       ($pseudo[0].result.value == "off" or
        $pseudo[0].result.value == "add_header" or
        $pseudo[0].result.value == "overwrite_header") and
       ([$zone_settings[0].result[] | select(.id == "pseudo_ipv4")] | length) == 1 and
       ([$zone_settings[0].result[] | select(.id == "pseudo_ipv4")][0].value) ==
         $pseudo[0].result.value and
       ($managed | length) == 1 and ($managed[0] | common) and
       ($managed[0].result.managed_request_headers | type) == "array" and
       ($managed[0].result.managed_response_headers | type) == "array" and
       ($workers | length) == 1 and ($workers[0] | common) and
       ($workers[0].result | type) == "array" and
       all($workers[0].result[];
         (.id | test("^[0-9a-f]{32}$")) and (.pattern | type) == "string") and
       ($worker_domain_bot | length) == 1 and
       worker_domain_filter($worker_domain_bot[0]; "bot.chrono-ai.fun") and
       ($worker_domain_api | length) == 1 and
       worker_domain_filter($worker_domain_api[0]; "bot-api.chrono-ai.fun") and
       ($access_account | length) == 1 and
       paginated($access_account[0];
         "/accounts/" + $zone_pages.result[0].account.id + "/access/apps") and
       ($access_zone | length) == 1 and ($access_zone[0] | common) and
       ($access_zone[0].result | type) == "array" and
       all($access_zone[0].result[]; (.id | type) == "string") and
       ($waiting_rooms | length) == 1 and
       paginated($waiting_rooms[0];
         "/zones/" + $zone_pages.result[0].id + "/waiting_rooms") and
       ($firewall_rules | length) == 1 and
       closed_empty_legacy_collection($firewall_rules[0]; 100) and
       ($rate_limits | length) == 1 and
       closed_empty_legacy_collection($rate_limits[0]; 1000) and
       ($account_rulesets | length) == 1 and
       cursor_paginated($account_rulesets[0];
         "/accounts/" + $zone_pages.result[0].account.id + "/rulesets") and
       ($rulesets | length) == 1 and
       cursor_paginated($rulesets[0]; "/zones/" + $zone_pages.result[0].id + "/rulesets") and
       ($details | length) == 1 and $details[0].schemaVersion == 1 and
       ($details[0].requests | type) == "array" and
       ([$rulesets[0].result[].id] | unique | sort) ==
         ([$details[0].requests[].id] | unique | sort) and
       all($details[0].requests[];
         (.response | common) and .id == .response.result.id and
         (.response.result.rules | type) == "array") and
       ($page_rules_active | length) == 1 and ($page_rules_active[0] | common) and
       ($page_rules_active[0].result | type) == "array" and
       all($page_rules_active[0].result[];
         (.id | type) == "string" and .status == "active") and
       ($page_rules_disabled | length) == 1 and ($page_rules_disabled[0] | common) and
       ($page_rules_disabled[0].result | type) == "array" and
       all($page_rules_disabled[0].result[];
         (.id | type) == "string" and .status == "disabled") and
       (([$page_rules_active[0].result[].id] +
         [$page_rules_disabled[0].result[].id]) | unique | length) ==
         (($page_rules_active[0].result | length) +
          ($page_rules_disabled[0].result | length)) and
       ($load_balancers | length) == 1 and ($load_balancers[0] | common) and
       ($load_balancers[0].result | type) == "array" and
       all($load_balancers[0].result[]; (.id | type) == "string") and
       ($origin_tls | length) == 1 and ($origin_tls[0] | common) and
       ($origin_tls[0].result.enabled | type) == "boolean" and
       ($origin_tls_hostnames | length) == 1 and
       paginated($origin_tls_hostnames[0];
         "/zones/" + $zone_pages.result[0].id +
           "/origin_tls_client_auth/hostnames?status=all") and
       all($origin_tls_hostnames[0].result[];
         (.hostname | type) == "string" and (.status | type) == "string" and
         (.enabled | type) == "boolean") and
       ($snippet_rules | length) == 1 and ($snippet_rules[0] | common) and
       ($snippet_rules[0].result | type) == "array" and
       all($snippet_rules[0].result[];
         (.id | test("^[0-9a-f]{32}$")) and (.snippet_name | type) == "string") and
       ($snippets | length) == 1 and
       paginated($snippets[0]; "/zones/" + $zone_pages.result[0].id + "/snippets") and
       ([$snippet_rules[0].result[].snippet_name] | unique -
         [$snippets[0].result[].snippet_name]) == [] and
       ($evidence | length) == 29 and
       ([$evidence[].file] | unique | length) == 29 and
       all($evidence[]; (.sha256 | test("^[0-9a-f]{64}$")))
    then
      ($zone_pages.result[0]) as $zone |
      ($pseudo[0].result.value) as $pseudo_value |
      ([$managed[0].result.managed_request_headers[] |
        {id,enabled,hasConflict:.has_conflict,conflictsWith:(.conflicts_with // [])}] |
        sort_by(.id)) as $managed_request |
      ([$workers[0].result[] | {id,pattern,script:(.script // null)}] | sort_by(.id)) as $worker_routes |
      ([$details[0].requests[].response.result as $ruleset |
        $ruleset.rules[]? |
        select((.enabled // true) == true) |
        (.action_parameters.headers // {}) as $headers |
        select(($headers | type) == "object") |
        ([$headers | keys[] | ascii_downcase] | sort) as $names |
        select(any($names[];
          . == "cf-connecting-ip" or . == "cf-connecting-ipv6" or
          . == "true-client-ip" or . == "x-forwarded-for" or . == "x-real-ip")) |
        {rulesetId:$ruleset.id,phase:$ruleset.phase,ruleId:.id,
         action:.action,headerNames:$names}] | sort_by(.rulesetId,.ruleId)) as $identity_transforms |
      ({
        zone:{id:$zone.id,name:$zone.name,status:$zone.status,type:($zone.type // null),
          paused:($zone.paused // false),developmentMode:($zone.development_mode // 0)},
        account:{id:$zone.account.id,name:($zone.account.name // null)},
        zoneSettings:($zone_settings[0].result | sort_by(.id)),
        urlNormalization:$url_normalization[0].result,
        cacheVariants:$cache_variants[0].result,
        tieredCaching:$tiered_caching[0].result,
        managedHeaders:$managed[0].result,
        workerRoutes:($workers[0].result | sort_by(.id)),
        workerDomains:(($worker_domain_bot[0].result + $worker_domain_api[0].result) |
          sort_by(.hostname,.id)),
        accessApplications:{account:($access_account[0].result | sort_by(.id)),
          zone:($access_zone[0].result | sort_by(.id))},
        waitingRooms:($waiting_rooms[0].result | sort_by(.id)),
        legacyFirewallRules:$firewall_rules[0].result,
        legacyRateLimits:$rate_limits[0].result,
        dnsProxyRecords:{
          "bot.chrono-ai.fun":($dns_bot[0].result | sort_by(.id)),
          "bot-api.chrono-ai.fun":($dns_api[0].result | sort_by(.id))
        },
        accountRulesets:($account_rulesets[0].result | sort_by(.id)),
        rulesets:([$details[0].requests[].response.result] | sort_by(.id)),
        pageRules:(($page_rules_active[0].result + $page_rules_disabled[0].result) |
          sort_by(.priority,.id)),
        loadBalancers:($load_balancers[0].result | sort_by(.id)),
        originTlsClientAuth:$origin_tls[0].result,
        originTlsClientAuthHostnames:($origin_tls_hostnames[0].result |
          sort_by(.hostname)),
        snippetRules:($snippet_rules[0].result | sort_by(.id)),
        snippets:($snippets[0].result | sort_by(.snippet_name))
      }) as $behavior_projection |
      {
        schemaVersion:1,kind:"CmaGatewayCloudflareCapture",status:"captured-read-only",
        capturedAt:$captured_at,completedAt:$completed_at,toolSourceSha256:$tool_sha,
        transport:{tokenDescriptor:7,requestMethod:"GET",apiRoot:"https://api.cloudflare.com/client/v4"},
        zone:{id:$zone.id,name:$zone.name,status:$zone.status,type:($zone.type // null)},
        token:{id:$token[0].result.id,status:$token[0].result.status,
          notBefore:($token[0].result.not_before // null),expiresOn:$token[0].result.expires_on},
        behaviorProjection:$behavior_projection,
        settings:{
          pseudoIPv4:(if $pseudo_value == "off" then "Disabled"
            elif $pseudo_value == "add_header" then "Add Header" else "Overwrite" end),
          removeVisitorIpHeaders:any($managed_request[];
            (.id == "remove_visitor_ip_headers" or .id == "remove_visitor_headers") and .enabled),
          addHeaderCompatibilityEvidence:(if $pseudo_value == "add_header"
            then "requires-live-origin-evidence" else "not-required" end),
          identityHeaderWorkers:$worker_routes,
          identityHeaderTransforms:$identity_transforms,
          managedRequestHeaders:$managed_request,
          dnsProxyRecords:{
            "bot.chrono-ai.fun":dns_records($dns_bot[0]; "bot.chrono-ai.fun"),
            "bot-api.chrono-ai.fun":dns_records($dns_api[0]; "bot-api.chrono-ai.fun")
          },
          cloudflareIpv4Cidrs:($ips[0].result.ipv4_cidrs | sort),
          cloudflareIpv6Cidrs:($ips[0].result.ipv6_cidrs | sort)
        },
        evidence:($evidence | sort_by(.file))
      }
    else error("Cloudflare capture evidence is incomplete or inconsistent") end
  ' >"${output_file}"
}

cma_cf_capture() {
  local output_dir="$1"
  local parent_dir work_dir evidence_dir token zone_id account_id captured_at completed_at tool_sha
  local manifest_file receipt_file receipt_sha

  [[ "${output_dir}" == /* && "${output_dir}" != / ]] || \
    cma_cf_die "capture destination must be an absolute non-root path"
  parent_dir="${output_dir%/*}"
  [[ -d "${parent_dir}" && ! -L "${parent_dir}" && -O "${parent_dir}" ]] || \
    cma_cf_die "capture parent must be an owned existing non-symlink directory"
  [[ ! -e "${output_dir}" && ! -L "${output_dir}" ]] || \
    cma_cf_die "capture destination is already occupied"
  cma_cf_read_token
  token="${CMA_CF_CAPTURE_TOKEN}"
  CMA_CF_CAPTURE_TOKEN=""
  export -n token
  work_dir="$(mktemp -d "${parent_dir}/.cma-cloudflare-capture.XXXXXXXX")" || \
    cma_cf_die "capture work directory could not be created"
  chmod 700 "${work_dir}"
  trap 'rm -rf "${work_dir}"' EXIT HUP INT QUIT TERM
  evidence_dir="${work_dir}/evidence"
  mkdir -m 700 "${evidence_dir}"
  captured_at="${CMA_CF_TEST_CAPTURED_AT:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"
  tool_sha="$(cma_cf_sha256 "${BASH_SOURCE[0]}")" || cma_cf_die "capture source could not be hashed"

  cma_cf_capture_single "${token}" /user/tokens/verify "${evidence_dir}/token.json"
  cma_cf_validate_token_window "${evidence_dir}/token.json" "${captured_at}" || \
    cma_cf_die "Cloudflare token is not active and bounded to at most one hour"
  cma_cf_capture_single "${token}" /ips "${evidence_dir}/ips.json"
  cma_cf_capture_paginated "${token}" "/zones?name=${CMA_CF_ZONE_NAME}&status=active&match=all" \
    "${evidence_dir}/zones.json" "${work_dir}/zones-pages"
  zone_id="$(jq -er '
    if (.result | length) == 1 and (.result[0].id | test("^[0-9a-f]{32}$"))
    then .result[0].id else error("zone identity is not exact") end
  ' "${evidence_dir}/zones.json")" || cma_cf_die "Cloudflare zone lookup is not exact"
  account_id="$(jq -er '
    if (.result[0].account.id | test("^[0-9a-f]{32}$"))
    then .result[0].account.id else error("account identity is not exact") end
  ' "${evidence_dir}/zones.json")" || cma_cf_die "Cloudflare account lookup is not exact"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/settings" \
    "${evidence_dir}/zone-settings.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/url_normalization" \
    "${evidence_dir}/url-normalization.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/cache/variants" \
    "${evidence_dir}/cache-variants.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/argo/tiered_caching" \
    "${evidence_dir}/tiered-caching.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/settings/pseudo_ipv4" \
    "${evidence_dir}/pseudo-ipv4.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/managed_headers" \
    "${evidence_dir}/managed-headers.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/workers/routes" \
    "${evidence_dir}/workers-routes.json"
  cma_cf_capture_single "${token}" \
    "/accounts/${account_id}/workers/domains?hostname=bot.chrono-ai.fun" \
    "${evidence_dir}/worker-domain-bot.json"
  cma_cf_capture_single "${token}" \
    "/accounts/${account_id}/workers/domains?hostname=bot-api.chrono-ai.fun" \
    "${evidence_dir}/worker-domain-api.json"
  cma_cf_capture_paginated "${token}" "/accounts/${account_id}/access/apps" \
    "${evidence_dir}/access-apps-account.json" "${work_dir}/access-app-pages"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/access/apps" \
    "${evidence_dir}/access-apps-zone.json"
  cma_cf_capture_paginated "${token}" "/zones/${zone_id}/waiting_rooms" \
    "${evidence_dir}/waiting-rooms.json" "${work_dir}/waiting-room-pages"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/firewall/rules?page=1&per_page=100" \
    "${evidence_dir}/firewall-rules.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/rate_limits?page=1&per_page=1000" \
    "${evidence_dir}/rate-limits.json"
  cma_cf_capture_paginated "${token}" "/zones/${zone_id}/dns_records?name=bot.chrono-ai.fun" \
    "${evidence_dir}/dns-bot.json" "${work_dir}/dns-bot-pages"
  cma_cf_capture_paginated "${token}" "/zones/${zone_id}/dns_records?name=bot-api.chrono-ai.fun" \
    "${evidence_dir}/dns-api.json" "${work_dir}/dns-api-pages"
  cma_cf_capture_cursor_paginated "${token}" "/accounts/${account_id}/rulesets" \
    "${evidence_dir}/account-rulesets.json" "${work_dir}/account-ruleset-pages"
  cma_cf_capture_cursor_paginated "${token}" "/zones/${zone_id}/rulesets" \
    "${evidence_dir}/rulesets.json" "${work_dir}/ruleset-pages"
  cma_cf_capture_ruleset_details "${token}" "${zone_id}" \
    "${evidence_dir}/rulesets.json" "${evidence_dir}/ruleset-details.json" \
    "${work_dir}/ruleset-details"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/pagerules?status=active" \
    "${evidence_dir}/page-rules-active.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/pagerules?status=disabled" \
    "${evidence_dir}/page-rules-disabled.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/load_balancers" \
    "${evidence_dir}/load-balancers.json"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/origin_tls_client_auth/settings" \
    "${evidence_dir}/origin-tls-settings.json"
  cma_cf_capture_paginated "${token}" "/zones/${zone_id}/origin_tls_client_auth/hostnames?status=all" \
    "${evidence_dir}/origin-tls-hostnames.json" "${work_dir}/origin-tls-hostname-pages"
  cma_cf_capture_single "${token}" "/zones/${zone_id}/snippets/snippet_rules" \
    "${evidence_dir}/snippet-rules.json"
  cma_cf_capture_paginated "${token}" "/zones/${zone_id}/snippets" \
    "${evidence_dir}/snippets.json" "${work_dir}/snippet-pages"
  unset token
  rm -rf "${work_dir}"/*-pages "${work_dir}/ruleset-details"

  manifest_file="${work_dir}/manifest.jsonl"
  cma_cf_evidence_manifest "${evidence_dir}" "${manifest_file}" || \
    cma_cf_die "Cloudflare evidence manifest could not be assembled"
  completed_at="${CMA_CF_TEST_COMPLETED_AT:-$(date -u +%Y-%m-%dT%H:%M:%SZ)}"
  cma_cf_validate_token_window "${evidence_dir}/token.json" "${captured_at}" \
    "${completed_at}" || cma_cf_die "Cloudflare capture outlived its short-lived bearer"
  receipt_file="${work_dir}/receipt.json"
  cma_cf_emit_receipt "${evidence_dir}" "${captured_at}" "${completed_at}" "${tool_sha}" \
    "${receipt_file}" "${manifest_file}" || \
    cma_cf_die "Cloudflare evidence did not satisfy the closed capture contract"
  rm "${manifest_file}"
  receipt_sha="$(cma_cf_sha256 "${receipt_file}")" || cma_cf_die "receipt could not be hashed"
  printf '%s  receipt.json\n' "${receipt_sha}" >"${work_dir}/receipt.sha256"
  chmod 400 "${evidence_dir}"/* "${receipt_file}" "${work_dir}/receipt.sha256"
  cma_cf_fsync "${evidence_dir}"/* "${evidence_dir}" "${receipt_file}" \
    "${work_dir}/receipt.sha256" "${work_dir}" "${parent_dir}" || \
    cma_cf_die "Cloudflare evidence could not be durably fsynced"
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    source, destination = ARGV
    raise "destination exists" if File.exist?(destination) || File.symlink?(destination)
    File.rename(source, destination)
    File.open(File.dirname(destination), File::RDONLY) { |directory| directory.fsync }
  ' "${work_dir}" "${output_dir}" || cma_cf_die "capture could not be atomically published"
  trap - EXIT HUP INT QUIT TERM
  printf 'CMA Cloudflare evidence directory: %s\n' "${output_dir}"
  printf 'CMA Cloudflare receipt SHA-256: %s\n' "${receipt_sha}"
}

cma_cf_verify_receipt() {
  local receipt_file="$1"
  local bundle_dir="${receipt_file%/*}"
  local evidence_dir="${bundle_dir}/evidence"
  local expected_sha live_sha recorded_sha tool_sha work_dir manifest_file captured_at completed_at
  local entry_count evidence_count

  [[ "${receipt_file}" == /*/receipt.json && -f "${receipt_file}" && \
    ! -L "${receipt_file}" && -O "${receipt_file}" && \
    -d "${bundle_dir}" && ! -L "${bundle_dir}" && -O "${bundle_dir}" ]] || return 1
  entry_count="$(find "${bundle_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" || return 1
  [[ "${entry_count}" == 3 && -d "${evidence_dir}" && ! -L "${evidence_dir}" ]] || return 1
  evidence_count="$(find "${evidence_dir}" -mindepth 1 -maxdepth 1 -print | wc -l | tr -d ' ')" || return 1
  [[ "${evidence_count}" == 29 ]] || return 1
  expected_sha="${CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256:-}"
  [[ "${expected_sha}" =~ ^[0-9a-f]{64}$ ]] || return 1
  live_sha="$(cma_cf_sha256 "${receipt_file}")" || return 1
  recorded_sha="$(awk 'NF == 2 && $2 == "receipt.json" {print $1}' \
    "${bundle_dir}/receipt.sha256")" || return 1
  [[ "$(wc -l < "${bundle_dir}/receipt.sha256" | tr -d ' ')" == 1 && \
    "${live_sha}" == "${expected_sha}" && "${live_sha}" == "${recorded_sha}" ]] || return 1
  tool_sha="$(cma_cf_sha256 "${BASH_SOURCE[0]}")" || return 1
  [[ "$(jq -er .toolSourceSha256 "${receipt_file}")" == "${tool_sha}" ]] || return 1
  captured_at="$(jq -er .capturedAt "${receipt_file}")" || return 1
  completed_at="$(jq -er .completedAt "${receipt_file}")" || return 1
  cma_cf_validate_token_window "${evidence_dir}/token.json" "${captured_at}" \
    "${completed_at}" || return 1
  work_dir="$(mktemp -d)" || return 1
  trap 'rm -rf "${work_dir}"' RETURN
  manifest_file="${work_dir}/manifest.jsonl"
  cma_cf_evidence_manifest "${evidence_dir}" "${manifest_file}" || return 1
  cma_cf_emit_receipt "${evidence_dir}" "${captured_at}" "${completed_at}" "${tool_sha}" \
    "${work_dir}/receipt.json" "${manifest_file}" || return 1
  cmp "${work_dir}/receipt.json" "${receipt_file}" >/dev/null || return 1
  rm -rf "${work_dir}"
  trap - RETURN
}

cma_cf_derive_policy() {
  local receipt_file="$1"
  local receipt_sha
  receipt_sha="$(cma_cf_sha256 "${receipt_file}")" || return 1
  cma_cf_verify_receipt "${receipt_file}" || return 1
  jq --arg source_sha "${receipt_sha}" '
    {
      schemaVersion:1,
      status:"candidate-pending-review",
      sourceEvidenceSha256:$source_sha,
      zoneId:.zone.id,
      accountId:.behaviorProjection.account.id,
      expected:{behaviorProjection:.behaviorProjection,settings:.settings}
    }
  ' "${receipt_file}"
}

cma_cf_usage() {
  cat >&2 <<'USAGE'
usage:
  cma-trigger-gateway-cloudflare-capture.sh --capture ABSOLUTE_NEW_DIRECTORY
  cma-trigger-gateway-cloudflare-capture.sh --verify-receipt ABSOLUTE_RECEIPT.json
  cma-trigger-gateway-cloudflare-capture.sh --derive-policy ABSOLUTE_RECEIPT.json

Capture performs authenticated GET requests only. The short-lived token must be
supplied on protected descriptor 7 through the trusted launcher.
USAGE
}

cma_cf_main() {
  case "${1:-}" in
    --capture)
      [[ "$#" -eq 2 ]] || cma_cf_die "--capture requires one absolute destination"
      cma_cf_require_launcher
      cma_cf_require_tools
      cma_cf_capture "$2"
      ;;
    --verify-receipt)
      [[ "$#" -eq 2 ]] || cma_cf_die "--verify-receipt requires one absolute receipt"
      cma_cf_require_tools
      cma_cf_verify_receipt "$2" || cma_cf_die "Cloudflare evidence receipt is invalid"
      printf '%s\n' "CMA Cloudflare evidence receipt is exact"
      ;;
    --derive-policy)
      [[ "$#" -eq 2 ]] || cma_cf_die "--derive-policy requires one absolute receipt"
      cma_cf_require_tools
      cma_cf_derive_policy "$2" || \
        cma_cf_die "Cloudflare policy candidate could not be derived from exact evidence"
      ;;
    *)
      cma_cf_usage
      return 2
      ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  cma_cf_main "$@"
fi
