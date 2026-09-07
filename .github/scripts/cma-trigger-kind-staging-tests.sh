#!/bin/bash -p

{ set +x; } 2>/dev/null
set -euo pipefail
umask 077

unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH

CMA_KIND_TEST_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)"
readonly CMA_KIND_TEST_ROOT
readonly CMA_KIND_TEST_SCRIPT="${CMA_KIND_TEST_ROOT}/.github/scripts/cma-trigger-kind-staging.sh"
readonly CMA_KIND_TEST_FIXTURE="${CMA_KIND_TEST_ROOT}/.github/scripts/cma-trigger-kind-staging-fixture.mjs"
CMA_KIND_TEST_TEMPORARY=""

cma_kind_test_die() {
  printf 'cma-trigger Kind staging tests: %s\n' "$*" >&2
  exit 1
}

cma_kind_test_require_commands() {
  local command_name
  for command_name in "$@"; do
    command -v "${command_name}" >/dev/null 2>&1 || \
      cma_kind_test_die "required command is unavailable: ${command_name}"
  done
}

cma_kind_test_sha256() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cma_kind_test_base64() {
  base64 < "$1" | tr -d '\r\n'
}

cma_kind_test_contract() {
  ruby - "${1:-${CMA_KIND_TEST_SCRIPT}}" "${CMA_KIND_TEST_FIXTURE}" <<'RUBY'
script_path, fixture_path = ARGV
source = File.read(script_path)
fixture = File.read(fixture_path)

def check(condition, message)
  raise message unless condition
end

def constant(source, name)
  match = source.match(/^readonly #{Regexp.escape(name)}="([^"]+)"$/)
  raise "missing closed constant #{name}" unless match
  match[1]
end

def in_order?(source, *needles)
  offset = 0
  needles.all? do |needle|
    found = source.index(needle, offset)
    next false unless found

    offset = found + needle.length
    true
  end
end

def function_body(source, name, following)
  match = source.match(/^#{Regexp.escape(name)}\(\) \{\n(.*?)^#{Regexp.escape(following)}\(\) \{/m)
  raise "cannot isolate #{name}" unless match
  match[1]
end

check(source.start_with?("#!/bin/bash -p\n\n{ set +x; }"), "staging must suppress tracing before strict mode")
check(constant(source, "CMA_STAGING_CLUSTER_SERVER") == "https://127.0.0.1:6443", "Kind endpoint drifted")
check(constant(source, "CMA_STAGING_PUBLIC_ORIGIN") == "https://127.0.0.1:8443", "local CMA TLS origin drifted")
check(constant(source, "CMA_STAGING_OBSERVER_KUBECONFIG_NAME") == "cma-trigger-smoke-kubeconfig", "observer path drifted")
check(constant(source, "CMA_STAGING_ATTESTATION_CONFIGMAP") == "cma-trigger-runtime-provenance", "redacted attestation object drifted")
check(constant(source, "CMA_STAGING_REPOSITORY_ID") == "1330597394", "approved image repository drifted")
check(source.match?(/^readonly CMA_STAGING_CLEANUP_SCHEMA_VERSION=2$/), "cleanup receipt schema drifted")
check(source.include?("cma_staging_require_value CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256"), "observer digest input disappeared")
check(source.include?('expected_digest="${CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256}"'), "observer digest binding disappeared")
check(source.include?("CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64"), "protected Kind CA input disappeared")
check(source.include?('exact_keys(user["user"], ["client-certificate-data", "client-key-data"]'), "token or exec kubeconfig auth became admissible")
check(source.include?('[[ "${subject}" == "CN=${CMA_STAGING_OBSERVER_CN}" ]]'), "exact observer certificate subject check disappeared")
check(source.include?('[[ "${ca_cert_key}" == "${ca_private_key}" ]]'), "observer CA/private-key match check disappeared")

check(source.include?('hostPath: "${root}/bootstrap/pki/ca.crt"'), "Kind CA certificate file mount disappeared")
check(source.include?('hostPath: "${root}/bootstrap/pki/ca.key"'), "Kind CA private-key file mount disappeared")
check(!source.include?("hostPath: \"\${root}/bootstrap/pki\"\n"), "whole Kubernetes PKI directory is host-mounted")
check(in_order?(source,
  'docker exec "${node}" umount /etc/kubernetes/pki/ca.key',
  'docker exec "${node}" rm -f -- /etc/kubernetes/pki/ca.key',
  'shred -u -- "${ca_key}"',
  '/etc/kubernetes/admin.conf /etc/kubernetes/super-admin.conf',
  'shred -u -- "${admin_kubeconfig}"',
  ': > "${root}/phase-sealed"'), "cluster provisioning authority is not sealed in order")
check(source.include?('shred -u -- "$credential"'), "in-node administrator credentials are retained")

ordinary_origin = constant(source, "CMA_STAGING_ORDINARY_PROVIDER_ORIGIN")
trigger_origin = constant(source, "CMA_STAGING_TRIGGER_PROVIDER_ORIGIN")
ordinary_namespace = constant(source, "CMA_STAGING_ORDINARY_NAMESPACE")
system_namespace = constant(source, "CMA_STAGING_SYSTEM_NAMESPACE")
check(ordinary_origin != trigger_origin, "ordinary and Trigger provider origins are reused")
check(ordinary_namespace != system_namespace, "ordinary and system namespaces are reused")
check(source.include?('! cmp -s "${ordinary_key}" "${trigger_key}"'), "provider-key separation check disappeared")
prepare = function_body(source, "cma_staging_prepare", "cma_staging_handoff_field")
check(in_order?(prepare,
  "cma_staging_provider_request_test opensandbox-ordinary",
  '"${CMA_STAGING_TRIGGER_PROVIDER_ORIGIN}" 401'), "cross-lane ordinary credential denial disappeared")
check(in_order?(prepare,
  "cma_staging_provider_request_test opensandbox-trigger",
  '"${CMA_STAGING_ORDINARY_PROVIDER_ORIGIN}" 401'), "cross-lane Trigger credential denial disappeared")
check(in_order?(prepare,
  'cma_staging_validate_approved_images "${root}"',
  'cma_staging_create_cluster "${root}"',
  'cma_staging_kind load image-archive',
  'cma_staging_capture_node_image "${root}" api',
  'cma_staging_capture_node_image "${root}" runtime'), "approved images are not validated, loaded, and captured in order")

approved_images = function_body(source, "cma_staging_validate_approved_images", "cma_staging_create_cluster")
%w[
  CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH
  CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE
  CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE
  CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID
  CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID
].each do |input|
  check(approved_images.include?("cma_staging_require_value #{input}"), "approved image input disappeared: #{input}")
end
check(approved_images.include?('docker image load --input "${archive}"'), "approved archive is not reloaded immediately before staging")
check(approved_images.include?("File.realpath"), "approved image custody may traverse a parent symlink")
check(approved_images.include?('.images[$component].archive_sha256'), "approved archive hash binding disappeared")
check(approved_images.include?('.images[$component].image_id'), "approved image ID binding disappeared")
check(!approved_images.include?("docker build"), "staging still rebuilds approved CI images")

check(source.include?("resources: [\"batchsandboxes\"]\n    verbs: [\"list\"]"), "observer is not list-only for BatchSandboxes")
check(source.include?("resourceNames: [\"\${CMA_STAGING_ATTESTATION_CONFIGMAP}\"]\n    verbs: [\"get\"]"), "observer lacks exact redacted attestation access")
check(source.include?('args: ["--auth", "--keyFile", "/var/run/mongo-keyfile/keyfile", "--replSet", "rs0", "--bind_ip_all"]'), "Mongo authentication or replica-set configuration drifted")
check(source.include?("?replicaSet=rs0&authSource=admin"), "CMA Mongo URI does not require authenticated replica-set access")
check(source.include?('cma-trigger-smoke-mongo-keyfile'), "Mongo keyfile custody disappeared")
check(source.include?('NODE_EXTRA_CA_CERTS="${root}/credentials/local-tls/ca.crt"'), "fixture lacks explicit local CA trust")
check(source.include?('--cacert "${tls_dir}/ca.crt"'), "CMA readiness lacks explicit local CA trust")
check(source.include?('shred -u -- "${tls_dir}/ca.key"'), "local TLS issuing key is retained after issuance")
check(source.include?('openssl x509 -in "${tls_dir}/server.crt" -checkip 127.0.0.1'), "local TLS certificate is not verified as an IP identity")
check(source.include?('crictl inspecti "${image}" -o json'), "loaded node image identity is not proven")
check(source.include?('--fail-open=false'), "network-policy enforcement became fail-open")
check(source.include?('--strict-mode=true'), "network-policy enforcement lost strict connection revocation")
check(constant(source, "CMA_STAGING_NETWORK_POLICY_AMD64_DIGEST") == "sha256:0070d44cdb811b280d171ffa98e23c2749d604c59ccc968b1fd59c1ccda2993e", "network-policy platform digest drifted")
check(source.include?('kind: MutatingAdmissionPolicy'), "sandbox privilege replacement policy disappeared")
check(source.include?("name: cma-trigger-sandbox-hardening\nspec:\n  failurePolicy: Fail"), "sandbox mutation policy became fail-open")
check(source.include?('kind: ValidatingAdmissionPolicy'), "sandbox fail-closed admission policy disappeared")
check(source.include?('kind: NetworkPolicy'), "staging network isolation disappeared")
check(source.include?('cma_staging_provider_network_denial_test opensandbox-ordinary'), "provider-to-Mongo denial probe disappeared")
check(!source.include?("except Exception:\n            pass"), "image attestor still suppresses provider evidence failures")

handoff = function_body(source, "cma_staging_validate_handoff", "cma_staging_materialize_runtime_credentials")
check(handoff.include?('"access_token_expires_at"'), "handoff expiry is not mandatory")
check(handoff.include?('. >= $minimum_expiry and . <= $maximum_expiry'), "handoff lifetime is not bounded")
check(handoff.include?('(explode | all(. >= 32 and . != 127))'), "handoff secrets admit control characters")
accepted_handoff = function_body(source, "cma_staging_accept_handoff", "cma_staging_generate_cma_secrets")
check(in_order?(accepted_handoff,
  'cma_staging_validate_handoff "${root}" "${handoff}"',
  'cma_staging_materialize_runtime_credentials "${root}" "${handoff}"',
  'cma_staging_validate_runtime_credentials "${root}" "${handoff}"',
  ': > "${root}/phase-handoff-accepted"'), "handoff is not accepted in a closed order")
deploy = function_body(source, "cma_staging_deploy", "cma_staging_assert_cma_authority")
check(deploy.include?('cma_staging_require_phase validated handoff-accepted prepared'), "deploy can precede handoff acceptance")
check(deploy.include?('cma_staging_validate_runtime_credentials "${root}" "${handoff}"'), "deploy does not revalidate materialized caller custody")

fixture_bootstrap = function_body(source, "cma_staging_bootstrap_fixture", "cma_staging_export_env")
check(fixture_bootstrap.include?('fixture bootstrap output schema drifted'), "fixture output is transformed before closed validation")
check(in_order?(fixture_bootstrap, 'fixture bootstrap output schema drifted', '. + {'), "fixture validation must precede transformation")
export_body = function_body(source, "cma_staging_export_env", "cma_staging_destroy")
%w[access_token caller-access-token service_id github-service-id credentials/].each do |secret_reference|
  check(!export_body.include?(secret_reference), "secret reference reached exported evidence: #{secret_reference}")
end

destroy = function_body(source, "cma_staging_destroy", "cma_staging_remove_observer")
check(destroy.include?('delete cluster --name "${CMA_STAGING_CLUSTER}" >/dev/null || status=1'), "cluster deletion failure is not terminal")
check(destroy.include?('cluster-create-attempted'), "cleanup does not cover a crash during Kind creation")
check(destroy.include?('cma_staging_die "Kind cluster destruction could not be proven"'), "cluster absence is not proven")
purge = function_body(source, "cma_staging_purge", "cma_staging_verify_cleanup")
check(purge.include?('refusing to purge custody before cluster destruction'), "credential purge can precede cluster destruction")
check(purge.include?('refusing to follow a symlink'), "credential purge can follow symlinks")
check(purge.include?('settlement_path="${RUNNER_TEMP}/cma-trigger-provider-object-settlement.json"'), "provider settlement receipt path drifted")
check(purge.include?('[[ -f "${settlement_path}" && ! -L "${settlement_path}" ]]'), "provider settlement receipt may follow a symlink")
check(purge.include?('--argjson provider_object_settlement "${provider_object_settlement}"'), "provider settlement is not embedded in the cleanup receipt")
check(purge.include?('provider_object_settlement:$provider_object_settlement'), "cleanup receipt dropped provider settlement")
verify_cleanup = function_body(source, "cma_staging_verify_cleanup", "cma_staging_main")
check(verify_cleanup.include?('"provider_object_settlement", "run_attempt", "run_id", "schema_version"'), "cleanup receipt closed keys dropped provider settlement")
check(verify_cleanup.include?('.provider_object_settlement == null or'), "cleanup receipt does not validate provider settlement type")

check(fixture.match?(/ALLOWED_GITHUB_SCOPES = new Set\(\[\s*"public_repo",\s*"read:user",\s*"user:email",?\s*\]\)/m), "fixture OAuth ceiling drifted")
check(fixture.include?('connection.account_login.toLowerCase() === contributorLogin.toLowerCase()'), "fixture does not bind the contributor login")
check(fixture.include?('scopes.includes("public_repo")'), "fixture does not require public_repo")
check(fixture.include?('!scopes.includes("repo")'), "fixture does not explicitly reject repo")
check(fixture.include?('!scopes.includes("workflow")'), "fixture does not explicitly reject workflow")
check(fixture.include?('scopes.every((scope) => ALLOWED_GITHUB_SCOPES.has(scope))'), "fixture admits unknown OAuth scopes")
check(fixture.include?('policy_reference: "public-zero-cost-v1"'), "fixture does not use the deployment-owned zero-cost policy")
RUBY
}

cma_kind_test_mutate() {
  local temporary="$1"
  local mutation="$2"
  cp "${CMA_KIND_TEST_SCRIPT}" "${temporary}"
  ruby - "${temporary}" "${mutation}" <<'RUBY'
path, mutation = ARGV
source = File.read(path)
replacements = {
  "wrong_endpoint" => ['readonly CMA_STAGING_CLUSTER_SERVER="https://127.0.0.1:6443"', 'readonly CMA_STAGING_CLUSTER_SERVER="https://127.0.0.1:7443"'],
  "missing_digest" => ["cma_staging_require_value CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256", "cma_staging_require_value CMA_TRIGGER_SMOKE_KUBECONFIG_DIGEST_REMOVED"],
  "ca_mismatch" => ['[[ "${ca_cert_key}" == "${ca_private_key}" ]]', '[[ "${ca_cert_key}" != "${ca_private_key}" ]]'],
  "whole_pki_mount" => ['hostPath: "${root}/bootstrap/pki/ca.crt"', 'hostPath: "${root}/bootstrap/pki"'],
  "reused_provider" => ['readonly CMA_STAGING_TRIGGER_PROVIDER_ORIGIN="http://opensandbox-trigger.cma-trigger-system.svc.cluster.local:8080"', 'readonly CMA_STAGING_TRIGGER_PROVIDER_ORIGIN="http://opensandbox-ordinary.cma-trigger-system.svc.cluster.local:8080"'],
  "standalone_mongo" => ['args: ["--auth", "--keyFile", "/var/run/mongo-keyfile/keyfile", "--replSet", "rs0", "--bind_ip_all"]', 'args: ["--replSet", "rs0", "--bind_ip_all"]'],
  "missing_tls_trust" => ['NODE_EXTRA_CA_CERTS="${root}/credentials/local-tls/ca.crt"', 'NODE_EXTRA_CA_CERTS_DISABLED="${root}/credentials/local-tls/ca.crt"'],
  "wrong_tls_identity_check" => ['openssl x509 -in "${tls_dir}/server.crt" -checkip 127.0.0.1', 'openssl x509 -in "${tls_dir}/server.crt" -checkhost 127.0.0.1'],
  "retained_ca" => ['shred -u -- "${ca_key}"', 'true # retained ca.key'],
  "overprivileged_observer" => ['verbs: ["list"]', 'verbs: ["get", "list"]'],
  "missing_image_provenance" => ["cma_staging_require_value CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH", "cma_staging_require_value CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_REMOVED"],
  "rebuilt_images" => ['docker image load --input "${archive}"', 'docker build --tag unapproved-image .'],
  "parent_symlink_allowed" => ["File.realpath(ARGV.fetch(0))", "File.expand_path(ARGV.fetch(0))"],
  "network_fail_open" => ["--fail-open=false", "--fail-open=true"],
  "network_platform_drift" => ['readonly CMA_STAGING_NETWORK_POLICY_AMD64_DIGEST="sha256:0070d44cdb811b280d171ffa98e23c2749d604c59ccc968b1fd59c1ccda2993e"', 'readonly CMA_STAGING_NETWORK_POLICY_AMD64_DIGEST="sha256:1070d44cdb811b280d171ffa98e23c2749d604c59ccc968b1fd59c1ccda2993e"'],
  "mutation_fail_open" => ["name: cma-trigger-sandbox-hardening\nspec:\n  failurePolicy: Fail", "name: cma-trigger-sandbox-hardening\nspec:\n  failurePolicy: Ignore"],
  "attestor_suppression" => ["        time.sleep(2)\n---", "        except Exception:\n            pass\n        time.sleep(2)\n---"],
  "retained_cluster_admin" => ['shred -u -- "$credential"', 'true # retained in-node administrator credential'],
  "runtime_node_binding_removed" => ['cma_staging_capture_node_image "${root}" runtime', 'true # runtime node image left unbound'],
  "cleanup_failure_ignored" => ['delete cluster --name "${CMA_STAGING_CLUSTER}" >/dev/null || status=1', 'delete cluster --name "${CMA_STAGING_CLUSTER}" >/dev/null || true'],
  "cleanup_schema_downgrade" => ['readonly CMA_STAGING_CLEANUP_SCHEMA_VERSION=2', 'readonly CMA_STAGING_CLEANUP_SCHEMA_VERSION=1'],
  "settlement_receipt_dropped" => ['--argjson provider_object_settlement "${provider_object_settlement}"', '--argjson provider_object_settlement null'],
  "settlement_type_opened" => ['.provider_object_settlement == null or', 'true or'],
  "secret_export" => ['printf \'NODE_EXTRA_CA_CERTS=%s\\n\' "$(jq -er .tls_ca_path "${result}")"', 'printf \'NODE_EXTRA_CA_CERTS=%s\\n\' "$(jq -er .tls_ca_path "${result}")"\n    printf \'LEAK=%s\\n\' "$(<"${root}/credentials/caller-access-token")"'],
}.freeze
old, replacement = replacements.fetch(mutation)
raise "mutation target missing or ambiguous: #{mutation}" unless source.scan(old).length == 1
File.binwrite(path, source.sub(old, replacement))
RUBY
}

cma_kind_test_mutations() {
  local directory="$1"
  local mutation mutated
  for mutation in \
    wrong_endpoint missing_digest ca_mismatch whole_pki_mount reused_provider \
    standalone_mongo missing_tls_trust wrong_tls_identity_check retained_ca overprivileged_observer \
    missing_image_provenance rebuilt_images parent_symlink_allowed network_fail_open \
    network_platform_drift mutation_fail_open attestor_suppression retained_cluster_admin \
    runtime_node_binding_removed \
    cleanup_failure_ignored cleanup_schema_downgrade settlement_receipt_dropped \
    settlement_type_opened secret_export; do
    mutated="${directory}/${mutation}.sh"
    cma_kind_test_mutate "${mutated}" "${mutation}"
    if cma_kind_test_contract "${mutated}" >/dev/null 2>&1; then
      cma_kind_test_die "contract admitted mutation: ${mutation}"
    fi
  done
}

cma_kind_test_write_uname() {
  local target="$1"
  cat > "${target}" <<'EOF'
#!/bin/sh
case "${1:-}" in
  -s) printf 'Linux\n' ;;
  -m) printf 'x86_64\n' ;;
  *) exit 1 ;;
esac
EOF
  chmod 0700 "${target}"
}

cma_kind_test_cleanup() {
  [[ -n "${CMA_KIND_TEST_TEMPORARY}" && \
     "${CMA_KIND_TEST_TEMPORARY}" != / && \
     "$(basename "${CMA_KIND_TEST_TEMPORARY}")" == cma-trigger-kind-tests.* && \
     -d "${CMA_KIND_TEST_TEMPORARY}" && \
     ! -L "${CMA_KIND_TEST_TEMPORARY}" ]] || return 0
  find "${CMA_KIND_TEST_TEMPORARY}" -type f -exec chmod u+w {} + 2>/dev/null || true
  rm -rf -- "${CMA_KIND_TEST_TEMPORARY}"
}

cma_kind_test_write_stat() {
  local target="$1"
  cat > "${target}" <<'EOF'
#!/bin/sh
if [ "${1:-}" = '-c' ] && ! /usr/bin/stat -c "${2:-}" "${3:-}" >/dev/null 2>&1; then
  case "${2:-}" in
    %a) exec /usr/bin/stat -f '%Lp' "${3:-}" ;;
    %s) exec /usr/bin/stat -f '%z' "${3:-}" ;;
  esac
fi
exec /usr/bin/stat "$@"
EOF
  chmod 0700 "${target}"
}

cma_kind_test_generated_manifests() {
  local directory="$1"
  local root="${directory}/generated-manifests"
  local attestor="${root}/attest.py"
  mkdir -p "${root}/generated"
  printf '%s\n' 'cma-runtime:ci' > "${root}/runtime-image"
  cma_staging_write_base_manifest "${root}" cma-trigger-sandboxes
  cma_staging_write_network_enforcer_manifest "${root}"
  ruby - "${root}/generated/base.yaml" \
    "${root}/generated/network-policy-enforcer.yaml" "${attestor}" <<'RUBY'
require "yaml"

base_path, enforcer_path, attestor_path = ARGV
base = YAML.load_stream(File.binread(base_path))
enforcer = YAML.load_stream(File.binread(enforcer_path))
raise "generated manifest contains an empty document" if (base + enforcer).any?(&:nil?)
raise "generated manifest contains a non-object" unless (base + enforcer).all? { |item| item.is_a?(Hash) }

identities = (base + enforcer).map do |item|
  metadata = item.fetch("metadata")
  [item.fetch("apiVersion"), item.fetch("kind"), metadata["namespace"], metadata.fetch("name")]
end
raise "generated manifest contains a duplicate identity" unless identities.uniq.length == identities.length

mutation = base.one? do |item|
  item["apiVersion"] == "admissionregistration.k8s.io/v1beta1" &&
    item["kind"] == "MutatingAdmissionPolicy" &&
    item.dig("metadata", "name") == "cma-trigger-sandbox-hardening" &&
    item.dig("spec", "failurePolicy") == "Fail" &&
    item.dig("spec", "mutations", 0, "patchType") == "JSONPatch"
end
raise "closed sandbox mutation policy is absent" unless mutation

validation_names = base.each_with_object([]) do |item, values|
  values << item.dig("metadata", "name") if item["kind"] == "ValidatingAdmissionPolicy"
end.sort
raise "sandbox validation policy inventory drifted" unless validation_names == [
  "cma-trigger-batchsandbox-boundary", "cma-trigger-sandbox-pod-boundary"
]

network_ids = base.each_with_object([]) do |item, values|
  next unless item["kind"] == "NetworkPolicy"
  values << "#{item.dig("metadata", "namespace")}/#{item.dig("metadata", "name")}"
end.sort
expected_network_ids = [
  "cma-trigger-ordinary/cma-api",
  "cma-trigger-ordinary/cma-ingress",
  "cma-trigger-ordinary/default-deny",
  "cma-trigger-ordinary/sandbox-runtime",
  "cma-trigger-sandboxes/default-deny",
  "cma-trigger-sandboxes/sandbox-runtime",
  "cma-trigger-system/default-deny",
  "cma-trigger-system/image-attestor-to-api",
  "cma-trigger-system/mongo",
  "cma-trigger-system/opensandbox-controller-to-api",
  "cma-trigger-system/opensandbox-provider",
]
raise "network policy inventory drifted" unless network_ids == expected_network_ids

daemon = enforcer.find { |item| item["kind"] == "DaemonSet" }
args = daemon.dig("spec", "template", "spec", "containers", 0, "args")
raise "network enforcement is not closed" unless args.include?("--fail-open=false") &&
  args.include?("--strict-mode=true") && args.include?("--disable-nri=true")

script_map = base.find do |item|
  item["kind"] == "ConfigMap" && item.dig("metadata", "name") == "cma-trigger-image-attestor-script"
end
script = script_map&.dig("data", "attest.py")
raise "image attestor script is absent" unless script.is_a?(String) && !script.empty?
File.binwrite(attestor_path, script, mode: "wx", perm: 0600)
RUBY
  python3 -m py_compile "${attestor}"
}

cma_kind_test_write_docker() {
  local target="$1"
  cat > "${target}" <<'EOF'
#!/bin/sh
set -eu
if [ "${1:-}" = image ] && [ "${2:-}" = load ] && [ "${3:-}" = --input ]; then
  test -f "${4:-}"
  exit 0
fi
if [ "${1:-}" = image ] && [ "${2:-}" = inspect ]; then
  for argument in "$@"; do image_reference=$argument; done
  case "$image_reference" in
    cma-api:ci) printf '%s\n' "$CMA_KIND_TEST_API_ID" ;;
    cma-runtime:ci) printf '%s\n' "$CMA_KIND_TEST_RUNTIME_ID" ;;
    *) exit 1 ;;
  esac
  exit 0
fi
exit 1
EOF
  chmod 0700 "${target}"
}

cma_kind_test_approved_images() {
  local directory="$1"
  local case_dir root image_root fake_bin provenance api_archive runtime_archive
  local api_sha runtime_sha api_size runtime_size api_id runtime_id
  case_dir="$(cd "${directory}" && pwd -P)/approved-images"
  root="${case_dir}/staging"
  image_root="${case_dir}/cma-trigger-ci-images"
  fake_bin="${case_dir}/fake-bin"
  provenance="${image_root}/approved-image-provenance.json"
  api_archive="${image_root}/api/cma-trigger-api.tar"
  runtime_archive="${image_root}/runtime/cma-trigger-runtime.tar"
  mkdir -p "${root}" "${fake_bin}" "${image_root}/api" "${image_root}/runtime"
  printf '%s' 'approved-api-archive' > "${api_archive}"
  printf '%s' 'approved-runtime-archive' > "${runtime_archive}"
  api_sha="$(cma_kind_test_sha256 "${api_archive}")"
  runtime_sha="$(cma_kind_test_sha256 "${runtime_archive}")"
  api_size="$(wc -c < "${api_archive}" | tr -d ' ')"
  runtime_size="$(wc -c < "${runtime_archive}" | tr -d ' ')"
  api_id="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
  runtime_id="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  jq -n \
    --arg api_sha "${api_sha}" --argjson api_size "${api_size}" --arg api_id "${api_id}" \
    --arg runtime_sha "${runtime_sha}" --argjson runtime_size "${runtime_size}" --arg runtime_id "${runtime_id}" '{
      schema_version:1,
      kind:"cma-trigger-approved-ci-images",
      repository_id:1330597394,
      source_sha:"0123456789abcdef0123456789abcdef01234567",
      ci_run:{id:91,attempt:1,event:"push",head_branch:"develop",head_sha:"0123456789abcdef0123456789abcdef01234567",conclusion:"success",workflow_path:".github/workflows/ci.yml"},
      images:{
        api:{artifact_id:101,artifact_digest:("sha256:" + ("1" * 64)),artifact_name:"cma-trigger-ci-image-api-0123456789abcdef0123456789abcdef01234567",archive_name:"cma-trigger-api.tar",archive_sha256:$api_sha,archive_size:$api_size,metadata_name:"cma-trigger-api.json",metadata_sha256:("2" * 64),image_reference:"cma-api:ci",image_id:$api_id},
        runtime:{artifact_id:102,artifact_digest:("sha256:" + ("3" * 64)),artifact_name:"cma-trigger-ci-image-runtime-0123456789abcdef0123456789abcdef01234567",archive_name:"cma-trigger-runtime.tar",archive_sha256:$runtime_sha,archive_size:$runtime_size,metadata_name:"cma-trigger-runtime.json",metadata_sha256:("4" * 64),image_reference:"cma-runtime:ci",image_id:$runtime_id}
      }
    }' > "${provenance}"
  chmod 0400 "${provenance}"
  cma_kind_test_write_docker "${fake_bin}/docker"
  cma_kind_test_write_stat "${fake_bin}/stat"
  # Positional parameters are expanded by the isolated Bash process.
  # shellcheck disable=SC2016
  if ! env \
      PATH="${fake_bin}:${PATH}" \
      RUNNER_TEMP="${case_dir}" \
      GITHUB_REPOSITORY_ID=1330597394 \
      GITHUB_SHA=0123456789abcdef0123456789abcdef01234567 \
      CMA_KIND_TEST_API_ID="${api_id}" \
      CMA_KIND_TEST_RUNTIME_ID="${runtime_id}" \
      CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH="${provenance}" \
      CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE="${api_archive}" \
      CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE="${runtime_archive}" \
      CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID="${api_id}" \
      CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID="${runtime_id}" \
      bash -c 'source "$1"; cma_staging_validate_approved_images "$2"' \
        _ "${CMA_KIND_TEST_SCRIPT}" "${root}"; then
    cma_kind_test_die "valid approved image provenance was rejected"
  fi
  [[ "$(<"${root}/api-image-digest")" == "${api_id}" && \
     "$(<"${root}/runtime-image-digest")" == "${runtime_id}" ]] || \
    cma_kind_test_die "approved image IDs were not preserved"

  printf '%s' 'tamper' >> "${runtime_archive}"
  # shellcheck disable=SC2016
  if env \
      PATH="${fake_bin}:${PATH}" \
      RUNNER_TEMP="${case_dir}" \
      GITHUB_REPOSITORY_ID=1330597394 \
      GITHUB_SHA=0123456789abcdef0123456789abcdef01234567 \
      CMA_KIND_TEST_API_ID="${api_id}" \
      CMA_KIND_TEST_RUNTIME_ID="${runtime_id}" \
      CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH="${provenance}" \
      CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE="${api_archive}" \
      CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE="${runtime_archive}" \
      CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID="${api_id}" \
      CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID="${runtime_id}" \
      bash -c 'source "$1"; cma_staging_validate_approved_images "$2"' \
        _ "${CMA_KIND_TEST_SCRIPT}" "${case_dir}/tampered" >/dev/null 2>&1; then
    cma_kind_test_die "tampered approved runtime archive was admitted"
  fi
}

cma_kind_test_handoff_case() {
  local directory="$1"
  local label="$2"
  local mutation="$3"
  local expected="$4"
  local root handoff now temporary_json fake_bin original_path
  root="${directory}/handoff-${label}/cma-trigger-kind-staging"
  handoff="${root}/credentials/nyxid-handoff.json"
  fake_bin="${directory}/handoff-${label}/fake-bin"
  mkdir -p "${root}/credentials" "${fake_bin}"
  chmod 0700 "${directory}/handoff-${label}" "${root}" \
    "${root}/credentials" "${fake_bin}"
  cma_kind_test_write_stat "${fake_bin}/stat"
  now="$(date +%s)"
  jq -n \
    --argjson expires_at "$((now + 8000))" \
    '{
      schema_version: 1,
      nyxid_base_url: "https://nyx-api.chrono-ai.fun",
      nyxid_client_id: "08b92325-c4e7-4b38-9e6e-47efc3ca2624",
      nyxid_client_secret: "nyxid-secret",
      storage_client_id: "sa_0123456789abcdef01234567",
      storage_client_secret: "storage-secret",
      storage_owner_service_account_id: "6a888888-8888-4888-8888-888888888888",
      context_bucket: "cma-trigger-context-test",
      trigger_workspace_bucket: "cma-trigger-workspace-test",
      public_github_service_slug: "api-github",
      subject_id: "12a5a3be-8357-4190-bc86-17f81390a765",
      access_token: "header.payload.signature",
      access_token_expires_at: $expires_at
    }' > "${handoff}"
  chmod 0600 "${handoff}"
  case "${mutation}" in
    valid) ;;
    stale)
      temporary_json="${handoff}.tmp"
      jq --argjson expires_at "$((now + 60))" \
        '.access_token_expires_at = $expires_at' "${handoff}" > "${temporary_json}"
      mv "${temporary_json}" "${handoff}"
      chmod 0600 "${handoff}"
      ;;
    excessive)
      temporary_json="${handoff}.tmp"
      jq --argjson expires_at "$((now + 30000))" \
        '.access_token_expires_at = $expires_at' "${handoff}" > "${temporary_json}"
      mv "${temporary_json}" "${handoff}"
      chmod 0600 "${handoff}"
      ;;
    unknown_field)
      temporary_json="${handoff}.tmp"
      jq '.unexpected = true' "${handoff}" > "${temporary_json}"
      mv "${temporary_json}" "${handoff}"
      chmod 0600 "${handoff}"
      ;;
    control_character)
      temporary_json="${handoff}.tmp"
      jq '.nyxid_client_secret = "unsafe\nCHRONO_TRIGGER_ENABLED=false"' \
        "${handoff}" > "${temporary_json}"
      mv "${temporary_json}" "${handoff}"
      chmod 0600 "${handoff}"
      ;;
    *) cma_kind_test_die "unknown handoff mutation: ${mutation}" ;;
  esac
  original_path="${PATH}"
  PATH="${fake_bin}:${PATH}"
  if (cma_staging_validate_handoff "${root}" "${handoff}") >/dev/null 2>&1; then
    PATH="${original_path}"
    [[ "${expected}" == success ]] || cma_kind_test_die "handoff admitted ${label}"
  else
    PATH="${original_path}"
    [[ "${expected}" == failure ]] || cma_kind_test_die "valid handoff was rejected"
  fi
}

cma_kind_test_runtime_credential_binding() {
  local directory="$1"
  local root="${directory}/runtime-binding/cma-trigger-kind-staging"
  local handoff="${root}/credentials/nyxid-handoff.json"
  local service_id="8a888888-8888-4888-8888-888888888888"
  local now original_path fake_bin
  fake_bin="${directory}/runtime-binding/fake-bin"
  mkdir -p "${root}/credentials" "${fake_bin}"
  chmod 0700 "${directory}/runtime-binding" "${root}" \
    "${root}/credentials" "${fake_bin}"
  cma_kind_test_write_stat "${fake_bin}/stat"
  now="$(date +%s)"
  jq -n --argjson expires_at "$((now + 8000))" '{
    schema_version: 1,
    nyxid_base_url: "https://nyx-api.chrono-ai.fun",
    nyxid_client_id: "08b92325-c4e7-4b38-9e6e-47efc3ca2624",
    nyxid_client_secret: "nyxid-secret",
    storage_client_id: "sa_0123456789abcdef01234567",
    storage_client_secret: "storage-secret",
    storage_owner_service_account_id: "6a888888-8888-4888-8888-888888888888",
    context_bucket: "cma-trigger-context-test",
    trigger_workspace_bucket: "cma-trigger-workspace-test",
    public_github_service_slug: "api-github",
    subject_id: "12a5a3be-8357-4190-bc86-17f81390a765",
    access_token: "header.payload.signature",
    access_token_expires_at: $expires_at
  }' > "${handoff}"
  chmod 0600 "${handoff}"
  original_path="${PATH}"
  PATH="${fake_bin}:${PATH}"
  CMA_TRIGGER_SMOKE_SERVICE_ID="${service_id}" \
    cma_staging_materialize_runtime_credentials "${root}" "${handoff}" || \
    cma_kind_test_die "valid runtime caller credentials could not be materialized"
  cma_staging_validate_runtime_credentials "${root}" "${handoff}" || \
    cma_kind_test_die "valid runtime caller credentials were rejected"
  printf '%s' 'different-token' > "${root}/credentials/caller-access-token"
  if (cma_staging_validate_runtime_credentials "${root}" "${handoff}") \
      >/dev/null 2>&1; then
    PATH="${original_path}"
    cma_kind_test_die "runtime caller-token drift was admitted"
  fi
  PATH="${original_path}"
}

cma_kind_test_export_env_case() {
  local directory="$1"
  local label="$2"
  local mutation="$3"
  local expected="$4"
  local case_dir root result github_env runtime_digest repo_digest
  case_dir="${directory}/export-${label}"
  root="${case_dir}/cma-trigger-kind-staging"
  result="${root}/staging-result.json"
  github_env="${case_dir}/github.env"
  runtime_digest="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
  repo_digest="cma-runtime@sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"
  mkdir -p "${root}"
  chmod 0700 "${case_dir}" "${root}"
  jq -n '{
    schema_version:1,
    cluster:"cma-trigger-smoke",
    run_id:"12345",
    run_attempt:"1"
  }' > "${root}/ownership.json"
  for phase in validated prepared deployed sealed fixture; do
    : > "${root}/phase-${phase}"
  done
  jq -n \
    --arg runtime_digest "${runtime_digest}" \
    --arg repo_digest "${repo_digest}" '{
      schema_version:1,
      base_url:"https://127.0.0.1:8443",
      configuration_revision:"kind-12345-1-0123456789abcdef0123456789abcdef01234567",
      expected_build_sha:"0123456789abcdef0123456789abcdef01234567",
      expected_runtime_version:"cma-runtime:ci",
      public_id:"00000000-0000-4000-8000-000000000001",
      revision_digest:("d" * 64),
      runtime_image_digest:$runtime_digest,
      runtime_image_ids:[$repo_digest,$runtime_digest],
      source_sha:"878dcb6288f94721de6724899ea0680e1086d511",
      tls_ca_path:"/tmp/cma-trigger-test-ca.crt"
    }' > "${result}"
  case "${mutation}" in
    valid) ;;
    missing_digest)
      jq --arg other "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" \
        '.runtime_image_ids = [$other]' "${result}" > "${result}.tmp"
      mv "${result}.tmp" "${result}"
      ;;
    unordered)
      jq '.runtime_image_ids |= reverse' "${result}" > "${result}.tmp"
      mv "${result}.tmp" "${result}"
      ;;
    *) cma_kind_test_die "unknown export mutation: ${mutation}" ;;
  esac
  : > "${github_env}"
  chmod 0600 "${result}" "${github_env}" "${root}/ownership.json" \
    "${root}"/phase-*
  # shellcheck disable=SC2030,SC2031
  if (
    export RUNNER_TEMP="${case_dir}"
    export GITHUB_RUN_ID=12345
    export GITHUB_RUN_ATTEMPT=1
    export CMA_TRIGGER_KIND_STAGING_ROOT="${root}"
    export GITHUB_ENV="${github_env}"
    cma_staging_export_env
  ) >/dev/null 2>&1; then
    [[ "${expected}" == success ]] || cma_kind_test_die "export admitted ${label}"
  else
    [[ "${expected}" == failure ]] || cma_kind_test_die "valid export was rejected"
    return
  fi
  [[ "$(grep -c '^CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_IDS_JSON=' "${github_env}")" -eq 1 ]] || \
    cma_kind_test_die "runtime image identity set was not exported exactly once"
  grep -Fqx \
    "CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_IDS_JSON=[\"${repo_digest}\",\"${runtime_digest}\"]" \
    "${github_env}" || cma_kind_test_die "runtime image identity export drifted"
}

cma_kind_test_cleanup_receipt_case() {
  local directory="$1"
  local label="$2"
  local settlement_mode="$3"
  local case_dir runner root receipt settlement
  case_dir="${directory}/${label}"
  runner="${case_dir}/runner"
  root="${runner}/cma-trigger-kind-staging"
  receipt="${runner}/cma-trigger-kind-staging-cleanup.json"
  settlement="${runner}/cma-trigger-provider-object-settlement.json"
  mkdir -p "${root}/credentials"
  chmod 0700 "${case_dir}" "${runner}" "${root}" "${root}/credentials"
  jq -n '{
    schema_version:1,
    cluster:"cma-trigger-smoke",
    run_id:"12345",
    run_attempt:"1"
  }' > "${root}/ownership.json"
  : > "${root}/cluster-create-attempted"
  : > "${root}/phase-destroyed"
  : > "${root}/phase-observer-removed"
  printf '%s\n' 'secret slated for purge' > "${root}/credentials/example"
  case "${settlement_mode}" in
    valid)
      jq -n '{
        schema_version:1,
        kind:"cma-trigger-provider-object-settlement",
        evidence_id:"cma-trigger-12345-1"
      }' > "${settlement}"
      ;;
    invalid)
      jq -n '[]' > "${settlement}"
      ;;
    absent) ;;
    *) cma_kind_test_die "unknown cleanup settlement mode: ${settlement_mode}" ;;
  esac
  chmod 0600 "${root}/ownership.json" "${root}/cluster-create-attempted" \
    "${root}/phase-destroyed" "${root}/phase-observer-removed" \
    "${root}/credentials/example"
  [[ ! -e "${settlement}" ]] || chmod 0600 "${settlement}"

  # shellcheck disable=SC2030,SC2031
  if ! (
    export RUNNER_TEMP="${runner}"
    export GITHUB_RUN_ID=12345
    export GITHUB_RUN_ATTEMPT=1
    export CMA_TRIGGER_KIND_STAGING_ROOT="${root}"
    cma_staging_purge
    cma_staging_verify_cleanup
  ) > "${case_dir}/cleanup.log" 2>&1; then
    sed -n '1,120p' "${case_dir}/cleanup.log" >&2
    cma_kind_test_die "cleanup receipt case failed: ${label}"
  fi
  [[ ! -e "${root}" ]] || cma_kind_test_die "cleanup retained staging custody: ${label}"
  jq -e '
    keys == [
      "cluster", "cluster_destroyed", "credentials_purged",
      "provider_object_settlement", "run_attempt", "run_id", "schema_version"
    ] and
    .schema_version == 2 and .cluster == "cma-trigger-smoke" and
    .run_id == "12345" and .run_attempt == "1" and
    .cluster_destroyed == true and .credentials_purged == true
  ' "${receipt}" >/dev/null || cma_kind_test_die "cleanup receipt drifted: ${label}"
  if [[ "${settlement_mode}" == valid ]]; then
    jq -e --slurpfile settlement "${settlement}" \
      '.provider_object_settlement == $settlement[0]' "${receipt}" >/dev/null || \
      cma_kind_test_die "cleanup receipt dropped provider settlement"
  else
    jq -e '.provider_object_settlement == null' "${receipt}" >/dev/null || \
      cma_kind_test_die "cleanup receipt adopted an invalid provider settlement"
  fi
}

cma_kind_test_validate_case() {
  local directory="$1"
  local label="$2"
  local mutation="$3"
  local expected="$4"
  local case_dir runner fake_bin ca_dir observer_dir kubeconfig endpoint subject
  local kubeconfig_digest kubeconfig_b64 ca_key_b64 other_ca_key observer_key_probe log
  case_dir="${directory}/${label}"
  runner="${case_dir}/runner"
  fake_bin="${case_dir}/fake-bin"
  ca_dir="${case_dir}/ca"
  observer_dir="${case_dir}/observer"
  kubeconfig="${case_dir}/observer.yaml"
  log="${case_dir}/validate.log"
  mkdir -p "${runner}" "${fake_bin}" "${ca_dir}" "${observer_dir}"
  chmod 0700 "${case_dir}" "${runner}" "${fake_bin}" "${ca_dir}" "${observer_dir}"
  cma_kind_test_write_uname "${fake_bin}/uname"

  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
    -out "${ca_dir}/ca.key" >/dev/null 2>&1
  openssl req -new -x509 -sha256 -days 2 -key "${ca_dir}/ca.key" \
    -subj '/CN=CMA Trigger test cluster CA' \
    -addext 'basicConstraints=critical,CA:TRUE' \
    -addext 'keyUsage=critical,keyCertSign,cRLSign' \
    -out "${ca_dir}/ca.crt" >/dev/null 2>&1
  openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
    -out "${observer_dir}/observer.key" >/dev/null 2>&1
  subject='/CN=cma-trigger-smoke-observer'
  if [[ "${mutation}" == privileged_subject ]]; then
    subject='/O=system:masters/CN=cma-trigger-smoke-observer'
  fi
  openssl req -new -sha256 -key "${observer_dir}/observer.key" \
    -subj "${subject}" -out "${observer_dir}/observer.csr" >/dev/null 2>&1
  cat > "${observer_dir}/extensions.cnf" <<'EOF'
basicConstraints=critical,CA:FALSE
keyUsage=critical,digitalSignature,keyEncipherment
extendedKeyUsage=clientAuth
EOF
  openssl x509 -req -sha256 -days 2 -in "${observer_dir}/observer.csr" \
    -CA "${ca_dir}/ca.crt" -CAkey "${ca_dir}/ca.key" -CAcreateserial \
    -extfile "${observer_dir}/extensions.cnf" \
    -out "${observer_dir}/observer.crt" >/dev/null 2>&1

  endpoint='https://127.0.0.1:6443'
  [[ "${mutation}" != wrong_endpoint ]] || endpoint='https://127.0.0.1:7443'
  local ca_data observer_cert_data observer_key_data
  ca_data="$(cma_kind_test_base64 "${ca_dir}/ca.crt")"
  observer_cert_data="$(cma_kind_test_base64 "${observer_dir}/observer.crt")"
  observer_key_data="$(cma_kind_test_base64 "${observer_dir}/observer.key")"
  if [[ "${mutation}" == token_auth ]]; then
    cat > "${kubeconfig}" <<EOF
apiVersion: v1
kind: Config
clusters:
  - name: cma-trigger-smoke
    cluster: {certificate-authority-data: ${ca_data}, server: "${endpoint}"}
contexts:
  - name: cma-trigger-smoke-observer
    context: {cluster: cma-trigger-smoke, namespace: cma-trigger-sandboxes, user: cma-trigger-smoke-observer}
current-context: cma-trigger-smoke-observer
preferences: {}
users:
  - name: cma-trigger-smoke-observer
    user: {token: forbidden-token}
EOF
  elif [[ "${mutation}" == exec_auth ]]; then
    cat > "${kubeconfig}" <<EOF
apiVersion: v1
kind: Config
clusters:
  - name: cma-trigger-smoke
    cluster: {certificate-authority-data: ${ca_data}, server: "${endpoint}"}
contexts:
  - name: cma-trigger-smoke-observer
    context: {cluster: cma-trigger-smoke, namespace: cma-trigger-sandboxes, user: cma-trigger-smoke-observer}
current-context: cma-trigger-smoke-observer
preferences: {}
users:
  - name: cma-trigger-smoke-observer
    user: {exec: {apiVersion: client.authentication.k8s.io/v1, command: forbidden}}
EOF
  else
    cat > "${kubeconfig}" <<EOF
apiVersion: v1
kind: Config
clusters:
  - name: cma-trigger-smoke
    cluster: {certificate-authority-data: ${ca_data}, server: "${endpoint}"}
contexts:
  - name: cma-trigger-smoke-observer
    context: {cluster: cma-trigger-smoke, namespace: cma-trigger-sandboxes, user: cma-trigger-smoke-observer}
current-context: cma-trigger-smoke-observer
preferences: {}
users:
  - name: cma-trigger-smoke-observer
    user: {client-certificate-data: ${observer_cert_data}, client-key-data: ${observer_key_data}}
EOF
  fi
  chmod 0600 "${kubeconfig}"
  kubeconfig_digest="$(cma_kind_test_sha256 "${kubeconfig}")"
  [[ "${mutation}" != digest_drift ]] || \
    kubeconfig_digest='0000000000000000000000000000000000000000000000000000000000000000'
  if [[ "${mutation}" == ca_mismatch ]]; then
    other_ca_key="${case_dir}/other-ca.key"
    openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 \
      -out "${other_ca_key}" >/dev/null 2>&1
    ca_key_b64="$(cma_kind_test_base64 "${other_ca_key}")"
  else
    ca_key_b64="$(cma_kind_test_base64 "${ca_dir}/ca.key")"
  fi
  kubeconfig_b64="$(cma_kind_test_base64 "${kubeconfig}")"

  if env \
    PATH="${fake_bin}:${PATH}" \
    RUNNER_TEMP="${runner}" \
    GITHUB_RUN_ID=12345 \
    GITHUB_RUN_ATTEMPT=1 \
    GITHUB_SHA=0123456789abcdef0123456789abcdef01234567 \
    CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE=cma-trigger-sandboxes \
    CMA_TRIGGER_SMOKE_KUBECONFIG_B64="${kubeconfig_b64}" \
    CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256="${kubeconfig_digest}" \
    CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64="${ca_key_b64}" \
    bash "${CMA_KIND_TEST_SCRIPT}" validate > "${log}" 2>&1; then
    [[ "${expected}" == success ]] || cma_kind_test_die "validate admitted ${label}"
  else
    [[ "${expected}" == failure ]] || {
      sed -n '1,120p' "${log}" >&2
      cma_kind_test_die "valid observer provenance was rejected"
    }
  fi

  observer_key_probe="${observer_key_data:80:48}"
  [[ -z "${observer_key_probe}" ]] || \
    ! grep -Fq -- "${observer_key_probe}" "${log}" || \
    cma_kind_test_die "observer private-key bytes reached logs"
  [[ -z "${ca_key_b64:80:48}" ]] || \
    ! grep -Fq -- "${ca_key_b64:80:48}" "${log}" || \
    cma_kind_test_die "cluster CA private-key bytes reached logs"
}

cma_kind_test_main() {
  [[ "$#" -eq 0 ]] || cma_kind_test_die "this test accepts no arguments"
  cma_kind_test_require_commands awk base64 bash cp grep jq node openssl python3 ruby sed shred tr wc
  [[ -f "${CMA_KIND_TEST_SCRIPT}" && -f "${CMA_KIND_TEST_FIXTURE}" ]] || \
    cma_kind_test_die "staging implementation is absent"
  CMA_KIND_TEST_TEMPORARY="$(mktemp -d "${TMPDIR:-/tmp}/cma-trigger-kind-tests.XXXXXX")"
  trap cma_kind_test_cleanup EXIT

  bash -n "${CMA_KIND_TEST_SCRIPT}"
  node --check "${CMA_KIND_TEST_FIXTURE}"
  node "${CMA_KIND_TEST_FIXTURE}" --self-test
  cma_kind_test_contract
  cma_kind_test_mutations "${CMA_KIND_TEST_TEMPORARY}"
  # Source only the command functions; its main guard remains false.
  # shellcheck disable=SC1090,SC1091
  source "${CMA_KIND_TEST_SCRIPT}"
  cma_kind_test_generated_manifests "${CMA_KIND_TEST_TEMPORARY}"
  cma_kind_test_approved_images "${CMA_KIND_TEST_TEMPORARY}"
  cma_kind_test_handoff_case "${CMA_KIND_TEST_TEMPORARY}" valid valid success
  cma_kind_test_handoff_case "${CMA_KIND_TEST_TEMPORARY}" stale stale failure
  cma_kind_test_handoff_case "${CMA_KIND_TEST_TEMPORARY}" excessive excessive failure
  cma_kind_test_handoff_case "${CMA_KIND_TEST_TEMPORARY}" unknown unknown_field failure
  cma_kind_test_handoff_case "${CMA_KIND_TEST_TEMPORARY}" control control_character failure
  cma_kind_test_runtime_credential_binding "${CMA_KIND_TEST_TEMPORARY}"
  cma_kind_test_export_env_case "${CMA_KIND_TEST_TEMPORARY}" valid valid success
  cma_kind_test_export_env_case "${CMA_KIND_TEST_TEMPORARY}" missing-digest missing_digest failure
  cma_kind_test_export_env_case "${CMA_KIND_TEST_TEMPORARY}" unordered unordered failure
  cma_kind_test_cleanup_receipt_case "${CMA_KIND_TEST_TEMPORARY}" cleanup-with-settlement valid
  cma_kind_test_cleanup_receipt_case "${CMA_KIND_TEST_TEMPORARY}" cleanup-without-settlement absent
  cma_kind_test_cleanup_receipt_case "${CMA_KIND_TEST_TEMPORARY}" cleanup-invalid-settlement invalid
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" valid valid success
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" wrong-endpoint wrong_endpoint failure
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" digest-drift digest_drift failure
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" ca-mismatch ca_mismatch failure
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" privileged-subject privileged_subject failure
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" token-auth token_auth failure
  cma_kind_test_validate_case "${CMA_KIND_TEST_TEMPORARY}" exec-auth exec_auth failure
  printf '%s\n' '[kind-staging-tests] all tests passed'
}

cma_kind_test_main "$@"
