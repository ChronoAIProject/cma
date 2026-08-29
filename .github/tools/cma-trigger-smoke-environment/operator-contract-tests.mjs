#!/usr/bin/env node

import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = dirname(fileURLToPath(import.meta.url));
const paths = {
  operator: join(root, "operator.mjs"),
  rust: join(root, "sealed-box", "src", "main.rs"),
  cargo: join(root, "sealed-box", "Cargo.toml"),
  lock: join(root, "sealed-box", "Cargo.lock"),
  config: join(root, "config.example.json"),
};

function required(source, needle, message) {
  assert.ok(source.includes(needle), message);
}

function forbidden(source, needle, message) {
  assert.ok(!source.includes(needle), message);
}

function contract({ operator, rust, cargo, lock, config }) {
  required(
    operator,
    '"86741372-764f-4423-8687-95bebece158c"',
    "canonical GitHub catalog ID drifted",
  );
  required(
    operator,
    "`${NYXID_BASE_URL}/api/v1/proxy/${GITHUB_CATALOG_SERVICE_ID}`",
    "GitHub REST no longer uses the catalog-ID route",
  );
  required(
    operator,
    'search.set("_nyxid_via", userServiceId);',
    "explicit UserService selector disappeared",
  );
  required(
    operator,
    '"CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE"',
    "file-based bearer input disappeared",
  );
  required(
    operator,
    '"CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN"',
    "environment bearer input disappeared",
  );
  forbidden(operator, '"--token"', "bearer became an argv option");
  required(
    operator,
    '"CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_"',
    "nonce-derived ephemeral handoff prefix drifted",
  );
  required(
    operator,
    'const RUNTIME_SERVICE_SECRET = "CMA_TRIGGER_SMOKE_SERVICE_ID";',
    "runtime UserService secret drifted",
  );
  required(
    operator,
    'const KUBECONFIG_SECRET = "CMA_TRIGGER_SMOKE_KUBECONFIG_B64";',
    "stable kubeconfig secret drifted",
  );
  required(
    operator,
    'const KIND_CA_MATERIAL_NAME = "CMA_TRIGGER_SMOKE_KIND_CA_" + "KEY_B64";',
    "stable Kind CA secret drifted",
  );
  required(
    operator,
    'const SETUP_CONFIRMATION = "setup-reviewed-cma-trigger-staging";',
    "setup confirmation drifted",
  );
  required(
    operator,
    'const RUN_CONFIRMATION = "dispatch-reviewed-cma-trigger-staging";',
    "dispatch confirmation drifted",
  );
  required(
    operator,
    'const CLEANUP_CONFIRMATION = "cleanup-reviewed-cma-trigger-staging";',
    "cleanup confirmation drifted",
  );
  required(
    operator,
    "if (state.caller.service_id === provisioningServiceId)",
    "runtime/provisioning authority separation disappeared",
  );
  required(
    operator,
    "exactSet(\n    claims.allowed_service_ids,\n    [state.caller.service_id]",
    "JWT exact UserService binding disappeared",
  );
  required(
    operator,
    '"public_repo",\n  "read:user",\n  "user:email"',
    "least-privilege runtime scope set drifted",
  );
  required(
    operator,
    '"repo",\n  "read:user",\n  "user:email"',
    "bounded staging-administration scope set drifted",
  );
  required(
    operator,
    'for (const signal of ["SIGHUP", "SIGINT", "SIGTERM"])',
    "signal cleanup fence drifted",
  );
  required(
    operator,
    "if (handoffMayExist) {\n      try {\n        await deleteEphemeralHandoff(transport, handoffSecretName);",
    "EXIT/finally handoff cleanup disappeared",
  );
  required(
    operator,
    "if (!isManagedSecretName(name))",
    "closed secret mutation allowlist disappeared",
  );
  required(
    operator,
    "dispatch_nonce: dispatchNonce",
    "one-run dispatch nonce input disappeared",
  );
  required(
    operator,
    "handoff_secret_name: handoffSecretName",
    "nonce-bound handoff input disappeared",
  );
  required(
    operator,
    "run.display_title === dispatchRunTitle(dispatchNonce)",
    "exact run-name correlation disappeared",
  );
  required(
    operator,
    "requireEphemeralSecretSet(beforeDispatch.inventory.secrets, [",
    "cross-process ephemeral lease fence disappeared",
  );
  required(
    operator,
    "the dispatch must not be retried automatically",
    "at-most-once dispatch recovery fence disappeared",
  );
  required(
    operator,
    "expected_head_sha: headSha",
    "immutable workflow head input disappeared",
  );
  required(
    operator,
    "requireApprovedHead(beforeDispatch, approvedHeadSha);",
    "post-secret approved-head fence disappeared",
  );
  required(
    operator,
    'completed.conclusion !== "success"',
    "failed provider-smoke runs stopped failing the operator",
  );
  required(
    operator,
    "write is ambiguous and must be explicitly replayed",
    "ambiguous secret write stopped failing closed",
  );
  required(
    operator,
    "approved staging preflight is future-dated",
    "future-dated plan refusal disappeared",
  );
  required(
    operator,
    '"approved plan",\n      MAX_JSON_BYTES',
    "approved plan secure-file custody disappeared",
  );
  required(
    operator,
    "MAX_GITHUB_SECRET_PLAINTEXT_BYTES = 48 * 1024",
    "GitHub secret plaintext bound drifted",
  );
  forbidden(
    operator,
    "absence_reconciled_after_delete_replay",
    "ambiguous handoff deletion regained an unsafe automatic replay",
  );
  required(
    operator,
    '"https://127.0.0.1:6443"',
    "fixed Kind API endpoint drifted",
  );
  required(
    operator,
    'observer_cn: "cma-trigger-smoke-observer"',
    "observer certificate identity drifted",
  );
  required(
    operator,
    'caCertificateText,\n      "Basic Constraints",\n      "CA:TRUE"',
    "Kind CA basic-constraints verification disappeared",
  );
  required(
    operator,
    'observerCertificateText,\n      "Basic Constraints",\n      "CA:FALSE"',
    "observer basic-constraints verification disappeared",
  );
  required(
    operator,
    'observerCertificateText,\n      "Extended Key Usage",\n      "TLS Web Client Authentication"',
    "observer client-auth EKU verification disappeared",
  );
  required(
    rust,
    'const PUBLIC_KEY_ENV: &str = "CMA_TRIGGER_SEAL_PUBLIC_KEY_B64";',
    "sealed-box public-key input drifted",
  );
  required(
    rust,
    "const MAX_PLAINTEXT_BYTES: u64 = 48 * 1024;",
    "sealed-box GitHub plaintext bound drifted",
  );
  required(
    rust,
    ".take(MAX_PLAINTEXT_BYTES + 1)",
    "sealed-box plaintext bound disappeared",
  );
  required(
    rust,
    "io::stdin()",
    "sealed-box plaintext no longer comes from stdin",
  );
  forbidden(
    rust,
    "plaintext_file",
    "sealed-box introduced an unaudited plaintext path",
  );
  required(
    rust,
    "ciphertext.len() != plaintext.len() + 48",
    "libsodium-compatible overhead self-test disappeared",
  );
  required(
    cargo,
    'base64 = "=0.22.1"',
    "base64 dependency is not exactly pinned",
  );
  required(
    cargo,
    'crypto_box = { version = "=0.9.1", features = ["seal"] }',
    "crypto_box dependency drifted",
  );
  required(
    cargo,
    'zeroize = "=1.8.1"',
    "zeroize dependency is not exactly pinned",
  );
  required(
    lock,
    'name = "crypto_box"\nversion = "0.9.1"',
    "Cargo.lock crypto_box pin drifted",
  );
  assert.deepEqual(Object.keys(JSON.parse(config)).sort(), [
    "dispatch",
    "environment",
    "nyxid_base_url",
    "provisioning_user_service_id",
    "schema_version",
    "static_variables",
  ]);
}

const original = {
  operator: await readFile(paths.operator, "utf8"),
  rust: await readFile(paths.rust, "utf8"),
  cargo: await readFile(paths.cargo, "utf8"),
  lock: await readFile(paths.lock, "utf8"),
  config: await readFile(paths.config, "utf8"),
};

contract(original);

const mutations = [
  [
    "operator",
    "86741372-764f-4423-8687-95bebece158c",
    "96741372-764f-4423-8687-95bebece158c",
  ],
  [
    "operator",
    'search.set("_nyxid_via", userServiceId);',
    'search.set("via", userServiceId);',
  ],
  ["operator", '"--handoff"', '"--token"'],
  [
    "operator",
    "CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_",
    "CMA_TRIGGER_SMOKE_ACCESS_TOKEN",
  ],
  ["operator", "setup-reviewed-cma-trigger-staging", "setup-staging"],
  ["operator", "dispatch-reviewed-cma-trigger-staging", "dispatch-staging"],
  ["operator", "cleanup-reviewed-cma-trigger-staging", "cleanup-staging"],
  [
    "operator",
    "state.caller.service_id === provisioningServiceId",
    "state.caller.service_id !== provisioningServiceId",
  ],
  ["operator", '["SIGHUP", "SIGINT", "SIGTERM"]', '["SIGTERM"]'],
  [
    "operator",
    "the dispatch must not be retried automatically",
    "retry dispatch",
  ],
  [
    "operator",
    "expected_head_sha: headSha",
    "expected_head_sha: config.dispatch.ref",
  ],
  ["operator", "dispatch_nonce: dispatchNonce", "dispatch_nonce: headSha"],
  [
    "operator",
    "run.display_title === dispatchRunTitle(dispatchNonce)",
    "run.display_title !== dispatchRunTitle(dispatchNonce)",
  ],
  [
    "operator",
    "requireApprovedHead(beforeDispatch, approvedHeadSha);",
    "requireApprovedHead(beforeDispatch, beforeDispatch.branch.head.sha);",
  ],
  [
    "operator",
    'completed.conclusion !== "success"',
    'completed.conclusion !== "failure"',
  ],
  [
    "operator",
    "write is ambiguous and must be explicitly replayed",
    "secret will retry",
  ],
  [
    "operator",
    "MAX_GITHUB_SECRET_PLAINTEXT_BYTES = 48 * 1024",
    "MAX_GITHUB_SECRET_PLAINTEXT_BYTES = 256 * 1024",
  ],
  ["operator", "https://127.0.0.1:6443", "https://127.0.0.1:7443"],
  [
    "operator",
    'caCertificateText,\n      "Basic Constraints",\n      "CA:TRUE"',
    'caCertificateText,\n      "Basic Constraints",\n      "CA:FALSE"',
  ],
  [
    "operator",
    'observerCertificateText,\n      "Basic Constraints",\n      "CA:FALSE"',
    'observerCertificateText,\n      "Basic Constraints",\n      "CA:TRUE"',
  ],
  [
    "operator",
    'observerCertificateText,\n      "Extended Key Usage",\n      "TLS Web Client Authentication"',
    'observerCertificateText,\n      "Extended Key Usage",\n      "TLS Web Server Authentication"',
  ],
  ["rust", ".take(MAX_PLAINTEXT_BYTES + 1)", ".take(u64::MAX)"],
  [
    "rust",
    "MAX_PLAINTEXT_BYTES: u64 = 48 * 1024",
    "MAX_PLAINTEXT_BYTES: u64 = 256 * 1024",
  ],
  ["rust", "io::stdin()", "io::empty()"],
  ["rust", "plaintext.len() + 48", "plaintext.len() + 16"],
  ["cargo", '=0.9.1", features', '=0.9.0", features'],
  ["cargo", 'zeroize = "=1.8.1"', 'zeroize = "1"'],
];

for (const [file, before, after] of mutations) {
  assert.ok(
    original[file].includes(before),
    `mutation target missing: ${file}:${before}`,
  );
  const mutated = {
    ...original,
    [file]: original[file].split(before).join(after),
  };
  assert.throws(
    () => contract(mutated),
    `contract admitted mutation: ${file}:${before}`,
  );
}

process.stdout.write(
  `${JSON.stringify({ status: "operator_contract_passed", mutations: mutations.length })}\n`,
);
