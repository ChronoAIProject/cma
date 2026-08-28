#!/bin/bash -p
set -euo pipefail
umask 077

{ set +x; } 2>/dev/null
unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH
unset ALL_PROXY FTP_PROXY HTTPS_PROXY HTTP_PROXY NO_PROXY all_proxy ftp_proxy https_proxy http_proxy no_proxy
unset CURL_CA_BUNDLE CURL_HOME CURL_SSL_BACKEND SSL_CERT_DIR SSL_CERT_FILE SSLKEYLOGFILE
for cma_origin_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_origin_name}"
done
unset cma_origin_name

readonly CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED=0
readonly CMA_ORIGIN_POLICY=.github/scripts/cma-trigger-gateway-cloudflare-policy.json
readonly CMA_GATEWAY_BEHAVIOR_POLICY=.github/scripts/cma-trigger-gateway-live-policy.json
readonly CMA_ORIGIN_PROJECTION=.github/scripts/cma-trigger-gateway-projection.sh
readonly CMA_CLOUDFLARE_CAPTURE=.github/scripts/cma-trigger-gateway-cloudflare-capture.sh
readonly CMA_CLOUDFLARE_IPV4='173.245.48.0/20 103.21.244.0/22 103.22.200.0/22 103.31.4.0/22 141.101.64.0/18 108.162.192.0/18 190.93.240.0/20 188.114.96.0/20 197.234.240.0/22 198.41.128.0/17 162.158.0.0/15 104.16.0.0/13 104.24.0.0/14 172.64.0.0/13 131.0.72.0/22'
readonly CMA_CLOUDFLARE_IPV6='2400:cb00::/32 2606:4700::/32 2803:f800::/32 2405:b500::/32 2405:8100::/32 2a06:98c0::/29 2c0f:f248::/32'
CMA_ORIGIN_CLOUDFLARE_TOKEN=""

cma_origin_die() {
  printf 'cma-trigger origin evidence: %s\n' "$*" >&2
  exit 1
}

cma_origin_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cma_origin_fsync() {
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    ARGV.each { |path| File.open(path, File::RDONLY) { |file| file.fsync } }
  ' "$@"
}

cma_origin_require_launcher() {
  [[ "${CMA_GATEWAY_PRODUCTION_MUTATION_ENABLED}" == 0 ]] || \
    cma_origin_die "the checked-in production mutation gate is unexpectedly open"
  [[ "${CMA_GATEWAY_TRUSTED_LAUNCH:-}" == env-i-v1 && \
    "${CMA_GATEWAY_TOOLCHAIN_TRUST_CONFIRM:-}" == reviewed-host-toolchain && \
    -n "${CMA_GATEWAY_TRUSTED_PATH:-}" && "${PATH}" == "${CMA_GATEWAY_TRUSTED_PATH}" ]] || \
    cma_origin_die "evidence capture requires the reviewed env-i host-toolchain launcher"
}

cma_origin_require_tools() {
  local command_name
  for command_name in awk chmod curl date find jq mktemp mv rm ruby wc; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_origin_die "required command is unavailable: ${command_name}"
  done
}

cma_origin_require_writer_freeze() {
  [[ "${CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_CONFIRM:-}" == \
      writers-disabled-through-origin-probes ]] || \
    cma_origin_die "the Cloudflare writer freeze is not explicitly attested"
  [[ "${CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID:-}" =~ \
      ^[A-Za-z0-9][A-Za-z0-9._:/-]{0,127}$ ]] || \
    cma_origin_die "the Cloudflare writer freeze needs a bounded external record ID"
}

cma_origin_read_cloudflare_token() {
  [[ "${CMA_GATEWAY_CLOUDFLARE_TOKEN_FD:-}" == 7 && -r /dev/fd/7 ]] || \
    cma_origin_die "origin capture requires a short-lived Cloudflare bearer on descriptor 7"
  if ! IFS= read -r CMA_ORIGIN_CLOUDFLARE_TOKEN <&7; then
    exec 7<&-
    cma_origin_die "the protected Cloudflare bearer could not be read exactly once"
  fi
  exec 7<&-
  unset CMA_GATEWAY_CLOUDFLARE_TOKEN_FD CLOUDFLARE_API_TOKEN
  [[ -n "${CMA_ORIGIN_CLOUDFLARE_TOKEN}" ]] || \
    cma_origin_die "the protected Cloudflare bearer is empty"
  case "${CMA_ORIGIN_CLOUDFLARE_TOKEN}" in
    *$'\r'*|*$'\n'*)
      CMA_ORIGIN_CLOUDFLARE_TOKEN=""
      cma_origin_die "the protected Cloudflare bearer must not contain CR or LF"
      ;;
  esac
}

cma_origin_capture_cloudflare() {
  local destination="$1"
  printf '%s\n' "${CMA_ORIGIN_CLOUDFLARE_TOKEN}" | (
    exec 7<&0
    exec 0</dev/null
    CMA_GATEWAY_CLOUDFLARE_TOKEN_FD=7
    export CMA_GATEWAY_CLOUDFLARE_TOKEN_FD
    "${CMA_CLOUDFLARE_CAPTURE}" --capture "${destination}" >/dev/null
  )
}

cma_origin_validate_projection() {
  local projection="$1"
  local behavior_policy_sha behavior_source_sha
  [[ "${projection}" == /* && -f "${projection}" && ! -L "${projection}" && -O "${projection}" ]] || \
    cma_origin_die "projection receipt is not a regular non-symlink file"
  behavior_policy_sha="$(cma_origin_sha256 "${CMA_GATEWAY_BEHAVIOR_POLICY}")" || \
    cma_origin_die "reviewed Gateway behavior policy could not be hashed"
  behavior_source_sha="$(jq -er '.sourceEvidenceSha256 | select(test("^[0-9a-f]{64}$"))' \
    "${CMA_GATEWAY_BEHAVIOR_POLICY}")" || \
    cma_origin_die "reviewed Gateway behavior policy is not approved evidence"
  "${CMA_ORIGIN_PROJECTION}" --check-live "${projection}" >/dev/null || \
    cma_origin_die "live graph differs from the sealed source projection"
  jq -er --arg policy_sha "${behavior_policy_sha}" --arg source_sha "${behavior_source_sha}" '
    if .schemaVersion == 1 and .mode == "source" and
       (.graph.gatewayIp | type) == "string" and
       .behaviorPolicy.policySha256 == $policy_sha and
       .behaviorPolicy.sourceEvidenceSha256 == $source_sha
    then .graph.gatewayIp else error("projection receipt is not an approved source graph") end
  ' "${projection}"
}

cma_origin_validate_cloudflare_contract() {
  local receipt="$1"
  local origin_ip="$2"
  local policy_file="${3:-${CMA_ORIGIN_POLICY}}"
  jq -e --slurpfile policy "${policy_file}" --arg origin_ip "${origin_ip}" \
    --arg ipv4 "${CMA_CLOUDFLARE_IPV4}" --arg ipv6 "${CMA_CLOUDFLARE_IPV6}" '
      def cma_dns_exact($records; $host; $ip):
        ($records[$host] | type) == "array" and
        ($records[$host] | length) == 1 and
        $records[$host][0].name == $host and
        $records[$host][0].type == "A" and
        $records[$host][0].proxied == true and
        $records[$host][0].content == $ip;
      def cma_indirection_key:
        . == "id" or . == "ids" or
        test("(^|_)(list|lists|ruleset|rulesets|pool|pools|monitor|monitors)(_id|_ids)?$");
      def cma_external_rule_reference:
        (.action == "execute") or
        ((((.expression // "") | type) == "string") and
          ((.expression // "") | test("\\$[A-Za-z_][A-Za-z0-9_]*"))) or
        (((.action_parameters // {}) |
          [paths(scalars) as $path |
           ($path[-1] | tostring | ascii_downcase) |
           select(cma_indirection_key)] | length) > 0);
      ($ipv4 | split(" ") | sort) as $official4 |
      ($ipv6 | split(" ") | sort) as $official6 |
      ["bot.chrono-ai.fun", "bot-api.chrono-ai.fun"] as $cma_hosts |
      $policy[0].schemaVersion == 1 and $policy[0].status == "approved" and
      ($policy[0].sourceEvidenceSha256 | test("^[0-9a-f]{64}$")) and
      ($policy[0].zoneId | test("^[0-9a-f]{32}$")) and
      ($policy[0].accountId | test("^[0-9a-f]{32}$")) and
      ($policy[0].expected | type) == "object" and
      (($policy[0].expected | keys | sort) == ["behaviorProjection", "settings"]) and
      .schemaVersion == 1 and .kind == "CmaGatewayCloudflareCapture" and
      .status == "captured-read-only" and
      .zone.id == $policy[0].zoneId and .zone.name == "chrono-ai.fun" and
      .behaviorProjection.account.id == $policy[0].accountId and
      .behaviorProjection == $policy[0].expected.behaviorProjection and
      .settings == $policy[0].expected.settings and
      .settings.pseudoIPv4 == "Disabled" and
      .settings.removeVisitorIpHeaders == false and
      .settings.identityHeaderWorkers == [] and
      .settings.identityHeaderTransforms == [] and
      (.settings.cloudflareIpv4Cidrs | sort) == $official4 and
      (.settings.cloudflareIpv6Cidrs | sort) == $official6 and
      .settings.addHeaderCompatibilityEvidence == "not-required" and
      ([$cma_hosts[] as $host |
        cma_dns_exact(.settings.dnsProxyRecords; $host; $origin_ip) and
        cma_dns_exact(.behaviorProjection.dnsProxyRecords; $host; $origin_ip)] |
        all) and
      .behaviorProjection.workerRoutes == [] and
      .behaviorProjection.workerDomains == [] and
      .behaviorProjection.accessApplications == {account:[],zone:[]} and
      .behaviorProjection.waitingRooms == [] and
      .behaviorProjection.legacyFirewallRules == [] and
      .behaviorProjection.legacyRateLimits == [] and
      .behaviorProjection.accountRulesets == [] and
      .behaviorProjection.loadBalancers == [] and
      .behaviorProjection.snippetRules == [] and
      ([.behaviorProjection.originTlsClientAuthHostnames[] |
        select(.hostname == $cma_hosts[])] | length) == 0 and
      ([.behaviorProjection.rulesets[].rules[]? |
        select(cma_external_rule_reference)] | length) == 0
    ' "${receipt}" >/dev/null
}

cma_origin_validate_cloudflare() {
  local receipt="$1"
  local origin_ip="$2"
  local receipt_sha recorded_sha policy_sha
  [[ "${receipt}" == /*/receipt.json && -f "${receipt}" && \
    ! -L "${receipt}" && -O "${receipt}" ]] || \
    cma_origin_die "Cloudflare evidence receipt is absent or not a regular file"
  receipt_sha="$(cma_origin_sha256 "${receipt}")" || \
    cma_origin_die "Cloudflare receipt could not be hashed"
  recorded_sha="$(awk 'NF == 2 && $2 == "receipt.json" {print $1}' \
    "${receipt%/*}/receipt.sha256")" || \
    cma_origin_die "Cloudflare receipt digest could not be read"
  [[ "$(wc -l < "${receipt%/*}/receipt.sha256" | tr -d ' ')" == 1 && \
    "${recorded_sha}" == "${receipt_sha}" ]] || \
    cma_origin_die "Cloudflare receipt differs from its capture digest"
  policy_sha="$(cma_origin_sha256 "${CMA_ORIGIN_POLICY}")" || \
    cma_origin_die "Cloudflare policy could not be hashed"
  [[ "${policy_sha}" == "${CMA_GATEWAY_CLOUDFLARE_POLICY_SHA256:-}" ]] || \
    cma_origin_die "Cloudflare policy differs from its reviewed digest"
  CMA_GATEWAY_CLOUDFLARE_RECEIPT_SHA256="${receipt_sha}" \
    "${CMA_CLOUDFLARE_CAPTURE}" --verify-receipt "${receipt}" >/dev/null || \
    cma_origin_die "Cloudflare evidence is not an exact tool-produced capture bundle"
  cma_origin_validate_cloudflare_contract "${receipt}" "${origin_ip}" || \
    cma_origin_die "Cloudflare behavior, origin binding, or dependency closure is unapproved"
  printf '%s\n' "${receipt_sha}"
}

cma_origin_compare_cloudflare_window() {
  local pre_receipt="$1"
  local post_receipt="$2"
  local probes_started_at="$3"
  local probes_completed_at="$4"
  jq -e -s '
    .[0].zone == .[1].zone and
    .[0].behaviorProjection == .[1].behaviorProjection and
    .[0].settings == .[1].settings
  ' "${pre_receipt}" "${post_receipt}" >/dev/null || return 1
  ruby --disable-gems -rjson -rtime -e '
    pre = JSON.parse(File.read(ARGV.fetch(0)))
    post = JSON.parse(File.read(ARGV.fetch(1)))
    probes_started = Time.iso8601(ARGV.fetch(2))
    probes_completed = Time.iso8601(ARGV.fetch(3))
    pre_started = Time.iso8601(pre.fetch("capturedAt"))
    pre_completed = Time.iso8601(pre.fetch("completedAt"))
    post_started = Time.iso8601(post.fetch("capturedAt"))
    post_completed = Time.iso8601(post.fetch("completedAt"))
    raise unless pre_started <= pre_completed
    raise unless pre_completed <= probes_started
    raise unless probes_started <= probes_completed
    raise unless probes_completed <= post_started
    raise unless post_started <= post_completed
  ' "${pre_receipt}" "${post_receipt}" "${probes_started_at}" "${probes_completed_at}"
}

cma_origin_validate_public_chain() {
  local meta_file="$1"
  local headers_file="$2"
  local origin_ip="$3"
  local cloudflare_receipt="$4"
  ruby --disable-gems -rjson -ripaddr -e '
    meta_path, headers_path, origin_text, receipt_path = ARGV
    meta_lines = File.readlines(meta_path, chomp: true)
    raise "public metadata line count" unless meta_lines.length == 1
    status, remote_text, tls, extra = meta_lines.fetch(0).split("|", -1)
    raise "public metadata shape" unless extra.nil? && status == "200" && tls == "0"
    remote = IPAddr.new(remote_text)
    origin = IPAddr.new(origin_text)
    raise "public IP is not canonical" unless remote.to_s == remote_text
    raise "origin IP is not canonical" unless origin.to_s == origin_text
    raise "public request reached the origin directly" if remote == origin
    receipt = JSON.parse(File.read(receipt_path))
    cidrs = receipt.fetch("settings").fetch(remote.ipv4? ? "cloudflareIpv4Cidrs" : "cloudflareIpv6Cidrs")
    networks = cidrs.map { |cidr| IPAddr.new(cidr) }
    raise "public IP is outside official Cloudflare networks" unless networks.any? { |network| network.include?(remote) }
    header_lines = File.binread(headers_path).split(/\r?\n/)
    statuses = header_lines.grep(/\AHTTP\/\S+\s+200(?:\s|\z)/i)
    values = lambda do |name|
      header_lines.map do |line|
        match = /\A#{Regexp.escape(name)}:\s*(.+)\z/i.match(line)
        match && match[1].strip
      end.compact
    end
    rays = values.call("cf-ray")
    servers = values.call("server")
    cache_statuses = values.call("cf-cache-status")
    ages = values.call("age")
    raise "public response block is not exact" unless statuses.length == 1
    raise "CF-Ray identity is absent or ambiguous" unless rays.length == 1
    raise "Cloudflare Server identity is absent or ambiguous" unless servers.length == 1 && servers.fetch(0).casecmp?("cloudflare")
    raise "public response is not a fresh dynamic origin response" unless cache_statuses.length == 1 && cache_statuses.fetch(0).casecmp?("dynamic")
    raise "public response carries a cache age" unless ages.empty?
    ray = rays.fetch(0)
    match = /\A([0-9a-fA-F]{16,32})-([A-Z]{3})\z/.match(ray)
    raise "CF-Ray identity is malformed" unless match
    family = remote.ipv4? ? "IPv4" : "IPv6"
    puts JSON.generate({remoteIp: remote.to_s, addressFamily: family,
                        cfRay: ray, edgeColo: match[2], server: "cloudflare",
                        cacheStatus: "DYNAMIC", ageHeader: false})
  ' "${meta_file}" "${headers_file}" "${origin_ip}" "${cloudflare_receipt}"
}

cma_origin_validate_probe_body() {
  local body_file="$1"
  local expected="$2"
  case "${expected}" in
    livez) [[ "$(<"${body_file}")" == ok ]] ;;
    ready) jq -e '. == {status:"ready"}' "${body_file}" >/dev/null ;;
    openapi) jq -e '.openapi == "3.1.0" and (.paths["/api/v1/sessions"].get | type) == "object"' "${body_file}" >/dev/null ;;
    agent) jq -e 'type == "object"' "${body_file}" >/dev/null ;;
    *) return 1 ;;
  esac
}

cma_origin_probe() {
  local host="$1" path="$2" expected="$3" origin_ip="$4" prefix="$5"
  local body_file="${prefix}.body" meta_file="${prefix}.meta"
  curl --disable --noproxy '*' --fail --show-error --silent --max-time 20 \
    --proto '=https' --tlsv1.2 --resolve "${host}:443:${origin_ip}" \
    --output "${body_file}" \
    --write-out '%{http_code}|%{remote_ip}|%{ssl_verify_result}\n' \
    "https://${host}${path}" >"${meta_file}" || return 1
  [[ "$(<"${meta_file}")" == "200|${origin_ip}|0" ]] || return 1
  cma_origin_validate_probe_body "${body_file}" "${expected}"
}

cma_origin_public_probe() {
  local host="$1" path="$2" expected="$3" nonce="$4" origin_ip="$5"
  local cloudflare_receipt="$6" prefix="$7"
  local body_file="${prefix}.body" headers_file="${prefix}.headers"
  local meta_file="${prefix}.meta" request_target edge
  [[ "${nonce}" =~ ^[0-9a-f]{32}$ ]] || return 1
  request_target="${path}?cma_probe=${nonce}"
  curl --disable --noproxy '*' --fail --show-error --silent --max-time 20 \
    --proto '=https' --tlsv1.2 --output "${body_file}" \
    --dump-header "${headers_file}" \
    --write-out '%{http_code}|%{remote_ip}|%{ssl_verify_result}\n' \
    "https://${host}${request_target}" >"${meta_file}" || return 1
  cma_origin_validate_probe_body "${body_file}" "${expected}" || return 1
  edge="$(cma_origin_validate_public_chain "${meta_file}" "${headers_file}" \
    "${origin_ip}" "${cloudflare_receipt}")" || return 1
  jq -cn --arg host "${host}" --arg target "${request_target}" \
    --arg nonce "${nonce}" --argjson edge "${edge}" \
    '$edge + {host:$host,requestTarget:$target,nonce:$nonce}'
}

cma_origin_main() {
  [[ "$#" -eq 1 ]] || cma_origin_die "usage: ${0##*/} PROJECTION.json"
  cma_origin_require_launcher
  cma_origin_require_tools
  cma_origin_require_writer_freeze
  local projection="$1" origin_ip output_dir parent_dir work_dir
  local projection_before projection_after behavior_policy_sha cloudflare_policy_sha
  local cloudflare_pre_receipt cloudflare_post_receipt cloudflare_pre_sha cloudflare_post_sha
  local probes_started_at probes_completed_at nonce public_api_edge public_app_edge
  local behavior_projection_file behavior_projection_sha
  local records_file receipt_file receipt_sha receipt_digest_file evidence_file evidence_sha

  output_dir="${CMA_GATEWAY_ORIGIN_EVIDENCE_DIR:-}"
  [[ "${output_dir}" == /* && "${output_dir}" != / && \
    ! -e "${output_dir}" && ! -L "${output_dir}" ]] || \
    cma_origin_die "CMA_GATEWAY_ORIGIN_EVIDENCE_DIR must be a new absolute non-root path"
  parent_dir="${output_dir%/*}"
  [[ -n "${parent_dir}" ]] || parent_dir=/
  [[ -d "${parent_dir}" && ! -L "${parent_dir}" && -O "${parent_dir}" ]] || \
    cma_origin_die "origin evidence parent must be an owned existing non-symlink directory"
  work_dir="$(mktemp -d "${parent_dir}/.cma-origin-evidence.XXXXXXXX")" || \
    cma_origin_die "origin evidence work directory could not be created"
  chmod 700 "${work_dir}"
  trap 'rm -rf "${work_dir}"' EXIT HUP INT QUIT TERM

  projection_before="$(cma_origin_sha256 "${projection}")" || \
    cma_origin_die "projection could not be hashed"
  origin_ip="$(cma_origin_validate_projection "${projection}")"
  behavior_policy_sha="$(cma_origin_sha256 "${CMA_GATEWAY_BEHAVIOR_POLICY}")" || \
    cma_origin_die "Gateway behavior policy could not be hashed"
  cloudflare_policy_sha="$(cma_origin_sha256 "${CMA_ORIGIN_POLICY}")" || \
    cma_origin_die "Cloudflare policy could not be hashed"

  cma_origin_read_cloudflare_token
  cma_origin_capture_cloudflare "${work_dir}/cloudflare-pre" || \
    cma_origin_die "fresh pre-probe Cloudflare capture failed"
  cloudflare_pre_receipt="${work_dir}/cloudflare-pre/receipt.json"
  cloudflare_pre_sha="$(cma_origin_validate_cloudflare \
    "${cloudflare_pre_receipt}" "${origin_ip}")"
  probes_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  nonce="${cloudflare_pre_sha:0:32}"

  cma_origin_probe bot-api.chrono-ai.fun /livez livez "${origin_ip}" \
    "${work_dir}/origin-api-livez" || \
    cma_origin_die "direct origin liveness probe failed TLS, remote-IP, status, or body validation"
  cma_origin_probe bot-api.chrono-ai.fun /ready ready "${origin_ip}" \
    "${work_dir}/origin-api-ready" || \
    cma_origin_die "direct origin readiness probe failed TLS, remote-IP, status, or body validation"
  cma_origin_probe bot.chrono-ai.fun /openapi.json openapi "${origin_ip}" \
    "${work_dir}/origin-app-openapi" || \
    cma_origin_die "direct origin app-routing probe failed TLS, remote-IP, status, or body validation"
  public_api_edge="$(cma_origin_public_probe bot-api.chrono-ai.fun /ready ready \
    "${nonce}" "${origin_ip}" "${cloudflare_pre_receipt}" \
    "${work_dir}/public-api-ready")" || \
    cma_origin_die "ordinary-DNS API probe did not prove a fresh Cloudflare-to-origin response"
  public_app_edge="$(cma_origin_public_probe bot.chrono-ai.fun /openapi.json openapi \
    "${nonce}" "${origin_ip}" "${cloudflare_pre_receipt}" \
    "${work_dir}/public-app-openapi")" || \
    cma_origin_die "ordinary-DNS app probe did not prove a fresh Cloudflare-to-origin response"
  probes_completed_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"

  cma_origin_capture_cloudflare "${work_dir}/cloudflare-post" || \
    cma_origin_die "fresh post-probe Cloudflare capture failed"
  CMA_ORIGIN_CLOUDFLARE_TOKEN=""
  unset CMA_ORIGIN_CLOUDFLARE_TOKEN
  cloudflare_post_receipt="${work_dir}/cloudflare-post/receipt.json"
  cloudflare_post_sha="$(cma_origin_validate_cloudflare \
    "${cloudflare_post_receipt}" "${origin_ip}")"
  cma_origin_compare_cloudflare_window "${cloudflare_pre_receipt}" \
    "${cloudflare_post_receipt}" "${probes_started_at}" "${probes_completed_at}" || \
    cma_origin_die "Cloudflare behavior or temporal identity drifted across the probe window"

  "${CMA_ORIGIN_PROJECTION}" --check-live "${projection}" >/dev/null || \
    cma_origin_die "live projection drifted across origin/public probes"
  projection_after="$(cma_origin_sha256 "${projection}")" || \
    cma_origin_die "projection reread failed"
  [[ "${projection_before}" == "${projection_after}" ]] || \
    cma_origin_die "sealed projection identity drifted across probes"

  behavior_projection_file="${work_dir}/.behavior-projection.json"
  jq -cS '.behaviorProjection' "${cloudflare_pre_receipt}" >"${behavior_projection_file}" || \
    cma_origin_die "Cloudflare behavior projection could not be canonicalized"
  behavior_projection_sha="$(cma_origin_sha256 "${behavior_projection_file}")" || \
    cma_origin_die "Cloudflare behavior projection could not be hashed"
  rm "${behavior_projection_file}"
  records_file="${work_dir}/.probe-records.jsonl"
  : >"${records_file}"
  for evidence_file in \
      origin-api-livez.body origin-api-livez.meta \
      origin-api-ready.body origin-api-ready.meta \
      origin-app-openapi.body origin-app-openapi.meta \
      public-api-ready.body public-api-ready.headers public-api-ready.meta \
      public-app-openapi.body public-app-openapi.headers public-app-openapi.meta; do
    evidence_sha="$(cma_origin_sha256 "${work_dir}/${evidence_file}")" || \
      cma_origin_die "probe evidence could not be hashed: ${evidence_file}"
    jq -cn --arg file "${evidence_file}" --arg sha256 "${evidence_sha}" \
      '{file:$file,sha256:$sha256}' >>"${records_file}" || \
      cma_origin_die "probe evidence record could not be written"
  done
  receipt_file="${work_dir}/receipt.json"
  jq -s --arg projection "${projection}" --arg projection_sha256 "${projection_before}" \
    --arg behavior_policy_sha256 "${behavior_policy_sha}" \
    --arg cloudflare_policy_sha256 "${cloudflare_policy_sha}" \
    --arg pre_receipt_sha256 "${cloudflare_pre_sha}" \
    --arg post_receipt_sha256 "${cloudflare_post_sha}" \
    --arg behavior_projection_sha256 "${behavior_projection_sha}" \
    --arg writer_freeze_id "${CMA_GATEWAY_CLOUDFLARE_WRITER_FREEZE_ID}" \
    --arg probes_started_at "${probes_started_at}" \
    --arg probes_completed_at "${probes_completed_at}" \
    --arg origin_ip "${origin_ip}" --argjson public_api_edge "${public_api_edge}" \
    --argjson public_app_edge "${public_app_edge}" \
    --slurpfile projection_body "${projection}" \
    --slurpfile pre "${cloudflare_pre_receipt}" \
    --slurpfile post "${cloudflare_post_receipt}" '
      {
        schemaVersion:2,kind:"CmaGatewayOriginEvidence",
        projection:{path:$projection,sha256:$projection_sha256,
          captureSha256:$projection_body[0].captureSha256,
          behaviorPolicy:$projection_body[0].behaviorPolicy},
        behaviorPolicyFileSha256:$behavior_policy_sha256,
        cloudflare:{policySha256:$cloudflare_policy_sha256,
          writerFreeze:{confirmation:"writers-disabled-through-origin-probes",
            externalRecordId:$writer_freeze_id},
          behaviorProjectionSha256:$behavior_projection_sha256,
          pre:{path:"cloudflare-pre/receipt.json",sha256:$pre_receipt_sha256,
            capturedAt:$pre[0].capturedAt,completedAt:$pre[0].completedAt,
            zone:$pre[0].zone},
          probes:{startedAt:$probes_started_at,completedAt:$probes_completed_at},
          post:{path:"cloudflare-post/receipt.json",sha256:$post_receipt_sha256,
            capturedAt:$post[0].capturedAt,completedAt:$post[0].completedAt,
            zone:$post[0].zone},exactBehaviorProjectionStable:true},
        originIp:$origin_ip,publicEdges:{api:$public_api_edge,app:$public_app_edge},probes:.
      }
    ' "${records_file}" >"${receipt_file}" || \
    cma_origin_die "closed origin evidence receipt could not be emitted"
  rm "${records_file}"
  receipt_sha="$(cma_origin_sha256 "${receipt_file}")" || \
    cma_origin_die "origin evidence receipt could not be hashed"
  receipt_digest_file="${work_dir}/receipt.sha256"
  printf '%s  receipt.json\n' "${receipt_sha}" >"${receipt_digest_file}"
  find "${work_dir}" -type f -exec chmod 400 {} +
  cma_origin_fsync "${work_dir}"/* "${work_dir}" "${parent_dir}" || \
    cma_origin_die "origin evidence could not be durably fsynced"
  env -i PATH="${PATH}" LC_ALL=C ruby --disable-gems -e '
    source, destination = ARGV
    raise "destination exists" if File.exist?(destination) || File.symlink?(destination)
    File.rename(source, destination)
    File.open(File.dirname(destination), File::RDONLY) { |directory| directory.fsync }
  ' "${work_dir}" "${output_dir}" || \
    cma_origin_die "origin evidence could not be atomically published"
  trap - EXIT HUP INT QUIT TERM
  printf '%s\n' "CMA origin/public evidence captured at ${output_dir}; SHA-256 ${receipt_sha}; origin ${origin_ip}; Cloudflare ${cloudflare_pre_sha} -> ${cloudflare_post_sha}"
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  cma_origin_main "$@"
fi
