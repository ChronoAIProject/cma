#!/usr/bin/env bash

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
SANITIZER="${SCRIPT_DIR}/cma-trigger-gateway-tls-sanitize.rb"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

{
  IFS= read -r cma_tls_secret_encoded
  IFS= read -r cma_tls_private_blob
  IFS= read -r cma_tls_mismatch_blob
  IFS= read -r cma_tls_ec_secret_encoded
  IFS= read -r cma_tls_ec_mismatch_blob
} < <(ruby --disable-gems -rbase64 -rjson -ropenssl -e '
  def pair(algorithm)
    key = if algorithm == :rsa
      OpenSSL::PKey::RSA.new(1024)
    else
      OpenSSL::PKey::EC.generate("prime256v1")
    end
    cert = OpenSSL::X509::Certificate.new
    cert.version = 2
    cert.serial = 1
    cert.subject = OpenSSL::X509::Name.parse("/CN=bot.chrono-ai.fun")
    cert.issuer = cert.subject
    cert.public_key = key
    cert.not_before = Time.utc(2026, 1, 1)
    cert.not_after = Time.utc(2027, 1, 1)
    cert.sign(key, OpenSSL::Digest::SHA256.new)
    [key, cert]
  end
  def secret(key, cert)
    {
    apiVersion:"v1",kind:"Secret",
    metadata:{name:"chrono-ai-fun-cert",namespace:"gateway-infra",uid:"secret-uid",resourceVersion:"1"},
    type:"kubernetes.io/tls",
    data:{"tls.crt"=>Base64.strict_encode64(cert.to_pem),"tls.key"=>Base64.strict_encode64(key.to_pem)}
    }
  end
  key, cert = pair(:rsa)
  mismatch, = pair(:rsa)
  ec_key, ec_cert = pair(:ec)
  ec_mismatch, = pair(:ec)
  puts Base64.strict_encode64(JSON.generate(secret(key, cert)))
  puts Base64.strict_encode64(key.to_pem)
  puts Base64.strict_encode64(mismatch.to_pem)
  puts Base64.strict_encode64(JSON.generate(secret(ec_key, ec_cert)))
  puts Base64.strict_encode64(ec_mismatch.to_pem)
')
[[ -n "${cma_tls_secret_encoded}" && -n "${cma_tls_private_blob}" && \
  -n "${cma_tls_mismatch_blob}" && -n "${cma_tls_ec_secret_encoded}" && \
  -n "${cma_tls_ec_mismatch_blob}" ]] || exit 1
cma_tls_secret="$(printf '%s' "${cma_tls_secret_encoded}" | \
  ruby --disable-gems -rbase64 -e 'print Base64.strict_decode64(STDIN.read)')"

success_dir="${TEST_ROOT}/success"
mkdir "${success_dir}"
printf '%s' "${cma_tls_secret}" | ruby --disable-gems "${SANITIZER}" \
  "${success_dir}/sanitized.json" >"${success_dir}/stdout" 2>"${success_dir}/stderr"
jq -e '
  .dataKeys == ["tls.crt","tls.key"] and .keyPresent == true and
  (.publicSpkiSha256 | test("^[0-9a-f]{64}$")) and
  has("certificatePem") and (has("data") | not) and
  (has("privateKeyPem") | not) and (has("privateKeySha256") | not)
' "${success_dir}/sanitized.json" >/dev/null

failure_dir="${TEST_ROOT}/failure"
mkdir "${failure_dir}"
cma_tls_mismatch_secret="$(printf '%s' "${cma_tls_secret}" | jq -c \
  --arg key "${cma_tls_mismatch_blob}" '.data["tls.key"] = $key')"
if printf '%s' "${cma_tls_mismatch_secret}" | ruby --disable-gems "${SANITIZER}" \
    "${failure_dir}/sanitized.json" >"${failure_dir}/stdout" 2>"${failure_dir}/stderr"; then
  echo "mismatched TLS key unexpectedly passed sanitization" >&2
  exit 1
fi
[[ ! -e "${failure_dir}/sanitized.json" ]] || {
  echo "failed TLS sanitization created a durable output" >&2
  exit 1
}
grep -qx 'cma-trigger Gateway TLS sanitizer: Secret validation failed' \
  "${failure_dir}/stderr"

ec_success_dir="${TEST_ROOT}/ec-success"
mkdir "${ec_success_dir}"
cma_tls_ec_secret="$(printf '%s' "${cma_tls_ec_secret_encoded}" | \
  ruby --disable-gems -rbase64 -e 'print Base64.strict_decode64(STDIN.read)')"
printf '%s' "${cma_tls_ec_secret}" | ruby --disable-gems "${SANITIZER}" \
  "${ec_success_dir}/sanitized.json" \
  >"${ec_success_dir}/stdout" 2>"${ec_success_dir}/stderr"
jq -e '
  .dataKeys == ["tls.crt","tls.key"] and .keyPresent == true and
  (.publicSpkiSha256 | test("^[0-9a-f]{64}$")) and
  (has("data") | not) and (has("privateKeyPem") | not)
' "${ec_success_dir}/sanitized.json" >/dev/null

ec_failure_dir="${TEST_ROOT}/ec-failure"
mkdir "${ec_failure_dir}"
cma_tls_ec_mismatch_secret="$(printf '%s' "${cma_tls_ec_secret}" | jq -c \
  --arg key "${cma_tls_ec_mismatch_blob}" '.data["tls.key"] = $key')"
if printf '%s' "${cma_tls_ec_mismatch_secret}" | ruby --disable-gems "${SANITIZER}" \
    "${ec_failure_dir}/sanitized.json" \
    >"${ec_failure_dir}/stdout" 2>"${ec_failure_dir}/stderr"; then
  echo "mismatched ECDSA TLS key unexpectedly passed sanitization" >&2
  exit 1
fi
[[ ! -e "${ec_failure_dir}/sanitized.json" ]] || {
  echo "failed ECDSA TLS sanitization created a durable output" >&2
  exit 1
}

interrupt_dir="${TEST_ROOT}/interrupt"
mkdir "${interrupt_dir}"
mkfifo "${interrupt_dir}/input"
ruby --disable-gems "${SANITIZER}" "${interrupt_dir}/sanitized.json" \
  <"${interrupt_dir}/input" >"${interrupt_dir}/stdout" 2>"${interrupt_dir}/stderr" &
cma_tls_sanitizer_pid=$!
{
  printf '%s' "{\"data\":{\"tls.key\":\"${cma_tls_private_blob}\""
  sleep 2
} >"${interrupt_dir}/input" &
cma_tls_writer_pid=$!
for _ in 1 2 3 4 5; do
  kill -0 "${cma_tls_sanitizer_pid}" 2>/dev/null || break
  sleep 0.05
done
kill -KILL "${cma_tls_sanitizer_pid}" 2>/dev/null || :
wait "${cma_tls_sanitizer_pid}" 2>/dev/null || :
kill "${cma_tls_writer_pid}" 2>/dev/null || :
wait "${cma_tls_writer_pid}" 2>/dev/null || :
[[ ! -e "${interrupt_dir}/sanitized.json" ]] || {
  echo "interrupted TLS sanitization created a durable output" >&2
  exit 1
}

if find "${TEST_ROOT}" -type f -exec grep -F -l -- "${cma_tls_private_blob}" {} + | grep -q .; then
  echo "TLS private-key bytes reached a created file or log" >&2
  exit 1
fi
if find "${TEST_ROOT}" -type f -exec grep -F -l -- "${cma_tls_mismatch_blob}" {} + | grep -q .; then
  echo "mismatched TLS private-key bytes reached a created file or log" >&2
  exit 1
fi
if find "${TEST_ROOT}" -type f -exec grep -F -l -- "${cma_tls_ec_mismatch_blob}" {} + | grep -q .; then
  echo "mismatched ECDSA private-key bytes reached a created file or log" >&2
  exit 1
fi

printf '%s\n' "CMA Trigger Gateway TLS sanitizer tests passed (5 cases)"
