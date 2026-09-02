#!/bin/bash -p

{ set +x; } 2>/dev/null
set -euo pipefail
umask 077

unset BASH_ENV ENV CDPATH GLOBIGNORE RUBYOPT RUBYLIB PYTHONHOME PYTHONPATH
for cma_staging_name in "${!GIT_@}" "${!DYLD_@}" "${!LD_@}"; do
  unset "${cma_staging_name}"
done
unset cma_staging_name

readonly CMA_STAGING_SCHEMA_VERSION=1
readonly CMA_STAGING_CLEANUP_SCHEMA_VERSION=2
readonly CMA_STAGING_CLUSTER="cma-trigger-smoke"
readonly CMA_STAGING_CLUSTER_SERVER="https://127.0.0.1:6443"
readonly CMA_STAGING_OBSERVER_CN="cma-trigger-smoke-observer"
readonly CMA_STAGING_OBSERVER_KUBECONFIG_NAME="cma-trigger-smoke-kubeconfig"
readonly CMA_STAGING_ATTESTATION_CONFIGMAP="cma-trigger-runtime-provenance"
readonly CMA_STAGING_ORDINARY_NAMESPACE="cma-trigger-ordinary"
readonly CMA_STAGING_SYSTEM_NAMESPACE="cma-trigger-system"
readonly CMA_STAGING_ORDINARY_PROVIDER_ORIGIN="http://opensandbox-ordinary.cma-trigger-system.svc.cluster.local:8080"
readonly CMA_STAGING_TRIGGER_PROVIDER_ORIGIN="http://opensandbox-trigger.cma-trigger-system.svc.cluster.local:8080"
readonly CMA_STAGING_PUBLIC_ORIGIN="https://127.0.0.1:8443"
readonly CMA_STAGING_NODE_PORT=30443
readonly CMA_STAGING_KIND_VERSION="v0.33.0"
readonly CMA_STAGING_KIND_SHA256="aee6151561422756b764a4ae28e7f44cda5af5a9eead3cc9985112b1de8d8e0d"
readonly CMA_STAGING_NODE_IMAGE="kindest/node:v1.34.11@sha256:44e222ee2132dab25ff87301682f89eb82c7880ea3a1bf543bfe9708fd08d67d"
readonly CMA_STAGING_OPENSANDBOX_SOURCE="ecb1b5b012cb14a7289c5c565cc379b17a828699"
readonly CMA_STAGING_BATCHSANDBOX_CRD_SHA256="e3a4a12d196b5ef239ce59a947b84153ce5ad871dc3733222df3fd071dc704bf"
readonly CMA_STAGING_POOL_CRD_SHA256="b587ba85f311045d3ea767b5472aee3621a135510af19559cfeb23f568bb0a1a"
readonly CMA_STAGING_SERVER_IMAGE="opensandbox/server@sha256:4b386f107a4222320928b0b4dd38df8dc5154250ea4c90b4d36767a62f69ce7c"
readonly CMA_STAGING_CONTROLLER_IMAGE="opensandbox/controller@sha256:d0e875c55dc645d05ca8bb57b8441f6ac715b542452009ac9e9e9c76ac5826f9"
readonly CMA_STAGING_EXECD_IMAGE="opensandbox/execd@sha256:662fbb4c4dcd3e220da888c5e472f45c271eef398f78d9cf402de44d0ec7a0ec"
readonly CMA_STAGING_MONGO_IMAGE="mongo@sha256:05b417e0f4e6c30f4d5bf8ef47cdb846ba377b366277b925493d4c42339eb533"
readonly CMA_STAGING_NGINX_IMAGE="nginx@sha256:60e48a050b6408d0c5dd59b98b6e36bf0937a0bbe99304e3e9c0e63b7563443a"
readonly CMA_STAGING_NETWORK_POLICY_IMAGE="registry.k8s.io/networking/kube-network-policies@sha256:e3d06fa362c163855bdbbd0d5fa72fe9fabe147a21de62e8e88d22cbfeafe2a4"
readonly CMA_STAGING_NETWORK_POLICY_AMD64_DIGEST="sha256:0070d44cdb811b280d171ffa98e23c2749d604c59ccc968b1fd59c1ccda2993e"
readonly CMA_STAGING_REPOSITORY_ID="1330597394"
readonly CMA_STAGING_MONGO_USERNAME="cma_trigger_smoke"
readonly CMA_STAGING_POD_SUBNET="10.244.0.0/16"
readonly CMA_STAGING_SERVICE_SUBNET="10.96.0.0/16"
readonly CMA_STAGING_KUBERNETES_SERVICE_IP="10.96.0.1/32"
readonly CMA_STAGING_MINIMUM_TOKEN_LIFETIME_SECONDS=7200
readonly CMA_STAGING_MAXIMUM_TOKEN_LIFETIME_SECONDS=28800

cma_staging_die() {
  printf 'cma-trigger Kind staging: %s\n' "$*" >&2
  exit 1
}

cma_staging_require_command() {
  command -v "$1" >/dev/null 2>&1 || cma_staging_die "required command is unavailable: $1"
}

cma_staging_require_commands() {
  local command_name
  for command_name in "$@"; do
    cma_staging_require_command "${command_name}"
  done
}

cma_staging_sha256_file() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum "$1" | awk '{print $1}'
  else
    shasum -a 256 "$1" | awk '{print $1}'
  fi
}

cma_staging_sha256_valid() {
  [[ "$1" =~ ^[0-9a-f]{64}$ ]]
}

cma_staging_require_value() {
  local name="$1"
  local value="${!name:-}"
  [[ -n "${value}" ]] || cma_staging_die "${name} is required"
}

cma_staging_dns_label() {
  [[ "$1" =~ ^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$ ]]
}

cma_staging_state_root() {
  cma_staging_require_value RUNNER_TEMP
  [[ "${RUNNER_TEMP}" == /* && "${RUNNER_TEMP}" != / && -d "${RUNNER_TEMP}" && ! -L "${RUNNER_TEMP}" ]] || \
    cma_staging_die "RUNNER_TEMP must be an existing absolute non-symlink directory"
  printf '%s\n' "${RUNNER_TEMP}/cma-trigger-kind-staging"
}

cma_staging_observer_kubeconfig() {
  cma_staging_require_value RUNNER_TEMP
  printf '%s\n' "${RUNNER_TEMP}/${CMA_STAGING_OBSERVER_KUBECONFIG_NAME}"
}

cma_staging_require_state_root() {
  local expected
  expected="$(cma_staging_state_root)"
  [[ "${CMA_TRIGGER_KIND_STAGING_ROOT:-}" == "${expected}" ]] || \
    cma_staging_die "CMA_TRIGGER_KIND_STAGING_ROOT must be ${expected}"
  [[ -d "${expected}" && ! -L "${expected}" ]] || \
    cma_staging_die "staging state root is absent or unsafe"
  printf '%s\n' "${expected}"
}

cma_staging_marker_valid() {
  local root="$1"
  jq -e \
    --arg cluster "${CMA_STAGING_CLUSTER}" \
    --arg run_id "${GITHUB_RUN_ID:-}" \
    --arg run_attempt "${GITHUB_RUN_ATTEMPT:-}" \
    --argjson schema "${CMA_STAGING_SCHEMA_VERSION}" '
      keys == ["cluster","run_attempt","run_id","schema_version"] and
      .schema_version == $schema and
      .cluster == $cluster and
      .run_id == $run_id and
      .run_attempt == $run_attempt
    ' "${root}/ownership.json" >/dev/null
}

cma_staging_require_marker() {
  local root
  root="$(cma_staging_require_state_root)"
  cma_staging_marker_valid "${root}" || cma_staging_die "staging ownership marker is invalid"
  printf '%s\n' "${root}"
}

cma_staging_require_phase() {
  local root phase
  root="$(cma_staging_require_marker)"
  shift
  for phase in "$@"; do
    [[ -f "${root}/phase-${phase}" && ! -L "${root}/phase-${phase}" ]] || \
      cma_staging_die "required staging phase is incomplete: ${phase}"
  done
  printf '%s\n' "${root}"
}

cma_staging_decode_base64() {
  local value="$1"
  local destination="$2"
  [[ ! -e "${destination}" && ! -L "${destination}" ]] || \
    cma_staging_die "refusing to overwrite a protected staging path"
  if ! printf '%s' "${value}" | base64 --decode > "${destination}" 2>/dev/null; then
    cma_staging_die "protected staging input is not valid base64"
  fi
  chmod 0600 "${destination}"
  [[ -s "${destination}" && ! -L "${destination}" ]] || \
    cma_staging_die "decoded protected staging input is empty or unsafe"
}

cma_staging_parse_observer_kubeconfig() {
  local kubeconfig="$1"
  local namespace="$2"
  local output_dir="$3"
  ruby - "${kubeconfig}" "${namespace}" "${output_dir}" \
    "${CMA_STAGING_CLUSTER}" "${CMA_STAGING_OBSERVER_CN}" \
    "${CMA_STAGING_CLUSTER_SERVER}" <<'RUBY'
require "base64"
require "fileutils"
require "yaml"

path, namespace, output, cluster_name, observer_name, server = ARGV
def exact_keys(value, expected, label)
  raise "#{label} has unexpected fields" unless value.is_a?(Hash) && value.keys.sort == expected.sort
end

raw = File.binread(path)
document = YAML.safe_load(raw, permitted_classes: [], permitted_symbols: [], aliases: false)
allowed_top = ["apiVersion", "kind", "clusters", "contexts", "current-context", "preferences", "users"]
exact_keys(document, allowed_top, "kubeconfig")
raise "wrong kubeconfig kind" unless document["apiVersion"] == "v1" && document["kind"] == "Config"
raise "preferences must be empty" unless document["preferences"] == {}
raise "wrong cluster count" unless document["clusters"].is_a?(Array) && document["clusters"].length == 1
raise "wrong context count" unless document["contexts"].is_a?(Array) && document["contexts"].length == 1
raise "wrong user count" unless document["users"].is_a?(Array) && document["users"].length == 1

cluster = document["clusters"].fetch(0)
exact_keys(cluster, ["name", "cluster"], "cluster entry")
raise "wrong cluster name" unless cluster["name"] == cluster_name
exact_keys(cluster["cluster"], ["certificate-authority-data", "server"], "cluster authority")
raise "wrong cluster server" unless cluster["cluster"]["server"] == server

user = document["users"].fetch(0)
exact_keys(user, ["name", "user"], "user entry")
raise "wrong user name" unless user["name"] == observer_name
exact_keys(user["user"], ["client-certificate-data", "client-key-data"], "user credential")

context = document["contexts"].fetch(0)
exact_keys(context, ["name", "context"], "context entry")
raise "wrong context name" unless context["name"] == observer_name
exact_keys(context["context"], ["cluster", "namespace", "user"], "context binding")
raise "wrong context cluster" unless context["context"]["cluster"] == cluster_name
raise "wrong context user" unless context["context"]["user"] == observer_name
raise "wrong context namespace" unless context["context"]["namespace"] == namespace
raise "wrong current context" unless document["current-context"] == observer_name

files = {
  "ca.crt" => cluster["cluster"]["certificate-authority-data"],
  "observer.crt" => user["user"]["client-certificate-data"],
  "observer.key" => user["user"]["client-key-data"],
}
FileUtils.mkdir_p(output, mode: 0700)
files.each do |name, encoded|
  raise "#{name} data is not a strict base64 string" unless encoded.is_a?(String)
  decoded = Base64.strict_decode64(encoded)
  raise "#{name} is empty" if decoded.empty?
  target = File.join(output, name)
  File.binwrite(target, decoded, mode: "wx", perm: 0600)
end
RUBY
}

cma_staging_public_key_digest() {
  local mode="$1"
  local path="$2"
  local temporary="$3"
  case "${mode}" in
    certificate) openssl x509 -in "${path}" -pubkey -noout > "${temporary}" ;;
    key) openssl pkey -in "${path}" -pubout > "${temporary}" 2>/dev/null ;;
    *) return 1 ;;
  esac
  cma_staging_sha256_file "${temporary}"
}

cma_staging_validate_certificates() {
  local root="$1"
  local material="${root}/observer-material"
  local pki="${root}/bootstrap/pki"
  local scratch="${root}/bootstrap/public-key.pem"
  local ca_cert_key ca_private_key observer_cert_key observer_private_key subject

  openssl x509 -in "${material}/ca.crt" -noout >/dev/null 2>&1 || \
    cma_staging_die "observer cluster CA certificate is invalid"
  openssl pkey -in "${pki}/ca.key" -noout >/dev/null 2>&1 || \
    cma_staging_die "Kind CA private key is invalid or encrypted"
  openssl x509 -in "${material}/observer.crt" -noout >/dev/null 2>&1 || \
    cma_staging_die "observer client certificate is invalid"
  openssl pkey -in "${material}/observer.key" -noout >/dev/null 2>&1 || \
    cma_staging_die "observer client private key is invalid or encrypted"

  ca_cert_key="$(cma_staging_public_key_digest certificate "${material}/ca.crt" "${scratch}")"
  ca_private_key="$(cma_staging_public_key_digest key "${pki}/ca.key" "${scratch}")"
  observer_cert_key="$(cma_staging_public_key_digest certificate "${material}/observer.crt" "${scratch}")"
  observer_private_key="$(cma_staging_public_key_digest key "${material}/observer.key" "${scratch}")"
  rm -f -- "${scratch}"
  [[ "${ca_cert_key}" == "${ca_private_key}" ]] || \
    cma_staging_die "Kind CA private key does not match the observer kubeconfig CA"
  [[ "${observer_cert_key}" == "${observer_private_key}" ]] || \
    cma_staging_die "observer client key does not match its certificate"

  openssl x509 -in "${material}/ca.crt" -checkend 10800 -noout >/dev/null 2>&1 || \
    cma_staging_die "observer cluster CA does not cover the staging run"
  openssl x509 -in "${material}/ca.crt" -noout -text | \
    grep -Eq 'CA:[[:space:]]*TRUE' || \
    cma_staging_die "observer cluster certificate is not a CA"
  openssl x509 -in "${material}/observer.crt" -checkend 10800 -noout >/dev/null 2>&1 || \
    cma_staging_die "observer client certificate does not cover the staging run"
  openssl verify -purpose sslclient -CAfile "${material}/ca.crt" \
    "${material}/observer.crt" >/dev/null 2>&1 || \
    cma_staging_die "observer certificate is not a valid client of the staging CA"
  subject="$(openssl x509 -in "${material}/observer.crt" -subject -nameopt RFC2253 -noout)"
  subject="${subject#subject=}"
  subject="${subject# }"
  [[ "${subject}" == "CN=${CMA_STAGING_OBSERVER_CN}" ]] || \
    cma_staging_die "observer certificate subject must contain only the reviewed CN"

  cp "${material}/ca.crt" "${pki}/ca.crt"
  chmod 0600 "${pki}/ca.crt" "${pki}/ca.key"
}

cma_staging_validate() {
  local root expected_root kubeconfig expected_digest actual_digest namespace
  cma_staging_require_commands awk base64 chmod cp grep jq mkdir openssl rm ruby uname
  if ! command -v sha256sum >/dev/null 2>&1 && ! command -v shasum >/dev/null 2>&1; then
    cma_staging_die "a SHA-256 command is required"
  fi
  [[ "$(uname -s)" == Linux && "$(uname -m)" == x86_64 ]] || \
    cma_staging_die "the reviewed staging topology supports only Linux x86_64 hosted runners"
  cma_staging_require_value GITHUB_RUN_ID
  cma_staging_require_value GITHUB_RUN_ATTEMPT
  cma_staging_require_value GITHUB_SHA
  [[ "${GITHUB_RUN_ID}" =~ ^[1-9][0-9]*$ && \
     "${GITHUB_RUN_ATTEMPT}" =~ ^[1-9][0-9]*$ ]] || \
    cma_staging_die "GitHub run identity must contain positive decimal values"
  [[ "${GITHUB_SHA}" =~ ^[0-9a-f]{40}$ ]] || cma_staging_die "GITHUB_SHA must be one full commit"
  cma_staging_require_value CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE
  namespace="${CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE}"
  cma_staging_dns_label "${namespace}" || cma_staging_die "Trigger namespace is not one DNS-1123 label"
  [[ "${namespace}" != "${CMA_STAGING_ORDINARY_NAMESPACE}" && \
     "${namespace}" != "${CMA_STAGING_SYSTEM_NAMESPACE}" ]] || \
    cma_staging_die "Trigger, ordinary, and system namespaces must be distinct"
  cma_staging_require_value CMA_TRIGGER_SMOKE_KUBECONFIG_B64
  cma_staging_require_value CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256
  cma_staging_require_value CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64
  expected_digest="${CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256}"
  cma_staging_sha256_valid "${expected_digest}" || \
    cma_staging_die "CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256 must be one lowercase SHA-256"

  expected_root="$(cma_staging_state_root)"
  [[ ! -e "${expected_root}" && ! -L "${expected_root}" ]] || \
    cma_staging_die "staging state root already exists"
  mkdir -m 0700 "${expected_root}"
  export CMA_TRIGGER_KIND_STAGING_ROOT="${expected_root}"
  root="${expected_root}"
  jq -n \
    --arg cluster "${CMA_STAGING_CLUSTER}" \
    --arg run_id "${GITHUB_RUN_ID}" \
    --arg run_attempt "${GITHUB_RUN_ATTEMPT}" \
    --argjson schema "${CMA_STAGING_SCHEMA_VERSION}" \
    '{schema_version:$schema,cluster:$cluster,run_id:$run_id,run_attempt:$run_attempt}' \
    > "${root}/ownership.json"
  chmod 0600 "${root}/ownership.json"
  mkdir -m 0700 \
    "${root}/bin" "${root}/bootstrap" "${root}/bootstrap/pki" \
    "${root}/credentials" "${root}/downloads" "${root}/generated" \
    "${root}/observer-material"

  kubeconfig="$(cma_staging_observer_kubeconfig)"
  cma_staging_decode_base64 "${CMA_TRIGGER_SMOKE_KUBECONFIG_B64}" "${kubeconfig}"
  actual_digest="$(cma_staging_sha256_file "${kubeconfig}")"
  [[ "${actual_digest}" == "${expected_digest}" ]] || \
    cma_staging_die "observer kubeconfig digest differs from the protected provenance binding"
  cma_staging_parse_observer_kubeconfig \
    "${kubeconfig}" "${namespace}" "${root}/observer-material"
  cma_staging_decode_base64 \
    "${CMA_TRIGGER_SMOKE_KIND_CA_KEY_B64}" "${root}/bootstrap/pki/ca.key"
  cma_staging_validate_certificates "${root}"
  printf '%s\n' "${namespace}" > "${root}/trigger-namespace"
  chmod 0600 "${root}/trigger-namespace"
  : > "${root}/phase-validated"
  printf '%s\n' "[staging] observer provenance and bootstrap CA validated"
}

cma_staging_kind() {
  local root
  root="$(cma_staging_require_marker)"
  "${root}/bin/kind" "$@"
}

cma_staging_kubectl() {
  local root
  root="$(cma_staging_require_marker)"
  kubectl --kubeconfig "${root}/bootstrap/admin-kubeconfig" "$@"
}

cma_staging_download() {
  local url="$1"
  local digest="$2"
  local destination="$3"
  local observed
  curl --fail --silent --show-error --location --proto '=https' --tlsv1.2 \
    --output "${destination}" "${url}"
  observed="$(cma_staging_sha256_file "${destination}")"
  [[ "${observed}" == "${digest}" ]] || \
    cma_staging_die "download digest mismatch for ${url}"
  chmod 0600 "${destination}"
}

cma_staging_install_kind() {
  local root="$1"
  cma_staging_download \
    "https://kind.sigs.k8s.io/dl/${CMA_STAGING_KIND_VERSION}/kind-linux-amd64" \
    "${CMA_STAGING_KIND_SHA256}" "${root}/bin/kind"
  chmod 0700 "${root}/bin/kind"
  [[ "$("${root}/bin/kind" version)" == *"${CMA_STAGING_KIND_VERSION}"* ]] || \
    cma_staging_die "downloaded Kind binary reports an unexpected version"
}

cma_staging_generate_local_secrets() {
  local root="$1"
  local ordinary_key trigger_key mongo_password mongo_keyfile tls_dir config
  ordinary_key="${root}/credentials/ordinary-provider-key"
  trigger_key="${root}/credentials/trigger-provider-key"
  mongo_password="${root}/credentials/mongo-password"
  mongo_keyfile="${root}/credentials/mongo-keyfile"
  tls_dir="${root}/credentials/local-tls"
  mkdir -m 0700 "${tls_dir}"
  openssl rand -hex 32 > "${ordinary_key}"
  openssl rand -hex 32 > "${trigger_key}"
  openssl rand -hex 32 > "${mongo_password}"
  openssl rand -base64 64 | tr -d '\r\n' > "${mongo_keyfile}"
  printf '\n' >> "${mongo_keyfile}"
  chmod 0600 "${ordinary_key}" "${trigger_key}" "${mongo_password}" "${mongo_keyfile}"
  ! cmp -s "${ordinary_key}" "${trigger_key}" || \
    cma_staging_die "ordinary and Trigger provider keys unexpectedly match"

  openssl genpkey -algorithm Ed25519 -out "${tls_dir}/ca.key" >/dev/null 2>&1
  openssl req -new -x509 -key "${tls_dir}/ca.key" -sha256 -days 2 \
    -subj "/CN=CMA Trigger smoke local CA" -out "${tls_dir}/ca.crt" >/dev/null 2>&1
  openssl genpkey -algorithm Ed25519 -out "${tls_dir}/server.key" >/dev/null 2>&1
  config="${tls_dir}/server.cnf"
  cat > "${config}" <<'EOF'
[req]
distinguished_name = dn
prompt = no
req_extensions = extensions
[dn]
CN = 127.0.0.1
[extensions]
subjectAltName = IP:127.0.0.1
extendedKeyUsage = serverAuth
keyUsage = critical,digitalSignature
EOF
  openssl req -new -key "${tls_dir}/server.key" -config "${config}" \
    -out "${tls_dir}/server.csr" >/dev/null 2>&1
  openssl x509 -req -in "${tls_dir}/server.csr" -CA "${tls_dir}/ca.crt" \
    -CAkey "${tls_dir}/ca.key" -CAcreateserial -days 2 -sha256 \
    -extfile "${config}" -extensions extensions -out "${tls_dir}/server.crt" \
    >/dev/null 2>&1
  openssl verify -purpose sslserver -CAfile "${tls_dir}/ca.crt" \
    "${tls_dir}/server.crt" >/dev/null 2>&1 || \
    cma_staging_die "generated local HTTPS certificate failed verification"
  openssl x509 -in "${tls_dir}/server.crt" -checkip 127.0.0.1 -noout \
    >/dev/null 2>&1 || cma_staging_die "local HTTPS certificate lacks the fixed IP SAN"
  shred -u -- "${tls_dir}/ca.key"
  chmod 0600 "${tls_dir}"/*
}

cma_staging_validate_approved_images() {
  local root="$1"
  local provenance api_archive runtime_archive api_image runtime_image
  local api_digest runtime_digest component archive archive_digest archive_size
  local expected_archive_digest expected_archive_size expected_name actual_image_id
  cma_staging_require_value GITHUB_REPOSITORY_ID
  cma_staging_require_value CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH
  cma_staging_require_value CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE
  cma_staging_require_value CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE
  cma_staging_require_value CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID
  cma_staging_require_value CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID
  [[ "${GITHUB_REPOSITORY_ID}" == "${CMA_STAGING_REPOSITORY_ID}" ]] || \
    cma_staging_die "approved images belong to an unreviewed repository"

  provenance="${CMA_TRIGGER_KIND_STAGING_IMAGE_PROVENANCE_PATH}"
  api_archive="${CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE}"
  runtime_archive="${CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE}"
  [[ "${provenance}" == "${RUNNER_TEMP}/cma-trigger-ci-images/approved-image-provenance.json" && \
     "${api_archive}" == "${RUNNER_TEMP}/cma-trigger-ci-images/api/cma-trigger-api.tar" && \
     "${runtime_archive}" == "${RUNNER_TEMP}/cma-trigger-ci-images/runtime/cma-trigger-runtime.tar" ]] || \
    cma_staging_die "approved image custody escaped its exact protected paths"
  for archive in "${provenance}" "${api_archive}" "${runtime_archive}"; do
    [[ -f "${archive}" && ! -L "${archive}" ]] || \
      cma_staging_die "an approved image custody file is absent or unsafe"
    [[ "$(ruby -e 'puts File.realpath(ARGV.fetch(0))' "${archive}")" == "${archive}" ]] || \
      cma_staging_die "an approved image custody path traverses a symlink"
  done

  jq -e \
    --arg source_sha "${GITHUB_SHA}" \
    --arg api_id "${CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID}" \
    --arg runtime_id "${CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID}" \
    --argjson repository_id "${CMA_STAGING_REPOSITORY_ID}" '
      def sha256: type == "string" and test("^[0-9a-f]{64}$");
      def image_id: type == "string" and test("^sha256:[0-9a-f]{64}$");
      def artifact_digest: type == "string" and test("^sha256:[0-9a-f]{64}$");
      def image($component; $approved_id):
        type == "object" and keys == [
          "archive_name", "archive_sha256", "archive_size", "artifact_digest",
          "artifact_id", "artifact_name", "image_id", "image_reference",
          "metadata_name", "metadata_sha256"
        ] and
        .artifact_id > 0 and (.artifact_id | floor) == .artifact_id and
        (.artifact_digest | artifact_digest) and
        .artifact_name == ("cma-trigger-ci-image-" + $component + "-" + $source_sha) and
        .archive_name == ("cma-trigger-" + $component + ".tar") and
        (.archive_sha256 | sha256) and
        .archive_size > 0 and (.archive_size | floor) == .archive_size and
        .metadata_name == ("cma-trigger-" + $component + ".json") and
        (.metadata_sha256 | sha256) and
        .image_reference == ("cma-" + $component + ":ci") and
        .image_id == $approved_id and (.image_id | image_id);
      type == "object" and
      keys == ["ci_run", "images", "kind", "repository_id", "schema_version", "source_sha"] and
      .schema_version == 1 and .kind == "cma-trigger-approved-ci-images" and
      .repository_id == $repository_id and .source_sha == $source_sha and
      (.ci_run | type) == "object" and (.ci_run | keys) == [
        "attempt", "conclusion", "event", "head_branch", "head_sha", "id", "workflow_path"
      ] and
      .ci_run.id > 0 and (.ci_run.id | floor) == .ci_run.id and
      .ci_run.attempt > 0 and (.ci_run.attempt | floor) == .ci_run.attempt and
      .ci_run.event == "push" and .ci_run.head_branch == "develop" and
      .ci_run.head_sha == $source_sha and .ci_run.conclusion == "success" and
      .ci_run.workflow_path == ".github/workflows/ci.yml" and
      (.images | type) == "object" and (.images | keys) == ["api", "runtime"] and
      (.images.api | image("api"; $api_id)) and
      (.images.runtime | image("runtime"; $runtime_id)) and
      .images.api.artifact_id != .images.runtime.artifact_id and
      .images.api.image_id != .images.runtime.image_id
    ' "${provenance}" >/dev/null || \
    cma_staging_die "approved image provenance is malformed or not bound to this source"

  for component in api runtime; do
    if [[ "${component}" == api ]]; then
      archive="${api_archive}"
    else
      archive="${runtime_archive}"
    fi
    expected_name="$(jq -er --arg component "${component}" \
      '.images[$component].archive_name' "${provenance}")"
    [[ "${archive##*/}" == "${expected_name}" ]] || \
      cma_staging_die "${component} archive filename differs from approved provenance"
    expected_archive_digest="$(jq -er --arg component "${component}" \
      '.images[$component].archive_sha256' "${provenance}")"
    expected_archive_size="$(jq -er --arg component "${component}" \
      '.images[$component].archive_size' "${provenance}")"
    archive_digest="$(cma_staging_sha256_file "${archive}")"
    archive_size="$(stat -c '%s' "${archive}")"
    [[ "${archive_digest}" == "${expected_archive_digest}" && \
       "${archive_size}" == "${expected_archive_size}" ]] || \
      cma_staging_die "${component} archive bytes differ from approved provenance"
    docker image load --input "${archive}" >/dev/null
    actual_image_id="$(docker image inspect --format '{{.Id}}' "cma-${component}:ci")"
    [[ "${actual_image_id}" == "$(jq -er --arg component "${component}" \
      '.images[$component].image_id' "${provenance}")" ]] || \
      cma_staging_die "${component} archive does not resolve to its approved image ID"
  done

  api_image="cma-api:ci"
  runtime_image="cma-runtime:ci"
  api_digest="${CMA_TRIGGER_KIND_STAGING_API_IMAGE_ID}"
  runtime_digest="${CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ID}"
  printf '%s\n' "${api_image}" > "${root}/api-image"
  printf '%s\n' "${runtime_image}" > "${root}/runtime-image"
  printf '%s\n' "${api_digest}" > "${root}/api-image-digest"
  printf '%s\n' "${runtime_digest}" > "${root}/runtime-image-digest"
  chmod 0600 "${root}/api-image" "${root}/runtime-image" \
    "${root}/api-image-digest" "${root}/runtime-image-digest"
}

cma_staging_create_cluster() {
  local root="$1"
  local kind_config="${root}/generated/kind.yaml"
  local create_status=0
  if cma_staging_kind get clusters 2>/dev/null | grep -Fxq "${CMA_STAGING_CLUSTER}"; then
    cma_staging_die "the fixed Kind cluster already exists before provisioning"
  fi
  if docker ps -a --filter "label=io.x-k8s.kind.cluster=${CMA_STAGING_CLUSTER}" \
    --format '{{.ID}}' | grep -q .; then
    cma_staging_die "a container already carries the fixed Kind cluster label"
  fi
  cat > "${kind_config}" <<EOF
kind: Cluster
apiVersion: kind.x-k8s.io/v1alpha4
featureGates:
  MutatingAdmissionPolicy: true
networking:
  apiServerAddress: "127.0.0.1"
  apiServerPort: 6443
  podSubnet: "${CMA_STAGING_POD_SUBNET}"
  serviceSubnet: "${CMA_STAGING_SERVICE_SUBNET}"
nodes:
  - role: control-plane
    extraPortMappings:
      - containerPort: ${CMA_STAGING_NODE_PORT}
        hostPort: 8443
        listenAddress: "127.0.0.1"
        protocol: TCP
    extraMounts:
      - hostPath: "${root}/bootstrap/pki/ca.crt"
        containerPath: /etc/kubernetes/pki/ca.crt
        readOnly: true
      - hostPath: "${root}/bootstrap/pki/ca.key"
        containerPath: /etc/kubernetes/pki/ca.key
        readOnly: true
EOF
  chmod 0600 "${kind_config}"
  : > "${root}/cluster-create-attempted"
  cma_staging_kind create cluster \
    --name "${CMA_STAGING_CLUSTER}" \
    --config "${kind_config}" \
    --image "${CMA_STAGING_NODE_IMAGE}" \
    --kubeconfig "${root}/bootstrap/admin-kubeconfig" \
    --wait 5m || create_status=$?
  if cma_staging_kind get clusters 2>/dev/null | grep -Fxq "${CMA_STAGING_CLUSTER}" || \
     docker ps -a --filter "label=io.x-k8s.kind.cluster=${CMA_STAGING_CLUSTER}" \
       --format '{{.ID}}' | grep -q .; then
    : > "${root}/cluster-created"
  fi
  [[ "${create_status}" -eq 0 && -f "${root}/cluster-created" ]] || \
    cma_staging_die "Kind cluster creation failed"
  chmod 0600 "${root}/bootstrap/admin-kubeconfig"
  [[ "$(cma_staging_kubectl config view --minify -o jsonpath='{.clusters[0].cluster.server}')" == \
      "${CMA_STAGING_CLUSTER_SERVER}" ]] || \
    cma_staging_die "Kind administrator kubeconfig has an unexpected API endpoint"
}

cma_staging_write_network_enforcer_manifest() {
  local root="$1"
  cat > "${root}/generated/network-policy-enforcer.yaml" <<EOF
apiVersion: v1
kind: ServiceAccount
metadata:
  name: cma-trigger-network-policies
  namespace: kube-system
automountServiceAccountToken: true
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: cma-trigger-network-policies
rules:
  - apiGroups: [""]
    resources: ["namespaces", "pods"]
    verbs: ["list", "watch"]
  - apiGroups: ["networking.k8s.io"]
    resources: ["networkpolicies"]
    verbs: ["list", "watch"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: cma-trigger-network-policies
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: cma-trigger-network-policies
subjects:
  - kind: ServiceAccount
    name: cma-trigger-network-policies
    namespace: kube-system
---
apiVersion: apps/v1
kind: DaemonSet
metadata:
  name: cma-trigger-network-policies
  namespace: kube-system
spec:
  selector:
    matchLabels: {app: cma-trigger-network-policies}
  template:
    metadata:
      labels: {app: cma-trigger-network-policies}
    spec:
      hostNetwork: true
      automountServiceAccountToken: true
      serviceAccountName: cma-trigger-network-policies
      nodeSelector: {kubernetes.io/os: linux}
      tolerations: [{operator: Exists}]
      containers:
        - name: network-policy-enforcer
          image: ${CMA_STAGING_NETWORK_POLICY_IMAGE}
          imagePullPolicy: IfNotPresent
          args:
            - /bin/netpol
            - --hostname-override=\$(MY_NODE_NAME)
            - --v=2
            - --nfqueue-id=98
            - --disable-nri=true
            - --fail-open=false
            - --strict-mode=true
          env:
            - name: MY_NODE_NAME
              valueFrom: {fieldRef: {fieldPath: spec.nodeName}}
          securityContext:
            privileged: true
            capabilities: {add: ["NET_ADMIN"]}
          resources:
            requests: {cpu: 100m, memory: 50Mi}
            limits: {cpu: 500m, memory: 256Mi}
          volumeMounts:
            - name: netns
              mountPath: /var/run/netns
              mountPropagation: HostToContainer
      volumes:
        - name: netns
          hostPath: {path: /var/run/netns, type: Directory}
EOF
  chmod 0600 "${root}/generated/network-policy-enforcer.yaml"
}

cma_staging_install_network_enforcer() {
  local root="$1"
  cma_staging_write_network_enforcer_manifest "${root}"
  cma_staging_kubectl apply -f "${root}/generated/network-policy-enforcer.yaml" >/dev/null
  cma_staging_kubectl -n kube-system rollout status \
    daemonset/cma-trigger-network-policies --timeout=5m >/dev/null
  [[ "$(cma_staging_kubectl -n kube-system get daemonset cma-trigger-network-policies \
      -o json | jq -er '.status.desiredNumberScheduled')" == 1 ]] || \
    cma_staging_die "network-policy enforcer is not scheduled on exactly one staging node"
  [[ "$(cma_staging_kubectl -n kube-system get daemonset cma-trigger-network-policies \
      -o json | jq -er '.status.numberReady')" == 1 ]] || \
    cma_staging_die "network-policy enforcer is not ready"
}

cma_staging_verify_cluster_ca() {
  local root="$1"
  local expected node_digest configmap_ca configmap_digest
  expected="$(cma_staging_sha256_file "${root}/observer-material/ca.crt")"
  node_digest="$(docker exec "${CMA_STAGING_CLUSTER}-control-plane" \
    sha256sum /etc/kubernetes/pki/ca.crt | awk '{print $1}')"
  [[ "${node_digest}" == "${expected}" ]] || \
    cma_staging_die "kubeadm did not consume the pre-provisioned cluster CA"
  configmap_ca="${root}/generated/cluster-info-ca.crt"
  cma_staging_kubectl -n kube-public get configmap cluster-info \
    -o jsonpath='{.data.kubeconfig}' | ruby -ryaml -rbase64 -e '
      document = YAML.safe_load(STDIN.read, permitted_classes: [], aliases: false)
      encoded = document.fetch("clusters").fetch(0).fetch("cluster").fetch("certificate-authority-data")
      File.binwrite(ARGV.fetch(0), Base64.strict_decode64(encoded), mode: "wx", perm: 0600)
    ' "${configmap_ca}"
  configmap_digest="$(cma_staging_sha256_file "${configmap_ca}")"
  [[ "${configmap_digest}" == "${expected}" ]] || \
    cma_staging_die "published cluster-info CA differs from the protected observer CA"
}

cma_staging_download_crds() {
  local root="$1"
  local base="https://raw.githubusercontent.com/alibaba/OpenSandbox/${CMA_STAGING_OPENSANDBOX_SOURCE}/kubernetes/config/crd/bases"
  cma_staging_download \
    "${base}/sandbox.opensandbox.io_batchsandboxes.yaml" \
    "${CMA_STAGING_BATCHSANDBOX_CRD_SHA256}" \
    "${root}/downloads/batchsandboxes.yaml"
  cma_staging_download \
    "${base}/sandbox.opensandbox.io_pools.yaml" \
    "${CMA_STAGING_POOL_CRD_SHA256}" \
    "${root}/downloads/pools.yaml"
}

cma_staging_provider_config() {
  local root="$1"
  local lane="$2"
  local namespace="$3"
  local api_key_file="$4"
  local config_dir="${root}/generated/provider-${lane}"
  local api_key
  mkdir -m 0700 "${config_dir}"
  api_key="$(<"${api_key_file}")"
  [[ "${api_key}" =~ ^[0-9a-f]{64}$ ]] || cma_staging_die "generated provider key is invalid"
  cat > "${config_dir}/config.toml" <<EOF
[server]
host = "0.0.0.0"
port = 8080
log_level = "INFO"
api_key = "${api_key}"

[runtime]
type = "kubernetes"
execd_image = "${CMA_STAGING_EXECD_IMAGE}"

[storage]
allowed_host_paths = []

[kubernetes]
namespace = "${namespace}"
workload_provider = "batchsandbox"
batchsandbox_template_file = "/etc/opensandbox/batchsandbox-template.yaml"
EOF
  cat > "${config_dir}/batchsandbox-template.yaml" <<EOF
metadata: {}
spec:
  replicas: 1
  template:
    spec:
      restartPolicy: Never
      serviceAccountName: cma-trigger-sandbox-workload
      automountServiceAccountToken: false
      enableServiceLinks: false
      securityContext:
        runAsUser: 10001
        runAsGroup: 10001
        runAsNonRoot: true
        fsGroup: 10001
        seccompProfile: {type: RuntimeDefault}
      tolerations:
        - operator: Exists
EOF
  chmod 0600 "${config_dir}/config.toml" "${config_dir}/batchsandbox-template.yaml"
}

cma_staging_write_base_manifest() {
  local root="$1"
  local trigger_namespace="$2"
  local runtime_image
  runtime_image="$(<"${root}/runtime-image")"
  cat > "${root}/generated/base.yaml" <<EOF
apiVersion: v1
kind: Namespace
metadata:
  name: ${CMA_STAGING_SYSTEM_NAMESPACE}
  labels: {cma-trigger.chrono-ai.fun/lane: system}
---
apiVersion: v1
kind: Namespace
metadata:
  name: ${CMA_STAGING_ORDINARY_NAMESPACE}
  labels: {cma-trigger.chrono-ai.fun/lane: ordinary}
---
apiVersion: v1
kind: Namespace
metadata:
  name: ${trigger_namespace}
  labels: {cma-trigger.chrono-ai.fun/lane: trigger}
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: opensandbox-controller
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
automountServiceAccountToken: true
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: opensandbox-ordinary
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
automountServiceAccountToken: true
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: opensandbox-trigger
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
automountServiceAccountToken: true
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: cma-trigger-smoke-cma
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
automountServiceAccountToken: true
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: cma-trigger-image-attestor
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
automountServiceAccountToken: true
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: cma-trigger-sandbox-workload
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
automountServiceAccountToken: false
---
apiVersion: v1
kind: ServiceAccount
metadata:
  name: cma-trigger-sandbox-workload
  namespace: ${trigger_namespace}
automountServiceAccountToken: false
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRole
metadata:
  name: cma-trigger-smoke-opensandbox-controller-read
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list", "watch"]
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes", "pools"]
    verbs: ["get", "list", "patch", "update", "watch"]
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes/finalizers", "pools/finalizers"]
    verbs: ["update"]
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes/status", "pools/status"]
    verbs: ["get", "patch", "update"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: ClusterRoleBinding
metadata:
  name: cma-trigger-smoke-opensandbox-controller-read
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: ClusterRole
  name: cma-trigger-smoke-opensandbox-controller-read
subjects:
  - kind: ServiceAccount
    name: opensandbox-controller
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-opensandbox-controller-workload
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
rules:
  - apiGroups: [""]
    resources: ["events", "pods"]
    verbs: ["create", "delete", "get", "list", "patch", "update", "watch"]
  - apiGroups: [""]
    resources: ["pods/status"]
    verbs: ["get", "patch", "update"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-opensandbox-controller-workload
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-opensandbox-controller-workload
subjects:
  - kind: ServiceAccount
    name: opensandbox-controller
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-opensandbox-controller-workload
  namespace: ${trigger_namespace}
rules:
  - apiGroups: [""]
    resources: ["events", "pods"]
    verbs: ["create", "delete", "get", "list", "patch", "update", "watch"]
  - apiGroups: [""]
    resources: ["pods/status"]
    verbs: ["get", "patch", "update"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-opensandbox-controller-workload
  namespace: ${trigger_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-opensandbox-controller-workload
subjects:
  - kind: ServiceAccount
    name: opensandbox-controller
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: opensandbox-lifecycle-ordinary
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
rules:
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes"]
    verbs: ["create", "get", "list", "watch", "update", "patch", "delete"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: opensandbox-lifecycle-ordinary
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: opensandbox-lifecycle-ordinary
subjects:
  - kind: ServiceAccount
    name: opensandbox-ordinary
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: opensandbox-lifecycle-trigger
  namespace: ${trigger_namespace}
rules:
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes"]
    verbs: ["create", "get", "list", "watch", "update", "patch", "delete"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: opensandbox-lifecycle-trigger
  namespace: ${trigger_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: opensandbox-lifecycle-trigger
subjects:
  - kind: ServiceAccount
    name: opensandbox-trigger
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-batchsandbox-reclaimer
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
rules:
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes"]
    verbs: ["get", "list", "delete"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-batchsandbox-reclaimer
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-batchsandbox-reclaimer
subjects:
  - kind: ServiceAccount
    name: cma-trigger-smoke-cma
    namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-batchsandbox-reclaimer
  namespace: ${trigger_namespace}
rules:
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes"]
    verbs: ["get", "list", "delete"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-batchsandbox-reclaimer
  namespace: ${trigger_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-batchsandbox-reclaimer
subjects:
  - kind: ServiceAccount
    name: cma-trigger-smoke-cma
    namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-smoke-observer
  namespace: ${trigger_namespace}
rules:
  - apiGroups: ["sandbox.opensandbox.io"]
    resources: ["batchsandboxes"]
    verbs: ["list"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-smoke-observer
  namespace: ${trigger_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-smoke-observer
subjects:
  - kind: User
    name: ${CMA_STAGING_OBSERVER_CN}
    apiGroup: rbac.authorization.k8s.io
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-image-attestor-pods
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-image-attestor-pods
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-image-attestor-pods
subjects:
  - kind: ServiceAccount
    name: cma-trigger-image-attestor
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-image-attestor-pods
  namespace: ${trigger_namespace}
rules:
  - apiGroups: [""]
    resources: ["pods"]
    verbs: ["get", "list"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-image-attestor-pods
  namespace: ${trigger_namespace}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-image-attestor-pods
subjects:
  - kind: ServiceAccount
    name: cma-trigger-image-attestor
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-image-attestation-writer
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
rules:
  - apiGroups: [""]
    resources: ["configmaps"]
    resourceNames: ["${CMA_STAGING_ATTESTATION_CONFIGMAP}"]
    verbs: ["get", "patch", "update"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-image-attestation-writer
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-image-attestation-writer
subjects:
  - kind: ServiceAccount
    name: cma-trigger-image-attestor
    namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
---
apiVersion: rbac.authorization.k8s.io/v1
kind: Role
metadata:
  name: cma-trigger-image-attestation-reader
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
rules:
  - apiGroups: [""]
    resources: ["configmaps"]
    resourceNames: ["${CMA_STAGING_ATTESTATION_CONFIGMAP}"]
    verbs: ["get"]
---
apiVersion: rbac.authorization.k8s.io/v1
kind: RoleBinding
metadata:
  name: cma-trigger-image-attestation-reader
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
roleRef:
  apiGroup: rbac.authorization.k8s.io
  kind: Role
  name: cma-trigger-image-attestation-reader
subjects:
  - kind: User
    name: ${CMA_STAGING_OBSERVER_CN}
    apiGroup: rbac.authorization.k8s.io
---
apiVersion: admissionregistration.k8s.io/v1beta1
kind: MutatingAdmissionPolicy
metadata:
  name: cma-trigger-sandbox-hardening
spec:
  failurePolicy: Fail
  reinvocationPolicy: Never
  matchConstraints:
    namespaceSelector:
      matchExpressions:
        - key: cma-trigger.chrono-ai.fun/lane
          operator: In
          values: [ordinary, trigger]
    resourceRules:
      - apiGroups: [""]
        apiVersions: ["v1"]
        operations: ["CREATE"]
        resources: ["pods"]
  matchConditions:
    - name: opensandbox-owned-pod
      expression: >-
        has(object.metadata.labels) &&
        "opensandbox.io/id" in object.metadata.labels
    - name: exact-controller
      expression: >-
        request.userInfo.username ==
        "system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-controller"
  mutations:
    - patchType: JSONPatch
      jsonPatch:
        expression: >-
          [
            JSONPatch{
              op: "add",
              path: "/spec/containers/0/securityContext",
              value: Object.spec.containers.securityContext{
                allowPrivilegeEscalation: false,
                capabilities: Object.spec.containers.securityContext.capabilities{
                  drop: ["ALL"]
                },
                privileged: false,
                readOnlyRootFilesystem: false,
                runAsGroup: 10001,
                runAsNonRoot: true,
                runAsUser: 10001,
                seccompProfile: Object.spec.containers.securityContext.seccompProfile{
                  type: "RuntimeDefault"
                }
              }
            },
            JSONPatch{
              op: "add",
              path: "/spec/initContainers/0/securityContext",
              value: Object.spec.initContainers.securityContext{
                allowPrivilegeEscalation: false,
                capabilities: Object.spec.initContainers.securityContext.capabilities{
                  drop: ["ALL"]
                },
                privileged: false,
                readOnlyRootFilesystem: true,
                runAsGroup: 10001,
                runAsNonRoot: true,
                runAsUser: 10001,
                seccompProfile: Object.spec.initContainers.securityContext.seccompProfile{
                  type: "RuntimeDefault"
                }
              }
            }
          ]
---
apiVersion: admissionregistration.k8s.io/v1beta1
kind: MutatingAdmissionPolicyBinding
metadata:
  name: cma-trigger-sandbox-hardening
spec:
  policyName: cma-trigger-sandbox-hardening
  matchResources:
    namespaceSelector:
      matchExpressions:
        - key: cma-trigger.chrono-ai.fun/lane
          operator: In
          values: [ordinary, trigger]
---
apiVersion: admissionregistration.k8s.io/v1
kind: ValidatingAdmissionPolicy
metadata:
  name: cma-trigger-batchsandbox-boundary
spec:
  failurePolicy: Fail
  matchConstraints:
    namespaceSelector:
      matchExpressions:
        - key: cma-trigger.chrono-ai.fun/lane
          operator: In
          values: [ordinary, trigger]
    resourceRules:
      - apiGroups: ["sandbox.opensandbox.io"]
        apiVersions: ["v1alpha1"]
        operations: ["CREATE", "UPDATE"]
        resources: ["batchsandboxes"]
  validations:
    - expression: >-
        request.userInfo.username in [
          "system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-ordinary",
          "system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-trigger",
          "system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-controller"
        ]
      message: BatchSandbox writes require one reviewed provider or controller identity
    - expression: object.spec.replicas == 1
      message: BatchSandbox replicas must be exactly one
    - expression: >-
        (!has(object.spec.poolRef) || object.spec.poolRef == "") &&
        (!has(object.spec.shardPatches) || size(object.spec.shardPatches) == 0) &&
        (!has(object.spec.shardTaskPatches) || size(object.spec.shardTaskPatches) == 0) &&
        !has(object.spec.taskTemplate)
      message: Pool, task, and shard-patch authority is forbidden in Trigger staging
    - expression: >-
        has(object.spec.template) && has(object.spec.template.spec) &&
        object.spec.template.spec.serviceAccountName == "cma-trigger-sandbox-workload" &&
        object.spec.template.spec.automountServiceAccountToken == false &&
        object.spec.template.spec.enableServiceLinks == false &&
        (!has(object.spec.template.spec.hostNetwork) || object.spec.template.spec.hostNetwork == false) &&
        (!has(object.spec.template.spec.hostPID) || object.spec.template.spec.hostPID == false) &&
        (!has(object.spec.template.spec.hostIPC) || object.spec.template.spec.hostIPC == false) &&
        (!has(object.spec.template.spec.shareProcessNamespace) || object.spec.template.spec.shareProcessNamespace == false)
      message: BatchSandbox host and service-account isolation drifted
    - expression: >-
        size(object.spec.template.spec.volumes) == 1 &&
        object.spec.template.spec.volumes[0].name == "opensandbox-bin" &&
        has(object.spec.template.spec.volumes[0].emptyDir) &&
        !has(object.spec.template.spec.volumes[0].hostPath) &&
        !has(object.spec.template.spec.volumes[0].secret) &&
        !has(object.spec.template.spec.volumes[0].configMap) &&
        !has(object.spec.template.spec.volumes[0].projected) &&
        !has(object.spec.template.spec.volumes[0].persistentVolumeClaim) &&
        !has(object.spec.template.spec.volumes[0].csi)
      message: BatchSandbox may mount only its one empty executable handoff volume
    - expression: >-
        size(object.spec.template.spec.containers) == 1 &&
        object.spec.template.spec.containers[0].name == "sandbox" &&
        object.spec.template.spec.containers[0].image == "${runtime_image}" &&
        size(object.spec.template.spec.containers[0].volumeMounts) == 1 &&
        object.spec.template.spec.containers[0].volumeMounts[0].name == "opensandbox-bin" &&
        object.spec.template.spec.containers[0].volumeMounts[0].mountPath == "/opt/opensandbox/bin" &&
        object.spec.template.spec.containers[0].env.all(e, !has(e.valueFrom))
      message: BatchSandbox runtime container escaped the approved image and literal environment contract
    - expression: >-
        size(object.spec.template.spec.initContainers) == 1 &&
        object.spec.template.spec.initContainers[0].name == "execd-installer" &&
        object.spec.template.spec.initContainers[0].image == "${CMA_STAGING_EXECD_IMAGE}" &&
        size(object.spec.template.spec.initContainers[0].volumeMounts) == 1 &&
        object.spec.template.spec.initContainers[0].volumeMounts[0].name == "opensandbox-bin" &&
        object.spec.template.spec.initContainers[0].volumeMounts[0].mountPath == "/opt/opensandbox/bin"
      message: BatchSandbox init container escaped the approved execd handoff contract
---
apiVersion: admissionregistration.k8s.io/v1
kind: ValidatingAdmissionPolicyBinding
metadata:
  name: cma-trigger-batchsandbox-boundary
spec:
  policyName: cma-trigger-batchsandbox-boundary
  validationActions: [Deny]
  matchResources:
    namespaceSelector:
      matchExpressions:
        - key: cma-trigger.chrono-ai.fun/lane
          operator: In
          values: [ordinary, trigger]
---
apiVersion: admissionregistration.k8s.io/v1
kind: ValidatingAdmissionPolicy
metadata:
  name: cma-trigger-sandbox-pod-boundary
spec:
  failurePolicy: Fail
  matchConstraints:
    namespaceSelector:
      matchExpressions:
        - key: cma-trigger.chrono-ai.fun/lane
          operator: In
          values: [ordinary, trigger]
    resourceRules:
      - apiGroups: [""]
        apiVersions: ["v1"]
        operations: ["CREATE", "UPDATE"]
        resources: ["pods"]
  matchConditions:
    - name: opensandbox-owned-pod
      expression: >-
        has(object.metadata.labels) &&
        "opensandbox.io/id" in object.metadata.labels
  validations:
    - expression: >-
        request.userInfo.username ==
        "system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-controller"
      message: Only the reviewed OpenSandbox controller may materialize sandbox Pods
    - expression: >-
        object.spec.serviceAccountName == "cma-trigger-sandbox-workload" &&
        object.spec.automountServiceAccountToken == false &&
        object.spec.enableServiceLinks == false &&
        (!has(object.spec.hostNetwork) || object.spec.hostNetwork == false) &&
        (!has(object.spec.hostPID) || object.spec.hostPID == false) &&
        (!has(object.spec.hostIPC) || object.spec.hostIPC == false) &&
        (!has(object.spec.shareProcessNamespace) || object.spec.shareProcessNamespace == false) &&
        !has(object.spec.nodeName) && !has(object.spec.runtimeClassName) &&
        (!has(object.spec.imagePullSecrets) || size(object.spec.imagePullSecrets) == 0)
      message: Sandbox Pod host, identity, or image-pull authority drifted
    - expression: >-
        object.spec.restartPolicy == "Never" &&
        object.spec.securityContext.runAsUser == 10001 &&
        object.spec.securityContext.runAsGroup == 10001 &&
        object.spec.securityContext.runAsNonRoot == true &&
        object.spec.securityContext.fsGroup == 10001 &&
        object.spec.securityContext.seccompProfile.type == "RuntimeDefault"
      message: Sandbox Pod-level process isolation drifted
    - expression: >-
        size(object.spec.volumes) == 1 &&
        object.spec.volumes[0].name == "opensandbox-bin" &&
        has(object.spec.volumes[0].emptyDir) &&
        !has(object.spec.volumes[0].hostPath) &&
        !has(object.spec.volumes[0].secret) &&
        !has(object.spec.volumes[0].configMap) &&
        !has(object.spec.volumes[0].projected) &&
        !has(object.spec.volumes[0].persistentVolumeClaim) &&
        !has(object.spec.volumes[0].csi)
      message: Sandbox Pod may mount only its one empty executable handoff volume
    - expression: >-
        size(object.spec.containers) == 1 &&
        object.spec.containers[0].name == "sandbox" &&
        object.spec.containers[0].image == "${runtime_image}" &&
        size(object.spec.containers[0].command) > 0 &&
        object.spec.containers[0].command[0] == "/opt/opensandbox/bin/bootstrap.sh" &&
        size(object.spec.containers[0].volumeMounts) == 1 &&
        object.spec.containers[0].volumeMounts[0].name == "opensandbox-bin" &&
        object.spec.containers[0].volumeMounts[0].mountPath == "/opt/opensandbox/bin" &&
        object.spec.containers[0].env.all(e, !has(e.valueFrom))
      message: Sandbox runtime container escaped the approved image and literal environment contract
    - expression: >-
        object.spec.containers[0].securityContext.allowPrivilegeEscalation == false &&
        object.spec.containers[0].securityContext.privileged == false &&
        object.spec.containers[0].securityContext.runAsNonRoot == true &&
        object.spec.containers[0].securityContext.runAsUser == 10001 &&
        object.spec.containers[0].securityContext.runAsGroup == 10001 &&
        object.spec.containers[0].securityContext.seccompProfile.type == "RuntimeDefault" &&
        object.spec.containers[0].securityContext.capabilities.drop == ["ALL"]
      message: Sandbox runtime container privilege controls drifted
    - expression: >-
        size(object.spec.initContainers) == 1 &&
        object.spec.initContainers[0].name == "execd-installer" &&
        object.spec.initContainers[0].image == "${CMA_STAGING_EXECD_IMAGE}" &&
        size(object.spec.initContainers[0].volumeMounts) == 1 &&
        object.spec.initContainers[0].volumeMounts[0].name == "opensandbox-bin" &&
        object.spec.initContainers[0].volumeMounts[0].mountPath == "/opt/opensandbox/bin" &&
        object.spec.initContainers[0].securityContext.allowPrivilegeEscalation == false &&
        object.spec.initContainers[0].securityContext.privileged == false &&
        object.spec.initContainers[0].securityContext.readOnlyRootFilesystem == true &&
        object.spec.initContainers[0].securityContext.runAsNonRoot == true &&
        object.spec.initContainers[0].securityContext.runAsUser == 10001 &&
        object.spec.initContainers[0].securityContext.runAsGroup == 10001 &&
        object.spec.initContainers[0].securityContext.seccompProfile.type == "RuntimeDefault" &&
        object.spec.initContainers[0].securityContext.capabilities.drop == ["ALL"]
      message: Sandbox init-container image or privilege controls drifted
---
apiVersion: admissionregistration.k8s.io/v1
kind: ValidatingAdmissionPolicyBinding
metadata:
  name: cma-trigger-sandbox-pod-boundary
spec:
  policyName: cma-trigger-sandbox-pod-boundary
  validationActions: [Deny]
  matchResources:
    namespaceSelector:
      matchExpressions:
        - key: cma-trigger.chrono-ai.fun/lane
          operator: In
          values: [ordinary, trigger]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: default-deny
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  podSelector: {}
  policyTypes: [Ingress, Egress]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: default-deny
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  podSelector: {}
  policyTypes: [Ingress, Egress]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: default-deny
  namespace: ${trigger_namespace}
spec:
  podSelector: {}
  policyTypes: [Ingress, Egress]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: opensandbox-controller-to-api
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  podSelector: {matchLabels: {app: opensandbox-controller}}
  policyTypes: [Egress]
  egress:
    - to: [{ipBlock: {cidr: ${CMA_STAGING_KUBERNETES_SERVICE_IP}}}]
      ports: [{protocol: TCP, port: 443}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: opensandbox-provider
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  podSelector:
    matchExpressions:
      - key: app
        operator: In
        values: [opensandbox-ordinary, opensandbox-trigger]
  policyTypes: [Ingress, Egress]
  ingress:
    - from:
        - namespaceSelector: {matchLabels: {cma-trigger.chrono-ai.fun/lane: ordinary}}
          podSelector: {matchLabels: {app: cma-trigger-smoke-api}}
        - podSelector:
            matchExpressions:
              - key: app
                operator: In
                values: [opensandbox-ordinary, opensandbox-trigger]
      ports: [{protocol: TCP, port: 8080}]
  egress:
    - to:
        - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: kube-system}}
          podSelector: {matchLabels: {k8s-app: kube-dns}}
      ports: [{protocol: UDP, port: 53}, {protocol: TCP, port: 53}]
    - to: [{ipBlock: {cidr: ${CMA_STAGING_KUBERNETES_SERVICE_IP}}}]
      ports: [{protocol: TCP, port: 443}]
    - to:
        - podSelector:
            matchExpressions:
              - key: app
                operator: In
                values: [opensandbox-ordinary, opensandbox-trigger]
      ports: [{protocol: TCP, port: 8080}]
    - to:
        - namespaceSelector:
            matchExpressions:
              - key: cma-trigger.chrono-ai.fun/lane
                operator: In
                values: [ordinary, trigger]
          podSelector:
            matchExpressions:
              - key: opensandbox.io/id
                operator: Exists
      ports: [{protocol: TCP, port: 9090}, {protocol: TCP, port: 44772}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: mongo
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  podSelector: {matchLabels: {app: cma-trigger-mongo}}
  policyTypes: [Ingress, Egress]
  ingress:
    - from:
        - namespaceSelector: {matchLabels: {cma-trigger.chrono-ai.fun/lane: ordinary}}
          podSelector: {matchLabels: {app: cma-trigger-smoke-api}}
        - podSelector: {matchLabels: {app: cma-trigger-mongo}}
      ports: [{protocol: TCP, port: 27017}]
  egress:
    - to:
        - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: kube-system}}
          podSelector: {matchLabels: {k8s-app: kube-dns}}
      ports: [{protocol: UDP, port: 53}, {protocol: TCP, port: 53}]
    - to: [{podSelector: {matchLabels: {app: cma-trigger-mongo}}}]
      ports: [{protocol: TCP, port: 27017}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: cma-api
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  podSelector: {matchLabels: {app: cma-trigger-smoke-api}}
  policyTypes: [Ingress, Egress]
  ingress:
    - from: [{podSelector: {matchLabels: {app: cma-trigger-smoke-nginx}}}]
      ports: [{protocol: TCP, port: 8080}]
  egress:
    - to:
        - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: kube-system}}
          podSelector: {matchLabels: {k8s-app: kube-dns}}
      ports: [{protocol: UDP, port: 53}, {protocol: TCP, port: 53}]
    - to:
        - namespaceSelector: {matchLabels: {cma-trigger.chrono-ai.fun/lane: system}}
          podSelector:
            matchExpressions:
              - key: app
                operator: In
                values: [opensandbox-ordinary, opensandbox-trigger]
      ports: [{protocol: TCP, port: 8080}]
    - to:
        - namespaceSelector: {matchLabels: {cma-trigger.chrono-ai.fun/lane: system}}
          podSelector: {matchLabels: {app: cma-trigger-mongo}}
      ports: [{protocol: TCP, port: 27017}]
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
            except:
              - 10.0.0.0/8
              - 100.64.0.0/10
              - 127.0.0.0/8
              - 169.254.0.0/16
              - 172.16.0.0/12
              - 192.168.0.0/16
              - 224.0.0.0/4
      ports: [{protocol: TCP, port: 443}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: cma-ingress
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  podSelector: {matchLabels: {app: cma-trigger-smoke-nginx}}
  policyTypes: [Ingress, Egress]
  ingress:
    - from: [{ipBlock: {cidr: 0.0.0.0/0}}]
      ports: [{protocol: TCP, port: 8443}]
  egress:
    - to:
        - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: kube-system}}
          podSelector: {matchLabels: {k8s-app: kube-dns}}
      ports: [{protocol: UDP, port: 53}, {protocol: TCP, port: 53}]
    - to: [{podSelector: {matchLabels: {app: cma-trigger-smoke-api}}}]
      ports: [{protocol: TCP, port: 8080}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: sandbox-runtime
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  podSelector:
    matchExpressions:
      - key: opensandbox.io/id
        operator: Exists
  policyTypes: [Ingress, Egress]
  ingress:
    - from:
        - namespaceSelector: {matchLabels: {cma-trigger.chrono-ai.fun/lane: system}}
          podSelector:
            matchExpressions:
              - key: app
                operator: In
                values: [opensandbox-ordinary, opensandbox-trigger]
      ports: [{protocol: TCP, port: 9090}, {protocol: TCP, port: 44772}]
  egress:
    - to:
        - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: kube-system}}
          podSelector: {matchLabels: {k8s-app: kube-dns}}
      ports: [{protocol: UDP, port: 53}, {protocol: TCP, port: 53}]
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
            except:
              - 10.0.0.0/8
              - 100.64.0.0/10
              - 127.0.0.0/8
              - 169.254.0.0/16
              - 172.16.0.0/12
              - 192.168.0.0/16
              - 224.0.0.0/4
      ports: [{protocol: TCP, port: 443}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: sandbox-runtime
  namespace: ${trigger_namespace}
spec:
  podSelector:
    matchExpressions:
      - key: opensandbox.io/id
        operator: Exists
  policyTypes: [Ingress, Egress]
  ingress:
    - from:
        - namespaceSelector: {matchLabels: {cma-trigger.chrono-ai.fun/lane: system}}
          podSelector:
            matchExpressions:
              - key: app
                operator: In
                values: [opensandbox-ordinary, opensandbox-trigger]
      ports: [{protocol: TCP, port: 9090}, {protocol: TCP, port: 44772}]
  egress:
    - to:
        - namespaceSelector: {matchLabels: {kubernetes.io/metadata.name: kube-system}}
          podSelector: {matchLabels: {k8s-app: kube-dns}}
      ports: [{protocol: UDP, port: 53}, {protocol: TCP, port: 53}]
    - to:
        - ipBlock:
            cidr: 0.0.0.0/0
            except:
              - 10.0.0.0/8
              - 100.64.0.0/10
              - 127.0.0.0/8
              - 169.254.0.0/16
              - 172.16.0.0/12
              - 192.168.0.0/16
              - 224.0.0.0/4
      ports: [{protocol: TCP, port: 443}]
---
apiVersion: networking.k8s.io/v1
kind: NetworkPolicy
metadata:
  name: image-attestor-to-api
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  podSelector: {matchLabels: {app: cma-trigger-image-attestor}}
  policyTypes: [Egress]
  egress:
    - to: [{ipBlock: {cidr: ${CMA_STAGING_KUBERNETES_SERVICE_IP}}}]
      ports: [{protocol: TCP, port: 443}]
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: ${CMA_STAGING_ATTESTATION_CONFIGMAP}
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
data:
  attestation.json: '{"api":[],"observed_at":"1970-01-01T00:00:00Z","runtime":[],"schema_version":1}'
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: cma-trigger-image-attestor-script
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
data:
  attest.py: |
    import datetime
    import json
    import time

    from kubernetes import client, config

    ORDINARY_NAMESPACE = "${CMA_STAGING_ORDINARY_NAMESPACE}"
    SYSTEM_NAMESPACE = "${CMA_STAGING_SYSTEM_NAMESPACE}"
    TRIGGER_NAMESPACE = "${trigger_namespace}"
    CONFIGMAP = "${CMA_STAGING_ATTESTATION_CONFIGMAP}"

    def retained_container(status):
        return {
            "image": status.image,
            "image_id": status.image_id,
            "name": status.name,
            "ready": bool(status.ready),
        }

    def retained_pod(pod):
        labels = pod.metadata.labels or {}
        statuses = sorted(
            [retained_container(value) for value in (pod.status.container_statuses or [])],
            key=lambda value: value["name"],
        )
        init_statuses = sorted(
            [retained_container(value) for value in (pod.status.init_container_statuses or [])],
            key=lambda value: value["name"],
        )
        return {
            "containers": statuses,
            "init_containers": init_statuses,
            "labels": {
                "canonical_kernel": labels.get("cma/kernel"),
                "canonical_session": labels.get("cma/session"),
                "runtime_generation": labels.get("cma/runtime-generation"),
                "sandbox_id": labels.get("opensandbox.io/id"),
            },
            "name": pod.metadata.name,
            "namespace": pod.metadata.namespace,
            "phase": pod.status.phase,
            "resource_version": pod.metadata.resource_version,
            "uid": pod.metadata.uid,
        }

    config.load_incluster_config()
    api = client.CoreV1Api()
    while True:
        api_pods = api.list_namespaced_pod(
            ORDINARY_NAMESPACE,
            label_selector="app=cma-trigger-smoke-api",
            _request_timeout=10,
        ).items
        runtime_pods = api.list_namespaced_pod(
            TRIGGER_NAMESPACE,
            label_selector="cma/kernel=codex-trigger-native-v1",
            _request_timeout=10,
        ).items
        payload = {
            "api": sorted([retained_pod(pod) for pod in api_pods], key=lambda pod: pod["uid"]),
            "observed_at": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
            "runtime": sorted([retained_pod(pod) for pod in runtime_pods], key=lambda pod: pod["uid"]),
            "schema_version": 1,
        }
        api.patch_namespaced_config_map(
            CONFIGMAP,
            SYSTEM_NAMESPACE,
            {"data": {"attestation.json": json.dumps(payload, separators=(",", ":"), sort_keys=True)}},
            _request_timeout=10,
        )
        time.sleep(2)
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cma-trigger-image-attestor
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: cma-trigger-image-attestor}
  template:
    metadata:
      labels: {app: cma-trigger-image-attestor}
    spec:
      serviceAccountName: cma-trigger-image-attestor
      automountServiceAccountToken: true
      enableServiceLinks: false
      securityContext:
        runAsUser: 10001
        runAsGroup: 10001
        runAsNonRoot: true
        seccompProfile: {type: RuntimeDefault}
      containers:
        - name: attestor
          image: ${CMA_STAGING_SERVER_IMAGE}
          imagePullPolicy: IfNotPresent
          command: ["python", "/opt/cma-attestor/attest.py"]
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            readOnlyRootFilesystem: true
            runAsNonRoot: true
          resources:
            requests: {cpu: 10m, memory: 32Mi}
            limits: {cpu: 100m, memory: 128Mi}
          volumeMounts:
            - {name: script, mountPath: /opt/cma-attestor, readOnly: true}
            - {name: tmp, mountPath: /tmp}
      volumes:
        - name: script
          configMap: {name: cma-trigger-image-attestor-script, defaultMode: 288}
        - name: tmp
          emptyDir: {}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensandbox-controller
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: opensandbox-controller}
  template:
    metadata:
      labels: {app: opensandbox-controller}
    spec:
      serviceAccountName: opensandbox-controller
      automountServiceAccountToken: true
      containers:
        - name: controller
          image: ${CMA_STAGING_CONTROLLER_IMAGE}
          imagePullPolicy: IfNotPresent
          args: ["--health-probe-bind-address=:8081", "--v=3"]
          readinessProbe:
            httpGet: {path: /readyz, port: 8081}
            periodSeconds: 5
          livenessProbe:
            httpGet: {path: /healthz, port: 8081}
            periodSeconds: 10
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            runAsNonRoot: true
          resources:
            requests: {cpu: 10m, memory: 64Mi}
            limits: {cpu: 500m, memory: 256Mi}
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensandbox-ordinary
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: opensandbox-ordinary}
  template:
    metadata:
      labels: {app: opensandbox-ordinary}
    spec:
      serviceAccountName: opensandbox-ordinary
      automountServiceAccountToken: true
      securityContext: {runAsUser: 10001, runAsGroup: 10001, fsGroup: 10001}
      containers:
        - name: server
          image: ${CMA_STAGING_SERVER_IMAGE}
          imagePullPolicy: IfNotPresent
          args: ["--config", "/etc/opensandbox/config.toml"]
          env: [{name: SANDBOX_CONFIG_PATH, value: /etc/opensandbox/config.toml}]
          ports: [{name: http, containerPort: 8080}]
          readinessProbe:
            httpGet: {path: /health, port: http}
            periodSeconds: 5
          livenessProbe:
            httpGet: {path: /health, port: http}
            periodSeconds: 10
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            readOnlyRootFilesystem: true
            runAsNonRoot: true
          resources:
            requests: {cpu: 50m, memory: 128Mi}
            limits: {cpu: 1000m, memory: 512Mi}
          volumeMounts:
            - {name: config, mountPath: /etc/opensandbox, readOnly: true}
            - {name: tmp, mountPath: /tmp}
      volumes:
        - name: config
          secret: {secretName: opensandbox-ordinary-config, defaultMode: 288}
        - name: tmp
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: opensandbox-ordinary
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  selector: {app: opensandbox-ordinary}
  ports: [{name: http, port: 8080, targetPort: http}]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: opensandbox-trigger
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: opensandbox-trigger}
  template:
    metadata:
      labels: {app: opensandbox-trigger}
    spec:
      serviceAccountName: opensandbox-trigger
      automountServiceAccountToken: true
      securityContext: {runAsUser: 10001, runAsGroup: 10001, fsGroup: 10001}
      containers:
        - name: server
          image: ${CMA_STAGING_SERVER_IMAGE}
          imagePullPolicy: IfNotPresent
          args: ["--config", "/etc/opensandbox/config.toml"]
          env: [{name: SANDBOX_CONFIG_PATH, value: /etc/opensandbox/config.toml}]
          ports: [{name: http, containerPort: 8080}]
          readinessProbe:
            httpGet: {path: /health, port: http}
            periodSeconds: 5
          livenessProbe:
            httpGet: {path: /health, port: http}
            periodSeconds: 10
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            readOnlyRootFilesystem: true
            runAsNonRoot: true
          resources:
            requests: {cpu: 50m, memory: 128Mi}
            limits: {cpu: 1000m, memory: 512Mi}
          volumeMounts:
            - {name: config, mountPath: /etc/opensandbox, readOnly: true}
            - {name: tmp, mountPath: /tmp}
      volumes:
        - name: config
          secret: {secretName: opensandbox-trigger-config, defaultMode: 288}
        - name: tmp
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: opensandbox-trigger
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  selector: {app: opensandbox-trigger}
  ports: [{name: http, port: 8080, targetPort: http}]
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: mongo
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: cma-trigger-mongo}
  template:
    metadata:
      labels: {app: cma-trigger-mongo}
    spec:
      automountServiceAccountToken: false
      securityContext: {runAsUser: 999, runAsGroup: 999, fsGroup: 999}
      initContainers:
        - name: mongo-keyfile
          image: ${CMA_STAGING_MONGO_IMAGE}
          imagePullPolicy: IfNotPresent
          command: ["sh", "-ceu", "cp /var/run/mongo-key-source/keyfile /var/run/mongo-keyfile/keyfile && chmod 0400 /var/run/mongo-keyfile/keyfile"]
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            readOnlyRootFilesystem: true
            runAsGroup: 999
            runAsNonRoot: true
            runAsUser: 999
          volumeMounts:
            - {name: mongo-key-source, mountPath: /var/run/mongo-key-source, readOnly: true}
            - {name: mongo-keyfile, mountPath: /var/run/mongo-keyfile}
      containers:
        - name: mongo
          image: ${CMA_STAGING_MONGO_IMAGE}
          imagePullPolicy: IfNotPresent
          args: ["--auth", "--keyFile", "/var/run/mongo-keyfile/keyfile", "--replSet", "rs0", "--bind_ip_all"]
          envFrom:
            - secretRef: {name: cma-trigger-smoke-mongo-auth}
          ports: [{name: mongo, containerPort: 27017}]
          readinessProbe:
            exec:
              command: ["sh", "-c", "mongosh --quiet --username \"\$MONGO_INITDB_ROOT_USERNAME\" --password \"\$MONGO_INITDB_ROOT_PASSWORD\" --authenticationDatabase admin --eval 'quit(db.adminCommand({ping:1}).ok == 1 ? 0 : 1)'"]
            periodSeconds: 5
          livenessProbe:
            exec:
              command: ["sh", "-c", "mongosh --quiet --username \"\$MONGO_INITDB_ROOT_USERNAME\" --password \"\$MONGO_INITDB_ROOT_PASSWORD\" --authenticationDatabase admin --eval 'quit(db.adminCommand({ping:1}).ok == 1 ? 0 : 1)'"]
            periodSeconds: 10
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            runAsNonRoot: true
          resources:
            requests: {cpu: 100m, memory: 256Mi}
            limits: {cpu: 1500m, memory: 1Gi}
          volumeMounts:
            - {name: data, mountPath: /data/db}
            - {name: mongo-keyfile, mountPath: /var/run/mongo-keyfile, readOnly: true}
            - {name: tmp, mountPath: /tmp}
      volumes:
        - name: data
          emptyDir: {}
        - name: mongo-key-source
          secret: {secretName: cma-trigger-smoke-mongo-keyfile, defaultMode: 288}
        - name: mongo-keyfile
          emptyDir: {}
        - name: tmp
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: mongo
  namespace: ${CMA_STAGING_SYSTEM_NAMESPACE}
spec:
  selector: {app: cma-trigger-mongo}
  ports: [{name: mongo, port: 27017, targetPort: mongo}]
EOF
  chmod 0600 "${root}/generated/base.yaml"
}

cma_staging_wait_rollout() {
  local namespace="$1"
  local deployment="$2"
  cma_staging_kubectl -n "${namespace}" rollout status \
    "deployment/${deployment}" --timeout=5m >/dev/null
}

cma_staging_wait_attestor() {
  local deadline=$((SECONDS + 120))
  local attestation
  while (( SECONDS < deadline )); do
    attestation="$(cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" get configmap \
      "${CMA_STAGING_ATTESTATION_CONFIGMAP}" -o jsonpath='{.data.attestation\.json}')"
    if jq -e '
      keys == ["api", "observed_at", "runtime", "schema_version"] and
      .schema_version == 1 and .api == [] and .runtime == [] and
      .observed_at != "1970-01-01T00:00:00Z" and
      (.observed_at | fromdateiso8601) <= now
    ' <<< "${attestation}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  cma_staging_die "the image attestor did not publish its closed initial observation"
}

cma_staging_wait_api_attestation() {
  local root="$1"
  local deadline=$((SECONDS + 120))
  local attestation api_image image_binding
  api_image="$(<"${root}/api-image")"
  image_binding="${root}/api-node-image.json"
  [[ -f "${image_binding}" && ! -L "${image_binding}" ]] || \
    cma_staging_die "approved API node-image binding is absent or unsafe"
  while (( SECONDS < deadline )); do
    attestation="$(cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" get configmap \
      "${CMA_STAGING_ATTESTATION_CONFIGMAP}" -o jsonpath='{.data.attestation\.json}')"
    if jq -e \
      --arg namespace "${CMA_STAGING_ORDINARY_NAMESPACE}" \
      --arg image "${api_image}" \
      --slurpfile image_binding "${image_binding}" '
        def normalized_image_id:
          sub("^docker-pullable://"; "") | sub("^docker://"; "");
        keys == ["api", "observed_at", "runtime", "schema_version"] and
        .schema_version == 1 and .runtime == [] and (.api | length) == 1 and
        (.observed_at | fromdateiso8601) <= now and
        (.observed_at | fromdateiso8601) >= (now - 30) and
        .api[0].namespace == $namespace and .api[0].phase == "Running" and
        (.api[0].name | test("^cma-api-[a-z0-9]+-[a-z0-9]+$")) and
        (.api[0].uid | test("^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")) and
        (.api[0].resource_version | test("^[1-9][0-9]*$")) and
        .api[0].init_containers == [] and (.api[0].containers | length) == 1 and
        .api[0].containers[0].name == "api" and
        .api[0].containers[0].image == $image and
        .api[0].containers[0].ready == true and
        ($image_binding | length) == 1 and
        ($image_binding[0].runtime_image_ids | type) == "array" and
        (.api[0].containers[0].image_id | normalized_image_id) as $observed and
        any($image_binding[0].runtime_image_ids[];
          (. | normalized_image_id) == $observed)
      ' <<< "${attestation}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  cma_staging_die "the running API Pod never produced exact reviewed image attestation"
}

cma_staging_provider_request_test() {
  local source_deployment="$1"
  local target_origin="$2"
  local expected_status="$3"
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" exec \
    "deployment/${source_deployment}" -- \
    python - "${target_origin}" "${expected_status}" <<'PY'
import sys
import urllib.error
import urllib.request

try:
    import tomllib
except ImportError:
    import tomli as tomllib

with open("/etc/opensandbox/config.toml", "rb") as handle:
    key = tomllib.load(handle)["server"]["api_key"]
request = urllib.request.Request(
    sys.argv[1] + "/v1/sandboxes",
    headers={"OPEN-SANDBOX-API-KEY": key},
)
try:
    with urllib.request.urlopen(request, timeout=10) as response:
        observed = response.status
except urllib.error.HTTPError as error:
    observed = error.code
if observed != int(sys.argv[2]):
    raise SystemExit(1)
PY
}

cma_staging_provider_network_denial_test() {
  local source_deployment="$1"
  local target_host="$2"
  local target_port="$3"
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" exec \
    "deployment/${source_deployment}" -- \
    python - "${target_host}" "${target_port}" <<'PY'
import socket
import sys

try:
    connection = socket.create_connection((sys.argv[1], int(sys.argv[2])), timeout=2)
except OSError:
    raise SystemExit(0)
connection.close()
raise SystemExit(1)
PY
}

cma_staging_initialize_mongo() {
  local deadline=$((SECONDS + 120))
  # Variables are expanded by the authenticated Mongo container, not this shell.
  # shellcheck disable=SC2016
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" exec deployment/mongo -- \
    sh -ceu 'mongosh --quiet --username "$MONGO_INITDB_ROOT_USERNAME" --password "$MONGO_INITDB_ROOT_PASSWORD" --authenticationDatabase admin --eval '\''rs.initiate({_id:"rs0",members:[{_id:0,host:"mongo.cma-trigger-system.svc.cluster.local:27017"}]})'\''' \
    >/dev/null
  while (( SECONDS < deadline )); do
    # shellcheck disable=SC2016
    if [[ "$(cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" exec \
      deployment/mongo -- sh -ceu 'mongosh --quiet --username "$MONGO_INITDB_ROOT_USERNAME" --password "$MONGO_INITDB_ROOT_PASSWORD" --authenticationDatabase admin --eval '\''db.hello().isWritablePrimary'\''' \
      2>/dev/null | tr -d '\r')" == true ]]; then
      return 0
    fi
    sleep 2
  done
  cma_staging_die "MongoDB replica set did not elect a writable primary"
}

cma_staging_capture_node_image() {
  local root="$1"
  local component="$2"
  local image approved_id output temporary
  image="$(<"${root}/${component}-image")"
  approved_id="$(<"${root}/${component}-image-digest")"
  output="${root}/${component}-node-image.json"
  temporary="${output}.tmp"
  [[ ! -e "${output}" && ! -L "${output}" && ! -e "${temporary}" && ! -L "${temporary}" ]] || \
    cma_staging_die "node-image binding output already exists"
  docker exec "${CMA_STAGING_CLUSTER}-control-plane" \
    crictl inspecti "${image}" -o json | jq -e \
      --arg approved_id "${approved_id}" \
      --arg component "${component}" \
      --arg image "${image}" '
        select(
          .status.id == $approved_id and
          ((.status.repoDigests // []) | type) == "array" and
          all((.status.repoDigests // [])[];
            type == "string" and test("^[^[:space:]]+@sha256:[0-9a-f]{64}$"))
        ) |
        {
          approved_image_id: $approved_id,
          component: $component,
          image_reference: $image,
          runtime_image_ids: ([.status.id] + (.status.repoDigests // []) | unique),
          schema_version: 1
        } |
        select(.runtime_image_ids | length > 0)
      ' > "${temporary}" || \
    cma_staging_die "Kind did not load the exact approved ${component} image identity"
  mv "${temporary}" "${output}"
  chmod 0600 "${output}"
}

cma_staging_prepare() {
  local root trigger_namespace ordinary_config trigger_config
  root="$(cma_staging_require_phase validated)"
  cma_staging_require_commands \
    cat chmod cmp curl docker git grep jq kubectl mv openssl ruby sleep stat tr
  [[ "$(pwd -P)" == "$(git rev-parse --show-toplevel)" ]] || \
    cma_staging_die "prepare must run from the repository root"
  [[ "$(git rev-parse HEAD)" == "${GITHUB_SHA}" ]] || \
    cma_staging_die "checkout HEAD differs from GITHUB_SHA"
  cma_staging_install_kind "${root}"
  cma_staging_generate_local_secrets "${root}"
  cma_staging_validate_approved_images "${root}"
  cma_staging_create_cluster "${root}"
  cma_staging_verify_cluster_ca "${root}"
  cma_staging_install_network_enforcer "${root}"
  cma_staging_download_crds "${root}"
  cma_staging_kubectl apply -f "${root}/downloads/batchsandboxes.yaml" >/dev/null
  cma_staging_kubectl apply -f "${root}/downloads/pools.yaml" >/dev/null
  trigger_namespace="$(<"${root}/trigger-namespace")"
  cma_staging_provider_config "${root}" ordinary "${CMA_STAGING_ORDINARY_NAMESPACE}" \
    "${root}/credentials/ordinary-provider-key"
  cma_staging_provider_config "${root}" trigger "${trigger_namespace}" \
    "${root}/credentials/trigger-provider-key"
  ordinary_config="${root}/generated/provider-ordinary"
  trigger_config="${root}/generated/provider-trigger"
  cma_staging_kubectl create namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" >/dev/null
  cma_staging_kubectl create namespace "${CMA_STAGING_ORDINARY_NAMESPACE}" >/dev/null
  cma_staging_kubectl create namespace "${trigger_namespace}" >/dev/null
  cma_staging_kubectl label namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" \
    cma-trigger.chrono-ai.fun/lane=system >/dev/null
  cma_staging_kubectl label namespace "${CMA_STAGING_ORDINARY_NAMESPACE}" \
    cma-trigger.chrono-ai.fun/lane=ordinary >/dev/null
  cma_staging_kubectl label namespace "${trigger_namespace}" \
    cma-trigger.chrono-ai.fun/lane=trigger >/dev/null
  cma_staging_write_base_manifest "${root}" "${trigger_namespace}"
  cma_staging_kubectl apply -f "${root}/generated/base.yaml" >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" create secret generic \
    opensandbox-ordinary-config \
    --from-file="config.toml=${ordinary_config}/config.toml" \
    --from-file="batchsandbox-template.yaml=${ordinary_config}/batchsandbox-template.yaml" \
    >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" create secret generic \
    opensandbox-trigger-config \
    --from-file="config.toml=${trigger_config}/config.toml" \
    --from-file="batchsandbox-template.yaml=${trigger_config}/batchsandbox-template.yaml" \
    >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" create secret generic \
    cma-trigger-smoke-mongo-auth \
    --from-literal="MONGO_INITDB_ROOT_USERNAME=${CMA_STAGING_MONGO_USERNAME}" \
    --from-file="MONGO_INITDB_ROOT_PASSWORD=${root}/credentials/mongo-password" \
    >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_SYSTEM_NAMESPACE}" create secret generic \
    cma-trigger-smoke-mongo-keyfile \
    --from-file="keyfile=${root}/credentials/mongo-keyfile" \
    >/dev/null
  cma_staging_wait_rollout "${CMA_STAGING_SYSTEM_NAMESPACE}" opensandbox-controller
  cma_staging_wait_rollout "${CMA_STAGING_SYSTEM_NAMESPACE}" opensandbox-ordinary
  cma_staging_wait_rollout "${CMA_STAGING_SYSTEM_NAMESPACE}" opensandbox-trigger
  cma_staging_wait_rollout "${CMA_STAGING_SYSTEM_NAMESPACE}" cma-trigger-image-attestor
  cma_staging_wait_rollout "${CMA_STAGING_SYSTEM_NAMESPACE}" mongo
  cma_staging_wait_attestor
  cma_staging_provider_request_test opensandbox-ordinary \
    "${CMA_STAGING_ORDINARY_PROVIDER_ORIGIN}" 200
  cma_staging_provider_request_test opensandbox-trigger \
    "${CMA_STAGING_TRIGGER_PROVIDER_ORIGIN}" 200
  cma_staging_provider_request_test opensandbox-ordinary \
    "${CMA_STAGING_TRIGGER_PROVIDER_ORIGIN}" 401
  cma_staging_provider_request_test opensandbox-trigger \
    "${CMA_STAGING_ORDINARY_PROVIDER_ORIGIN}" 401
  cma_staging_initialize_mongo
  cma_staging_provider_network_denial_test opensandbox-ordinary \
    "mongo.${CMA_STAGING_SYSTEM_NAMESPACE}.svc.cluster.local" 27017
  cma_staging_provider_network_denial_test opensandbox-trigger \
    "mongo.${CMA_STAGING_SYSTEM_NAMESPACE}.svc.cluster.local" 27017
  cma_staging_kind load image-archive \
    "${CMA_TRIGGER_KIND_STAGING_API_IMAGE_ARCHIVE}" \
    "${CMA_TRIGGER_KIND_STAGING_RUNTIME_IMAGE_ARCHIVE}" \
    --name "${CMA_STAGING_CLUSTER}" >/dev/null
  cma_staging_capture_node_image "${root}" api
  cma_staging_capture_node_image "${root}" runtime
  : > "${root}/phase-prepared"
  printf '%s\n' "[staging] Kind, provider lanes, image pins, and Mongo replica set prepared"
}

cma_staging_handoff_field() {
  local handoff="$1"
  local field="$2"
  jq -er --arg field "${field}" '.[$field] | select(type == "string" and length > 0)' \
    "${handoff}"
}

cma_staging_validate_handoff() {
  local root="$1"
  local handoff="$2"
  local mode now minimum_expiry maximum_expiry
  [[ "${handoff}" == "${root}/credentials/nyxid-handoff.json" ]] || \
    cma_staging_die "NyxID handoff must stay in the staging credential directory"
  [[ -f "${handoff}" && ! -L "${handoff}" ]] || \
    cma_staging_die "NyxID handoff is absent or unsafe"
  mode="$(stat -c '%a' "${handoff}")"
  [[ "${mode}" == 600 ]] || cma_staging_die "NyxID handoff mode must be 0600"
  [[ "$(wc -c < "${handoff}")" -le 131072 ]] || cma_staging_die "NyxID handoff is oversized"
  now="$(date +%s)"
  minimum_expiry="$((now + CMA_STAGING_MINIMUM_TOKEN_LIFETIME_SECONDS))"
  maximum_expiry="$((now + CMA_STAGING_MAXIMUM_TOKEN_LIFETIME_SECONDS))"
  jq -e \
    --argjson minimum_expiry "${minimum_expiry}" \
    --argjson maximum_expiry "${maximum_expiry}" '
    keys == [
      "access_token",
      "access_token_expires_at",
      "context_bucket",
      "nyxid_base_url",
      "nyxid_client_id",
      "nyxid_client_secret",
      "public_github_service_slug",
      "schema_version",
      "storage_client_id",
      "storage_client_secret",
      "storage_owner_service_account_id",
      "subject_id",
      "trigger_workspace_bucket"
    ] and
    .schema_version == 1 and
    (.access_token_expires_at | type == "number" and floor == . and
      . >= $minimum_expiry and . <= $maximum_expiry) and
    .nyxid_base_url == "https://nyx-api.chrono-ai.fun" and
    (.nyxid_client_id | test("^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")) and
    (.subject_id | test("^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")) and
    (.storage_client_id | test("^sa_[0-9a-f]{24}$")) and
    (.storage_owner_service_account_id | test("^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$")) and
    (.context_bucket | test("^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$")) and
    (.trigger_workspace_bucket | test("^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$")) and
    .context_bucket != .trigger_workspace_bucket and
    (.public_github_service_slug | test("^[a-z0-9](?:[a-z0-9_-]{0,98}[a-z0-9])?$")) and
    all(.nyxid_client_secret, .storage_client_secret, .access_token;
      type == "string" and length > 0 and length <= 32768 and
      (explode | all(. >= 32 and . != 127)))
  ' "${handoff}" >/dev/null || cma_staging_die "NyxID handoff failed its closed schema"
}

cma_staging_materialize_runtime_credentials() {
  local root="$1"
  local handoff="$2"
  local service_id token
  cma_staging_require_value CMA_TRIGGER_SMOKE_SERVICE_ID
  service_id="${CMA_TRIGGER_SMOKE_SERVICE_ID}"
  [[ "${service_id}" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] || \
    cma_staging_die "CMA_TRIGGER_SMOKE_SERVICE_ID must be the exact NyxID UserService UUID"
  printf '%s' "${service_id}" > "${root}/credentials/github-service-id"
  token="$(cma_staging_handoff_field "${handoff}" access_token)"
  printf '%s' "${token}" > "${root}/credentials/caller-access-token"
  chmod 0600 "${root}/credentials/github-service-id" \
    "${root}/credentials/caller-access-token"
  unset service_id token
}

cma_staging_validate_runtime_credentials() {
  local root="$1"
  local handoff="$2"
  local service_path="${root}/credentials/github-service-id"
  local token_path="${root}/credentials/caller-access-token"
  local service_id token
  [[ -f "${service_path}" && ! -L "${service_path}" && \
     -f "${token_path}" && ! -L "${token_path}" ]] || \
    cma_staging_die "materialized caller credentials are absent or unsafe"
  [[ "$(stat -c '%a' "${service_path}")" == 600 && \
     "$(stat -c '%a' "${token_path}")" == 600 ]] || \
    cma_staging_die "materialized caller credential modes must be 0600"
  service_id="$(<"${service_path}")"
  token="$(<"${token_path}")"
  [[ "${service_id}" =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$ ]] || \
    cma_staging_die "materialized GitHub UserService identity is invalid"
  [[ "${token}" == "$(cma_staging_handoff_field "${handoff}" access_token)" ]] || \
    cma_staging_die "materialized caller token differs from the accepted handoff"
  unset service_id token
}

cma_staging_accept_handoff() {
  local root handoff
  root="$(cma_staging_require_phase validated)"
  cma_staging_require_value CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH
  handoff="${CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH}"
  cma_staging_validate_handoff "${root}" "${handoff}"
  cma_staging_materialize_runtime_credentials "${root}" "${handoff}"
  cma_staging_validate_runtime_credentials "${root}" "${handoff}"
  : > "${root}/phase-handoff-accepted"
  printf '%s\n' "[staging] closed NyxID handoff and exact UserService identity accepted"
}

cma_staging_generate_cma_secrets() {
  local root="$1"
  local handoff="$2"
  local secret_env="${root}/credentials/cma-secret.env"
  local signing_key="${root}/credentials/capability-signing.pem"
  local auth_key signing_pkcs8 key_id mongo_password
  openssl rand 32 | openssl base64 -A > "${root}/credentials/auth-encryption-key"
  openssl genpkey -algorithm Ed25519 -out "${signing_key}" >/dev/null 2>&1
  openssl pkey -in "${signing_key}" -outform DER 2>/dev/null | \
    openssl base64 -A > "${root}/credentials/capability-signing-pkcs8"
  auth_key="$(<"${root}/credentials/auth-encryption-key")"
  signing_pkcs8="$(<"${root}/credentials/capability-signing-pkcs8")"
  mongo_password="$(<"${root}/credentials/mongo-password")"
  key_id="cma-trigger-smoke-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}"
  [[ "$(printf '%s' "${auth_key}" | base64 --decode 2>/dev/null | wc -c)" -eq 32 ]] || \
    cma_staging_die "generated authentication encryption key has the wrong length"
  openssl pkey -inform DER -in <(printf '%s' "${signing_pkcs8}" | base64 --decode) \
    -noout >/dev/null 2>&1 || cma_staging_die "generated capability key is not PKCS#8"
  [[ "${mongo_password}" =~ ^[0-9a-f]{64}$ ]] || \
    cma_staging_die "generated MongoDB password is invalid"
  {
    printf 'CHRONO_NYXID_CLIENT_SECRET=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" nyxid_client_secret)"
    printf 'CHRONO_MONGODB_URI=%s\n' \
      "mongodb://${CMA_STAGING_MONGO_USERNAME}:${mongo_password}@mongo.${CMA_STAGING_SYSTEM_NAMESPACE}.svc.cluster.local:27017/cma_trigger_smoke?replicaSet=rs0&authSource=admin"
    printf 'CHRONO_AUTH_ENCRYPTION_KEY=%s\n' "${auth_key}"
    printf 'CHRONO_STORAGE_CLIENT_ID=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" storage_client_id)"
    printf 'CHRONO_STORAGE_CLIENT_SECRET=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" storage_client_secret)"
    printf 'CHRONO_CONTEXT_BUCKET=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" context_bucket)"
    printf 'CHRONO_TRIGGER_WORKSPACE_BUCKET=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" trigger_workspace_bucket)"
    printf 'CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_ID=%s\n' "${key_id}"
    printf 'CHRONO_GITHUB_CAPABILITY_SIGNING_KEY_PKCS8=%s\n' "${signing_pkcs8}"
  } > "${secret_env}"
  chmod 0600 "${secret_env}" "${signing_key}" \
    "${root}/credentials/auth-encryption-key" \
    "${root}/credentials/capability-signing-pkcs8"
  unset auth_key signing_pkcs8 key_id mongo_password
}

cma_staging_write_cma_config() {
  local root="$1"
  local handoff="$2"
  local trigger_namespace runtime_image config_env
  trigger_namespace="$(<"${root}/trigger-namespace")"
  runtime_image="$(<"${root}/runtime-image")"
  config_env="${root}/generated/cma-config.env"
  {
    printf 'CHRONO_SERVER_BIND=0.0.0.0:8080\n'
    printf 'CHRONO_SERVER_PUBLIC_URL=%s\n' "${CMA_STAGING_PUBLIC_ORIGIN}"
    printf 'CHRONO_SERVER_CORS_ORIGIN=%s\n' "${CMA_STAGING_PUBLIC_ORIGIN}"
    printf 'CHRONO_AUTH_MODE=provider\n'
    printf 'CHRONO_NYXID_BASE_URL=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" nyxid_base_url)"
    printf 'CHRONO_NYXID_CLIENT_ID=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" nyxid_client_id)"
    printf 'CHRONO_MONGODB_DATABASE=cma_trigger_smoke\n'
    printf 'CHRONO_PLATFORM_BASE_URL=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" nyxid_base_url)"
    printf 'CHRONO_UPSTREAM_BASE_URL=%s/api/v1/proxy/s/chrono-llm\n' \
      "$(cma_staging_handoff_field "${handoff}" nyxid_base_url)"
    printf 'CHRONO_UPSTREAM_MODEL=gpt-5.6-sol\n'
    printf 'CHRONO_UPSTREAM_WIRE_API=responses\n'
    printf 'CHRONO_SANDBOX_BASE_URL=%s\n' "${CMA_STAGING_ORDINARY_PROVIDER_ORIGIN}"
    printf 'CHRONO_SANDBOX_IMAGE=%s\n' "${runtime_image}"
    printf 'CHRONO_SANDBOX_CPU=1\n'
    printf 'CHRONO_SANDBOX_MEMORY=1Gi\n'
    printf 'CHRONO_SANDBOX_API_KEY_FILE=/var/secrets/providers/ordinary-provider-api-key\n'
    printf 'CHRONO_DEPLOYMENT_ID=cma-trigger-smoke\n'
    printf 'CHRONO_ADOPT_UNTAGGED_RUNTIMES=false\n'
    printf 'CHRONO_BASELINE_PACKAGES=nyxid\n'
    printf 'CHRONO_BASELINE_SKILLS=nyxid,ornn-agent-manual-cli\n'
    printf 'CHRONO_BASELINE_SERVICES=chrono-llm,ornn-api,ornn\n'
    printf 'CHRONO_GITHUB_CAPABILITY_PREVIOUS_VERIFY_KEYS={}\n'
    printf 'CHRONO_TRIGGER_ENABLED=true\n'
    printf 'CHRONO_TRIGGER_ORCHESTRATION_PAUSED=false\n'
    printf 'CHRONO_TRIGGER_CAPABILITY_KILL_SWITCH=false\n'
    printf 'CHRONO_TRIGGER_USER_COHORT_PERCENT=100\n'
    printf 'CHRONO_TRIGGER_PUBLISHER_SUBJECT_IDS=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" subject_id)"
    printf 'CHRONO_TRIGGER_REPOSITORY_IDS=%s\n' \
      "${CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID}"
    printf 'CHRONO_TRIGGER_PUBLIC_GITHUB_SERVICE_SLUG=%s\n' \
      "$(cma_staging_handoff_field "${handoff}" public_github_service_slug)"
    printf 'CHRONO_TRIGGER_SANDBOX_BASE_URL=%s\n' "${CMA_STAGING_TRIGGER_PROVIDER_ORIGIN}"
    printf 'CHRONO_TRIGGER_SANDBOX_NAMESPACE=%s\n' "${trigger_namespace}"
    printf 'CHRONO_TRIGGER_SANDBOX_API_KEY_FILE=/var/secrets/providers/trigger-provider-api-key\n'
    printf 'CHRONO_TRIGGER_CLEANUP_MODE=dry_run\n'
    printf 'CHRONO_TRIGGER_RUNTIME_CAPABILITIES=%s\n' \
      'conversation,durable_conversation,workspace_checkpoint,restricted_service_gateway,credential_refresh,credential_revocation'
    printf 'RUST_LOG=info\n'
  } > "${config_env}"
  chmod 0600 "${config_env}"
}

cma_staging_write_cma_manifest() {
  local root="$1"
  local api_image
  api_image="$(<"${root}/api-image")"
  cat > "${root}/generated/cma.yaml" <<EOF
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cma-api
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  replicas: 1
  strategy: {type: Recreate}
  selector:
    matchLabels: {app: cma-trigger-smoke-api}
  template:
    metadata:
      labels: {app: cma-trigger-smoke-api}
    spec:
      serviceAccountName: cma-trigger-smoke-cma
      automountServiceAccountToken: true
      terminationGracePeriodSeconds: 20
      securityContext:
        runAsUser: 10001
        runAsGroup: 10001
        runAsNonRoot: true
        fsGroup: 10001
        seccompProfile: {type: RuntimeDefault}
      containers:
        - name: api
          image: ${api_image}
          imagePullPolicy: Never
          envFrom:
            - configMapRef: {name: cma-trigger-smoke-api}
            - secretRef: {name: cma-trigger-smoke-api}
          ports: [{name: http, containerPort: 8080}]
          startupProbe:
            httpGet: {path: /livez, port: http}
            periodSeconds: 5
            failureThreshold: 120
          readinessProbe:
            httpGet: {path: /ready, port: http}
            periodSeconds: 5
          livenessProbe:
            httpGet: {path: /livez, port: http}
            periodSeconds: 10
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            readOnlyRootFilesystem: true
            runAsNonRoot: true
          resources:
            requests: {cpu: 100m, memory: 256Mi}
            limits: {cpu: 2000m, memory: 1Gi}
          volumeMounts:
            - {name: provider-keys, mountPath: /var/secrets/providers, readOnly: true}
            - {name: tmp, mountPath: /tmp}
      volumes:
        - name: provider-keys
          secret: {secretName: cma-trigger-smoke-provider-keys, defaultMode: 288}
        - name: tmp
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: cma-api
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  selector: {app: cma-trigger-smoke-api}
  ports: [{name: http, port: 8080, targetPort: http}]
---
apiVersion: v1
kind: ConfigMap
metadata:
  name: cma-trigger-smoke-nginx
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
data:
  nginx.conf: |
    pid /tmp/nginx.pid;
    events { worker_connections 256; }
    http {
      access_log off;
      error_log /dev/stderr warn;
      client_body_temp_path /tmp/client;
      proxy_temp_path /tmp/proxy;
      server {
        listen 8443 ssl;
        server_name 127.0.0.1;
        ssl_certificate /etc/nginx/tls/tls.crt;
        ssl_certificate_key /etc/nginx/tls/tls.key;
        ssl_protocols TLSv1.2 TLSv1.3;
        location / {
          proxy_pass http://cma-api.${CMA_STAGING_ORDINARY_NAMESPACE}.svc.cluster.local:8080;
          proxy_http_version 1.1;
          proxy_set_header Host \$host;
          proxy_set_header X-Forwarded-Proto https;
          proxy_set_header X-Forwarded-For \$remote_addr;
          proxy_read_timeout 660s;
          proxy_send_timeout 660s;
        }
      }
    }
---
apiVersion: apps/v1
kind: Deployment
metadata:
  name: cma-trigger-smoke-nginx
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  replicas: 1
  selector:
    matchLabels: {app: cma-trigger-smoke-nginx}
  template:
    metadata:
      labels: {app: cma-trigger-smoke-nginx}
    spec:
      automountServiceAccountToken: false
      securityContext: {runAsUser: 101, runAsGroup: 101, fsGroup: 101, runAsNonRoot: true}
      containers:
        - name: nginx
          image: ${CMA_STAGING_NGINX_IMAGE}
          imagePullPolicy: IfNotPresent
          ports: [{name: https, containerPort: 8443}]
          readinessProbe:
            tcpSocket: {port: https}
            periodSeconds: 5
          livenessProbe:
            tcpSocket: {port: https}
            periodSeconds: 10
          securityContext:
            allowPrivilegeEscalation: false
            capabilities: {drop: ["ALL"]}
            readOnlyRootFilesystem: true
            runAsNonRoot: true
          resources:
            requests: {cpu: 10m, memory: 32Mi}
            limits: {cpu: 200m, memory: 128Mi}
          volumeMounts:
            - {name: config, mountPath: /etc/nginx/nginx.conf, subPath: nginx.conf, readOnly: true}
            - {name: tls, mountPath: /etc/nginx/tls, readOnly: true}
            - {name: tmp, mountPath: /tmp}
      volumes:
        - name: config
          configMap: {name: cma-trigger-smoke-nginx}
        - name: tls
          secret: {secretName: cma-trigger-smoke-tls, defaultMode: 288}
        - name: tmp
          emptyDir: {}
---
apiVersion: v1
kind: Service
metadata:
  name: cma-trigger-smoke-nginx
  namespace: ${CMA_STAGING_ORDINARY_NAMESPACE}
spec:
  type: NodePort
  selector: {app: cma-trigger-smoke-nginx}
  ports:
    - name: https
      port: 8443
      targetPort: https
      nodePort: ${CMA_STAGING_NODE_PORT}
EOF
  chmod 0600 "${root}/generated/cma.yaml"
}

cma_staging_deploy() {
  local root handoff tls_dir
  root="$(cma_staging_require_phase validated handoff-accepted prepared)"
  cma_staging_require_commands base64 chmod curl jq kubectl node openssl stat wc
  cma_staging_require_value CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH
  cma_staging_require_value CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID
  [[ "${CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID}" =~ ^[1-9][0-9]*$ ]] || \
    cma_staging_die "upstream repository ID must be a positive decimal identity"
  handoff="${CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_PATH}"
  cma_staging_validate_handoff "${root}" "${handoff}"
  cma_staging_validate_runtime_credentials "${root}" "${handoff}"
  cma_staging_generate_cma_secrets "${root}" "${handoff}"
  cma_staging_write_cma_config "${root}" "${handoff}"
  cma_staging_write_cma_manifest "${root}"
  tls_dir="${root}/credentials/local-tls"
  cma_staging_kubectl -n "${CMA_STAGING_ORDINARY_NAMESPACE}" create configmap \
    cma-trigger-smoke-api --from-env-file="${root}/generated/cma-config.env" >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_ORDINARY_NAMESPACE}" create secret generic \
    cma-trigger-smoke-api --from-env-file="${root}/credentials/cma-secret.env" >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_ORDINARY_NAMESPACE}" create secret generic \
    cma-trigger-smoke-provider-keys \
    --from-file="ordinary-provider-api-key=${root}/credentials/ordinary-provider-key" \
    --from-file="trigger-provider-api-key=${root}/credentials/trigger-provider-key" >/dev/null
  cma_staging_kubectl -n "${CMA_STAGING_ORDINARY_NAMESPACE}" create secret tls \
    cma-trigger-smoke-tls --cert="${tls_dir}/server.crt" --key="${tls_dir}/server.key" \
    >/dev/null
  cma_staging_kubectl apply -f "${root}/generated/cma.yaml" >/dev/null
  cma_staging_wait_rollout "${CMA_STAGING_ORDINARY_NAMESPACE}" cma-api
  cma_staging_wait_rollout "${CMA_STAGING_ORDINARY_NAMESPACE}" cma-trigger-smoke-nginx
  cma_staging_wait_api_attestation "${root}"
  curl --fail --silent --show-error --proto '=https' --tlsv1.2 \
    --cacert "${tls_dir}/ca.crt" "${CMA_STAGING_PUBLIC_ORIGIN}/ready" >/dev/null
  : > "${root}/phase-deployed"
  printf '%s\n' "[staging] CMA is ready on the fixed locally trusted HTTPS origin"
}

cma_staging_assert_cma_authority() {
  local root="$1"
  local trigger_namespace resource identity namespace verb allowed
  trigger_namespace="$(<"${root}/trigger-namespace")"
  resource="batchsandboxes.sandbox.opensandbox.io"
  identity="system:serviceaccount:${CMA_STAGING_ORDINARY_NAMESPACE}:cma-trigger-smoke-cma"
  for namespace in "${CMA_STAGING_ORDINARY_NAMESPACE}" "${trigger_namespace}"; do
    for verb in get list delete; do
      allowed="$(cma_staging_kubectl auth can-i "${verb}" "${resource}" \
        --namespace "${namespace}" --as "${identity}")"
      [[ "${allowed}" == yes ]] || cma_staging_die "CMA lacks ${verb} in ${namespace}"
    done
    for verb in watch create update patch deletecollection; do
      allowed="$(cma_staging_kubectl auth can-i "${verb}" "${resource}" \
        --namespace "${namespace}" --as "${identity}")"
      [[ "${allowed}" == no ]] || cma_staging_die "CMA unexpectedly has ${verb} in ${namespace}"
    done
    for resource in secrets configmaps pods; do
      allowed="$(cma_staging_kubectl auth can-i list "${resource}" \
        --namespace "${namespace}" --as "${identity}")"
      [[ "${allowed}" == no ]] || cma_staging_die "CMA unexpectedly reads ${resource}"
    done
    resource="batchsandboxes.sandbox.opensandbox.io"
  done
  allowed="$(cma_staging_kubectl auth can-i list "${resource}" --all-namespaces \
    --as "${identity}")"
  [[ "${allowed}" == no ]] || cma_staging_die "CMA unexpectedly has cluster-wide provider access"
  allowed="$(cma_staging_kubectl auth can-i get \
    "configmap/${CMA_STAGING_ATTESTATION_CONFIGMAP}" \
    --namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" --as "${CMA_STAGING_OBSERVER_CN}")"
  [[ "${allowed}" == yes ]] || cma_staging_die "smoke observer cannot read the one redacted image attestation"
  allowed="$(cma_staging_kubectl auth can-i list configmaps \
    --namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" --as "${CMA_STAGING_OBSERVER_CN}")"
  [[ "${allowed}" == no ]] || cma_staging_die "smoke observer can enumerate system ConfigMaps"
}

cma_staging_assert_infrastructure_authority() {
  local root="$1"
  local trigger_namespace controller ordinary_provider trigger_provider attestor allowed namespace
  trigger_namespace="$(<"${root}/trigger-namespace")"
  controller="system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-controller"
  ordinary_provider="system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-ordinary"
  trigger_provider="system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-trigger"
  attestor="system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:cma-trigger-image-attestor"

  for namespace in "${CMA_STAGING_ORDINARY_NAMESPACE}" "${trigger_namespace}"; do
    allowed="$(cma_staging_kubectl auth can-i create pods --namespace "${namespace}" --as "${controller}")"
    [[ "${allowed}" == yes ]] || cma_staging_die "OpenSandbox controller lacks Pod creation in ${namespace}"
  done
  for namespace in default "${CMA_STAGING_SYSTEM_NAMESPACE}" kube-system; do
    allowed="$(cma_staging_kubectl auth can-i create pods --namespace "${namespace}" --as "${controller}")"
    [[ "${allowed}" == no ]] || cma_staging_die "OpenSandbox controller can create Pods in ${namespace}"
  done
  allowed="$(cma_staging_kubectl auth can-i list pods --all-namespaces --as "${controller}")"
  [[ "${allowed}" == yes ]] || \
    cma_staging_die "OpenSandbox controller lacks the cluster cache read required by its pinned manager"

  allowed="$(cma_staging_kubectl auth can-i create batchsandboxes.sandbox.opensandbox.io \
    --namespace "${CMA_STAGING_ORDINARY_NAMESPACE}" --as "${ordinary_provider}")"
  [[ "${allowed}" == yes ]] || cma_staging_die "ordinary provider lacks its BatchSandbox lane"
  allowed="$(cma_staging_kubectl auth can-i create batchsandboxes.sandbox.opensandbox.io \
    --namespace "${trigger_namespace}" --as "${ordinary_provider}")"
  [[ "${allowed}" == no ]] || cma_staging_die "ordinary provider crossed into the Trigger lane"
  allowed="$(cma_staging_kubectl auth can-i create batchsandboxes.sandbox.opensandbox.io \
    --namespace "${trigger_namespace}" --as "${trigger_provider}")"
  [[ "${allowed}" == yes ]] || cma_staging_die "Trigger provider lacks its BatchSandbox lane"
  allowed="$(cma_staging_kubectl auth can-i create batchsandboxes.sandbox.opensandbox.io \
    --namespace "${CMA_STAGING_ORDINARY_NAMESPACE}" --as "${trigger_provider}")"
  [[ "${allowed}" == no ]] || cma_staging_die "Trigger provider crossed into the ordinary lane"
  for namespace in "${CMA_STAGING_ORDINARY_NAMESPACE}" "${trigger_namespace}"; do
    for allowed in "${ordinary_provider}" "${trigger_provider}"; do
      [[ "$(cma_staging_kubectl auth can-i create pods --namespace "${namespace}" --as "${allowed}")" == no ]] || \
        cma_staging_die "a provider server can directly create Pods in ${namespace}"
      [[ "$(cma_staging_kubectl auth can-i list secrets --namespace "${namespace}" --as "${allowed}")" == no ]] || \
        cma_staging_die "a provider server can read Secrets in ${namespace}"
    done
  done

  for namespace in "${CMA_STAGING_ORDINARY_NAMESPACE}" "${trigger_namespace}"; do
    [[ "$(cma_staging_kubectl auth can-i list pods --namespace "${namespace}" --as "${attestor}")" == yes ]] || \
      cma_staging_die "image attestor lacks Pod evidence in ${namespace}"
  done
  [[ "$(cma_staging_kubectl auth can-i list pods --namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" --as "${attestor}")" == no ]] || \
    cma_staging_die "image attestor can read system Pods"
  [[ "$(cma_staging_kubectl auth can-i list pods --all-namespaces --as "${attestor}")" == no ]] || \
    cma_staging_die "image attestor has cluster-wide Pod access"
  [[ "$(cma_staging_kubectl auth can-i patch \
      "configmap/${CMA_STAGING_ATTESTATION_CONFIGMAP}" \
      --namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" --as "${attestor}")" == yes ]] || \
    cma_staging_die "image attestor cannot update its one evidence object"
  [[ "$(cma_staging_kubectl auth can-i list configmaps \
      --namespace "${CMA_STAGING_SYSTEM_NAMESPACE}" --as "${attestor}")" == no ]] || \
    cma_staging_die "image attestor can enumerate system ConfigMaps"
}

cma_staging_assert_admission_boundaries() {
  local root="$1"
  local trigger_namespace runtime_image controller provider valid_pod hardened_pod wrong_image
  local valid_batch hostpath_batch deadline
  trigger_namespace="$(<"${root}/trigger-namespace")"
  runtime_image="$(<"${root}/runtime-image")"
  controller="system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-controller"
  provider="system:serviceaccount:${CMA_STAGING_SYSTEM_NAMESPACE}:opensandbox-trigger"
  valid_pod="${root}/generated/admission-valid-pod.json"
  hardened_pod="${root}/generated/admission-hardened-pod.json"
  wrong_image="${root}/generated/admission-wrong-image.json"
  valid_batch="${root}/generated/admission-valid-batchsandbox.json"
  hostpath_batch="${root}/generated/admission-hostpath-batchsandbox.json"

  jq -n --arg namespace "${trigger_namespace}" --arg image "${runtime_image}" \
    --arg execd "${CMA_STAGING_EXECD_IMAGE}" '{
      apiVersion:"v1", kind:"Pod",
      metadata:{name:"cma-trigger-admission-probe",namespace:$namespace,labels:{"opensandbox.io/id":"probe"}},
      spec:{
        restartPolicy:"Never",serviceAccountName:"cma-trigger-sandbox-workload",
        automountServiceAccountToken:false,enableServiceLinks:false,
        securityContext:{runAsUser:10001,runAsGroup:10001,runAsNonRoot:true,fsGroup:10001,seccompProfile:{type:"RuntimeDefault"}},
        containers:[{name:"sandbox",image:$image,command:["/opt/opensandbox/bin/bootstrap.sh","true"],env:[],volumeMounts:[{name:"opensandbox-bin",mountPath:"/opt/opensandbox/bin"}],securityContext:{privileged:true}}],
        initContainers:[{name:"execd-installer",image:$execd,command:["/bin/sh","-c"],args:["true"],volumeMounts:[{name:"opensandbox-bin",mountPath:"/opt/opensandbox/bin"}],securityContext:{privileged:true}}],
        volumes:[{name:"opensandbox-bin",emptyDir:{}}]
      }
    }' > "${valid_pod}"
  deadline=$((SECONDS + 120))
  while (( SECONDS < deadline )); do
    if cma_staging_kubectl --as="${controller}" create --dry-run=server \
        -f "${valid_pod}" -o json > "${hardened_pod}" 2>/dev/null; then
      break
    fi
    sleep 2
  done
  [[ -s "${hardened_pod}" ]] || \
    cma_staging_die "sandbox mutating and validating admission policies never became effective"
  jq -e '
    .spec.containers[0].securityContext == {
      allowPrivilegeEscalation:false,
      capabilities:{drop:["ALL"]},
      privileged:false,
      readOnlyRootFilesystem:false,
      runAsGroup:10001,
      runAsNonRoot:true,
      runAsUser:10001,
      seccompProfile:{type:"RuntimeDefault"}
    } and
    .spec.initContainers[0].securityContext == {
      allowPrivilegeEscalation:false,
      capabilities:{drop:["ALL"]},
      privileged:false,
      readOnlyRootFilesystem:true,
      runAsGroup:10001,
      runAsNonRoot:true,
      runAsUser:10001,
      seccompProfile:{type:"RuntimeDefault"}
    }
  ' "${hardened_pod}" >/dev/null || \
    cma_staging_die "sandbox admission did not replace requested privilege with the closed security context"

  jq '.spec.containers[0].image = "forbidden.example/runtime:latest"' \
    "${valid_pod}" > "${wrong_image}"
  if cma_staging_kubectl --as="${controller}" create --dry-run=server \
      -f "${wrong_image}" >/dev/null 2>&1; then
    cma_staging_die "sandbox Pod admission accepted an unreviewed runtime image"
  fi

  jq -n --arg namespace "${trigger_namespace}" --arg image "${runtime_image}" \
    --arg execd "${CMA_STAGING_EXECD_IMAGE}" '{
      apiVersion:"sandbox.opensandbox.io/v1alpha1",kind:"BatchSandbox",
      metadata:{name:"cma-trigger-batchsandbox-probe",namespace:$namespace},
      spec:{replicas:1,template:{spec:{
        restartPolicy:"Never",serviceAccountName:"cma-trigger-sandbox-workload",
        automountServiceAccountToken:false,enableServiceLinks:false,
        securityContext:{runAsUser:10001,runAsGroup:10001,runAsNonRoot:true,fsGroup:10001,seccompProfile:{type:"RuntimeDefault"}},
        containers:[{name:"sandbox",image:$image,env:[],volumeMounts:[{name:"opensandbox-bin",mountPath:"/opt/opensandbox/bin"}]}],
        initContainers:[{name:"execd-installer",image:$execd,volumeMounts:[{name:"opensandbox-bin",mountPath:"/opt/opensandbox/bin"}]}],
        volumes:[{name:"opensandbox-bin",emptyDir:{}}]
      }}}
    }' > "${valid_batch}"
  cma_staging_kubectl --as="${provider}" create --dry-run=server \
    -f "${valid_batch}" >/dev/null || \
    cma_staging_die "BatchSandbox admission rejected the exact reviewed provider shape"
  jq '.spec.template.spec.volumes[0] = {name:"opensandbox-bin",hostPath:{path:"/"}}' \
    "${valid_batch}" > "${hostpath_batch}"
  if cma_staging_kubectl --as="${provider}" create --dry-run=server \
      -f "${hostpath_batch}" >/dev/null 2>&1; then
    cma_staging_die "BatchSandbox admission accepted a hostPath"
  fi
}

cma_staging_assert_network_boundaries() {
  local root="$1"
  local trigger_namespace expected actual image_id expected_digest
  trigger_namespace="$(<"${root}/trigger-namespace")"
  expected="$(printf '%s\n' \
    "${CMA_STAGING_SYSTEM_NAMESPACE}/default-deny" \
    "${CMA_STAGING_SYSTEM_NAMESPACE}/opensandbox-controller-to-api" \
    "${CMA_STAGING_SYSTEM_NAMESPACE}/opensandbox-provider" \
    "${CMA_STAGING_SYSTEM_NAMESPACE}/mongo" \
    "${CMA_STAGING_SYSTEM_NAMESPACE}/image-attestor-to-api" \
    "${CMA_STAGING_ORDINARY_NAMESPACE}/default-deny" \
    "${CMA_STAGING_ORDINARY_NAMESPACE}/cma-api" \
    "${CMA_STAGING_ORDINARY_NAMESPACE}/cma-ingress" \
    "${CMA_STAGING_ORDINARY_NAMESPACE}/sandbox-runtime" \
    "${trigger_namespace}/default-deny" \
    "${trigger_namespace}/sandbox-runtime" | sort)"
  actual="$(cma_staging_kubectl get networkpolicy --all-namespaces -o json | \
    jq -r '.items[] | .metadata.namespace + "/" + .metadata.name' | \
    grep '^cma-trigger-' | sort)"
  [[ "${actual}" == "${expected}" ]] || \
    cma_staging_die "the staging NetworkPolicy inventory differs from its closed manifest"
  [[ "$(cma_staging_kubectl -n kube-system get daemonset cma-trigger-network-policies \
      -o jsonpath='{.spec.template.spec.containers[0].image}')" == \
      "${CMA_STAGING_NETWORK_POLICY_IMAGE}" ]] || \
    cma_staging_die "network-policy enforcement image drifted"
  image_id="$(cma_staging_kubectl -n kube-system get pods \
    -l app=cma-trigger-network-policies -o json | \
    jq -er '.items | select(length == 1) | .[0].status.containerStatuses | select(length == 1) | .[0].imageID')"
  expected_digest="${CMA_STAGING_NETWORK_POLICY_AMD64_DIGEST}"
  [[ "${image_id}" == *"${expected_digest}" ]] || \
    cma_staging_die "running network-policy enforcer does not match its reviewed image digest"
}

cma_staging_seal() {
  local root node ca_key admin_kubeconfig
  root="$(cma_staging_require_phase validated prepared deployed)"
  cma_staging_require_commands docker find grep jq kubectl shred sleep sort
  cma_staging_assert_cma_authority "${root}"
  cma_staging_assert_infrastructure_authority "${root}"
  cma_staging_assert_admission_boundaries "${root}"
  cma_staging_assert_network_boundaries "${root}"
  node="${CMA_STAGING_CLUSTER}-control-plane"
  ca_key="${root}/bootstrap/pki/ca.key"
  admin_kubeconfig="${root}/bootstrap/admin-kubeconfig"
  [[ -f "${ca_key}" && -f "${admin_kubeconfig}" ]] || \
    cma_staging_die "provisioning authority disappeared before sealing"
  docker exec "${node}" umount /etc/kubernetes/pki/ca.key || \
    cma_staging_die "cluster CA private-key mount could not be detached"
  docker exec "${node}" rm -f -- /etc/kubernetes/pki/ca.key
  shred -u -- "${ca_key}"
  docker exec "${node}" test ! -e /etc/kubernetes/pki/ca.key || \
    cma_staging_die "cluster CA private key remains in the Kind node"
  docker exec "${node}" sh -ceu '
    for credential in /etc/kubernetes/admin.conf /etc/kubernetes/super-admin.conf; do
      if [ -e "$credential" ]; then
        shred -u -- "$credential"
      fi
    done
  '
  docker exec "${node}" sh -ceu '
    test ! -e /etc/kubernetes/admin.conf
    test ! -e /etc/kubernetes/super-admin.conf
  ' || cma_staging_die "an in-node Kubernetes administrator credential remains"
  shred -u -- "${admin_kubeconfig}"
  [[ ! -e "${ca_key}" && ! -e "${admin_kubeconfig}" ]] || \
    cma_staging_die "provisioning authority remains on the host"
  if find "${root}/bootstrap" -type f \
      \( -name '*admin*kubeconfig*' -o -name 'ca.key' \) -print -quit | \
    grep -q .; then
    cma_staging_die "a provisioning credential remains after sealing"
  fi
  kubectl --kubeconfig "$(cma_staging_observer_kubeconfig)" get --raw=/readyz >/dev/null
  : > "${root}/phase-sealed"
  printf '%s\n' "[staging] cluster CA key and every administrator kubeconfig sealed before the smoke"
}

cma_staging_bootstrap_fixture() {
  local root fixture result configuration_revision
  root="$(cma_staging_require_phase validated prepared deployed sealed)"
  cma_staging_require_commands jq node
  fixture="${root}/generated/fixture.json"
  result="${root}/staging-result.json"
  configuration_revision="kind-${GITHUB_RUN_ID}-${GITHUB_RUN_ATTEMPT}-${GITHUB_SHA}"
  env \
    NODE_EXTRA_CA_CERTS="${root}/credentials/local-tls/ca.crt" \
    CMA_TRIGGER_SMOKE_BASE_URL="${CMA_STAGING_PUBLIC_ORIGIN}" \
    CMA_TRIGGER_SMOKE_ACCESS_TOKEN_PATH="${root}/credentials/caller-access-token" \
    CMA_TRIGGER_SMOKE_SERVICE_ID_PATH="${root}/credentials/github-service-id" \
    CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID="${CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID}" \
    CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH="${CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH}" \
    CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN="${CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN}" \
    CMA_TRIGGER_SMOKE_MARKER_INPUT="${CMA_TRIGGER_SMOKE_MARKER_INPUT}" \
    CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE="${CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE}" \
    CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE="${CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE}" \
    CMA_TRIGGER_SMOKE_RUN_MARKER="${CMA_TRIGGER_SMOKE_RUN_MARKER}" \
    node .github/scripts/cma-trigger-kind-staging-fixture.mjs --publish "${fixture}"
  jq -e \
    'keys == ["public_id","revision_digest","schema_version","source_sha"] and
     .schema_version == 1' \
    "${fixture}" >/dev/null || cma_staging_die "fixture bootstrap output schema drifted"
  jq \
    --arg base_url "${CMA_STAGING_PUBLIC_ORIGIN}" \
    --arg build_sha "${GITHUB_SHA}" \
    --arg runtime_image_digest "$(<"${root}/runtime-image-digest")" \
    --arg expected_runtime_version "$(<"${root}/runtime-image")" \
    --arg configuration_revision "${configuration_revision}" \
    --arg tls_ca_path "${root}/credentials/local-tls/ca.crt" \
    --slurpfile runtime_node_image "${root}/runtime-node-image.json" '
      . + {
        base_url:$base_url,
        expected_build_sha:$build_sha,
        runtime_image_digest:$runtime_image_digest,
        runtime_image_ids:$runtime_node_image[0].runtime_image_ids,
        expected_runtime_version:$expected_runtime_version,
        configuration_revision:$configuration_revision,
        tls_ca_path:$tls_ca_path
      }
    ' "${fixture}" > "${result}"
  chmod 0600 "${result}"
  : > "${root}/phase-fixture"
  printf '%s\n' "[staging] ephemeral provider-smoke Trigger fixture is published"
}

cma_staging_export_env() {
  local root result
  root="$(cma_staging_require_phase validated prepared deployed sealed fixture)"
  cma_staging_require_value GITHUB_ENV
  [[ "${GITHUB_ENV}" == /* && -f "${GITHUB_ENV}" && ! -L "${GITHUB_ENV}" ]] || \
    cma_staging_die "GITHUB_ENV must be an existing absolute regular file"
  result="${root}/staging-result.json"
  jq -er '
    .runtime_image_digest as $runtime_image_digest |
    (keys == [
      "base_url", "configuration_revision", "expected_build_sha",
      "expected_runtime_version", "public_id", "revision_digest",
      "runtime_image_digest", "runtime_image_ids", "schema_version",
      "source_sha", "tls_ca_path"
    ] and .schema_version == 1 and
    ($runtime_image_digest | type) == "string" and
    (.runtime_image_ids | type) == "array" and
    (.runtime_image_ids | length) >= 1 and (.runtime_image_ids | length) <= 16 and
    .runtime_image_ids == (.runtime_image_ids | unique) and
    (.runtime_image_ids | index($runtime_image_digest)) != null and
    all(.runtime_image_ids[];
      type == "string" and
      (test("^sha256:[0-9a-f]{64}$") or
       test("^[^[:space:]@]+@sha256:[0-9a-f]{64}$"))))
  ' "${result}" >/dev/null || cma_staging_die "staging result schema drifted"
  {
    printf 'CMA_TRIGGER_SMOKE_BASE_URL=%s\n' "$(jq -er .base_url "${result}")"
    printf 'CMA_TRIGGER_SMOKE_EXPECTED_BUILD_SHA=%s\n' "$(jq -er .expected_build_sha "${result}")"
    printf 'CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_DIGEST=%s\n' "$(jq -er .runtime_image_digest "${result}")"
    printf 'CMA_TRIGGER_SMOKE_RUNTIME_IMAGE_IDS_JSON=%s\n' "$(jq -cer .runtime_image_ids "${result}")"
    printf 'CMA_TRIGGER_SMOKE_EXPECTED_RUNTIME_VERSION=%s\n' "$(jq -er .expected_runtime_version "${result}")"
    printf 'CMA_TRIGGER_SMOKE_CONFIGURATION_REVISION=%s\n' "$(jq -er .configuration_revision "${result}")"
    printf 'CMA_TRIGGER_SMOKE_PUBLIC_ID=%s\n' "$(jq -er .public_id "${result}")"
    printf 'CMA_TRIGGER_SMOKE_REVISION_DIGEST=%s\n' "$(jq -er .revision_digest "${result}")"
    printf 'CMA_TRIGGER_SMOKE_SOURCE_SHA=%s\n' "$(jq -er .source_sha "${result}")"
    printf 'NODE_EXTRA_CA_CERTS=%s\n' "$(jq -er .tls_ca_path "${result}")"
  } >> "${GITHUB_ENV}"
  printf '%s\n' "[staging] verified non-secret deployment provenance exported"
}

cma_staging_destroy() {
  local root status=0
  root="$(cma_staging_require_marker)"
  cma_staging_require_commands docker grep
  if [[ -f "${root}/cluster-create-attempted" ]]; then
    if [[ -x "${root}/bin/kind" ]]; then
      "${root}/bin/kind" delete cluster --name "${CMA_STAGING_CLUSTER}" >/dev/null || status=1
    else
      status=1
    fi
  fi
  if [[ -x "${root}/bin/kind" ]] && \
     "${root}/bin/kind" get clusters 2>/dev/null | grep -Fxq "${CMA_STAGING_CLUSTER}"; then
    status=1
  fi
  if docker ps -a --filter "label=io.x-k8s.kind.cluster=${CMA_STAGING_CLUSTER}" \
    --format '{{.ID}}' | grep -q .; then
    status=1
  fi
  [[ "${status}" -eq 0 ]] || cma_staging_die "Kind cluster destruction could not be proven"
  : > "${root}/phase-destroyed"
  printf '%s\n' "[staging] ephemeral Kind cluster destroyed"
}

cma_staging_remove_observer() {
  local root kubeconfig
  root="$(cma_staging_require_marker)"
  kubeconfig="$(cma_staging_observer_kubeconfig)"
  if [[ -e "${kubeconfig}" ]]; then
    [[ -f "${kubeconfig}" && ! -L "${kubeconfig}" ]] || \
      cma_staging_die "observer kubeconfig path is unsafe"
    shred -u -- "${kubeconfig}"
  fi
  [[ ! -e "${kubeconfig}" ]] || cma_staging_die "observer kubeconfig removal failed"
  : > "${root}/phase-observer-removed"
}

cma_staging_purge() {
  local root receipt settlement_path provider_object_settlement
  root="$(cma_staging_require_marker)"
  cma_staging_require_commands chmod find jq shred wc
  [[ ! -f "${root}/cluster-create-attempted" || -f "${root}/phase-destroyed" ]] || \
    cma_staging_die "refusing to purge custody before cluster destruction"
  [[ -f "${root}/phase-observer-removed" ]] || \
    cma_staging_die "refusing to purge custody before observer removal"
  if find "${root}" -type l -print -quit | grep -q .; then
    cma_staging_die "refusing to follow a symlink while purging staging custody"
  fi
  while IFS= read -r -d '' file; do
    chmod u+w "${file}"
    shred -u -- "${file}"
  done < <(find "${root}" -type f -print0)
  find "${root}" -depth -type d -empty -delete
  [[ ! -e "${root}" ]] || cma_staging_die "staging custody directory is not empty after purge"
  receipt="${RUNNER_TEMP}/cma-trigger-kind-staging-cleanup.json"
  [[ ! -e "${receipt}" ]] || cma_staging_die "cleanup receipt already exists"
  settlement_path="${RUNNER_TEMP}/cma-trigger-provider-object-settlement.json"
  provider_object_settlement='null'
  if [[ -e "${settlement_path}" ]]; then
    if [[ -f "${settlement_path}" && ! -L "${settlement_path}" ]] \
      && [[ "$(wc -c < "${settlement_path}")" -le 12288 ]] \
      && jq -e 'type == "object"' "${settlement_path}" >/dev/null 2>&1; then
      provider_object_settlement="$(jq -c . "${settlement_path}")"
    else
      printf '%s\n' \
        "[staging] provider-object settlement receipt is invalid; recording null" >&2
    fi
  fi
  jq -n \
    --arg cluster "${CMA_STAGING_CLUSTER}" \
    --arg run_id "${GITHUB_RUN_ID}" \
    --arg run_attempt "${GITHUB_RUN_ATTEMPT}" \
    --argjson schema "${CMA_STAGING_CLEANUP_SCHEMA_VERSION}" \
    --argjson provider_object_settlement "${provider_object_settlement}" \
    '{schema_version:$schema,cluster:$cluster,run_id:$run_id,run_attempt:$run_attempt,cluster_destroyed:true,credentials_purged:true,provider_object_settlement:$provider_object_settlement}' \
    > "${receipt}"
  chmod 0600 "${receipt}"
  printf '%s\n' "[staging] local staging credentials and TLS keys purged"
}

cma_staging_verify_cleanup() {
  local root receipt
  root="$(cma_staging_state_root)"
  receipt="${RUNNER_TEMP}/cma-trigger-kind-staging-cleanup.json"
  [[ ! -e "${root}" ]] || cma_staging_die "staging state remains after cleanup"
  jq -e \
    --arg cluster "${CMA_STAGING_CLUSTER}" \
    --arg run_id "${GITHUB_RUN_ID}" \
    --arg run_attempt "${GITHUB_RUN_ATTEMPT}" \
    --argjson schema "${CMA_STAGING_CLEANUP_SCHEMA_VERSION}" '
      keys == [
        "cluster", "cluster_destroyed", "credentials_purged",
        "provider_object_settlement", "run_attempt", "run_id", "schema_version"
      ] and
      .schema_version == $schema and .cluster == $cluster and
      .run_id == $run_id and .run_attempt == $run_attempt and
      .cluster_destroyed == true and .credentials_purged == true and
      (.provider_object_settlement == null or
        (.provider_object_settlement | type) == "object")
    ' "${receipt}" >/dev/null || cma_staging_die "cleanup receipt is absent or invalid"
  printf '%s\n' "[staging] terminal cluster and credential cleanup verified"
}

cma_staging_main() {
  local command="${1:-}"
  [[ "$#" -eq 1 ]] || cma_staging_die "expected exactly one staging command"
  case "${command}" in
    validate) cma_staging_validate ;;
    accept-handoff) cma_staging_accept_handoff ;;
    prepare) cma_staging_prepare ;;
    deploy) cma_staging_deploy ;;
    seal) cma_staging_seal ;;
    bootstrap-fixture) cma_staging_bootstrap_fixture ;;
    export-env) cma_staging_export_env ;;
    destroy) cma_staging_destroy ;;
    remove-observer) cma_staging_remove_observer ;;
    purge) cma_staging_purge ;;
    verify-cleanup) cma_staging_verify_cleanup ;;
    *) cma_staging_die "unknown staging command: ${command}" ;;
  esac
}

if [[ "${BASH_SOURCE[0]}" == "$0" ]]; then
  cma_staging_main "$@"
fi
