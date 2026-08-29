#!/usr/bin/env bash
# shellcheck disable=SC2317

set -euo pipefail
umask 077

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
# shellcheck source=.github/scripts/cma-trigger-gateway-projection.sh
source "${SCRIPT_DIR}/cma-trigger-gateway-projection.sh"

TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "${TEST_ROOT}"' EXIT

cma_projection_capture_state() {
  local state_dir="$1"
  local purpose="$2"
  [[ "${purpose}" == review ]] || return 1
  mkdir -m 700 "${state_dir}"
  jq -n '{
    routeMode:"legacy",
    kubernetes:{admissionPolicy:null,admissionBinding:null},
    compute:{fixture:"stable-live-behavior"}
  }' > "${state_dir}/state.json"
}

cma_projection_derive_policy() {
  local capture="$1"
  local output="$2"
  jq -e '
    .schemaVersion == 1 and .mode == "source" and .first == .second and
    .first.kubernetes.admissionPolicy == null and
    .first.kubernetes.admissionBinding == null
  ' "${capture}" >/dev/null || return 1
  jq -n '{
    schemaVersion:1,status:"candidate-requires-maintainer-review",
    sourceEvidenceSha256:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    expected:{fixture:"stable-live-behavior"}
  }' > "${output}"
  chmod 600 "${output}"
}

review_dir="${TEST_ROOT}/review"
mkdir "${TEST_ROOT}/.cma-gateway-review.orphaned-partial"
printf '%s\n' partial > "${TEST_ROOT}/.cma-gateway-review.orphaned-partial/incomplete"
cma_projection_capture_review "${review_dir}" >/dev/null
cma_projection_validate_review_capture "${review_dir}"
jq -e '
  .kind == "CmaGatewayBehaviorPolicyReviewCapture" and .nonConsumable == true and
  .first.kubernetes.admissionPolicy == null and
  .first.kubernetes.admissionBinding == null
' "${review_dir}/review-capture.json" >/dev/null

cp -R "${review_dir}" "${TEST_ROOT}/tampered"
chmod 600 "${TEST_ROOT}/tampered/candidate-policy.json"
jq '.expected.fixture = "tampered"' "${TEST_ROOT}/tampered/candidate-policy.json" \
  > "${TEST_ROOT}/tampered/candidate-policy.next"
mv "${TEST_ROOT}/tampered/candidate-policy.next" \
  "${TEST_ROOT}/tampered/candidate-policy.json"
if cma_projection_validate_review_capture "${TEST_ROOT}/tampered"; then
  echo "tampered review capture unexpectedly validated" >&2
  exit 1
fi

mkdir "${TEST_ROOT}/occupied"
printf '%s\n' retained > "${TEST_ROOT}/occupied/sentinel"
if (cma_projection_capture_review "${TEST_ROOT}/occupied") >/dev/null 2>&1; then
  echo "review capture overwrote an existing destination" >&2
  exit 1
fi
grep -qx retained "${TEST_ROOT}/occupied/sentinel"

if ruby --disable-gems "${SCRIPT_DIR}/cma-trigger-gateway-projection.rb" --emit \
    "${review_dir}/review-capture.json" "${TEST_ROOT}/forbidden-projection.json" \
    >/dev/null 2>&1; then
  echo "review-only evidence was accepted as an adoption projection" >&2
  exit 1
fi
[[ ! -e "${TEST_ROOT}/forbidden-projection.json" ]]

cma_projection_admission_receipt() {
  jq -n '{
    receiptSha256:"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    releaseRevision:"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
    sourceSetSha256:"cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    manifestSha256:"dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd",
    policy:{uid:"policy-uid",generation:1,resourceVersion:"11"},
    binding:{uid:"binding-uid",generation:1,resourceVersion:"12"}
  }'
}
cma_projection_emit() {
  local capture="$1"
  local output="$2"
  jq '{
    schemaVersion:1,mode,lifecycle,
    captureSha256:"eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    behaviorPolicy:{policySha256:"ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
      sourceEvidenceSha256:"1111111111111111111111111111111111111111111111111111111111111111"},
    admissionReceipt,graph:.first,fullCapture:.first
  }' "${capture}" > "${output}"
}
cma_projection_capture_state() {
  local state_dir="$1"
  local fixture=stable
  [[ -z "${CMA_PROJECTION_TEST_STATE_FILE:-}" ]] || \
    fixture="$(sed -n '1p' "${CMA_PROJECTION_TEST_STATE_FILE}")"
  mkdir -m 700 "${state_dir}"
  jq -n --arg fixture "${fixture}" '{
    fixture:$fixture,kubernetes:{
      admissionPolicy:{metadata:{uid:"policy-uid",generation:1,resourceVersion:"11"}},
      admissionBinding:{metadata:{uid:"binding-uid",generation:1,resourceVersion:"12"}}
    }
  }' > "${state_dir}/state.json"
}

predecessor_digest=genesis
for lifecycle_stage in source adopted fenced-pre-scale fenced-post-scale fenced-pre-reopen final-reopen rollback-source; do
  lifecycle_dir="${TEST_ROOT}/${lifecycle_stage}"
  lifecycle_mode=source
  [[ "${lifecycle_stage}" == fenced-* ]] && lifecycle_mode=fenced
  CMA_GATEWAY_PROJECTION_STAGE="${lifecycle_stage}"
  CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="${predecessor_digest}"
  export CMA_GATEWAY_PROJECTION_STAGE CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256
  mkdir "${TEST_ROOT}/.cma-gateway-projection.${lifecycle_stage}.orphan"
  printf '%s\n' partial > \
    "${TEST_ROOT}/.cma-gateway-projection.${lifecycle_stage}.orphan/partial"
  cma_projection_capture "${lifecycle_dir}" "${lifecycle_mode}" >/dev/null
  first_digest="$(cma_projection_sha256_file "${lifecycle_dir}/projection.json")"
  cma_projection_capture "${lifecycle_dir}" "${lifecycle_mode}" >/dev/null
  [[ "$(cma_projection_sha256_file "${lifecycle_dir}/projection.json")" == "${first_digest}" ]]
  predecessor_digest="${first_digest}"
done

eval "$(declare -f cma_projection_sync_published_directory |
  sed '1s/^cma_projection_sync_published_directory /cma_test_real_projection_sync_published_directory /')"
original_projection_sync_definition="$(declare -f cma_projection_sync_published_directory)"
publication_fsync_marker="${TEST_ROOT}/publication-fsync-failed"
cma_projection_sync_published_directory() {
  if [[ "$1" == "${TEST_ROOT}/rename-fsync-recovery" && \
        ! -e "${publication_fsync_marker}" ]]; then
    : > "${publication_fsync_marker}"
    return 1
  fi
  cma_test_real_projection_sync_published_directory "$@"
}
CMA_GATEWAY_PROJECTION_STAGE=fenced-pre-scale
CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="$(jq -er .lifecycle.predecessorSha256 \
  "${TEST_ROOT}/fenced-pre-scale/projection.json")"
export CMA_GATEWAY_PROJECTION_STAGE CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256
if (cma_projection_capture "${TEST_ROOT}/rename-fsync-recovery" fenced) \
    >/dev/null 2>&1; then
  echo "rename-success/parent-fsync failure was accepted as durable publication" >&2
  exit 1
fi
[[ -d "${TEST_ROOT}/rename-fsync-recovery" && -e "${publication_fsync_marker}" ]]
cma_projection_capture "${TEST_ROOT}/rename-fsync-recovery" fenced >/dev/null
eval "${original_projection_sync_definition}"

cp -R "${TEST_ROOT}/fenced-pre-scale" "${TEST_ROOT}/tampered-existing"
CMA_GATEWAY_PROJECTION_STAGE=fenced-pre-scale
CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="$(jq -er .lifecycle.predecessorSha256 \
  "${TEST_ROOT}/tampered-existing/projection.json")"
export CMA_GATEWAY_PROJECTION_STAGE CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256
chmod 600 "${TEST_ROOT}/tampered-existing/projection.json"
jq '.behaviorPolicy.policySha256 = "2222222222222222222222222222222222222222222222222222222222222222"' \
  "${TEST_ROOT}/tampered-existing/projection.json" > "${TEST_ROOT}/tampered-existing/next"
mv "${TEST_ROOT}/tampered-existing/next" "${TEST_ROOT}/tampered-existing/projection.json"
if (cma_projection_capture "${TEST_ROOT}/tampered-existing" fenced) >/dev/null 2>&1; then
  echo "tampered existing projection was reused" >&2
  exit 1
fi

cp "${TEST_ROOT}/fenced-pre-scale/projection.json" "${TEST_ROOT}/policy-tamper.json"
chmod 600 "${TEST_ROOT}/policy-tamper.json"
jq '.behaviorPolicy.sourceEvidenceSha256 = "3333333333333333333333333333333333333333333333333333333333333333"' \
  "${TEST_ROOT}/policy-tamper.json" > "${TEST_ROOT}/policy-tamper.next"
mv "${TEST_ROOT}/policy-tamper.next" "${TEST_ROOT}/policy-tamper.json"
if (cma_projection_check_live "${TEST_ROOT}/policy-tamper.json") >/dev/null 2>&1; then
  echo "behavior-policy-only projection tamper passed live equality" >&2
  exit 1
fi

CMA_PROJECTION_TEST_STATE_FILE="${TEST_ROOT}/race-state"
export CMA_PROJECTION_TEST_STATE_FILE
printf '%s\n' loser > "${CMA_PROJECTION_TEST_STATE_FILE}"
CMA_GATEWAY_PROJECTION_STAGE=fenced-pre-scale
CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256="$(jq -er .lifecycle.predecessorSha256 \
  "${TEST_ROOT}/fenced-pre-scale/projection.json")"
export CMA_GATEWAY_PROJECTION_STAGE CMA_GATEWAY_PROJECTION_PREDECESSOR_SHA256
cma_projection_publish_directory() {
  local source_dir="$1"
  local destination_dir="$2"
  mkdir "${destination_dir}"
  cp "${source_dir}/capture.json" "${destination_dir}/capture.json"
  chmod 600 "${destination_dir}/capture.json"
  jq '.first.fixture = "winner" | .second.fixture = "winner"' \
    "${destination_dir}/capture.json" > "${destination_dir}/capture.next"
  if [[ "${CMA_PROJECTION_TEST_DIFFERENT_LIFECYCLE:-0}" == 1 ]]; then
    jq '.lifecycle.stage = "fenced-post-scale"' \
      "${destination_dir}/capture.next" > "${destination_dir}/capture.different"
    mv "${destination_dir}/capture.different" "${destination_dir}/capture.next"
  fi
  mv "${destination_dir}/capture.next" "${destination_dir}/capture.json"
  cma_projection_emit "${destination_dir}/capture.json" "${destination_dir}/projection.json"
  printf '%s  projection.json\n' \
    "$(cma_projection_sha256_file "${destination_dir}/projection.json")" \
    > "${destination_dir}/projection.sha256"
  chmod 400 "${destination_dir}"/*
  printf '%s\n' winner > "${CMA_PROJECTION_TEST_STATE_FILE}"
  return 1
}
race_output="$(cma_projection_capture "${TEST_ROOT}/race-winner" fenced)"
race_winner_digest="$(cma_projection_sha256_file "${TEST_ROOT}/race-winner/projection.json")"
grep -Fqx "CMA Gateway projection SHA-256: ${race_winner_digest}" <<<"${race_output}"
if find "${TEST_ROOT}" -maxdepth 1 -type d -name '.cma-gateway-projection.*' \
    ! -name '*.orphan' -print | grep -q .; then
  echo "losing projection publisher left its temporary directory" >&2
  exit 1
fi

printf '%s\n' loser > "${CMA_PROJECTION_TEST_STATE_FILE}"
CMA_PROJECTION_TEST_DIFFERENT_LIFECYCLE=1
export CMA_PROJECTION_TEST_DIFFERENT_LIFECYCLE
if (cma_projection_capture "${TEST_ROOT}/race-different-lifecycle" fenced) \
    >/dev/null 2>&1; then
  echo "race winner with a different valid lifecycle was reused" >&2
  exit 1
fi
unset CMA_PROJECTION_TEST_DIFFERENT_LIFECYCLE
unset CMA_PROJECTION_TEST_STATE_FILE

ln -s "${TEST_ROOT}/fenced-pre-scale" "${TEST_ROOT}/symlink-capture"
if (cma_projection_capture "${TEST_ROOT}/symlink-capture" fenced) >/dev/null 2>&1; then
  echo "symlink projection destination was accepted" >&2
  exit 1
fi

printf '%s\n' "CMA Trigger Gateway projection tests passed (16 cases)"
