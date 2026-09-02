#!/usr/bin/env bash
# Mock overrides are invoked indirectly, and each case intentionally isolates its log in a subshell.
# shellcheck disable=SC2030,SC2031,SC2317,SC2329

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=.github/scripts/cma-trigger-gateway-restore-traffic.sh
source "${SCRIPT_DIR}/cma-trigger-gateway-restore-traffic.sh"

cma_test_tmp="$(mktemp -d)"
trap 'rm -rf "${cma_test_tmp}"' EXIT

index_flag_fixture="$({
  printf '%s\n' 'H ordinary' 'h assume-unchanged' 'S skip-worktree' \
    's assume-unchanged-and-skip-worktree'
} | cma_step10_forbidden_index_entries)"
[[ "${index_flag_fixture}" == $'h assume-unchanged\nS skip-worktree\ns assume-unchanged-and-skip-worktree' ]] || {
  echo "combined lowercase-s index flags were not rejected" >&2
  exit 1
}

cma_test_configure_mocks() {
  cma_test_selector=fenced
  cma_test_route_checks=0
  cma_test_reopen_checks=0
  cma_test_workload_checks=0

  cma_test_assert_no_secret_exports() {
    if env | grep -Eq \
      '^(CHRONO_RELEASE_ACCESS_TOKEN|CURL_CA_BUNDLE|CURL_SSL_BACKEND|SSL_CERT_DIR|SSL_CERT_FILE|SSLKEYLOGFILE|cma_release_access_token|cma_direct_livez|cma_direct_ready|cma_app_openapi|cma_direct_sessions|cma_app_sessions)='; then
      printf '%s\n' credential-leak >>"${CMA_TEST_LOG}"
      return 1
    fi
  }

  cma_step10_require_checkout() {
    cma_test_assert_no_secret_exports
    CMA_GATEWAY_RELEASE_TAG=v1.2.3
  }
  cma_step10_require_privileged_bash() { return 0; }
  cma_step10_require_trusted_launcher() { return 0; }
  cma_step10_validate_snapshot_seal() { return 0; }
  cma_final_pod_topology() {
    printf '%s\n' '{"nodeName":"node-a","providerZone":"asia-northeast1-a","providerInstance":"instance-a"}'
  }
  cma_final_workload_is_exact() {
    cma_test_workload_checks=$((cma_test_workload_checks + 1))
    case "${CMA_TEST_CASE}:${cma_test_workload_checks}" in
      pod-failure:1 | preopen-workload-replacement:2 | \
        final-projection-workload-replacement:4) return 1 ;;
      *) return 0 ;;
    esac
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
  cma_step10_projection_digest() {
    printf '%s\n' unbound-projection-digest >>"${CMA_TEST_LOG}"
    return 1
  }
  cma_step10_validate_fence_projection() {
    case "${CMA_TEST_CASE}" in
      pre-reopen-semantic-replacement | pre-reopen-live-replacement) return 1 ;;
      *) printf '%s\n' ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff ;;
    esac
  }
  cma_step10_validate_source_projection() {
    case "${CMA_TEST_CASE}" in
      final-source-semantic-replacement | final-source-live-replacement) return 1 ;;
      *) printf '%s\n' ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff ;;
    esac
  }
  # shellcheck disable=SC2317 # Harness override is invoked by sourced production code.
  cma_step10_projection_check_live() { return 0; }
  # shellcheck disable=SC2317 # Harness override is invoked by sourced production code.
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
      printf '%s\n' service-open >>"${CMA_TEST_LOG}"
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
    export CMA_FINAL_DEPLOYMENT_UID=deployment-uid
    export CMA_FINAL_REPLICA_SET=cma-api-abc123
    export CMA_FINAL_REPLICA_SET_UID=replica-set-uid
    export CMA_FINAL_POD_IMAGE=us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api:develop-e03b5e7@sha256:a7d371277d88e44c8cd687c539fb0523e07a9ef12314880de3c4b39887cf5abe
    export CMA_FINAL_POD_IMAGE_ID=us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
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
    if [ "${case_name}" = confirmation-mismatch ]; then
      export CMA_GATEWAY_CONFIRM=restore-v9.9.9-traffic
    else
      export CMA_GATEWAY_CONFIRM=restore-v1.2.3-traffic
    fi
    export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_ROLLBACK_DIR=/test/rollback
    export CMA_GATEWAY_ROLLBACK_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
    export CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256=eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee
    export CMA_GATEWAY_FENCED_POST_SCALE_PROJECTION_SHA256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
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
  case "${case_name}" in
    pre-reopen-semantic-replacement | pre-reopen-live-replacement)
      if grep -q '^service-open$' "${case_log}"; then
        echo "${case_name}: replaced pre-reopen evidence reached the Service-open patch" >&2
        return 1
      fi
      ;;
    final-source-semantic-replacement | final-source-live-replacement)
      if grep -q '^CMA traffic restoration is complete and verified;' "${case_output}"; then
        echo "${case_name}: replaced final evidence emitted success" >&2
        return 1
      fi
      ;;
  esac
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
      export CMA_GATEWAY_CONFIRM=restore-v1.2.3-traffic
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
      # shellcheck disable=SC2317 # Boundary sentinel is reached through the production entrypoint.
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
      export CMA_GATEWAY_CONFIRM=fence-v1.2.3-traffic
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
      # shellcheck disable=SC2317 # Boundary sentinel is reached through the production entrypoint.
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
    export CMA_GATEWAY_CONFIRM=fence-v1.2.3-traffic
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
    cma_step10_projection_digest() {
      printf '%s\n' unbound-projection-digest >>"${case_dir}/boundaries.log"
      return 1
    }
    cma_step10_validate_fence_projection() {
      printf '%s\n' cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
    }
    cma_step10_write_fence_receipt() {
      printf '%s\n' '/test/fence-receipt.json|0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef'
    }
    cma_step10_load_fence_receipt() {
      cma_pinned_api_root_identity='{"uid":"service-uid","name":"k8s1-002d2683-chrono-bot-cma-api-80-7201e85f","zones":["asia-northeast1-a","asia-northeast1-b"]}'
      CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${case_dir}/fence-receipt.json"
      CMA_GATEWAY_FENCE_RECEIPT_SHA256=0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef
      printf '%s\n' \
        '{"sourceProjection":{"path":"/test/source-projection.json","sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","lifecycle":{"stage":"adopted","predecessorSha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}' \
        >"${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}"
      return 0
    }
    export CMA_GATEWAY_SOURCE_PROJECTION=/test/source-projection.json
    export CMA_GATEWAY_SOURCE_PROJECTION_SHA256=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    export CMA_GATEWAY_SOURCE_PROJECTION_PREDECESSOR_SHA256=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
    export CMA_GATEWAY_FENCE_RECEIPT_DIR=/test/fence-receipts
    export CMA_GATEWAY_FENCE_RECEIPT_CONFIRM=durable-prepatch-root-v1
    # shellcheck disable=SC2120 # The assertion rejects obsolete positional confirmation input.
    cma_step10_require_checkout() {
      [ "$#" -eq 0 ] || return 96
      CMA_GATEWAY_RELEASE_TAG=v1.2.3
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
    # shellcheck disable=SC2317 # Boundary sentinel is reached through the production entrypoint.
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
    grep -q '^CMA durable fence root committed before Service mutation;' \
      "${case_dir}/stdout" || {
      echo "fence-${case_name}: successful fence did not emit its pre-mutation receipt" >&2
      return 1
    }
  elif [ -s "${case_dir}/stdout" ]; then
    grep -Eq '^CMA durable fence root committed before Service mutation;.*SHA-256 [0-9a-f]{64}$' \
      "${case_dir}/stdout" || {
      echo "fence-${case_name}: post-patch failure did not retain a recoverable receipt digest" >&2
      return 1
    }
    if grep -q '^CMA traffic is fenced and externally drained;' "${case_dir}/stdout"; then
      echo "fence-${case_name}: failed drain emitted a final success proof" >&2
      return 1
    fi
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

cma_test_prove_fence_case() {
  local case_name="$1"
  local expected_status="$2"
  local expected_captures="$3"
  local case_dir="${cma_test_tmp}/prove-fence-${case_name}"
  local revision=0123456789abcdef0123456789abcdef01234567
  local parent_dir pre_scale_dir actual_status actual_captures

  mkdir -p "${case_dir}/projections"
  parent_dir="${case_dir}/projections"
  pre_scale_dir="${parent_dir}/fenced-pre-scale-${revision}"
  mkdir "${pre_scale_dir}"
  printf '%s\n' \
    '{"schemaVersion":1,"mode":"fenced","lifecycle":{"stage":"fenced-pre-scale","predecessorSha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}}' \
    >"${pre_scale_dir}/projection.json"
  set +e
  (
    set -euo pipefail
    export CMA_GATEWAY_CONFIRM=prove-v1.2.3-fence
    export CMA_GATEWAY_RELEASE_REVISION="${revision}"
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_FENCED_PROJECTION_PARENT="${parent_dir}"
    export CMA_GATEWAY_FENCED_PRE_SCALE_PROJECTION_SHA256=ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff
    cma_step10_require_privileged_bash() { return 0; }
    cma_step10_require_trusted_launcher() { return 0; }
    cma_step10_reject_unexpected_token_fd() { return 0; }
    cma_step10_require_checkout() { CMA_GATEWAY_RELEASE_TAG=v1.2.3; }
    cma_step10_load_fence_receipt() {
      CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${case_dir}/receipt.json"
      printf '%s\n' '{
        "sourceProjection":{
          "sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        },
        "workload":{
          "deployment":{"uid":"deployment-uid"},
          "pods":[{"name":"old-api-pod","uid":"old-pod-uid"}]
        }
      }' >"${CMA_GATEWAY_ACTIVE_FENCE_RECEIPT}"
    }
    cma_step10_validate_admission_reservation() { return 0; }
    cma_step10_validate_fence_projection_receipt() {
      [[ "$1" == "${pre_scale_dir}" ]]
    }
    cma_wait_for_fenced_endpoints() {
      printf '%s\n' drain >>"${case_dir}/boundaries.log"
    }
    cma_step10_projection_capture() {
      printf '%s\n' capture >>"${case_dir}/boundaries.log"
      printf '%s\n' "${case_dir}/post-scale"
    }
    cma_step10_projection_digest() {
      printf '%s\n' unbound-projection-digest >>"${case_dir}/boundaries.log"
      return 1
    }
    cma_step10_validate_fence_projection() {
      case "${case_name}" in
        projection-semantic-replacement | projection-live-replacement) return 1 ;;
        *) printf '%s\n' eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee ;;
      esac
    }
    cma_step10_kubectl() {
      case "$*" in
        'get deployment cma-api -o json')
          jq -cn --arg case_name "${case_name}" '{
            apiVersion:"apps/v1",kind:"Deployment",
            metadata:{name:"cma-api",namespace:"chrono-bot",uid:
              (if $case_name == "deployment-uid-drift" then "replacement" else "deployment-uid" end),
              generation:7,deletionTimestamp:null},
            spec:{replicas:(if $case_name == "replicas-nonzero" then 1 else 0 end)},
            status:{observedGeneration:7,
              replicas:(if $case_name == "replicas-nonzero" then 1 else 0 end),
              updatedReplicas:(if $case_name == "replicas-nonzero" then 1 else 0 end),
              readyReplicas:(if $case_name == "replicas-nonzero" then 1 else 0 end),
              availableReplicas:(if $case_name == "replicas-nonzero" then 1 else 0 end),
              unavailableReplicas:0}
          }'
          ;;
        'get pod -l app.kubernetes.io/name=cma-api -o json')
          if [[ "${case_name}" == labelled-pod-present ]]; then
            printf '%s\n' '{"apiVersion":"v1","kind":"List","items":[{"metadata":{"uid":"other"}}]}'
          else
            printf '%s\n' '{"apiVersion":"v1","kind":"List","items":[]}'
          fi
          ;;
        'get pod old-api-pod --ignore-not-found -o json')
          if [[ "${case_name}" == old-pod-present ]]; then
            printf '%s\n' '{"apiVersion":"v1","kind":"Pod","metadata":{"name":"old-api-pod","uid":"old-pod-uid"}}'
          fi
          ;;
        *) return 1 ;;
      esac
    }
    cma_step10_prove_fence_main
  ) >"${case_dir}/stdout" 2>"${case_dir}/stderr"
  actual_status=$?
  set -e
  actual_captures="$(awk '$0 == "capture" { count += 1 } END { print count + 0 }' \
    "${case_dir}/boundaries.log" 2>/dev/null || printf '%s\n' 0)"
  if [[ "${actual_status}" -ne "${expected_status}" || \
        "${actual_captures}" -ne "${expected_captures}" ]]; then
    echo "prove-fence-${case_name}: status or artifact count mismatch" >&2
    sed -n '1,120p' "${case_dir}/stderr" >&2
    return 1
  fi
  if [[ "${expected_status}" -ne 0 ]] && \
     grep -q '^CMA traffic fence is exact and externally drained;' \
       "${case_dir}/stdout"; then
    echo "prove-fence-${case_name}: failed validation acknowledged a projection digest" >&2
    return 1
  fi
}

cma_test_fence_receipt_fsync_recovery() {
  local case_dir="${cma_test_tmp}/fence-receipt-fsync"
  local revision=0123456789abcdef0123456789abcdef01234567

  mkdir -p "${case_dir}/custody" "${case_dir}/source"
  jq -n '{
    schemaVersion:1,mode:"source",
    lifecycle:{stage:"adopted",predecessorSha256:
      "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},
    captureSha256:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    behaviorPolicy:{policySha256:
      "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
      sourceEvidenceSha256:
      "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},
    admissionReceipt:{receiptSha256:
      "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"},
    graph:{
      services:{"cma-api":{uid:"service-uid",name:"neg-name",
        zones:["asia-northeast1-a"],backendSelfLink:"backend-link",
        backendFingerprint:"backend-fingerprint",backendGroups:["group-link"]}},
      fenceAdmission:{policy:{uid:"policy-uid"},binding:{uid:"binding-uid"}},
      apiWorkload:{deployment:{generation:3,name:"cma-api",namespace:"chrono-bot",
        replicas:1,resourceVersion:"10",uid:"deployment-uid"},
        pods:[{name:"old-api-pod",uid:"old-pod-uid"}],verified:null}
    }
  }' >"${case_dir}/source/projection.json"
  (
    set -euo pipefail
    export CMA_GATEWAY_RELEASE_REVISION="${revision}"
    export CMA_GATEWAY_FENCE_RECEIPT_DIR="${case_dir}/custody"
    export CMA_GATEWAY_FENCE_RECEIPT_CONFIRM=durable-prepatch-root-v1
    cma_step10_require_projection_file() { return 0; }
    eval "$(declare -f cma_step10_sync_published_fence_bundle |
      sed '1s/^cma_step10_sync_published_fence_bundle /cma_test_real_fence_bundle_sync /')"
    cma_step10_sync_published_fence_bundle() {
      if [[ ! -e "${case_dir}/sync-failure.injected" ]]; then
        : >"${case_dir}/sync-failure.injected"
        return 1
      fi
      cma_test_real_fence_bundle_sync "$@"
    }
    if cma_step10_write_fence_receipt "${case_dir}/source/projection.json" \
        '{"uid":"service-uid","name":"neg-name","zones":["asia-northeast1-a"]}' \
        >"${case_dir}/first.stdout" 2>"${case_dir}/first.stderr"; then
      echo "fence receipt accepted rename success followed by fsync failure" >&2
      return 1
    fi
    [[ -d "${case_dir}/custody/fence-root-${revision}" && \
      -e "${case_dir}/sync-failure.injected" && ! -s "${case_dir}/first.stdout" ]]
    CMA_GATEWAY_FENCE_RECEIPT_SHA256="$(cma_step10_sha256_file \
      "${case_dir}/custody/fence-root-${revision}/receipt.json")"
    CMA_GATEWAY_FENCE_RECEIPT_SHA256="${CMA_GATEWAY_FENCE_RECEIPT_SHA256%% *}"
    export CMA_GATEWAY_FENCE_RECEIPT_SHA256
    cma_external_receipt_sha256="${CMA_GATEWAY_FENCE_RECEIPT_SHA256}"
    CMA_GATEWAY_FENCE_RECEIPT_SHA256=0000000000000000000000000000000000000000000000000000000000000000
    export CMA_GATEWAY_FENCE_RECEIPT_SHA256
    if cma_step10_write_fence_receipt "${case_dir}/source/projection.json" \
        '{"uid":"service-uid","name":"neg-name","zones":["asia-northeast1-a"]}' \
        >/dev/null 2>&1; then
      echo "fence receipt retry replaced an external digest from local custody" >&2
      return 1
    fi
    [[ "${CMA_GATEWAY_FENCE_RECEIPT_SHA256}" == \
      0000000000000000000000000000000000000000000000000000000000000000 ]]
    CMA_GATEWAY_FENCE_RECEIPT_SHA256="${cma_external_receipt_sha256}"
    export CMA_GATEWAY_FENCE_RECEIPT_SHA256
    cma_step10_write_fence_receipt "${case_dir}/source/projection.json" \
      '{"uid":"service-uid","name":"neg-name","zones":["asia-northeast1-a"]}' \
      >"${case_dir}/retry.stdout"
    grep -Eq '/receipt\.json\|[0-9a-f]{64}$' "${case_dir}/retry.stdout"
  )
}

cma_test_fence_projection_workload_chain() {
  local case_dir="${cma_test_tmp}/fence-projection-workload-chain"
  local source_digest=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

  mkdir "${case_dir}"
  jq -n --arg source_digest "${source_digest}" '{
    sourceProjection:{sha256:$source_digest,
      behaviorPolicy:{policySha256:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
      admissionReceipt:{receiptSha256:"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"}},
    service:{uid:"service-uid",name:"neg-name",zones:["asia-northeast1-a"],
      backendSelfLink:"backend-link",backendFingerprint:"fingerprint",
      backendGroups:["group-link"]},
    admission:{policy:{uid:"policy-uid"},binding:{uid:"binding-uid"}},
    workload:{deployment:{generation:3,name:"cma-api",namespace:"chrono-bot",
      replicas:1,resourceVersion:"10",uid:"deployment-uid"},
      pods:[{name:"old-api-pod",uid:"old-pod-uid"}],verified:null}
  }' >"${case_dir}/receipt.json"
  (
    set -euo pipefail
    CMA_GATEWAY_ACTIVE_FENCE_RECEIPT="${case_dir}/receipt.json"
    export CMA_GATEWAY_ACTIVE_FENCE_RECEIPT
    cma_step10_projection_emit() { jq -e '.projection' "$1" >"$2"; }
    cma_step10_projection_matches_final_workload() { return 0; }
    cma_test_make_fence_projection() {
      local name="$1"
      local stage="$2"
      local replicas="$3"
      local pod_uid="$4"
      local predecessor="$5"
      local projection_dir="${case_dir}/${name}"
      local digest
      mkdir "${projection_dir}"
      jq -n --arg stage "${stage}" --arg predecessor "${predecessor}" \
        --argjson replicas "${replicas}" --arg pod_uid "${pod_uid}" '{
        schemaVersion:1,mode:"fenced",lifecycle:{stage:$stage,predecessorSha256:$predecessor},
        behaviorPolicy:{policySha256:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"},
        admissionReceipt:{receiptSha256:"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc"},
        graph:{
          services:{"cma-api":{uid:"service-uid",name:"neg-name",
            zones:["asia-northeast1-a"],backendSelfLink:"backend-link",
            backendFingerprint:"fingerprint",backendGroups:["group-link"],
            effectiveMembers:[],effectiveHealth:[]}},
          fenceAdmission:{policy:{uid:"policy-uid"},binding:{uid:"binding-uid"}},
          apiWorkload:{deployment:{generation:4,name:"cma-api",namespace:"chrono-bot",
            replicas:$replicas,resourceVersion:"11",uid:"deployment-uid"},
            pods:(if $pod_uid == "" then [] else [{name:"api-pod",uid:$pod_uid}] end),
            verified:(if $stage == "fenced-pre-reopen" then {} else null end)}
        },
        fullCapture:{kubernetes:{services:{items:[{metadata:{name:"cma-api",
          namespace:"chrono-bot",uid:"service-uid"}}]}}}
      }' >"${projection_dir}/projection.json"
      if [[ "${stage}" == fenced-pre-scale ]]; then
        jq '.graph.apiWorkload.deployment.generation = 3 |
          .graph.apiWorkload.deployment.resourceVersion = "10" |
          .graph.apiWorkload.pods = [{name:"old-api-pod",uid:"old-pod-uid"}]' \
          "${projection_dir}/projection.json" >"${projection_dir}/next"
        mv "${projection_dir}/next" "${projection_dir}/projection.json"
      fi
      jq -n --slurpfile projection "${projection_dir}/projection.json" \
        '{projection:$projection[0]}' >"${projection_dir}/capture.json"
      digest="$(cma_step10_sha256_file "${projection_dir}/projection.json")"
      digest="${digest%% *}"
      printf '%s  projection.json\n' "${digest}" >"${projection_dir}/projection.sha256"
      printf '%s\n' "${projection_dir}"
    }
    pre_dir="$(cma_test_make_fence_projection \
      pre fenced-pre-scale 1 old-pod-uid "${source_digest}")"
    pre_digest="$(cma_step10_sha256_file "${pre_dir}/projection.json")"
    pre_digest="${pre_digest%% *}"
    post_dir="$(cma_test_make_fence_projection \
      post fenced-post-scale 0 '' "${pre_digest}")"
    post_digest="$(cma_step10_sha256_file "${post_dir}/projection.json")"
    post_digest="${post_digest%% *}"
    reopen_dir="$(cma_test_make_fence_projection \
      reopen fenced-pre-reopen 1 new-pod-uid "${post_digest}")"
    cma_step10_validate_fence_projection_receipt \
      "${pre_dir}" fenced-pre-scale "${source_digest}"
    cma_step10_validate_fence_projection_receipt \
      "${post_dir}" fenced-post-scale "${pre_digest}"
    cma_step10_validate_fence_projection_receipt \
      "${reopen_dir}" fenced-pre-reopen "${post_digest}"
    for mutation in post-replicas post-pod post-uid pre-pod \
        post-predecessor reopen-predecessor extra-checksum-line; do
      source_dir="${post_dir}"
      expected_stage=fenced-post-scale
      expected_predecessor="${pre_digest}"
      if [[ "${mutation}" == pre-pod ]]; then
        source_dir="${pre_dir}"
        expected_stage=fenced-pre-scale
        expected_predecessor="${source_digest}"
      elif [[ "${mutation}" == reopen-predecessor ]]; then
        source_dir="${reopen_dir}"
        expected_stage=fenced-pre-reopen
        expected_predecessor="${post_digest}"
      fi
      cp -R "${source_dir}" "${case_dir}/${mutation}"
      case "${mutation}" in
        post-replicas)
          jq '.graph.apiWorkload.deployment.replicas = 1' \
            "${case_dir}/${mutation}/projection.json" >"${case_dir}/${mutation}/next"
          ;;
        post-pod)
          jq '.graph.apiWorkload.pods = [{name:"old-api-pod",uid:"old-pod-uid"}]' \
            "${case_dir}/${mutation}/projection.json" >"${case_dir}/${mutation}/next"
          ;;
        post-uid)
          jq '.graph.apiWorkload.deployment.uid = "replacement"' \
            "${case_dir}/${mutation}/projection.json" >"${case_dir}/${mutation}/next"
          ;;
        pre-pod)
          jq '.graph.apiWorkload.pods[0].uid = "replacement"' \
            "${case_dir}/${mutation}/projection.json" >"${case_dir}/${mutation}/next"
          ;;
        post-predecessor | reopen-predecessor)
          jq '.lifecycle.predecessorSha256 = ("f" * 64)' \
            "${case_dir}/${mutation}/projection.json" >"${case_dir}/${mutation}/next"
          ;;
        extra-checksum-line)
          cp "${case_dir}/${mutation}/projection.json" \
            "${case_dir}/${mutation}/next"
          ;;
      esac
      mv "${case_dir}/${mutation}/next" "${case_dir}/${mutation}/projection.json"
      jq -n --slurpfile projection "${case_dir}/${mutation}/projection.json" \
        '{projection:$projection[0]}' >"${case_dir}/${mutation}/capture.json"
      digest="$(cma_step10_sha256_file "${case_dir}/${mutation}/projection.json")"
      digest="${digest%% *}"
      printf '%s  projection.json\n' "${digest}" >"${case_dir}/${mutation}/projection.sha256"
      if [[ "${mutation}" == extra-checksum-line ]]; then
        printf '%s  projection.json\n' "${digest}" \
          >>"${case_dir}/${mutation}/projection.sha256"
      fi
      if cma_step10_validate_fence_projection_receipt \
          "${case_dir}/${mutation}" "${expected_stage}" \
          "${expected_predecessor}" \
          >/dev/null 2>&1; then
        echo "fence projection accepted invalid workload chain: ${mutation}" >&2
        return 1
      fi
    done

    eval "$(declare -f cma_step10_validate_fence_projection_receipt |
      sed '1s/cma_step10_validate_fence_projection_receipt/cma_test_real_validate_fence_projection_receipt/')"
    cma_test_replace_projection_bundle() {
      local projection_dir="$1"
      local digest

      jq '.lifecycle.predecessorSha256 = ("d" * 64)' \
        "${projection_dir}/projection.json" >"${projection_dir}/next"
      mv "${projection_dir}/next" "${projection_dir}/projection.json"
      jq -n --slurpfile projection "${projection_dir}/projection.json" \
        '{projection:$projection[0]}' >"${projection_dir}/capture.json"
      digest="$(cma_step10_sha256_file "${projection_dir}/projection.json")"
      digest="${digest%% *}"
      printf '%s  projection.json\n' "${digest}" \
        >"${projection_dir}/projection.sha256"
    }
    cma_step10_validate_fence_projection_receipt() {
      cma_test_real_validate_fence_projection_receipt "$@" || return 1
      case "${cma_test_boundary_case:-}" in
        post-semantic-replacement | reopen-semantic-replacement)
          cma_test_replace_projection_bundle "$1"
          ;;
      esac
    }
    cma_step10_projection_matches_final_workload() {
      if [[ "${cma_test_boundary_case:-}" == final-semantic-replacement ]]; then
        cma_test_replace_projection_bundle "${1%/projection.json}"
      fi
    }
    cma_step10_projection_check_live() {
      case "${cma_test_boundary_case:-}" in
        post-live-replacement | reopen-live-replacement | final-live-replacement)
          cma_test_replace_projection_bundle "${1%/projection.json}"
          ;;
      esac
    }
    for cma_test_boundary_case in \
        post-semantic-replacement post-live-replacement \
        reopen-semantic-replacement reopen-live-replacement; do
      source_dir="${post_dir}"
      expected_stage=fenced-post-scale
      expected_predecessor="${pre_digest}"
      if [[ "${cma_test_boundary_case}" == reopen-* ]]; then
        source_dir="${reopen_dir}"
        expected_stage=fenced-pre-reopen
        expected_predecessor="${post_digest}"
      fi
      projection_dir="${case_dir}/${cma_test_boundary_case}"
      cp -R "${source_dir}" "${projection_dir}"
      if cma_step10_validate_fence_projection \
          "${projection_dir}" "${expected_stage}" "${expected_predecessor}" \
          >"${projection_dir}.stdout" 2>"${projection_dir}.stderr"; then
        echo "fence projection replacement crossed a validation boundary: ${cma_test_boundary_case}" >&2
        return 1
      fi
      if [[ -s "${projection_dir}.stdout" ]]; then
        echo "failed fence projection validation acknowledged a digest: ${cma_test_boundary_case}" >&2
        return 1
      fi
    done

    reopen_digest="$(cma_step10_sha256_file "${reopen_dir}/projection.json")"
    reopen_digest="${reopen_digest%% *}"
    final_source_dir="${case_dir}/final-source"
    cp -R "${reopen_dir}" "${final_source_dir}"
    jq --arg predecessor "${reopen_digest}" \
      '.mode = "source" |
       .lifecycle = {stage:"final-reopen",predecessorSha256:$predecessor}' \
      "${final_source_dir}/projection.json" >"${final_source_dir}/next"
    mv "${final_source_dir}/next" "${final_source_dir}/projection.json"
    jq -n --slurpfile projection "${final_source_dir}/projection.json" \
      '{projection:$projection[0]}' >"${final_source_dir}/capture.json"
    digest="$(cma_step10_sha256_file "${final_source_dir}/projection.json")"
    digest="${digest%% *}"
    printf '%s  projection.json\n' "${digest}" \
      >"${final_source_dir}/projection.sha256"
    cma_fenced_projection_sha256="${reopen_digest}"
    for cma_test_boundary_case in \
        final-semantic-replacement final-live-replacement; do
      projection_dir="${case_dir}/${cma_test_boundary_case}"
      cp -R "${final_source_dir}" "${projection_dir}"
      if cma_step10_validate_source_projection "${projection_dir}" \
          >"${projection_dir}.stdout" 2>"${projection_dir}.stderr"; then
        echo "source projection replacement crossed a validation boundary: ${cma_test_boundary_case}" >&2
        return 1
      fi
      if [[ -s "${projection_dir}.stdout" ]]; then
        echo "failed source projection validation acknowledged a digest: ${cma_test_boundary_case}" >&2
        return 1
      fi
    done
  )
}

cma_test_intermediate_projection_external_digest() {
  local case_dir="${cma_test_tmp}/intermediate-projection-digest"
  local predecessor_dir="${case_dir}/predecessor"
  local expected_digest replacement_digest

  mkdir -p "${case_dir}/parent" "${predecessor_dir}"
  jq -n '{schemaVersion:1,mode:"fenced",
    lifecycle:{stage:"fenced-post-scale",predecessorSha256:("a" * 64)}}' \
    >"${predecessor_dir}/projection.json"
  expected_digest="$(cma_step10_sha256_file "${predecessor_dir}/projection.json")"
  expected_digest="${expected_digest%% *}"
  printf '%s  projection.json\n' "${expected_digest}" \
    >"${predecessor_dir}/projection.sha256"
  jq '.lifecycle.predecessorSha256 = ("b" * 64)' \
    "${predecessor_dir}/projection.json" >"${predecessor_dir}/next"
  mv "${predecessor_dir}/next" "${predecessor_dir}/projection.json"
  replacement_digest="$(cma_step10_sha256_file "${predecessor_dir}/projection.json")"
  replacement_digest="${replacement_digest%% *}"
  printf '%s  projection.json\n' "${replacement_digest}" \
    >"${predecessor_dir}/projection.sha256"
  (
    cma_step10_validate_fence_projection_receipt() {
      : >"${case_dir}/semantic-validation-reached"
      return 0
    }
    if cma_step10_projection_capture "${case_dir}/parent" candidate fenced \
        fenced-pre-reopen "${predecessor_dir}/projection.json" \
        fenced-post-scale "${expected_digest}" \
        aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa \
        >/dev/null 2>&1; then
      echo "coherently replaced predecessor bypassed its external digest" >&2
      return 1
    fi
  )
  [[ ! -e "${case_dir}/semantic-validation-reached" ]] || {
    echo "coherently replaced predecessor reached semantic validation" >&2
    return 1
  }
}

cma_test_run_case initial-source 1 1 0
cma_test_run_case initial-unknown 1 1 0
cma_test_run_case initial-read-failure 1 1 0
cma_test_run_case pod-failure 1 1 0
cma_test_run_case preopen-workload-replacement 1 0 0
cma_test_run_case pre-reopen-semantic-replacement 1 1 0
cma_test_run_case pre-reopen-live-replacement 1 1 0
cma_test_run_case token-crlf 1 0 0
cma_test_run_case confirmation-mismatch 1 0 0
cma_test_run_case initial-signal 1 1 0
cma_test_run_case initial-quit 1 1 0
cma_test_run_case initial-fence-failure 1 1 0
cma_test_run_case initial-route-failure 1 0 0
cma_test_run_case patch-failure-source 1 1 0
cma_test_run_case reopen-proof-failure 1 1 0
cma_test_run_case post-route-failure 1 1 2
cma_test_run_case post-endpoint-failure 1 1 2
cma_test_run_case final-projection-workload-replacement 1 1 2
cma_test_run_case final-source-semantic-replacement 1 1 2
cma_test_run_case final-source-live-replacement 1 1 2
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
cma_test_prove_fence_case success 0 1
cma_test_prove_fence_case replicas-nonzero 1 0
cma_test_prove_fence_case old-pod-present 1 0
cma_test_prove_fence_case labelled-pod-present 1 0
cma_test_prove_fence_case deployment-uid-drift 1 0
cma_test_prove_fence_case projection-semantic-replacement 1 1
cma_test_prove_fence_case projection-live-replacement 1 1
cma_test_fence_receipt_fsync_recovery
cma_test_fence_projection_workload_chain
cma_test_intermediate_projection_external_digest
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
       admissionReceipt:{receiptSha256:"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd"},
       graph:{apiWorkload:{deployment:{generation:1,name:"cma-api",namespace:"chrono-bot",
         replicas:1,resourceVersion:"1",uid:"deployment-uid"},
         pods:[{name:"old-api-pod",uid:"old-pod-uid"}]}}}}
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
  local expected_digest="$3"
  local expected_predecessor="$4"
  if (
      cma_step10_projection_emit() { cma_test_projection_emit_fixture "$@"; }
      cma_step10_require_projection_file "${projection_file}" \
        "${expected_digest}" "${expected_predecessor}"
    ) >/dev/null 2>&1; then
    echo "invalid adopted projection bundle was accepted: ${label}" >&2
    exit 1
  fi
}

projection_fixture_root="${cma_test_tmp}/projection-custody"
mkdir "${projection_fixture_root}"
valid_projection_dir="${projection_fixture_root}/valid"
cma_test_make_projection_bundle "${valid_projection_dir}" adopted
valid_projection_digest="$(cma_step10_projection_digest \
  "${valid_projection_dir}/projection.json")"
valid_projection_predecessor="$(jq -er .lifecycle.predecessorSha256 \
  "${valid_projection_dir}/projection.json")"
(
  cma_step10_projection_emit() { cma_test_projection_emit_fixture "$@"; }
  cma_step10_require_projection_file "${valid_projection_dir}/projection.json" \
    "${valid_projection_digest}" "${valid_projection_predecessor}"
)

source_stage_dir="${projection_fixture_root}/source-stage"
cma_test_make_projection_bundle "${source_stage_dir}" source
cma_test_projection_rejects "${source_stage_dir}/projection.json" source-stage \
  "${valid_projection_digest}" "${valid_projection_predecessor}"

bad_digest_dir="${projection_fixture_root}/bad-digest"
cp -R "${valid_projection_dir}" "${bad_digest_dir}"
printf '%064d  projection.json\n' 0 >"${bad_digest_dir}/projection.sha256"
cma_test_projection_rejects "${bad_digest_dir}/projection.json" sibling-digest \
  "${valid_projection_digest}" "${valid_projection_predecessor}"

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
cma_test_projection_rejects "${bad_regeneration_dir}/projection.json" regeneration \
  "${valid_projection_digest}" "${valid_projection_predecessor}"

coherent_replacement_dir="${projection_fixture_root}/coherent-replacement"
cp -R "${valid_projection_dir}" "${coherent_replacement_dir}"
jq '.projection.captureSha256 = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee"' \
  "${coherent_replacement_dir}/capture.json" >"${coherent_replacement_dir}/next"
mv "${coherent_replacement_dir}/next" "${coherent_replacement_dir}/capture.json"
cma_test_projection_emit_fixture "${coherent_replacement_dir}/capture.json" \
  "${coherent_replacement_dir}/projection.json"
coherent_replacement_digest="$(cma_step10_sha256_file \
  "${coherent_replacement_dir}/projection.json")"
coherent_replacement_digest="${coherent_replacement_digest%% *}"
printf '%s  projection.json\n' "${coherent_replacement_digest}" \
  >"${coherent_replacement_dir}/projection.sha256"
cma_test_projection_rejects "${coherent_replacement_dir}/projection.json" \
  coherent-replacement "${valid_projection_digest}" "${valid_projection_predecessor}"

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
      admissionReceipt:$source[0].admissionReceipt},
      workload:$source[0].graph.apiWorkload}
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
cma_test_receipt_rejects '.workload.deployment.uid = "replacement"' workload

prepatch_log="${projection_fixture_root}/prepatch.log"
: >"${prepatch_log}"
if (
    export CMA_GATEWAY_CONFIRM=fence-v1.2.3-traffic
    export CMA_GATEWAY_RELEASE_REVISION=0123456789abcdef0123456789abcdef01234567
    export CMA_GATEWAY_EXCLUSIVE_CHANGE_CONFIRM=sole-spec-writer
    export CMA_GATEWAY_SOURCE_PROJECTION="${source_stage_dir}/projection.json"
    cma_step10_require_production_mutation_enabled() { return 0; }
    cma_step10_require_privileged_bash() { return 0; }
    cma_step10_require_trusted_launcher() { return 0; }
    cma_step10_reject_unexpected_token_fd() { return 0; }
    cma_step10_validate_admission_reservation() { return 0; }
    cma_step10_require_checkout() { CMA_GATEWAY_RELEASE_TAG=v1.2.3; }
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

workload_fixture_root="${cma_test_tmp}/workload-identity"
mkdir "${workload_fixture_root}"
cma_final_pod=api-pod
cma_final_pod_uid=pod-uid
cma_final_pod_ip=10.0.0.8
cma_final_deployment_uid=deployment-uid
cma_final_replica_set=cma-api-abc123
cma_final_replica_set_uid=replica-set-uid
cma_final_pod_image=us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api:develop-e03b5e7@sha256:a7d371277d88e44c8cd687c539fb0523e07a9ef12314880de3c4b39887cf5abe
cma_final_pod_image_id=us-central1-docker.pkg.dev/aelf-shared-k8s-prod/aelf-prod-registry/cma-api@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa

jq -n \
  --arg image "${cma_final_pod_image}" \
  --arg image_id "${cma_final_pod_image_id}" '{
    apiVersion:"v1",kind:"Pod",
    metadata:{name:"api-pod",namespace:"chrono-bot",uid:"pod-uid",ownerReferences:[{
      apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,
      kind:"ReplicaSet",name:"cma-api-abc123",uid:"replica-set-uid"
    }]},
    spec:{containers:[{name:"api",image:$image}]},
    status:{phase:"Running",podIP:"10.0.0.8",conditions:[{type:"Ready",status:"True"}],
      containerStatuses:[{name:"api",image:$image,imageID:$image_id,ready:true,
        started:true,restartCount:0,state:{running:{startedAt:"2026-01-01T00:00:00Z"}}}]}
  }' >"${workload_fixture_root}/pod.original.json"
jq -n --arg image "${cma_final_pod_image}" '{
    apiVersion:"apps/v1",kind:"ReplicaSet",
    metadata:{name:"cma-api-abc123",namespace:"chrono-bot",uid:"replica-set-uid",
      generation:1,labels:{"app.kubernetes.io/name":"cma-api",
        "app.kubernetes.io/part-of":"cma","pod-template-hash":"abc123"},
      ownerReferences:[{apiVersion:"apps/v1",blockOwnerDeletion:true,controller:true,
        kind:"Deployment",name:"cma-api",uid:"deployment-uid"}]},
    spec:{replicas:1,selector:{matchLabels:{"app.kubernetes.io/name":"cma-api",
      "pod-template-hash":"abc123"}},template:{metadata:{labels:{
        "app.kubernetes.io/name":"cma-api","app.kubernetes.io/part-of":"cma",
        "pod-template-hash":"abc123"}},spec:{containers:[{name:"api",image:$image}]}}},
    status:{observedGeneration:1,replicas:1,fullyLabeledReplicas:1,
      readyReplicas:1,availableReplicas:1}
  }' >"${workload_fixture_root}/replicaset.original.json"
jq -n --arg image "${cma_final_pod_image}" '{
    apiVersion:"apps/v1",kind:"Deployment",
    metadata:{name:"cma-api",namespace:"chrono-bot",uid:"deployment-uid",generation:1},
    spec:{replicas:1,strategy:{type:"Recreate"},
      selector:{matchLabels:{"app.kubernetes.io/name":"cma-api"}},
      template:{metadata:{labels:{"app.kubernetes.io/name":"cma-api",
        "app.kubernetes.io/part-of":"cma"}},
        spec:{containers:[{name:"api",image:$image}]}}},
    status:{observedGeneration:1,replicas:1,updatedReplicas:1,readyReplicas:1,
      availableReplicas:1}
  }' >"${workload_fixture_root}/deployment.original.json"
cp "${workload_fixture_root}/pod.original.json" "${workload_fixture_root}/pod.json"
cp "${workload_fixture_root}/replicaset.original.json" \
  "${workload_fixture_root}/replicaset.json"
cp "${workload_fixture_root}/deployment.original.json" \
  "${workload_fixture_root}/deployment.json"

cma_step10_kubectl() {
  case "$*" in
    'get pod api-pod -o json') cat "${workload_fixture_root}/pod.json" ;;
    'get replicaset cma-api-abc123 -o json') cat "${workload_fixture_root}/replicaset.json" ;;
    'get deployment cma-api -o json') cat "${workload_fixture_root}/deployment.json" ;;
    *) return 1 ;;
  esac
}
cma_step10_validate_workload_contract() {
  jq -e '
    (.spec.template.spec.containers[0] |
      has("command") == false and has("args") == false and has("env") == false)
  ' "$1" >/dev/null &&
    jq -e '
      (.spec.template.spec.containers[0] |
        has("command") == false and has("args") == false and has("env") == false)
    ' "$2" >/dev/null &&
    jq -e '
      (.spec.containers[0] |
        has("command") == false and has("args") == false and has("env") == false)
    ' "$3" >/dev/null
}

cma_final_workload_is_exact

sealed_workload_projection="${workload_fixture_root}/projection.json"
jq -n \
  --slurpfile deployment "${workload_fixture_root}/deployment.original.json" \
  --slurpfile replica_set_object "${workload_fixture_root}/replicaset.original.json" \
  --slurpfile pod "${workload_fixture_root}/pod.original.json" \
  --arg deployment_uid "${cma_final_deployment_uid}" \
  --arg replica_set "${cma_final_replica_set}" \
  --arg replica_set_uid "${cma_final_replica_set_uid}" \
  --arg pod_name "${cma_final_pod}" \
  --arg pod_uid "${cma_final_pod_uid}" \
  --arg pod_ip "${cma_final_pod_ip}" \
  --arg image "${cma_final_pod_image}" \
  --arg image_id "${cma_final_pod_image_id}" '{
    graph:{apiWorkload:{verified:{
      deployment:{uid:$deployment_uid},
      replicaSet:{name:$replica_set,uid:$replica_set_uid},
      pod:{name:$pod_name,uid:$pod_uid,ip:$pod_ip,image:$image,imageID:$image_id}
    }}},
    fullCapture:{kubernetes:{
      apiDeploymentWorkload:$deployment[0],
      apiReplicaSetWorkload:$replica_set_object[0],
      apiPodWorkload:$pod[0]
    }}
  }' >"${sealed_workload_projection}"
cma_step10_projection_matches_final_workload "${sealed_workload_projection}"

coherent_replacement_projection="${workload_fixture_root}/projection-replacement.json"
jq '
  .fullCapture.kubernetes.apiDeploymentWorkload.metadata.uid = "replacement-deployment" |
  .fullCapture.kubernetes.apiReplicaSetWorkload.metadata.name = "replacement-rs" |
  .fullCapture.kubernetes.apiReplicaSetWorkload.metadata.uid = "replacement-rs-uid" |
  .fullCapture.kubernetes.apiReplicaSetWorkload.metadata.ownerReferences[0].uid =
    "replacement-deployment" |
  .fullCapture.kubernetes.apiPodWorkload.metadata.name = "replacement-pod" |
  .fullCapture.kubernetes.apiPodWorkload.metadata.uid = "replacement-pod-uid" |
  .fullCapture.kubernetes.apiPodWorkload.metadata.ownerReferences[0].name =
    "replacement-rs" |
  .fullCapture.kubernetes.apiPodWorkload.metadata.ownerReferences[0].uid =
    "replacement-rs-uid" |
  .graph.apiWorkload.verified.deployment.uid = "replacement-deployment" |
  .graph.apiWorkload.verified.replicaSet.name = "replacement-rs" |
  .graph.apiWorkload.verified.replicaSet.uid = "replacement-rs-uid" |
  .graph.apiWorkload.verified.pod.name = "replacement-pod" |
  .graph.apiWorkload.verified.pod.uid = "replacement-pod-uid"
' "${sealed_workload_projection}" >"${coherent_replacement_projection}"
if cma_step10_projection_matches_final_workload \
    "${coherent_replacement_projection}" >/dev/null 2>&1; then
  echo "sealed replacement workload bypassed the retained step-9 identities" >&2
  exit 1
fi

cma_test_workload_rejects() {
  local resource="$1"
  local mutation="$2"
  local label="$3"
  jq "${mutation}" "${workload_fixture_root}/${resource}.original.json" \
    >"${workload_fixture_root}/${resource}.json"
  if cma_final_workload_is_exact >/dev/null 2>&1; then
    echo "final workload gate accepted ${label}" >&2
    exit 1
  fi
  cp "${workload_fixture_root}/${resource}.original.json" \
    "${workload_fixture_root}/${resource}.json"
}

cma_test_workload_rejects pod \
  '.metadata.ownerReferences[0].uid = "replacement-rs"' \
  'a Pod outside the retained ReplicaSet'
cma_test_workload_rejects replicaset \
  '.metadata.ownerReferences[0].uid = "replacement-deployment"' \
  'a ReplicaSet outside the retained Deployment'
cma_test_workload_rejects pod \
  '.spec.containers[0].image = "example.invalid/cma-api:other"' \
  'a Pod spec image outside the reviewed manifest'
cma_test_workload_rejects pod \
  '.status.containerStatuses[0].imageID = "example.invalid/cma-api@sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"' \
  'an unexpected immutable runtime imageID'
cma_test_workload_rejects deployment \
  '.spec.template.spec.containers[0].image = "example.invalid/cma-api:other"' \
  'a live Deployment image outside the reviewed manifest'
cma_test_workload_rejects deployment \
  '.spec.template.spec.containers[0].command = ["/unexpected"]' \
  'an admission-mutated Deployment command'
cma_test_workload_rejects replicaset \
  '.spec.template.spec.containers[0].args = ["--unexpected"]' \
  'an admission-mutated ReplicaSet argument list'
cma_test_workload_rejects pod \
  '.spec.containers[0].env = [{name:"UNEXPECTED",value:"1"}]' \
  'an admission-mutated Pod environment'

printf '%s\n' "CMA Trigger Gateway traffic-restoration tests passed (103 cases)"
