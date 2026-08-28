#!/usr/bin/env bash
# Mock overrides are invoked indirectly, and each case intentionally isolates its log in a subshell.
# shellcheck disable=SC2030,SC2031,SC2329

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=.github/scripts/cma-trigger-gateway-restore-traffic.sh
source "${SCRIPT_DIR}/cma-trigger-gateway-restore-traffic.sh"

cma_test_tmp="$(mktemp -d)"
trap 'rm -rf "${cma_test_tmp}"' EXIT

cma_test_configure_mocks() {
  cma_test_selector=fenced
  cma_test_route_checks=0
  cma_test_reopen_checks=0

  cma_test_assert_no_secret_exports() {
    if env | grep -Eq \
      '^(CHRONO_RELEASE_ACCESS_TOKEN|CURL_CA_BUNDLE|CURL_SSL_BACKEND|SSL_CERT_DIR|SSL_CERT_FILE|SSLKEYLOGFILE|cma_release_access_token|cma_direct_livez|cma_direct_ready|cma_app_openapi|cma_direct_sessions|cma_app_sessions)='; then
      printf '%s\n' credential-leak >>"${CMA_TEST_LOG}"
      return 1
    fi
  }

  cma_step10_require_checkout() {
    cma_test_assert_no_secret_exports
  }
  cma_step10_require_privileged_bash() { return 0; }
  cma_step10_require_trusted_launcher() { return 0; }
  cma_step10_validate_snapshot_seal() { return 0; }
  cma_final_pod_topology() {
    printf '%s\n' '{"nodeName":"node-a","providerZone":"asia-northeast1-a","providerInstance":"instance-a"}'
  }
  cma_step10_require_production_mutation_enabled() { return 0; }
  cma_step10_load_fence_receipt() {
    cma_pinned_api_root_identity='{"uid":"service-uid","name":"neg-name","zones":["asia-northeast1-a"]}'
    return 0
  }
  cma_step10_validate_admission_reservation() { return 0; }
  cma_step10_projection_capture() {
    printf '%s\n' /test/projection
  }
  cma_step10_validate_fence_projection() { return 0; }
  cma_step10_validate_source_projection() { return 0; }
  cma_step10_projection_check_live() { return 0; }
  cma_step10_gcloud() { return 1; }

  cma_step10_kubectl() {
    cma_test_assert_no_secret_exports || return 1
    if [ "$1" = get ] && [ "$2" = pod ]; then
      if [ "${CMA_TEST_CASE}" = pod-failure ]; then
        return 1
      fi
      printf '%s\n' \
        '{"metadata":{"uid":"pod-uid"},"status":{"podIP":"10.0.0.8","containerStatuses":[{"name":"api","started":true,"restartCount":0}],"conditions":[{"type":"Ready","status":"True"}]}}'
      return 0
    fi
    if [ "$1" = patch ] && [ "$2" = service ]; then
      cma_test_selector=source
      if [ "${CMA_TEST_CASE}" = patch-failure-source ]; then
        return 1
      fi
      return 0
    fi
    return 1
  }

  cma_read_selector_state() {
    case "${CMA_TEST_CASE}" in
      initial-source) printf '%s\n' source; return 0 ;;
      initial-unknown) printf '%s\n' unknown; return 0 ;;
      initial-read-failure) return 1 ;;
      *) printf '%s\n' "${cma_test_selector}"; return 0 ;;
    esac
  }

  cma_wait_for_fenced_endpoints() {
    case "${CMA_TEST_CASE}" in
      initial-fence-failure) return 1 ;;
      initial-signal)
        sh -c 'kill -TERM "$PPID"'
        return 1
        ;;
      initial-quit)
        sh -c 'kill -QUIT "$PPID"'
        return 1
        ;;
      *) return 0 ;;
    esac
  }

  cma_route_and_policies_ready() {
    cma_test_route_checks=$((cma_test_route_checks + 1))
    case "${CMA_TEST_CASE}:${cma_test_route_checks}" in
      initial-route-failure:1|post-route-failure:2) return 1 ;;
      *) return 0 ;;
    esac
  }

  cma_wait_for_reopened_endpoint() {
    cma_test_reopen_checks=$((cma_test_reopen_checks + 1))
    case "${CMA_TEST_CASE}:${cma_test_reopen_checks}" in
      reopen-proof-failure:1|post-endpoint-failure:2) return 1 ;;
      *) return 0 ;;
    esac
  }

  cma_refence_api() {
    if { : <&9; } 2>/dev/null; then
      printf '%s\n' credential-fd-leak >>"${CMA_TEST_LOG}"
      return 1
    fi
    printf '%s\n' refence >>"${CMA_TEST_LOG}"
    cma_test_selector=fenced
    return 0
  }

  curl() {
    local cma_test_header
    if [ "$(ulimit -c)" != 0 ]; then
      printf '%s\n' credential-core-limit-leak >>"${CMA_TEST_LOG}"
      return 1
    fi
    if [ -n "${SSLKEYLOGFILE:-}" ]; then
      printf '%s\n' key-material >"${SSLKEYLOGFILE}"
      printf '%s\n' credential-keylog-leak >>"${CMA_TEST_LOG}"
      return 1
    fi
    cma_test_assert_no_secret_exports || return 1
    case "$*" in
      */api/v1/sessions*)
        if [ "${CMA_TEST_CASE}" = auth-signal ]; then
          sh -c 'kill -TERM "$PPID"'
          return 1
        fi
        if [ "${CMA_TEST_CASE}" = auth-quit ]; then
          sh -c 'kill -QUIT "$PPID"'
          return 1
        fi
        if ! IFS= read -r cma_test_header <&9 || \
           [ "${cma_test_header}" != "Authorization: Bearer test-release-token" ]; then
          printf '%s\n' credential-fd-invalid >>"${CMA_TEST_LOG}"
          return 1
        fi
        printf '%s\n' auth-private >>"${CMA_TEST_LOG}"
        printf '%s' '[]'
        ;;
      *bot-api.chrono-ai.fun/livez*) printf '%s' 'ok|200' ;;
      *bot-api.chrono-ai.fun/ready*)
        printf '%s' '{"status":"ready"}|200'
        if [ "${CMA_TEST_CASE}" = truncated-ready ]; then
          return 18
        fi
        ;;
      *bot.chrono-ai.fun/openapi.json*)
        printf '%s' '{"openapi":"3.1.0","paths":{"/api/v1/sessions":{"get":{}}}}'
        ;;
      *) return 1 ;;
    esac
    if [ "${CMA_TEST_CASE}" = truncated-livez ] && \
       [[ "$*" == *bot-api.chrono-ai.fun/livez* ]]; then
      return 18
    fi
  }
}

cma_test_run_case() {
  local case_name="$1"
  local expected_status="$2"
  local expected_refences="$3"
  local expected_auth_calls="$4"
  local case_log="${cma_test_tmp}/${case_name}.log"
  local case_output="${cma_test_tmp}/${case_name}.out"
  local case_error="${cma_test_tmp}/${case_name}.err"
  local actual_status actual_refences actual_auth_calls

  : >"${case_log}"
  set +e
  (
    set -euo pipefail
    export CMA_TEST_CASE="${case_name}"
    export CMA_TEST_LOG="${case_log}"
    export CMA_FINAL_POD=api-pod
    export CMA_FINAL_POD_UID=pod-uid
    export CMA_FINAL_POD_IP=10.0.0.8
    export CMA_GATEWAY_TOKEN_FD=8
    exec 8<<<"test-release-token"
    if [ "${case_name}" = token-crlf ]; then
      exec 8<<<$'invalid\r\nInjected: yes'
    fi
    export CURL_CA_BUNDLE="${cma_test_tmp}/untrusted-ca.pem"
    export CURL_SSL_BACKEND=poisoned-backend
    export SSL_CERT_DIR="${cma_test_tmp}/untrusted-ca-dir"
    export SSL_CERT_FILE="${cma_test_tmp}/untrusted-ca-file.pem"
    export SSLKEYLOGFILE="${cma_test_tmp}/${case_name}.tls-keys"
    export cma_release_access_token=stale-exported-token
    export cma_app_openapi=stale-exported-openapi
    export cma_direct_sessions=stale-exported-direct-response
    export cma_app_sessions=stale-exported-app-response
    export CMA_GATEWAY_CONFIRM=restore-v0.10-traffic
    export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_ROLLBACK_DIR=/test/rollback
    export CMA_GATEWAY_ROLLBACK_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    ulimit -c 1024 >/dev/null 2>&1 || :
    cma_test_configure_mocks
    cma_step10_main
  ) >"${case_output}" 2>"${case_error}"
  actual_status=$?
  set -e

  actual_refences="$(awk '$0 == "refence" { count += 1 } END { print count + 0 }' "${case_log}")"
  actual_auth_calls="$(awk '$0 == "auth-private" { count += 1 } END { print count + 0 }' "${case_log}")"
  if [ "${actual_status}" -ne "${expected_status}" ]; then
    echo "${case_name}: expected status ${expected_status}, got ${actual_status}" >&2
    sed -n '1,120p' "${case_error}" >&2
    return 1
  fi
  if [ "${actual_refences}" -ne "${expected_refences}" ]; then
    echo "${case_name}: expected ${expected_refences} re-fence calls, got ${actual_refences}" >&2
    sed -n '1,120p' "${case_error}" >&2
    return 1
  fi
  if [ "${actual_auth_calls}" -ne "${expected_auth_calls}" ]; then
    echo "${case_name}: expected ${expected_auth_calls} private auth calls, got ${actual_auth_calls}" >&2
    sed -n '1,120p' "${case_error}" >&2
    return 1
  fi
  if grep -Eq '^credential-(core-limit-leak|leak|fd-leak|fd-invalid|keylog-leak)$' "${case_log}" || \
     [ -e "${cma_test_tmp}/${case_name}.tls-keys" ]; then
    echo "${case_name}: release credential escaped its private command-scoped channel" >&2
    return 1
  fi
}

cma_test_exit_handler_signal_case() {
  local case_log="${cma_test_tmp}/exit-handler-signal.log"
  local actual_status actual_refences

  : >"${case_log}"
  set +e
  (
    set -euo pipefail
    export CMA_TEST_LOG="${case_log}"
    cma_traffic_open=1
    cma_refence_api() {
      if { : <&9; } 2>/dev/null; then
        printf '%s\n' credential-fd-leak >>"${CMA_TEST_LOG}"
        return 1
      fi
      sh -c 'kill -TERM "$PPID"'
      printf '%s\n' refence >>"${CMA_TEST_LOG}"
    }
    exec 9<<<"Authorization: Bearer sentinel"
    cma_refence_on_exit 23
  ) >/dev/null 2>"${cma_test_tmp}/exit-handler-signal.err"
  actual_status=$?
  set -e

  actual_refences="$(awk '$0 == "refence" { count += 1 } END { print count + 0 }' "${case_log}")"
  if [ "${actual_status}" -ne 23 ] || [ "${actual_refences}" -ne 1 ] || \
     grep -q '^credential-fd-leak$' "${case_log}"; then
    echo "exit-handler-signal: status, repeated-signal mask, or FD closure failed" >&2
    sed -n '1,120p' "${cma_test_tmp}/exit-handler-signal.err" >&2
    return 1
  fi
}

cma_test_exit_handler_zero_case() {
  local case_log="${cma_test_tmp}/exit-handler-zero.log"
  local actual_status actual_refences

  : >"${case_log}"
  set +e
  (
    set -euo pipefail
    cma_traffic_open=1
    cma_refence_api() {
      printf '%s\n' refence >>"${case_log}"
      return 0
    }
    cma_refence_on_exit 0
  ) >/dev/null 2>"${cma_test_tmp}/exit-handler-zero.err"
  actual_status=$?
  set -e

  actual_refences="$(awk '$0 == "refence" { count += 1 } END { print count + 0 }' "${case_log}")"
  if [ "${actual_status}" -eq 0 ] || [ "${actual_refences}" -ne 1 ]; then
    echo "exit-handler-zero: armed re-fence handler reported false success" >&2
    return 1
  fi
}

cma_test_production_gate_case() {
  local invocation case_log case_error actual_status

  for invocation in direct conditional or_list; do
    case_log="${cma_test_tmp}/production-gate-${invocation}.log"
    case_error="${cma_test_tmp}/production-gate-${invocation}.err"
    : >"${case_log}"
    set +e
    (
      set +e
      export CMA_FINAL_POD=api-pod
      export CMA_FINAL_POD_UID=pod-uid
      export CMA_FINAL_POD_IP=10.0.0.8
      export CHRONO_RELEASE_ACCESS_TOKEN=disabled-gate-sentinel
      export CMA_GATEWAY_CONFIRM=restore-v0.10-traffic
      export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
      export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      export CMA_GATEWAY_ROLLBACK_DIR=/test/rollback
      export CMA_GATEWAY_ROLLBACK_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
      cma_step10_require_checkout() { return 0; }
      cma_step10_validate_snapshot_seal() { return 0; }
      cma_final_pod_is_exact() { return 0; }
      cma_read_selector_state() { printf '%s\n' fenced; }
      cma_wait_for_fenced_endpoints() { return 0; }
      cma_route_and_policies_ready() { return 0; }
      cma_step10_kubectl() {
        printf '%s\n' kubectl >>"${case_log}"
        return 97
      }
      cma_step10_gcloud() {
        printf '%s\n' gcloud >>"${case_log}"
        return 97
      }
      curl() {
        printf '%s\n' curl >>"${case_log}"
        return 97
      }
      cma_refence_api() {
        printf '%s\n' refence >>"${case_log}"
        return 1
      }
      case "${invocation}" in
        direct)
          cma_step10_main
          exit $?
          ;;
        conditional)
          if cma_step10_main; then
            exit 0
          else
            exit $?
          fi
          ;;
        or_list)
          cma_gate_status=0
          cma_step10_main || cma_gate_status=$?
          exit "${cma_gate_status}"
          ;;
      esac
    ) >/dev/null 2>"${case_error}"
    actual_status=$?
    set -e

    if [ "${actual_status}" -eq 0 ] || [ -s "${case_log}" ] || \
       ! grep -q 'production traffic restoration is disabled' "${case_error}"; then
      echo "production-gate-${invocation}: disabled restoration reached an external boundary" >&2
      sed -n '1,120p' "${case_error}" >&2
      return 1
    fi
  done
}

cma_test_fence_production_gate_case() {
  local invocation case_log case_error actual_status

  for invocation in direct conditional or_list; do
    case_log="${cma_test_tmp}/fence-production-gate-${invocation}.log"
    case_error="${cma_test_tmp}/fence-production-gate-${invocation}.err"
    : >"${case_log}"
    set +e
    (
      set +e
      export CMA_GATEWAY_CONFIRM=fence-v0.10-traffic
      export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
      export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
      cma_step10_require_privileged_bash() {
        printf '%s\n' privileged >>"${case_log}"
        return 97
      }
      cma_step10_require_checkout() {
        printf '%s\n' checkout >>"${case_log}"
        return 97
      }
      cma_step10_kubectl() {
        printf '%s\n' kubectl >>"${case_log}"
        return 97
      }
      cma_step10_gcloud() {
        printf '%s\n' gcloud >>"${case_log}"
        return 97
      }
      cma_wait_for_fenced_endpoints() {
        printf '%s\n' drain >>"${case_log}"
        return 97
      }
      case "${invocation}" in
        direct)
          cma_step10_fence_main
          exit $?
          ;;
        conditional)
          if cma_step10_fence_main; then
            exit 0
          else
            exit $?
          fi
          ;;
        or_list)
          cma_gate_status=0
          cma_step10_fence_main || cma_gate_status=$?
          exit "${cma_gate_status}"
          ;;
      esac
    ) >/dev/null 2>"${case_error}"
    actual_status=$?
    set -e

    if [ "${actual_status}" -eq 0 ] || [ -s "${case_log}" ] || \
       ! grep -q 'production traffic restoration is disabled' "${case_error}"; then
      echo "fence-production-gate-${invocation}: disabled fence reached an external boundary" >&2
      sed -n '1,120p' "${case_error}" >&2
      return 1
    fi
  done
}

cma_test_fence_extra_args_case() {
  local case_log="${cma_test_tmp}/fence-extra-args.log"
  local case_error="${cma_test_tmp}/fence-extra-args.err"
  local actual_status

  : >"${case_log}"
  set +e
  (
    cma_step10_require_production_mutation_enabled() {
      printf '%s\n' gate >>"${case_log}"
      return 97
    }
    cma_step10_fence_main unexpected
  ) >/dev/null 2>"${case_error}"
  actual_status=$?
  set -e
  if [ "${actual_status}" -ne 2 ] || [ -s "${case_log}" ] || \
     ! grep -q 'usage: .* --fence' "${case_error}"; then
    echo "fence-extra-args: usage validation did not precede every boundary" >&2
    sed -n '1,120p' "${case_error}" >&2
    return 1
  fi
}

cma_test_privileged_startup_is_closed() {
  local case_dir="${cma_test_tmp}/privileged-startup"
  local startup_file="${case_dir}/bash-env"
  local startup_marker="${case_dir}/bash-env-ran"
  local function_marker="${case_dir}/imported-function-ran"
  local actual_status

  mkdir "${case_dir}"
  # shellcheck disable=SC2016 # Write literal code for the hostile startup file.
  printf '%s\n' \
    'printf "%s\n" "${CHRONO_RELEASE_ACCESS_TOKEN:-missing}" > "${CMA_STARTUP_MARKER}"' \
    >"${startup_file}"
  set +e
  # shellcheck disable=SC2016 # Pass a literal exported-function definition.
  env \
    BASH_ENV="${startup_file}" \
    ENV="${startup_file}" \
    CHRONO_RELEASE_ACCESS_TOKEN=privileged-startup-bearer-sentinel \
    CMA_STARTUP_MARKER="${startup_marker}" \
    CMA_FUNCTION_MARKER="${function_marker}" \
    'BASH_FUNC_ulimit%%=() { : > "${CMA_FUNCTION_MARKER}"; builtin ulimit "$@"; }' \
    "${SCRIPT_DIR}/cma-trigger-gateway-restore-traffic.sh" \
      >"${case_dir}/stdout" 2>"${case_dir}/stderr"
  actual_status=$?
  set -e

  if [ "${actual_status}" -eq 0 ] || [ -e "${startup_marker}" ] || \
     [ -e "${function_marker}" ] || \
     grep -R -q 'privileged-startup-bearer-sentinel' "${case_dir}" || \
     ! grep -q 'production traffic restoration is disabled' "${case_dir}/stderr"; then
    echo "privileged-startup: Bash startup input or imported function executed" >&2
    return 1
  fi
}

cma_test_curl_environment_is_closed() {
  local case_dir="${cma_test_tmp}/curl-environment"
  local trace_file="${case_dir}/curl.trace"
  local port_file="${case_dir}/port"
  local output_file="${case_dir}/stdout"
  local error_file="${case_dir}/stderr"
  local payload_file="${case_dir}/payload"
  local server_pid server_port cma_curl_status

  mkdir -p "${case_dir}/home"
  printf '%s\n' \
    "--trace-ascii \"${trace_file}\"" \
    "--proxy http://127.0.0.1:1" \
    >"${case_dir}/home/.curlrc"
  printf '%s' curl-environment-closed >"${payload_file}"

  HOME="${case_dir}/home" CURL_HOME="${case_dir}/home" \
    command curl --silent "file://${payload_file}" >/dev/null 2>&1 || :
  if [ ! -s "${trace_file}" ]; then
    echo "curl-environment: the hostile curlrc negative control was not loaded" >&2
    return 1
  fi
  rm -f "${trace_file}"

  ruby -rsocket -rtimeout -e '
    server = TCPServer.new("127.0.0.1", 0)
    File.write(ARGV.fetch(0), server.addr.fetch(1).to_s)
    Timeout.timeout(5) do
      socket = server.accept
      request = +""
      request << socket.readpartial(1024) until request.include?("\r\n\r\n")
      body = "curl-environment-closed"
      socket.write("HTTP/1.1 200 OK\r\nContent-Length: #{body.bytesize}\r\nConnection: close\r\n\r\n#{body}")
      socket.close
    end
    server.close
  ' "${port_file}" >"${case_dir}/server.out" 2>"${case_dir}/server.err" &
  server_pid=$!
  server_port=""
  for _ in 1 2 3 4 5 6 7 8 9 10; do
    if [ -s "${port_file}" ]; then
      server_port="$(sed -n '1p' "${port_file}")"
      break
    fi
    sleep 0.1
  done
  if [ -z "${server_port}" ]; then
    kill "${server_pid}" >/dev/null 2>&1 || :
    wait "${server_pid}" >/dev/null 2>&1 || :
    echo "curl-environment: local test server did not start" >&2
    return 1
  fi

  set +e
  (
    export HOME="${case_dir}/home" CURL_HOME="${case_dir}/home"
    export HTTP_PROXY=http://127.0.0.1:1 HTTPS_PROXY=http://127.0.0.1:1
    export ALL_PROXY=http://127.0.0.1:1 NO_PROXY='' no_proxy=''
    cma_release_access_token=curl-environment-bearer-sentinel
    export -n cma_release_access_token
    cma_step10_authenticated_curl \
      "http://127.0.0.1:${server_port}/sessions"
  ) >"${output_file}" 2>"${error_file}"
  cma_curl_status=$?
  set -e
  if [ "${cma_curl_status}" -ne 0 ]; then
    kill "${server_pid}" >/dev/null 2>&1 || :
    wait "${server_pid}" >/dev/null 2>&1 || :
  elif ! wait "${server_pid}"; then
    echo "curl-environment: local test server failed" >&2
    return 1
  fi

  if [ "${cma_curl_status}" -ne 0 ] || \
     [ "$(cat "${output_file}")" != curl-environment-closed ] || \
     [ -s "${error_file}" ] || [ -e "${trace_file}" ] || \
     grep -R -q 'curl-environment-bearer-sentinel' "${case_dir}"; then
    echo "curl-environment: curlrc, proxy, or credential isolation failed" >&2
    return 1
  fi
}

cma_test_gcloud_environment_is_closed() {
  local case_dir="${cma_test_tmp}/gcloud-environment"
  local gcloud_path observed_environment

  mkdir -p "${case_dir}/bin" "${case_dir}/home"
  printf '%s\n' '#!/bin/sh' 'env' >"${case_dir}/bin/gcloud"
  chmod 700 "${case_dir}/bin/gcloud"
  gcloud_path="${case_dir}/bin:${PATH}"

  if ! observed_environment="$(
    HOME="${case_dir}/home" \
    PATH="${gcloud_path}" \
    CLOUDSDK_CONFIG="${case_dir}/hostile-config" \
    CLOUDSDK_ACTIVE_CONFIG_NAME=hostile \
    CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE=http://127.0.0.1:1 \
    CLOUDSDK_AUTH_CREDENTIAL_FILE_OVERRIDE="${case_dir}/hostile-auth.json" \
    CLOUDSDK_CORE_CUSTOM_CA_CERTS_FILE="${case_dir}/hostile-ca.pem" \
    CLOUDSDK_CORE_HTTP_TIMEOUT=999 \
    CLOUDSDK_PYTHON="${case_dir}/hostile-python-bin" \
    GOOGLE_APPLICATION_CREDENTIALS="${case_dir}/hostile-adc.json" \
    GOOGLE_API_USE_MTLS_ENDPOINT=always \
    GOOGLE_API_USE_CLIENT_CERTIFICATE=true \
    GOOGLE_AUTH_TOKEN_ENDPOINT=http://127.0.0.1:1 \
    GCE_METADATA_HOST=127.0.0.1 \
    GRPC_DEFAULT_SSL_ROOTS_FILE_PATH="${case_dir}/hostile-grpc-ca.pem" \
    CURL_CA_BUNDLE="${case_dir}/hostile-curl-ca.pem" \
    REQUESTS_CA_BUNDLE="${case_dir}/hostile-requests-ca.pem" \
    SSL_CERT_DIR="${case_dir}/hostile-ssl-ca-dir" \
    SSL_CERT_FILE="${case_dir}/hostile-ssl-ca.pem" \
    PYTHONPATH="${case_dir}/hostile-python" \
    HTTP_PROXY=http://127.0.0.1:1 \
    HTTPS_PROXY=http://127.0.0.1:1 \
    ALL_PROXY=http://127.0.0.1:1 \
    NO_PROXY='' \
    http_proxy=http://127.0.0.1:1 \
    https_proxy=http://127.0.0.1:1 \
    all_proxy=http://127.0.0.1:1 \
    no_proxy='' \
      cma_step10_gcloud compute network-endpoint-groups \
        list-network-endpoints fixture
  )"; then
    echo "gcloud-environment: isolated gcloud child could not execute" >&2
    return 1
  fi
  if ! grep -qx "HOME=${case_dir}/home" <<<"${observed_environment}" || \
     ! grep -qx "PATH=${gcloud_path}" <<<"${observed_environment}" || \
     ! grep -qx 'LC_ALL=C' <<<"${observed_environment}" || \
     ! grep -qx 'CLOUDSDK_CORE_HTTP_TIMEOUT=15' <<<"${observed_environment}" || \
     [ "$(grep -Ec '^CLOUDSDK_' <<<"${observed_environment}")" -ne 1 ] || \
     grep -Eq '^(GOOGLE_.*|GCE_.*|GRPC_.*|CURL_.*|REQUESTS_CA_BUNDLE|SSL_CERT_.*|PYTHON.*|HTTP_PROXY|HTTPS_PROXY|ALL_PROXY|NO_PROXY|http_proxy|https_proxy|all_proxy|no_proxy)=' \
       <<<"${observed_environment}"; then
    echo "gcloud-environment: endpoint, auth, trust, proxy, or SDK override leaked" >&2
    return 1
  fi
}

cma_test_source_service() {
  local service_uid="$1"
  local service_resource_version="$2"
  local neg_status last_applied

  neg_status="$(jq -cn '{
    network_endpoint_groups:{"80":"k8s1-002d2683-chrono-bot-cma-api-80-7201e85f"},
    zones:["asia-northeast1-a","asia-northeast1-b"]
  }')"
  last_applied="$(jq -cn '{
    apiVersion:"v1",kind:"Service",
    metadata:{
      annotations:{},
      labels:{
        "app.kubernetes.io/name":"cma-api",
        "app.kubernetes.io/part-of":"cma"
      },
      name:"cma-api",namespace:"chrono-bot"
    },
    spec:{
      ports:[{name:"http",port:80,protocol:"TCP",targetPort:"http"}],
      selector:{"app.kubernetes.io/name":"cma-api"},
      type:"ClusterIP"
    }
  }')"
  jq -cn --arg uid "${service_uid}" --arg rv "${service_resource_version}" \
    --arg neg_status "${neg_status}" --arg last_applied "${last_applied}" '{
      apiVersion:"v1",kind:"Service",
      metadata:{
        annotations:{
          "cloud.google.com/neg":"{\"exposed_ports\":{\"80\":{}}}",
          "cloud.google.com/neg-status":$neg_status,
          "kubectl.kubernetes.io/last-applied-configuration":$last_applied
        },
        creationTimestamp:"2026-01-01T00:00:00Z",
        labels:{
          "app.kubernetes.io/name":"cma-api",
          "app.kubernetes.io/part-of":"cma"
        },
        name:"cma-api",namespace:"chrono-bot",resourceVersion:$rv,uid:$uid
      },
      spec:{
        clusterIP:"10.96.0.8",clusterIPs:["10.96.0.8"],
        internalTrafficPolicy:"Cluster",ipFamilies:["IPv4"],
        ipFamilyPolicy:"SingleStack",
        ports:[{name:"http",port:80,protocol:"TCP",targetPort:"http"}],
        selector:{"app.kubernetes.io/name":"cma-api"},
        sessionAffinity:"None",type:"ClusterIP"
      },
      status:{loadBalancer:{}}
    }'
}

cma_test_fenced_service() {
  cma_test_source_service "$1" "$2" | jq -c '
    .spec.selector = {"release.chrono-ai.fun/cutover-fence":"mongodb-715"}
  '
}

cma_test_endpoint_slices() {
  local endpoint_state="$1"
  local service_uid="$2"
  local slice_uid="$3"
  local slice_resource_version="$4"
  local endpoints='[]'

  if [ "${endpoint_state}" != pending ]; then
    endpoints='[{
      "addresses":["10.0.0.8"],
      "conditions":{"ready":true,"serving":true,"terminating":false},
      "nodeName":"node-a","zone":"asia-northeast1-a",
      "targetRef":{
        "kind":"Pod","name":"api-pod","namespace":"chrono-bot","uid":"pod-uid"
      }
    }]'
    case "${endpoint_state}" in
      serving_false)
        endpoints="$(printf '%s' "${endpoints}" | jq -c '.[0].conditions.serving = false')"
        ;;
      terminating_true)
        endpoints="$(printf '%s' "${endpoints}" | jq -c '.[0].conditions.terminating = true')"
        ;;
      node_drift)
        endpoints="$(printf '%s' "${endpoints}" | jq -c '.[0].nodeName = "node-b"')"
        ;;
      zone_drift)
        endpoints="$(printf '%s' "${endpoints}" | jq -c '.[0].zone = "asia-northeast1-b"')"
        ;;
    esac
  fi
  jq -cn --arg service_uid "${service_uid}" --arg slice_uid "${slice_uid}" \
    --arg rv "${slice_resource_version}" --argjson endpoints "${endpoints}" '{
      apiVersion:"v1",kind:"List",metadata:{resourceVersion:""},items:[{
        apiVersion:"discovery.k8s.io/v1",kind:"EndpointSlice",addressType:"IPv4",
        endpoints:$endpoints,
        metadata:{
          labels:{
            "app.kubernetes.io/name":"cma-api",
            "app.kubernetes.io/part-of":"cma",
            "endpointslice.kubernetes.io/managed-by":"endpointslice-controller.k8s.io",
            "kubernetes.io/service-name":"cma-api"
          },
          name:"cma-api-fixture",namespace:"chrono-bot",resourceVersion:$rv,
          uid:$slice_uid,
          ownerReferences:[{
            apiVersion:"v1",blockOwnerDeletion:true,controller:true,
            kind:"Service",name:"cma-api",uid:$service_uid
          }]
        },
        ports:[{name:"http",port:8080,protocol:"TCP"}]
      }]
    }'
}

cma_test_increment_counter() {
  local counter_file="$1"
  local count=0

  if [ -s "${counter_file}" ]; then
    count="$(sed -n '1p' "${counter_file}")"
  fi
  count=$((count + 1))
  printf '%s\n' "${count}" >"${counter_file}"
  printf '%s\n' "${count}"
}

cma_test_fence_case() {
  local case_name="$1"
  local expected_status="$2"
  local expected_patches="$3"
  local expected_drains="$4"
  local expected_error="${5:-}"
  local case_dir="${cma_test_tmp}/fence-${case_name}"
  local actual_status actual_patches actual_drains

  mkdir "${case_dir}"
  printf '%s\n' source >"${case_dir}/selector-state"
  : >"${case_dir}/boundaries.log"
  : >"${case_dir}/patches.log"
  : >"${case_dir}/drains.log"
  set +e
  (
    set -euo pipefail
    export CMA_GATEWAY_CONFIRM=fence-v0.10-traffic
    export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    cma_pinned_api_root_identity=""
    if [ "${case_name}" = already_fenced ]; then
      printf '%s\n' fenced >"${case_dir}/selector-state"
    fi

    cma_step10_require_production_mutation_enabled() { return 0; }
    cma_step10_require_privileged_bash() { return 0; }
    cma_step10_require_trusted_launcher() { return 0; }
    cma_step10_reject_unexpected_token_fd() { return 0; }
    cma_step10_validate_admission_reservation() { return 0; }
    cma_step10_projection_check_live() { return 0; }
    cma_step10_require_projection_file() { return 0; }
    cma_step10_source_projection_matches_active_receipt() { return 0; }
    cma_step10_projection_capture() { printf '%s\n' /test/fenced-projection; }
    cma_step10_validate_fence_projection() { return 0; }
    cma_step10_write_fence_receipt() {
      printf '%s\n' '/test/fence-receipt.json|0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
    }
    cma_step10_load_fence_receipt() {
      cma_pinned_api_root_identity='{"uid":"service-uid","name":"k8s1-002d2683-chrono-bot-cma-api-80-7201e85f","zones":["asia-northeast1-a","asia-northeast1-b"]}'
      CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${case_dir}/fence-receipt.json"
      CMA_GATEWAY_FENCE_RECEIPT_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
      printf '%s\n' \
        '{"sourceProjection":{"path":"/test/source-projection.json"}}' \
        >"${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}"
      return 0
    }
    export CMA_GATEWAY_SOURCE_PROJECTION=/test/source-projection.json
    export CMA_GATEWAY_FENCE_RECEIPT_DIR=/test/fence-receipts
    export CMA_GATEWAY_FENCE_RECEIPT_CONFIRM=durable-prepatch-root-v1
    cma_step10_require_checkout() {
      if [ "$#" -ne 1 ] || [ "$1" != fence-v0.10-traffic ]; then
        return 96
      fi
      printf '%s\n' checkout >>"${case_dir}/boundaries.log"
    }
    cma_step10_kubectl() {
      local selector_state service_json neg_status

      if [ "$*" = 'get pod -l release.chrono-ai.fun/cutover-fence=mongodb-715 -o json' ]; then
        printf '%s\n' no-pod >>"${case_dir}/boundaries.log"
        if [ "${case_name}" = reserved_fence_pod ]; then
          printf '%s\n' '{"apiVersion":"v1","kind":"List","items":[{}]}'
        else
          printf '%s\n' '{"apiVersion":"v1","kind":"List","items":[]}'
        fi
        return 0
      fi
      if [ "$*" = 'get service cma-api -o json' ]; then
        printf '%s\n' get-service >>"${case_dir}/boundaries.log"
        if [ "${case_name}" = initial_read_failure ] && \
           [ ! -s "${case_dir}/patches.log" ]; then
          return 42
        fi
        selector_state="$(sed -n '1p' "${case_dir}/selector-state")"
        if [ "${case_name}" = initial_unknown ] && \
           [ ! -s "${case_dir}/patches.log" ]; then
          selector_state=unknown
        fi
        case "${selector_state}" in
          source)
            service_json="$(cma_test_source_service service-uid service-rv-1)"
            ;;
          fenced)
            service_json="$(cma_test_fenced_service service-uid service-rv-2)"
            case "${case_name}" in
              service_uid_drift)
                service_json="$(printf '%s' "${service_json}" | \
                  jq -c '.metadata.uid = "service-uid-recreated"')"
                ;;
              neg_identity_drift)
                neg_status="$(printf '%s' "${service_json}" | jq -cer '
                  (.metadata.annotations["cloud.google.com/neg-status"] | fromjson) |
                  .network_endpoint_groups["80"] =
                    "k8s1-002d2683-chrono-bot-cma-api-80-drifted"
                ')"
                service_json="$(printf '%s' "${service_json}" | jq -c \
                  --arg neg_status "${neg_status}" \
                  '.metadata.annotations["cloud.google.com/neg-status"] = $neg_status')"
                ;;
            esac
            ;;
          unknown)
            service_json="$(cma_test_source_service service-uid service-rv-1 | \
              jq -c '.spec.selector = {"unexpected":"selector"}')"
            ;;
          *) return 95 ;;
        esac
        if [ "${case_name}" = initial_root_invalid ] && \
           [ ! -s "${case_dir}/patches.log" ]; then
          service_json="$(printf '%s' "${service_json}" | jq -c '
            .metadata.annotations["cloud.google.com/neg-status"] = "{}"
          ')"
        fi
        printf '%s\n' "${service_json}"
        return 0
      fi
      if [ "$1" = patch ] && [ "$2" = service ]; then
        printf '%s\n' patch >>"${case_dir}/boundaries.log"
        printf '%s\n' patch >>"${case_dir}/patches.log"
        if [ "$#" -ne 5 ] || [ "$3" != cma-api ] || \
           [ "$4" != --type=json ] || \
           [ "$5" != '-p=[{"op":"test","path":"/spec/selector","value":{"app.kubernetes.io/name":"cma-api"}},{"op":"replace","path":"/spec/selector","value":{"release.chrono-ai.fun/cutover-fence":"mongodb-715"}}]' ]; then
          printf '%s\n' invalid-cas >>"${case_dir}/patches.log"
          return 94
        fi
        case "${case_name}" in
          patch_failure_source) return 42 ;;
          patch_unknown)
            printf '%s\n' unknown >"${case_dir}/selector-state"
            return 0
            ;;
          *) printf '%s\n' fenced >"${case_dir}/selector-state" ;;
        esac
        if [ "${case_name}" = ambiguous_applied ]; then
          return 42
        fi
        return 0
      fi
      return 93
    }
    cma_step10_gcloud() {
      printf '%s\n' unexpected-gcloud >>"${case_dir}/boundaries.log"
      return 92
    }
    cma_wait_for_fenced_endpoints() {
      printf '%s\n' drain >>"${case_dir}/boundaries.log"
      printf '%s\n' drain >>"${case_dir}/drains.log"
      if [ "${case_name}" = drain_failure ]; then
        return 1
      fi
      return 0
    }

    cma_step10_fence_main
  ) >"${case_dir}/stdout" 2>"${case_dir}/stderr"
  actual_status=$?
  set -e

  actual_patches="$(awk '$0 == "patch" { count += 1 } END { print count + 0 }' \
    "${case_dir}/patches.log")"
  actual_drains="$(awk '$0 == "drain" { count += 1 } END { print count + 0 }' \
    "${case_dir}/drains.log")"
  if [ "${actual_status}" -ne "${expected_status}" ] || \
     [ "${actual_patches}" -ne "${expected_patches}" ] || \
     [ "${actual_drains}" -ne "${expected_drains}" ] || \
     grep -q '^invalid-cas$\|^unexpected-gcloud$' "${case_dir}"/*.log; then
    echo "fence-${case_name}: status, exact CAS count, or drain boundary mismatch" >&2
    sed -n '1,160p' "${case_dir}/stderr" >&2
    sed -n '1,160p' "${case_dir}/boundaries.log" >&2
    return 1
  fi
  if [ "${expected_status}" -eq 0 ]; then
    if ! grep -q '^CMA traffic is fenced and externally drained;' \
        "${case_dir}/stdout"; then
      echo "fence-${case_name}: successful fence did not emit its final proof" >&2
      return 1
    fi
  elif [ -s "${case_dir}/stdout" ]; then
    echo "fence-${case_name}: failed fence emitted a success proof" >&2
    return 1
  fi
  if [ -n "${expected_error}" ] && \
     ! grep -Fq "${expected_error}" "${case_dir}/stderr"; then
    echo "fence-${case_name}: expected failure or reconciliation warning was absent" >&2
    sed -n '1,160p' "${case_dir}/stderr" >&2
    return 1
  fi
}

cma_test_real_reopen_case() {
  local case_name="$1"
  local expected_status="$2"
  local expected_gcloud_calls="$3"
  local expected_error="${4:-}"
  local case_dir="${cma_test_tmp}/real-reopen-${case_name}"
  local actual_status actual_gcloud_calls

  mkdir "${case_dir}"
  set +e
  (
    set -euo pipefail
    cma_final_pod=api-pod
    cma_final_pod_uid=pod-uid
    cma_final_pod_ip=10.0.0.8
    cma_final_pod_node_name=node-a
    cma_final_pod_provider_zone=asia-northeast1-a
    cma_reopen_committed_projection=""
    cma_pinned_api_root_identity='{"uid":"service-uid","name":"k8s1-002d2683-chrono-bot-cma-api-80-7201e85f","zones":["asia-northeast1-a","asia-northeast1-b"]}'

    cma_step10_deadline_reached() {
      if [ "${case_name}" = late_neg_deadline ] && \
         [ -e "${case_dir}/late-neg-read" ]; then
        return 0
      fi
      return 1
    }
    cma_step10_kubectl() {
      local call_count service_uid=service-uid service_rv=service-rv-1
      local slice_uid=slice-uid slice_rv=slice-rv-1 endpoint_state=exact

      if [ "$1" = get ] && [ "$2" = service ]; then
        call_count="$(cma_test_increment_counter "${case_dir}/service-calls")"
        if [ "${case_name}" = service_uid_drift ] && [ "${call_count}" -ge 2 ]; then
          service_uid=service-uid-recreated
          service_rv=service-rv-2
        elif [ "${case_name}" = service_rv_drift ] && [ "${call_count}" -ge 2 ]; then
          service_rv=service-rv-2
        fi
        cma_test_source_service "${service_uid}" "${service_rv}"
        return 0
      fi
      if [ "$1" = get ] && [ "$2" = endpointslice ]; then
        call_count="$(cma_test_increment_counter "${case_dir}/slice-calls")"
        case "${case_name}" in
          pending_retry)
            if [ ! -e "${case_dir}/retry-transition" ]; then
              endpoint_state=pending
            fi
            ;;
          pending_nonempty | pending_wrong | pending_extra | malformed_neg | \
            late_neg_deadline)
            endpoint_state=pending
            ;;
          endpoint_serving_false) endpoint_state=serving_false ;;
          endpoint_terminating) endpoint_state=terminating_true ;;
          endpoint_node_drift) endpoint_state=node_drift ;;
          endpoint_zone_drift) endpoint_state=zone_drift ;;
        esac
        if [ "${case_name}" = service_uid_drift ] && [ "${call_count}" -ge 2 ]; then
          service_uid=service-uid-recreated
        fi
        if [ "${case_name}" = slice_rv_drift ] && [ "${call_count}" -ge 2 ]; then
          slice_rv=slice-rv-2
        fi
        cma_test_endpoint_slices \
          "${endpoint_state}" "${service_uid}" "${slice_uid}" "${slice_rv}"
        return 0
      fi
      return 1
    }
    cma_step10_gcloud() {
      local zone=unknown

      cma_test_increment_counter "${case_dir}/gcloud-calls" >/dev/null
      case "$*" in
        *--zone=asia-northeast1-a*) zone=a ;;
        *--zone=asia-northeast1-b*) zone=b ;;
      esac
      case "${case_name}:${zone}" in
        pending_retry:a)
          if [ -e "${case_dir}/retry-transition" ]; then
            printf '%s\n' '[{"ipAddress":"10.0.0.8","port":8080}]'
          else
            printf '%s\n' '[]'
          fi
          ;;
        pending_nonempty:a | pending_extra:a | pending_extra:b | \
          exact_success:a | service_uid_drift:a | service_rv_drift:a | \
          slice_rv_drift:a)
          printf '%s\n' '[{"ipAddress":"10.0.0.8","port":8080}]'
          ;;
        pending_wrong:b)
          printf '%s\n' '[{"ipAddress":"10.0.0.99","port":8080}]'
          ;;
        malformed_neg:b)
          printf '%s\n' '{"ipAddress":"10.0.0.8","port":8080}'
          ;;
        late_neg_deadline:a)
          : >"${case_dir}/late-neg-read"
          printf '%s\n' '[]'
          ;;
        *)
          printf '%s\n' '[]'
          ;;
      esac
    }
    sleep() {
      if [ "${case_name}" = pending_retry ]; then
        : >"${case_dir}/retry-transition"
        return 0
      fi
      return 1
    }

    cma_wait_for_reopened_endpoint
    if [ "${expected_status}" -eq 0 ] && \
       [ -z "${cma_reopen_committed_projection}" ]; then
      echo "successful real reopen proof did not retain its normalized projection" >&2
      exit 97
    fi
  ) >"${case_dir}/stdout" 2>"${case_dir}/stderr"
  actual_status=$?
  set -e

  actual_gcloud_calls="$(sed -n '1p' "${case_dir}/gcloud-calls" 2>/dev/null || printf 0)"
  if [ "${actual_status}" -ne "${expected_status}" ] || \
     [ "${actual_gcloud_calls}" -ne "${expected_gcloud_calls}" ]; then
    echo "real-reopen-${case_name}: status or bounded NEG-read count mismatch" >&2
    sed -n '1,120p' "${case_dir}/stderr" >&2
    return 1
  fi
  if [ -n "${expected_error}" ] && \
     ! grep -q "${expected_error}" "${case_dir}/stderr"; then
    echo "real-reopen-${case_name}: expected failure was not reported" >&2
    sed -n '1,120p' "${case_dir}/stderr" >&2
    return 1
  fi
}

cma_test_run_case initial-source 1 1 0
cma_test_run_case initial-unknown 1 1 0
cma_test_run_case initial-read-failure 1 1 0
cma_test_run_case pod-failure 1 1 0
cma_test_run_case token-crlf 1 0 0
cma_test_run_case initial-signal 1 1 0
cma_test_run_case initial-quit 1 1 0
cma_test_run_case initial-fence-failure 1 1 0
cma_test_run_case initial-route-failure 1 0 0
cma_test_run_case patch-failure-source 1 1 0
cma_test_run_case reopen-proof-failure 1 1 0
cma_test_run_case post-route-failure 1 1 2
cma_test_run_case post-endpoint-failure 1 1 2
cma_test_run_case auth-signal 1 1 0
cma_test_run_case auth-quit 1 1 0
cma_test_run_case truncated-livez 1 1 0
cma_test_run_case truncated-ready 1 1 0
cma_test_run_case success 0 0 2
cma_test_exit_handler_signal_case
cma_test_exit_handler_zero_case
cma_test_production_gate_case
cma_test_fence_production_gate_case
cma_test_fence_extra_args_case
cma_test_privileged_startup_is_closed
cma_test_curl_environment_is_closed
cma_test_gcloud_environment_is_closed
cma_test_fence_case source_success 0 1 1
cma_test_fence_case already_fenced 0 0 1
cma_test_fence_case ambiguous_applied 0 1 1 \
  'fence patch reported failure, but the exact applied fence'
cma_test_fence_case patch_failure_source 1 1 0 \
  'cma-api remains at the exact source selector'
cma_test_fence_case patch_unknown 1 1 0 \
  'fence patch left an unknown cma-api selector'
cma_test_fence_case initial_unknown 1 0 0 \
  'cma-api has an unknown selector before fencing'
cma_test_fence_case initial_read_failure 1 0 0 \
  'cma-api could not be read before fencing'
cma_test_fence_case initial_root_invalid 1 0 0
cma_test_fence_case service_uid_drift 1 1 0 \
  'fence patch completed ambiguously and cma-api could not be read'
cma_test_fence_case neg_identity_drift 1 1 0 \
  'fence patch completed ambiguously and cma-api could not be read'
cma_test_fence_case reserved_fence_pod 1 0 0 \
  'cutover fence selector unexpectedly matches a pod'
cma_test_fence_case drain_failure 1 1 1 \
  'exact EndpointSlice and zonal NEG traffic fence is not proved'
cma_test_real_reopen_case pending_retry 0 4
cma_test_real_reopen_case pending_nonempty 1 2 \
  'before the verified EndpointSlice projection'
cma_test_real_reopen_case pending_wrong 1 2 \
  'not the exact verified pod address and port'
cma_test_real_reopen_case pending_extra 1 2 \
  'more than the one verified pod projection'
cma_test_real_reopen_case malformed_neg 1 2 'membership is malformed'
cma_test_real_reopen_case exact_success 0 2
cma_test_real_reopen_case service_uid_drift 1 2 'identities changed'
cma_test_real_reopen_case service_rv_drift 1 2 'identities changed'
cma_test_real_reopen_case slice_rv_drift 1 2 'identities changed'
cma_test_real_reopen_case late_neg_deadline 1 1 \
  'read crossed the five-minute reopen deadline'
cma_test_real_reopen_case endpoint_serving_false 1 0 \
  'EndpointSlices could not be normalized'
cma_test_real_reopen_case endpoint_terminating 1 0 \
  'EndpointSlices could not be normalized'
cma_test_real_reopen_case endpoint_node_drift 1 2 \
  'routable EndpointSlice entry is not exactly the verified pod'
cma_test_real_reopen_case endpoint_zone_drift 1 2 \
  'routable EndpointSlice entry is not exactly the verified pod'

cma_test_projection_emit_fixture() {
  jq -e '.projection' "$1" >"$2"
}

cma_test_make_projection_bundle() {
  local projection_dir="$1"
  local stage="$2"
  local predecessor=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
  local digest

  mkdir "${projection_dir}"
  jq -n --arg stage "${stage}" --arg predecessor "${predecessor}" '
    {schemaVersion:1,mode:"source",
     lifecycle:{stage:$stage,predecessorSha256:$predecessor},
     projection:{schemaVersion:1,mode:"source",
       lifecycle:{stage:$stage,predecessorSha256:$predecessor},
       captureSha256:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
       behaviorPolicy:{policySha256:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
         sourceEvidenceSha256:"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},
       admissionReceipt:{receiptSha256:"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"}}}
  ' >"${projection_dir}/capture.json"
  cma_test_projection_emit_fixture "${projection_dir}/capture.json" \
    "${projection_dir}/projection.json"
  digest="$(cma_step10_sha256_file "${projection_dir}/projection.json")"
  digest="${digest%% *}"
  printf '%s  projection.json\n' "${digest}" >"${projection_dir}/projection.sha256"
}

cma_test_projection_rejects() {
  local projection_file="$1"
  local label="$2"
  if (
      cma_step10_projection_emit() { cma_test_projection_emit_fixture "$@"; }
      cma_step10_require_projection_file "${projection_file}"
    ) >/dev/null 2>&1; then
    echo "invalid adopted projection bundle was accepted: ${label}" >&2
    exit 1
  fi
}

projection_fixture_root="${cma_test_tmp}/projection-custody"
mkdir "${projection_fixture_root}"
valid_projection_dir="${projection_fixture_root}/valid"
cma_test_make_projection_bundle "${valid_projection_dir}" adopted
(
  cma_step10_projection_emit() { cma_test_projection_emit_fixture "$@"; }
  cma_step10_require_projection_file "${valid_projection_dir}/projection.json"
)

source_stage_dir="${projection_fixture_root}/source-stage"
cma_test_make_projection_bundle "${source_stage_dir}" source
cma_test_projection_rejects "${source_stage_dir}/projection.json" source-stage

bad_digest_dir="${projection_fixture_root}/bad-digest"
cp -R "${valid_projection_dir}" "${bad_digest_dir}"
printf '%064d  projection.json\n' 0 >"${bad_digest_dir}/projection.sha256"
cma_test_projection_rejects "${bad_digest_dir}/projection.json" sibling-digest

bad_regeneration_dir="${projection_fixture_root}/bad-regeneration"
cp -R "${valid_projection_dir}" "${bad_regeneration_dir}"
jq '.captureSha256 = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"' \
  "${bad_regeneration_dir}/projection.json" >"${bad_regeneration_dir}/next"
mv "${bad_regeneration_dir}/next" "${bad_regeneration_dir}/projection.json"
bad_regeneration_digest="$(cma_step10_sha256_file \
  "${bad_regeneration_dir}/projection.json")"
bad_regeneration_digest="${bad_regeneration_digest%% *}"
printf '%s  projection.json\n' "${bad_regeneration_digest}" \
  >"${bad_regeneration_dir}/projection.sha256"
cma_test_projection_rejects "${bad_regeneration_dir}/projection.json" regeneration

cma_test_make_source_receipt() {
  local projection_file="$1"
  local output_file="$2"
  local projection_digest
  projection_digest="$(cma_step10_sha256_file "${projection_file}")"
  projection_digest="${projection_digest%% *}"
  jq -n --arg path "${projection_file}" --arg digest "${projection_digest}" \
    --slurpfile source "${projection_file}" '
    {sourceProjection:{path:$path,sha256:$digest,lifecycle:$source[0].lifecycle,
      captureSha256:$source[0].captureSha256,
      behaviorPolicy:$source[0].behaviorPolicy,
      admissionReceipt:$source[0].admissionReceipt}}
  ' >"${output_file}"
}

valid_source_receipt="${projection_fixture_root}/source-receipt.json"
cma_test_make_source_receipt "${valid_projection_dir}/projection.json" \
  "${valid_source_receipt}"
CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${valid_source_receipt}"
export CMA_GATEWAY_ACTIVE_FENCE_RECEIPT
cma_step10_source_projection_matches_active_receipt \
  "${valid_projection_dir}/projection.json"

cma_test_receipt_rejects() {
  local mutation="$1"
  local label="$2"
  local candidate="${projection_fixture_root}/receipt-${label}.json"
  jq "${mutation}" "${valid_source_receipt}" >"${candidate}"
  CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${candidate}"
  export CMA_GATEWAY_ACTIVE_FENCE_RECEIPT
  if cma_step10_source_projection_matches_active_receipt \
      "${valid_projection_dir}/projection.json" >/dev/null 2>&1; then
    echo "mismatched durable source receipt was accepted: ${label}" >&2
    exit 1
  fi
}

cma_test_receipt_rejects '.sourceProjection.path = "/wrong/projection.json"' path
cma_test_receipt_rejects '.sourceProjection.sha256 = ("0" * 64)' digest
cma_test_receipt_rejects '.sourceProjection.lifecycle.stage = "final-reopen"' lifecycle
cma_test_receipt_rejects '.sourceProjection.captureSha256 = ("f" * 64)' content

prepatch_log="${projection_fixture_root}/prepatch.log"
: >"${prepatch_log}"
if (
    export CMA_GATEWAY_CONFIRM=fence-v0.10-traffic
    export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_SOURCE_PROJECTION="${source_stage_dir}/projection.json"
    cma_step10_require_production_mutation_enabled() { return 0; }
    cma_step10_require_privileged_bash() { return 0; }
    cma_step10_require_trusted_launcher() { return 0; }
    cma_step10_reject_unexpected_token_fd() { return 0; }
    cma_step10_validate_admission_reservation() { return 0; }
    cma_step10_require_checkout() { return 0; }
    cma_fence_label_has_no_pods() { return 0; }
    cma_parse_source_api_neg() { printf '%s\n' '{}'; }
    cma_step10_projection_emit() { cma_test_projection_emit_fixture "$@"; }
    cma_step10_kubectl() {
      if [[ "$*" == 'get service cma-api -o json' ]]; then
        printf '%s\n' '{"spec":{"selector":{"app.kubernetes.io/name":"cma-api"}}}'
      elif [[ "$1" == patch ]]; then
        printf '%s\n' patch >>"${prepatch_log}"
      fi
    }
    cma_step10_write_fence_receipt() {
      printf '%s\n' receipt-write >>"${prepatch_log}"
    }
    cma_step10_fence_main
  ) >/dev/null 2>&1; then
  echo "fence accepted a non-adopted source projection" >&2
  exit 1
fi
[[ ! -s "${prepatch_log}" ]] || {
  echo "fence wrote a receipt or patched Service before adopted-stage validation" >&2
  exit 1
}

printf '%s\n' "CMA Trigger Gateway traffic-restoration tests passed (62 cases)"
