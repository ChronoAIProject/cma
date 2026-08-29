#!/usr/bin/env node

import { createHash } from "node:crypto";
import { execFile } from "node:child_process";
import { access, readFile, rename, writeFile } from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

const TRIGGER_CONFORMANCE = "https://spec.chrono-ai.fun/cma/1.0/conf/trigger";
const TRIGGER_KERNEL_ID = "codex-trigger-native-v1";
const API_ATTESTATION_NAMESPACE = "cma-trigger-ordinary";
const API_ATTESTATION_IMAGE = "cma-api:ci";
const EXECD_ATTESTATION_IMAGE =
  "opensandbox/execd@sha256:662fbb4c4dcd3e220da888c5e472f45c271eef398f78d9cf402de44d0ec7a0ec";
const RUNTIME_ATTESTATION_NAMESPACE = "cma-trigger-system";
const RUNTIME_ATTESTATION_CONFIGMAP = "cma-trigger-runtime-provenance";
const BATCH_SANDBOX_API_VERSION = "sandbox.opensandbox.io/v1alpha1";
const BATCH_SANDBOX_KIND = "BatchSandbox";
const BATCH_SANDBOX_RESOURCE = "batchsandboxes.sandbox.opensandbox.io";
const BATCH_SANDBOX_LIST_KIND = "BatchSandboxList";
const SANDBOX_LABELS = Object.freeze({
  sandbox_id: "opensandbox.io/id",
  canonical_kernel: "cma/kernel",
  legacy_kernel: "chrono-bot/kernel",
  canonical_session: "cma/session",
  legacy_session: "chrono-bot/session",
  runtime_generation: "cma/runtime-generation",
  runtime_reservation: "cma/runtime-reservation",
});
const SANDBOX_PHASES = Object.freeze([
  "before_launch",
  "live_session",
  "after_stop",
]);
const SANDBOX_ISSUES = new Set([
  "object_present_before_launch",
  "api_version_mismatch",
  "kind_mismatch",
  "namespace_mismatch",
  "name_missing_or_invalid",
  "uid_missing_or_invalid",
  "resource_version_missing_or_invalid",
  "creation_timestamp_missing_or_invalid",
  "deletion_timestamp_invalid",
  "deletion_in_progress",
  "sandbox_id_mismatch",
  "canonical_kernel_mismatch",
  "legacy_kernel_mismatch",
  "kernel_labels_ambiguous",
  "canonical_session_mismatch",
  "legacy_session_mismatch",
  "session_labels_ambiguous",
  "runtime_generation_mismatch",
  "runtime_reservation_missing",
  "runtime_reservation_invalid",
  "runtime_reservation_mismatch",
  "runtime_reservation_ambiguous",
]);
const execFileAsync = promisify(execFile);
const REQUIRED_OPENAPI_PATHS = [
  "/api/v1/triggers/public/{public_id}",
  "/api/v1/triggers/public/{public_id}/launches",
  "/api/v1/launches/{launch_id}",
  "/api/v1/launches/{launch_id}/start",
  "/api/v1/launches/{launch_id}/contributions/review",
  "/api/v1/launches/{launch_id}/contributions/publish",
  "/api/v1/launches/{launch_id}/pull-request/review",
  "/api/v1/launches/{launch_id}/pull-request/open",
  "/api/v1/launches/{launch_id}/pull-request/refresh",
  "/api/v1/launches/{launch_id}/complete",
  "/api/v1/launches/{launch_id}/events",
];
const FAILURE_STATES = new Set([
  "failed_retryable",
  "failed_terminal",
  "cancelled",
]);
const SETTLED_PRIOR_LAUNCH_STATES = new Set([
  "completed",
  "failed_terminal",
  "cancelled",
]);
const MAX_CONFIG_JSON_BYTES = 64 * 1024;
const MAX_PUBLIC_JSON_BYTES = 8 * 1024 * 1024;
const CMA_STATE_CLEANUP_POLICY = "destroy_with_terminal_ephemeral_environment";
const SESSION_CLEANUP_POLICY =
  "stop_runtime_before_terminal_ephemeral_teardown";
const CMA_LAUNCH_RETAINED = "retained_until_terminal_ephemeral_teardown";
const SESSION_TEARDOWN_PENDING = "terminal_ephemeral_teardown_pending";
const SESSION_IDLE_AWAITING_TEARDOWN =
  "idle_awaiting_terminal_ephemeral_teardown";
const EPHEMERAL_STATE_DESTROYED =
  "destroyed_with_terminal_ephemeral_environment";
const STORAGE_CONTRACT_REPOSITORY = "ChronoAIProject/chrono-bucket";
const STORAGE_CONTRACT_SOURCE_SHA = "4087662e2a65e3747b3203ba619eca7342089272";
const STORAGE_TOMBSTONE_RETENTION =
  "storage_contract_lifetime_not_logically_enumerable";
const SDK_ENTRY = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../sdk/typescript/dist/index.js",
);
const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SHA1 = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const IMAGE_DIGEST = /^sha256:[0-9a-f]{64}$/;
const LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
const REPOSITORY = /^[A-Za-z0-9_.-]{1,100}$/;
const INPUT_KEY = /^[a-z][a-z0-9_]{0,63}$/;
const RUN_MARKER = /^[a-z0-9][a-z0-9-]{0,62}$/;
const CONFIGURATION_REVISION = /^[A-Za-z0-9][A-Za-z0-9._:/@+~-]{0,254}$/;
const DNS1123_LABEL = /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/;
const KUBERNETES_NAME = /^[a-z0-9](?:[a-z0-9.-]{0,251}[a-z0-9])?$/;
const KUBERNETES_LABEL_VALUE =
  /^(?:[A-Za-z0-9](?:[A-Za-z0-9._-]{0,61}[A-Za-z0-9])?)?$/;
const SAFE_KUBERNETES_METADATA = /^[A-Za-z0-9][A-Za-z0-9._:/+-]{0,254}$/;
const RUNTIME_RESERVATION = /^rres_[0-9a-f]{32}$/;
const UTC_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/u;
const CONFIG_CREDENTIAL_PATTERNS = [
  /\bBearer\s+[^\s"]+/iu,
  /\b(?:github_pat_|gh[opusr]_|sk-[A-Za-z0-9])/u,
  /-----BEGIN [A-Z ]*PRIVATE KEY-----/u,
  /"(?:access_token|refresh_token|authorization|cookie|set-cookie|kubeconfig|api_key|client_secret|password|token|client-key-data|client-certificate-data|certificate-authority-data)"\s*:/iu,
  /\b(?:kubeconfig|current-context|client-key-data|client-certificate-data|certificate-authority-data)\b/iu,
];
const EVIDENCE_RESULTS = new Set([
  "running",
  "pass",
  "fail",
  "inconclusive",
  "blocked",
]);

class SmokeFailure extends Error {
  constructor(code, message) {
    super(message);
    this.name = "SmokeFailure";
    this.code = code;
  }
}

function fail(code, message) {
  throw new SmokeFailure(code, message);
}

function object(value, path, code = "invalid_config") {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(code, `${path} must be an object`);
  }
  return value;
}

function exactKeys(value, expected, path, code = "invalid_config") {
  const actual = Object.keys(object(value, path, code)).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    fail(code, `${path} must contain exactly ${wanted.join(", ")}`);
  }
}

function text(value, path, maximum = 1024, code = "invalid_config") {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    fail(code, `${path} must be bounded non-control text`);
  }
  return value;
}

function integer(value, path, minimum, maximum, code = "invalid_config") {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    fail(code, `${path} must be an integer from ${minimum} through ${maximum}`);
  }
  return value;
}

function template(value, path, maximum, allowNewlines = false) {
  const checked =
    allowNewlines &&
    typeof value === "string" &&
    value.length > 0 &&
    value.length <= maximum
      ? value
      : text(value, path, maximum);
  if (
    allowNewlines &&
    /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u.test(checked)
  ) {
    fail("invalid_config", `${path} contains a forbidden control character`);
  }
  if (checked.split("{marker}").length !== 2) {
    fail("invalid_config", `${path} must contain {marker} exactly once`);
  }
  return checked;
}

function repositoryPath(value, path) {
  const parts = value.split("/");
  if (
    value.startsWith("/") ||
    value.endsWith("/") ||
    value.includes("\\") ||
    parts.some((part) => part === "" || part === "." || part === "..")
  ) {
    fail("invalid_config", `${path} must be a safe repository-relative path`);
  }
}

function runtimeImageIdentity(value, path, code = "invalid_config") {
  const checked = text(value, path, 640, code)
    .replace(/^docker-pullable:\/\//u, "")
    .replace(/^docker:\/\//u, "");
  const at = checked.lastIndexOf("@");
  const digest = at === -1 ? checked : checked.slice(at + 1);
  const repository = at === -1 ? null : checked.slice(0, at);
  if (
    !IMAGE_DIGEST.test(digest) ||
    (repository !== null &&
      (!/^[A-Za-z0-9][A-Za-z0-9._:/-]{0,511}$/u.test(repository) ||
        repository.includes("..")))
  ) {
    fail(code, `${path} must be an immutable runtime image identity`);
  }
  return checked;
}

function runtimeImageIdentities(value, path, code = "invalid_config") {
  if (!Array.isArray(value) || value.length < 1 || value.length > 16) {
    fail(code, `${path} must be a bounded runtime image identity array`);
  }
  const normalized = value.map((entry, index) =>
    runtimeImageIdentity(entry, `${path}[${String(index)}]`, code),
  );
  if (
    new Set(normalized).size !== normalized.length ||
    JSON.stringify(normalized) !== JSON.stringify([...normalized].sort())
  ) {
    fail(code, `${path} must be unique and ordered`);
  }
  return normalized;
}

function validateConfigRedaction(value) {
  const serialized = typeof value === "string" ? value : JSON.stringify(value);
  if (CONFIG_CREDENTIAL_PATTERNS.some((pattern) => pattern.test(serialized))) {
    fail(
      "invalid_config",
      "config must contain only non-secret fixture and deployment bindings",
    );
  }
}

function validateConfig(raw) {
  exactKeys(
    raw,
    [
      "schema_version",
      "base_url",
      "expected_build_sha",
      "runtime_image_digest",
      "runtime_image_ids",
      "expected_runtime_version",
      "configuration_revision",
      "trigger_sandbox_namespace",
      "public_id",
      "revision_digest",
      "source_sha",
      "upstream",
      "contributor",
      "task",
      "fixture_policy",
      "timeouts",
      "github_api_url",
    ],
    "config",
  );
  if (raw.schema_version !== 3)
    fail("invalid_config", "config.schema_version must be 3");

  let baseUrl;
  try {
    baseUrl = new URL(text(raw.base_url, "config.base_url", 512));
  } catch {
    fail("invalid_config", "config.base_url must be an absolute URL");
  }
  if (
    baseUrl.protocol !== "https:" ||
    baseUrl.username !== "" ||
    baseUrl.password !== "" ||
    baseUrl.pathname !== "/" ||
    baseUrl.search !== "" ||
    baseUrl.hash !== ""
  ) {
    fail(
      "invalid_config",
      "config.base_url must be an HTTPS origin without user information",
    );
  }
  raw.base_url = baseUrl.origin;
  if (!SHA1.test(raw.expected_build_sha)) {
    fail(
      "invalid_config",
      "config.expected_build_sha must pin one full Git commit",
    );
  }
  if (!IMAGE_DIGEST.test(raw.runtime_image_digest)) {
    fail(
      "invalid_config",
      "config.runtime_image_digest must pin one sha256 image digest",
    );
  }
  raw.runtime_image_ids = runtimeImageIdentities(
    raw.runtime_image_ids,
    "config.runtime_image_ids",
  );
  if (!raw.runtime_image_ids.includes(raw.runtime_image_digest)) {
    fail(
      "invalid_config",
      "config.runtime_image_ids must contain the approved image digest",
    );
  }
  text(raw.expected_runtime_version, "config.expected_runtime_version", 512);
  if (!CONFIGURATION_REVISION.test(raw.configuration_revision)) {
    fail(
      "invalid_config",
      "config.configuration_revision must be one safe immutable revision",
    );
  }
  if (
    !DNS1123_LABEL.test(
      text(
        raw.trigger_sandbox_namespace,
        "config.trigger_sandbox_namespace",
        63,
      ),
    )
  ) {
    fail(
      "invalid_config",
      "config.trigger_sandbox_namespace must be one exact DNS-1123 label",
    );
  }
  if (!UUID.test(raw.public_id))
    fail("invalid_config", "config.public_id must be a UUID");
  if (!SHA256.test(raw.revision_digest)) {
    fail("invalid_config", "config.revision_digest must be a SHA-256 digest");
  }
  if (!SHA1.test(raw.source_sha))
    fail("invalid_config", "config.source_sha must be a Git SHA-1");

  exactKeys(
    raw.upstream,
    ["repository_id", "owner", "repository", "default_branch"],
    "config.upstream",
  );
  integer(
    raw.upstream.repository_id,
    "config.upstream.repository_id",
    1,
    Number.MAX_SAFE_INTEGER,
  );
  if (!LOGIN.test(raw.upstream.owner))
    fail("invalid_config", "config.upstream.owner is invalid");
  if (!REPOSITORY.test(raw.upstream.repository)) {
    fail("invalid_config", "config.upstream.repository is invalid");
  }
  const defaultBranch = text(
    raw.upstream.default_branch,
    "config.upstream.default_branch",
    255,
  );
  if (
    defaultBranch.startsWith("/") ||
    defaultBranch.endsWith("/") ||
    defaultBranch.includes("..") ||
    defaultBranch.includes("@{") ||
    /[ ~^:?*[\\]/u.test(defaultBranch)
  ) {
    fail("invalid_config", "config.upstream.default_branch is not a safe ref");
  }

  exactKeys(
    raw.contributor,
    ["login", "fork_repository", "expected_fork_mode"],
    "config.contributor",
  );
  if (!LOGIN.test(raw.contributor.login)) {
    fail("invalid_config", "config.contributor.login is invalid");
  }
  if (!REPOSITORY.test(raw.contributor.fork_repository)) {
    fail("invalid_config", "config.contributor.fork_repository is invalid");
  }
  if (!["new", "reuse"].includes(raw.contributor.expected_fork_mode)) {
    fail(
      "invalid_config",
      "config.contributor.expected_fork_mode must be new or reuse",
    );
  }

  exactKeys(
    raw.task,
    ["marker_input", "expected_path_template", "expected_content_template"],
    "config.task",
  );
  if (!INPUT_KEY.test(raw.task.marker_input)) {
    fail("invalid_config", "config.task.marker_input is invalid");
  }
  template(
    raw.task.expected_path_template,
    "config.task.expected_path_template",
    512,
  );
  template(
    raw.task.expected_content_template,
    "config.task.expected_content_template",
    4096,
    true,
  );
  repositoryPath(
    raw.task.expected_path_template.replace("{marker}", "fixture"),
    "expected task path",
  );
  if (!raw.task.expected_path_template.startsWith("cma-trigger-smoke/")) {
    fail(
      "invalid_config",
      "config.task.expected_path_template must stay under cma-trigger-smoke/",
    );
  }

  exactKeys(
    raw.fixture_policy,
    [
      "repository_owner",
      "lifecycle",
      "secrets",
      "github_cleanup",
      "cma_state_cleanup",
      "session_cleanup",
    ],
    "config.fixture_policy",
  );
  for (const [field, expected] of Object.entries({
    repository_owner: "ChronoAIProject",
    lifecycle: "persistent_public_fixture",
    secrets: "forbidden",
    github_cleanup: "preserve",
    cma_state_cleanup: CMA_STATE_CLEANUP_POLICY,
    session_cleanup: SESSION_CLEANUP_POLICY,
  })) {
    if (raw.fixture_policy[field] !== expected) {
      fail(
        "invalid_config",
        `config.fixture_policy.${field} must be ${expected}`,
      );
    }
  }
  if (raw.upstream.owner !== raw.fixture_policy.repository_owner) {
    fail(
      "invalid_config",
      "fixture policy owner must match the current upstream binding",
    );
  }

  exactKeys(
    raw.timeouts,
    ["launch_seconds", "effect_seconds", "poll_seconds"],
    "config.timeouts",
  );
  integer(
    raw.timeouts.launch_seconds,
    "config.timeouts.launch_seconds",
    60,
    3300,
  );
  integer(
    raw.timeouts.effect_seconds,
    "config.timeouts.effect_seconds",
    30,
    1200,
  );
  integer(raw.timeouts.poll_seconds, "config.timeouts.poll_seconds", 1, 30);
  if (raw.timeouts.effect_seconds > raw.timeouts.launch_seconds) {
    fail("invalid_config", "effect timeout cannot exceed launch timeout");
  }
  if (raw.github_api_url !== "https://api.github.com") {
    fail(
      "invalid_config",
      "config.github_api_url must be the public GitHub API origin",
    );
  }
  validateConfigRedaction(raw);
  return raw;
}

async function loadConfig(path) {
  let serialized;
  try {
    serialized = await readFile(path, "utf8");
  } catch {
    fail("invalid_config", "configuration is not readable JSON");
  }
  if (Buffer.byteLength(serialized, "utf8") > MAX_CONFIG_JSON_BYTES) {
    fail("invalid_config", "configuration exceeds the closed size bound");
  }
  validateConfigRedaction(serialized);
  let parsed;
  try {
    parsed = JSON.parse(serialized);
  } catch {
    fail("invalid_config", "configuration is not readable JSON");
  }
  return {
    value: validateConfig(parsed),
    sha256: sha256(serialized),
  };
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function now() {
  return new Date().toISOString();
}

function sleep(milliseconds) {
  return new Promise((resolveSleep) => setTimeout(resolveSleep, milliseconds));
}

function requiredEnvironment(name, maximum = 16384) {
  const value = process.env[name];
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    fail("missing_environment", `${name} is missing or invalid`);
  }
  return value;
}

function assertEqual(actual, expected, code, message) {
  if (actual !== expected) fail(code, message);
}

function assert(condition, code, message) {
  if (!condition) fail(code, message);
}

function assertThrowsCode(operation, expectedCode) {
  let observed = null;
  try {
    operation();
  } catch (cause) {
    if (cause instanceof SmokeFailure) observed = cause.code;
  }
  assertEqual(
    observed,
    expectedCode,
    "self_test_failed",
    `expected ${expectedCode} refusal`,
  );
}

const EVIDENCE_ERROR = "invalid_evidence";

function evidenceExactKeys(value, expected, path) {
  exactKeys(value, expected, path, EVIDENCE_ERROR);
}

function evidenceText(value, path, maximum = 1024) {
  return text(value, path, maximum, EVIDENCE_ERROR);
}

function evidenceInteger(value, path, minimum = 0) {
  return integer(value, path, minimum, Number.MAX_SAFE_INTEGER, EVIDENCE_ERROR);
}

function nullable(value, validator) {
  if (value === null) return null;
  return validator(value);
}

function evidenceBoolean(value, path) {
  if (typeof value !== "boolean")
    fail(EVIDENCE_ERROR, `${path} must be boolean`);
  return value;
}

function isExactUtcTimestamp(value) {
  if (typeof value !== "string" || !UTC_TIMESTAMP.test(value)) return false;
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) return false;
  const canonical = new Date(parsed).toISOString();
  return value === canonical || value === canonical.replace(".000Z", "Z");
}

function evidenceTimestamp(value, path) {
  evidenceText(value, path, 64);
  if (!isExactUtcTimestamp(value)) {
    fail(EVIDENCE_ERROR, `${path} must be an exact UTC timestamp`);
  }
  return value;
}

function evidenceUrl(value, path, options = {}) {
  const parsed = new URL(evidenceText(value, path, 2048));
  if (
    parsed.protocol !== "https:" ||
    parsed.username !== "" ||
    parsed.password !== "" ||
    (options.hash === false && parsed.hash !== "")
  ) {
    fail(EVIDENCE_ERROR, `${path} must be a credential-free HTTPS URL`);
  }
  return parsed;
}

function evidenceSha1(value, path) {
  if (!SHA1.test(value)) fail(EVIDENCE_ERROR, `${path} must be a Git SHA-1`);
}

function evidenceSha256(value, path) {
  if (!SHA256.test(value)) fail(EVIDENCE_ERROR, `${path} must be a SHA-256`);
}

function validateActor(value, path) {
  evidenceExactKeys(value, ["login", "account_id"], path);
  if (!LOGIN.test(value.login))
    fail(EVIDENCE_ERROR, `${path}.login is invalid`);
  evidenceInteger(value.account_id, `${path}.account_id`, 1);
}

const INVENTORY_KEYS = [
  "launches",
  "contributor_forks",
  "launch_branches",
  "launch_pull_requests",
];

function validateInventory(value, path) {
  evidenceExactKeys(value, INVENTORY_KEYS, path);
  for (const key of INVENTORY_KEYS) {
    nullable(value[key], (entry) => evidenceInteger(entry, `${path}.${key}`));
  }
}

function evidenceKubernetesMetadata(value, path, maximum = 255) {
  const checked = evidenceText(value, path, maximum);
  if (!SAFE_KUBERNETES_METADATA.test(checked)) {
    fail(EVIDENCE_ERROR, `${path} is not safe Kubernetes metadata`);
  }
  return checked;
}

function evidenceKubernetesLabel(value, path) {
  const checked = evidenceText(value, path, 63);
  if (!KUBERNETES_LABEL_VALUE.test(checked)) {
    fail(EVIDENCE_ERROR, `${path} is not a Kubernetes label value`);
  }
  return checked;
}

function validateSandboxObject(value, path, { namespace, expected, entitled }) {
  evidenceExactKeys(
    value,
    [
      "api_version",
      "kind",
      "namespace",
      "name",
      "uid",
      "resource_version",
      "creation_timestamp",
      "deletion_timestamp",
      "labels",
      "issues",
    ],
    path,
  );
  for (const key of ["api_version", "kind"]) {
    nullable(value[key], (entry) => evidenceText(entry, `${path}.${key}`, 128));
  }
  nullable(value.namespace, (entry) => {
    if (!DNS1123_LABEL.test(evidenceText(entry, `${path}.namespace`, 63))) {
      fail(EVIDENCE_ERROR, `${path}.namespace is not a DNS-1123 label`);
    }
  });
  nullable(value.name, (entry) => {
    if (!KUBERNETES_NAME.test(evidenceText(entry, `${path}.name`, 253))) {
      fail(EVIDENCE_ERROR, `${path}.name is not a Kubernetes object name`);
    }
  });
  for (const key of ["uid", "resource_version"]) {
    nullable(value[key], (entry) =>
      evidenceKubernetesMetadata(entry, `${path}.${key}`),
    );
  }
  nullable(value.creation_timestamp, (entry) =>
    evidenceTimestamp(entry, `${path}.creation_timestamp`),
  );
  nullable(value.deletion_timestamp, (entry) =>
    evidenceTimestamp(entry, `${path}.deletion_timestamp`),
  );

  const labels = value.labels;
  evidenceExactKeys(
    labels,
    [
      "sandbox_id",
      "canonical_kernel",
      "legacy_kernel",
      "canonical_session",
      "legacy_session",
      "runtime_generation",
      "runtime_reservation_sha256",
    ],
    `${path}.labels`,
  );
  for (const key of [
    "sandbox_id",
    "canonical_kernel",
    "legacy_kernel",
    "canonical_session",
    "legacy_session",
    "runtime_generation",
  ]) {
    nullable(labels[key], (entry) =>
      evidenceKubernetesLabel(entry, `${path}.labels.${key}`),
    );
  }
  nullable(labels.runtime_reservation_sha256, (entry) =>
    evidenceSha256(entry, `${path}.labels.runtime_reservation_sha256`),
  );

  if (!Array.isArray(value.issues)) {
    fail(EVIDENCE_ERROR, `${path}.issues must be an array`);
  }
  value.issues.forEach((issue, index) => {
    if (!SANDBOX_ISSUES.has(issue)) {
      fail(
        EVIDENCE_ERROR,
        `${path}.issues[${String(index)}] is not a closed inventory issue`,
      );
    }
  });
  const canonicalIssues = [...new Set(value.issues)].sort();
  if (JSON.stringify(value.issues) !== JSON.stringify(canonicalIssues)) {
    fail(EVIDENCE_ERROR, `${path}.issues must be unique and ordered`);
  }

  if (entitled) {
    if (
      value.api_version !== BATCH_SANDBOX_API_VERSION ||
      value.kind !== BATCH_SANDBOX_KIND ||
      value.namespace !== namespace ||
      value.name === null ||
      value.uid === null ||
      value.resource_version === null ||
      value.creation_timestamp === null ||
      value.deletion_timestamp !== null ||
      labels.sandbox_id !== value.name ||
      labels.canonical_kernel !== TRIGGER_KERNEL_ID ||
      labels.legacy_kernel !== TRIGGER_KERNEL_ID ||
      labels.canonical_session !== expected.session_id ||
      labels.legacy_session !== expected.session_id ||
      labels.runtime_generation !== String(expected.runtime_generation) ||
      labels.runtime_reservation_sha256 !==
        expected.runtime_reservation_sha256 ||
      value.issues.length !== 0
    ) {
      fail(EVIDENCE_ERROR, `${path} is not an exactly entitled BatchSandbox`);
    }
  } else if (value.issues.length === 0) {
    fail(EVIDENCE_ERROR, `${path} lacks its unexpected-object classification`);
  }
}

function validateSandboxObservation(value, path, phase, namespace) {
  evidenceExactKeys(
    value,
    [
      "phase",
      "observed_at",
      "list_resource_version",
      "expected",
      "total_objects",
      "entitled_objects",
      "unexpected_objects",
    ],
    path,
  );
  if (value.phase !== phase) {
    fail(EVIDENCE_ERROR, `${path}.phase does not match its inventory slot`);
  }
  evidenceTimestamp(value.observed_at, `${path}.observed_at`);
  evidenceKubernetesMetadata(
    value.list_resource_version,
    `${path}.list_resource_version`,
  );
  evidenceExactKeys(
    value.expected,
    ["session_id", "runtime_generation", "runtime_reservation_sha256"],
    `${path}.expected`,
  );
  nullable(value.expected.session_id, (entry) =>
    evidenceKubernetesLabel(entry, `${path}.expected.session_id`),
  );
  nullable(value.expected.runtime_generation, (entry) =>
    evidenceInteger(entry, `${path}.expected.runtime_generation`, 1),
  );
  nullable(value.expected.runtime_reservation_sha256, (entry) =>
    evidenceSha256(entry, `${path}.expected.runtime_reservation_sha256`),
  );
  if (
    phase === "before_launch" &&
    (value.expected.session_id !== null ||
      value.expected.runtime_generation !== null ||
      value.expected.runtime_reservation_sha256 !== null)
  ) {
    fail(EVIDENCE_ERROR, "pre-launch sandbox inventory invented an identity");
  }
  if (
    phase !== "before_launch" &&
    (value.expected.session_id === null ||
      value.expected.runtime_generation === null)
  ) {
    fail(EVIDENCE_ERROR, `${path} lacks the exact session generation`);
  }
  evidenceInteger(value.total_objects, `${path}.total_objects`);
  for (const key of ["entitled_objects", "unexpected_objects"]) {
    if (!Array.isArray(value[key])) {
      fail(EVIDENCE_ERROR, `${path}.${key} must be an array`);
    }
  }
  value.entitled_objects.forEach((entry, index) =>
    validateSandboxObject(entry, `${path}.entitled_objects[${String(index)}]`, {
      namespace,
      expected: value.expected,
      entitled: true,
    }),
  );
  value.unexpected_objects.forEach((entry, index) =>
    validateSandboxObject(
      entry,
      `${path}.unexpected_objects[${String(index)}]`,
      { namespace, expected: value.expected, entitled: false },
    ),
  );
  if (
    value.total_objects !==
    value.entitled_objects.length + value.unexpected_objects.length
  ) {
    fail(EVIDENCE_ERROR, `${path} does not account for its exact object total`);
  }
  const identities = [
    ...value.entitled_objects,
    ...value.unexpected_objects,
  ].map((entry) =>
    JSON.stringify([
      entry.namespace,
      entry.name,
      entry.uid,
      entry.resource_version,
    ]),
  );
  if (new Set(identities).size !== identities.length) {
    fail(EVIDENCE_ERROR, `${path} contains a duplicate object version`);
  }
  return value;
}

function validateSandboxInventory(value, path) {
  evidenceExactKeys(
    value,
    [
      "namespace",
      "api_version",
      "kind",
      "resource",
      "label_contract",
      "observations",
    ],
    path,
  );
  if (
    !DNS1123_LABEL.test(evidenceText(value.namespace, `${path}.namespace`, 63))
  ) {
    fail(EVIDENCE_ERROR, `${path}.namespace is not one exact DNS-1123 label`);
  }
  if (
    value.api_version !== BATCH_SANDBOX_API_VERSION ||
    value.kind !== BATCH_SANDBOX_KIND ||
    value.resource !== BATCH_SANDBOX_RESOURCE
  ) {
    fail(EVIDENCE_ERROR, `${path} does not bind the BatchSandbox resource`);
  }
  evidenceExactKeys(
    value.label_contract,
    Object.keys(SANDBOX_LABELS),
    `${path}.label_contract`,
  );
  if (JSON.stringify(value.label_contract) !== JSON.stringify(SANDBOX_LABELS)) {
    fail(EVIDENCE_ERROR, `${path}.label_contract does not bind exact labels`);
  }
  evidenceExactKeys(value.observations, SANDBOX_PHASES, `${path}.observations`);
  for (const phase of SANDBOX_PHASES) {
    nullable(value.observations[phase], (observation) =>
      validateSandboxObservation(
        observation,
        `${path}.observations.${phase}`,
        phase,
        value.namespace,
      ),
    );
  }
  return value;
}

function validateRuntimeAttestationEvidence(value, path) {
  evidenceExactKeys(
    value,
    [
      "configmap_name",
      "configmap_namespace",
      "observed_at",
      "pod",
      "verified_at",
    ],
    path,
  );
  if (
    value.configmap_name !== RUNTIME_ATTESTATION_CONFIGMAP ||
    value.configmap_namespace !== RUNTIME_ATTESTATION_NAMESPACE
  ) {
    fail(
      EVIDENCE_ERROR,
      `${path} does not bind the redacted attestation object`,
    );
  }
  evidenceTimestamp(value.observed_at, `${path}.observed_at`);
  evidenceTimestamp(value.verified_at, `${path}.verified_at`);
  if (
    Date.parse(value.observed_at) > Date.parse(value.verified_at) ||
    Date.parse(value.verified_at) - Date.parse(value.observed_at) > 60_000
  ) {
    fail(EVIDENCE_ERROR, `${path} does not prove one fresh observation`);
  }

  const pod = value.pod;
  evidenceExactKeys(
    pod,
    [
      "image_id",
      "image_reference",
      "image_ready",
      "init_image_id",
      "init_image_reference",
      "init_ready",
      "name",
      "namespace",
      "resource_version",
      "runtime_generation",
      "sandbox_id",
      "session_id",
      "uid",
    ],
    `${path}.pod`,
  );
  const imageId = runtimeImageIdentity(
    pod.image_id,
    `${path}.pod.image_id`,
    EVIDENCE_ERROR,
  );
  if (imageId !== pod.image_id) {
    fail(EVIDENCE_ERROR, `${path}.pod.image_id is not canonical`);
  }
  evidenceText(pod.image_reference, `${path}.pod.image_reference`, 512);
  if (pod.image_ready !== true || pod.init_ready !== true) {
    fail(EVIDENCE_ERROR, `${path}.pod does not prove ready image containers`);
  }
  const initImageId = runtimeImageIdentity(
    pod.init_image_id,
    `${path}.pod.init_image_id`,
    EVIDENCE_ERROR,
  );
  if (
    initImageId !== pod.init_image_id ||
    pod.init_image_reference !== EXECD_ATTESTATION_IMAGE
  ) {
    fail(EVIDENCE_ERROR, `${path}.pod does not bind the pinned execd image`);
  }
  if (!KUBERNETES_NAME.test(evidenceText(pod.name, `${path}.pod.name`, 253))) {
    fail(EVIDENCE_ERROR, `${path}.pod.name is not a Kubernetes object name`);
  }
  if (
    !DNS1123_LABEL.test(
      evidenceText(pod.namespace, `${path}.pod.namespace`, 63),
    )
  ) {
    fail(EVIDENCE_ERROR, `${path}.pod.namespace is not one DNS-1123 label`);
  }
  evidenceKubernetesMetadata(
    pod.resource_version,
    `${path}.pod.resource_version`,
  );
  evidenceInteger(pod.runtime_generation, `${path}.pod.runtime_generation`, 1);
  if (
    !KUBERNETES_NAME.test(
      evidenceText(pod.sandbox_id, `${path}.pod.sandbox_id`, 253),
    )
  ) {
    fail(
      EVIDENCE_ERROR,
      `${path}.pod.sandbox_id is not a Kubernetes object name`,
    );
  }
  evidenceKubernetesLabel(pod.session_id, `${path}.pod.session_id`);
  if (!UUID.test(evidenceText(pod.uid, `${path}.pod.uid`, 64))) {
    fail(EVIDENCE_ERROR, `${path}.pod.uid is not an exact Pod UUID`);
  }
  return value;
}

function validateProviderSettlementBucket(value, path) {
  evidenceExactKeys(
    value,
    [
      "bucket_name_sha256",
      "selected_objects",
      "selected_bytes_observed",
      "selected_objects_without_size",
      "legacy_deletions_acknowledged",
      "fenced_deletions_acknowledged",
      "unrelated_live_objects_observed",
      "current_run_live_objects_after",
      "tombstone_retention",
    ],
    path,
  );
  evidenceSha256(value.bucket_name_sha256, `${path}.bucket_name_sha256`);
  for (const field of [
    "selected_objects",
    "selected_bytes_observed",
    "selected_objects_without_size",
    "legacy_deletions_acknowledged",
    "fenced_deletions_acknowledged",
    "unrelated_live_objects_observed",
    "current_run_live_objects_after",
  ]) {
    evidenceInteger(value[field], `${path}.${field}`);
  }
  if (
    value.selected_objects !==
      value.legacy_deletions_acknowledged +
        value.fenced_deletions_acknowledged ||
    value.selected_objects_without_size > value.selected_objects ||
    value.current_run_live_objects_after !== 0 ||
    value.tombstone_retention !== STORAGE_TOMBSTONE_RETENTION
  ) {
    fail(EVIDENCE_ERROR, `${path} settlement accounting is invalid`);
  }
  return value;
}

function validateProviderObjectSettlement(value, evidence) {
  evidenceExactKeys(
    value,
    [
      "schema_version",
      "kind",
      "evidence_id",
      "workflow",
      "storage_contract",
      "owner_service_account_id_sha256",
      "launch_id_sha256",
      "session_id_sha256",
      "settled_at",
      "buckets",
    ],
    "evidence.cleanup.provider_object_settlement",
  );
  if (
    value.schema_version !== 1 ||
    value.kind !== "cma-trigger-provider-object-settlement" ||
    value.evidence_id !== evidence.evidence_id
  ) {
    fail(EVIDENCE_ERROR, "provider-object settlement identity is invalid");
  }
  evidenceExactKeys(
    value.workflow,
    ["repository_id", "run_id", "run_attempt", "source_sha"],
    "evidence.cleanup.provider_object_settlement.workflow",
  );
  if (
    value.workflow.repository_id !==
      evidence.provenance.workflow.repository_id ||
    value.workflow.run_id !== evidence.provenance.workflow.run_id ||
    value.workflow.run_attempt !== evidence.provenance.workflow.run_attempt ||
    value.workflow.source_sha !== evidence.provenance.workflow.workflow_commit
  ) {
    fail(
      EVIDENCE_ERROR,
      "provider-object settlement does not bind the evidence workflow",
    );
  }
  evidenceInteger(
    value.workflow.repository_id,
    "evidence.cleanup.provider_object_settlement.workflow.repository_id",
    1,
  );
  evidenceInteger(
    value.workflow.run_id,
    "evidence.cleanup.provider_object_settlement.workflow.run_id",
    1,
  );
  evidenceInteger(
    value.workflow.run_attempt,
    "evidence.cleanup.provider_object_settlement.workflow.run_attempt",
    1,
  );
  evidenceSha1(
    value.workflow.source_sha,
    "evidence.cleanup.provider_object_settlement.workflow.source_sha",
  );
  evidenceExactKeys(
    value.storage_contract,
    ["repository", "source_sha"],
    "evidence.cleanup.provider_object_settlement.storage_contract",
  );
  if (
    value.storage_contract.repository !== STORAGE_CONTRACT_REPOSITORY ||
    value.storage_contract.source_sha !== STORAGE_CONTRACT_SOURCE_SHA
  ) {
    fail(EVIDENCE_ERROR, "provider-object settlement storage contract drifted");
  }
  for (const field of [
    "owner_service_account_id_sha256",
    "launch_id_sha256",
    "session_id_sha256",
  ]) {
    evidenceSha256(
      value[field],
      `evidence.cleanup.provider_object_settlement.${field}`,
    );
  }
  if (
    evidence.resources.launch_id === null ||
    evidence.resources.session_id === null ||
    value.launch_id_sha256 !== sha256(evidence.resources.launch_id) ||
    value.session_id_sha256 !== sha256(evidence.resources.session_id)
  ) {
    fail(
      EVIDENCE_ERROR,
      "provider-object settlement does not bind the Launch and session",
    );
  }
  evidenceTimestamp(
    value.settled_at,
    "evidence.cleanup.provider_object_settlement.settled_at",
  );
  if (
    evidence.finished_at === null ||
    Date.parse(value.settled_at) < Date.parse(evidence.finished_at)
  ) {
    fail(EVIDENCE_ERROR, "provider-object settlement predates closed evidence");
  }
  evidenceExactKeys(
    value.buckets,
    ["context", "trigger_workspace"],
    "evidence.cleanup.provider_object_settlement.buckets",
  );
  const context = validateProviderSettlementBucket(
    value.buckets.context,
    "evidence.cleanup.provider_object_settlement.buckets.context",
  );
  const workspace = validateProviderSettlementBucket(
    value.buckets.trigger_workspace,
    "evidence.cleanup.provider_object_settlement.buckets.trigger_workspace",
  );
  if (
    context.bucket_name_sha256 === workspace.bucket_name_sha256 ||
    context.unrelated_live_objects_observed !== 0 ||
    workspace.unrelated_live_objects_observed !== 0
  ) {
    fail(
      EVIDENCE_ERROR,
      "provider-object settlement did not close the dedicated bucket inventory",
    );
  }
  return value;
}

function validateEvidence(raw, { publishable = false } = {}) {
  evidenceExactKeys(
    raw,
    [
      "schema_version",
      "evidence_id",
      "ledger_id",
      "result",
      "expected_result",
      "actual_result",
      "started_at",
      "finished_at",
      "provenance",
      "actors",
      "fixture",
      "action",
      "external_inventory",
      "sandbox_inventory",
      "counts",
      "resources",
      "correlations",
      "stages",
      "runtime_attestation",
      "provider_settlement",
      "cleanup",
      "redaction",
      "failure",
    ],
    "evidence",
  );
  if (raw.schema_version !== 6)
    fail(EVIDENCE_ERROR, "evidence.schema_version must be 6");
  evidenceText(raw.evidence_id, "evidence.evidence_id", 255);
  if (raw.ledger_id !== "P-01")
    fail(EVIDENCE_ERROR, "evidence.ledger_id must be P-01");
  if (!EVIDENCE_RESULTS.has(raw.result))
    fail(EVIDENCE_ERROR, "evidence.result is not closed");
  if (
    raw.expected_result !==
    "one_completed_launch_to_one_reviewed_contribution_and_open_pull_request_with_provider_settlement"
  ) {
    fail(EVIDENCE_ERROR, "evidence.expected_result is not the P-01 contract");
  }
  if (
    ![
      "running",
      "matched_expected",
      "smoke_failed",
      "inconclusive",
      "blocked",
    ].includes(raw.actual_result)
  ) {
    fail(EVIDENCE_ERROR, "evidence.actual_result is not closed");
  }
  evidenceTimestamp(raw.started_at, "evidence.started_at");
  nullable(raw.finished_at, (value) =>
    evidenceTimestamp(value, "evidence.finished_at"),
  );
  if (
    raw.finished_at !== null &&
    Date.parse(raw.finished_at) < Date.parse(raw.started_at)
  ) {
    fail(EVIDENCE_ERROR, "evidence UTC interval runs backwards");
  }

  evidenceExactKeys(
    raw.provenance,
    ["workflow", "deployment"],
    "evidence.provenance",
  );
  const workflow = raw.provenance.workflow;
  evidenceExactKeys(
    workflow,
    [
      "repository",
      "repository_id",
      "workflow_commit",
      "run_id",
      "run_attempt",
      "run_url",
      "artifact_name",
      "artifact_url",
    ],
    "evidence.provenance.workflow",
  );
  const repositoryParts = evidenceText(
    workflow.repository,
    "evidence.provenance.workflow.repository",
    201,
  ).split("/");
  if (
    repositoryParts.length !== 2 ||
    !LOGIN.test(repositoryParts[0]) ||
    !REPOSITORY.test(repositoryParts[1])
  ) {
    fail(EVIDENCE_ERROR, "evidence workflow repository is invalid");
  }
  evidenceSha1(
    workflow.workflow_commit,
    "evidence.provenance.workflow.workflow_commit",
  );
  evidenceInteger(
    workflow.repository_id,
    "evidence.provenance.workflow.repository_id",
    1,
  );
  evidenceInteger(workflow.run_id, "evidence.provenance.workflow.run_id", 1);
  evidenceInteger(
    workflow.run_attempt,
    "evidence.provenance.workflow.run_attempt",
    1,
  );
  const runUrl = evidenceUrl(
    workflow.run_url,
    "evidence.provenance.workflow.run_url",
    {
      hash: false,
    },
  );
  evidenceText(
    workflow.artifact_name,
    "evidence.provenance.workflow.artifact_name",
    255,
  );
  const artifactUrl = evidenceUrl(
    workflow.artifact_url,
    "evidence.provenance.workflow.artifact_url",
  );
  const expectedRunSuffix = `/${workflow.repository}/actions/runs/${String(workflow.run_id)}`;
  if (runUrl.pathname !== expectedRunSuffix || runUrl.search !== "") {
    fail(EVIDENCE_ERROR, "evidence workflow URL does not bind the exact run");
  }
  if (artifactUrl.href !== `${runUrl.href}#artifacts`) {
    fail(
      EVIDENCE_ERROR,
      "evidence artifact URL does not bind the run artifact landing",
    );
  }
  const expectedArtifactName = `cma-trigger-provider-smoke-${String(workflow.run_id)}-${String(workflow.run_attempt)}`;
  if (workflow.artifact_name !== expectedArtifactName) {
    fail(
      EVIDENCE_ERROR,
      "evidence artifact name does not bind the run attempt",
    );
  }
  if (
    raw.evidence_id !==
    `p-01-gha-${String(workflow.run_id)}-${String(workflow.run_attempt)}`
  ) {
    fail(EVIDENCE_ERROR, "evidence ID does not bind the run attempt");
  }

  const deployment = raw.provenance.deployment;
  evidenceExactKeys(
    deployment,
    [
      "environment",
      "version",
      "build_commit",
      "runtime_image_digest",
      "runtime_image_ids",
      "expected_runtime_version",
      "observed_runtime_version",
      "expected_kernel",
      "observed_kernel",
      "configuration_revision",
      "fixture_config_sha256",
      "trigger_conformance",
    ],
    "evidence.provenance.deployment",
  );
  if (deployment.environment !== "cma-trigger-smoke")
    fail(
      EVIDENCE_ERROR,
      "evidence deployment environment is not the protected smoke environment",
    );
  nullable(deployment.version, (value) =>
    evidenceText(value, "evidence.provenance.deployment.version", 255),
  );
  evidenceSha1(
    deployment.build_commit,
    "evidence.provenance.deployment.build_commit",
  );
  if (!IMAGE_DIGEST.test(deployment.runtime_image_digest)) {
    fail(EVIDENCE_ERROR, "evidence runtime image digest is invalid");
  }
  const runtimeImageIds = runtimeImageIdentities(
    deployment.runtime_image_ids,
    "evidence.provenance.deployment.runtime_image_ids",
    EVIDENCE_ERROR,
  );
  if (
    JSON.stringify(runtimeImageIds) !==
      JSON.stringify(deployment.runtime_image_ids) ||
    !runtimeImageIds.includes(deployment.runtime_image_digest)
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence runtime image identities do not contain the approved digest",
    );
  }
  evidenceText(
    deployment.expected_runtime_version,
    "evidence.provenance.deployment.expected_runtime_version",
    512,
  );
  nullable(deployment.observed_runtime_version, (value) =>
    evidenceText(
      value,
      "evidence.provenance.deployment.observed_runtime_version",
      512,
    ),
  );
  if (deployment.expected_kernel !== TRIGGER_KERNEL_ID) {
    fail(EVIDENCE_ERROR, "evidence expected kernel is not the Trigger kernel");
  }
  nullable(deployment.observed_kernel, (value) =>
    evidenceText(value, "evidence.provenance.deployment.observed_kernel", 128),
  );
  if (!CONFIGURATION_REVISION.test(deployment.configuration_revision)) {
    fail(EVIDENCE_ERROR, "evidence configuration revision is invalid");
  }
  evidenceSha256(
    deployment.fixture_config_sha256,
    "evidence.provenance.deployment.fixture_config_sha256",
  );
  evidenceBoolean(
    deployment.trigger_conformance,
    "evidence.provenance.deployment.trigger_conformance",
  );

  evidenceExactKeys(raw.actors, ["operator", "reviewers"], "evidence.actors");
  validateActor(raw.actors.operator, "evidence.actors.operator");
  if (
    !Array.isArray(raw.actors.reviewers) ||
    raw.actors.reviewers.length === 0
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence must retain at least one independent reviewer",
    );
  }
  const reviewerIds = new Set();
  raw.actors.reviewers.forEach((reviewer, index) => {
    const path = `evidence.actors.reviewers[${String(index)}]`;
    evidenceExactKeys(
      reviewer,
      ["login", "account_id", "approval_observed_at"],
      path,
    );
    validateActor(
      { login: reviewer.login, account_id: reviewer.account_id },
      path,
    );
    evidenceTimestamp(
      reviewer.approval_observed_at,
      `${path}.approval_observed_at`,
    );
    if (
      Date.parse(reviewer.approval_observed_at) > Date.parse(raw.started_at)
    ) {
      fail(
        EVIDENCE_ERROR,
        "evidence approval observation occurred after evidence started",
      );
    }
    if (
      reviewer.account_id === raw.actors.operator.account_id ||
      reviewer.login.toLowerCase() === raw.actors.operator.login.toLowerCase()
    ) {
      fail(
        EVIDENCE_ERROR,
        "evidence reviewer must be independent of the operator",
      );
    }
    if (reviewerIds.has(reviewer.account_id))
      fail(EVIDENCE_ERROR, "evidence reviewers must be unique");
    reviewerIds.add(reviewer.account_id);
  });

  const fixture = raw.fixture;
  evidenceExactKeys(
    fixture,
    [
      "upstream_repository_id",
      "upstream_owner_account_id",
      "upstream_owner",
      "upstream_repository",
      "source_sha",
      "public_trigger_id",
      "trigger_revision_digest",
      "contributor_account_id",
      "contributor_login",
      "service_id_sha256",
      "expected_fork_mode",
      "launch_creation_mode",
      "policy",
    ],
    "evidence.fixture",
  );
  evidenceInteger(
    fixture.upstream_repository_id,
    "evidence.fixture.upstream_repository_id",
    1,
  );
  nullable(fixture.upstream_owner_account_id, (value) =>
    evidenceInteger(value, "evidence.fixture.upstream_owner_account_id", 1),
  );
  if (!LOGIN.test(fixture.upstream_owner))
    fail(EVIDENCE_ERROR, "evidence fixture upstream owner is invalid");
  if (!REPOSITORY.test(fixture.upstream_repository))
    fail(EVIDENCE_ERROR, "evidence fixture upstream repository is invalid");
  evidenceSha1(fixture.source_sha, "evidence.fixture.source_sha");
  if (!UUID.test(fixture.public_trigger_id))
    fail(EVIDENCE_ERROR, "evidence fixture public Trigger ID is invalid");
  evidenceSha256(
    fixture.trigger_revision_digest,
    "evidence.fixture.trigger_revision_digest",
  );
  nullable(fixture.contributor_account_id, (value) =>
    evidenceInteger(value, "evidence.fixture.contributor_account_id", 1),
  );
  if (!LOGIN.test(fixture.contributor_login))
    fail(EVIDENCE_ERROR, "evidence fixture contributor login is invalid");
  evidenceSha256(
    fixture.service_id_sha256,
    "evidence.fixture.service_id_sha256",
  );
  if (!["new", "reuse"].includes(fixture.expected_fork_mode))
    fail(EVIDENCE_ERROR, "evidence fixture fork mode is invalid");
  if (
    fixture.launch_creation_mode !== null &&
    !["create_or_resume", "new_after_settled"].includes(
      fixture.launch_creation_mode,
    )
  ) {
    fail(EVIDENCE_ERROR, "evidence fixture Launch creation mode is invalid");
  }
  evidenceExactKeys(
    fixture.policy,
    [
      "repository_owner",
      "lifecycle",
      "secrets",
      "github_cleanup",
      "cma_state_cleanup",
      "session_cleanup",
    ],
    "evidence.fixture.policy",
  );
  const expectedPolicy = {
    repository_owner: "ChronoAIProject",
    lifecycle: "persistent_public_fixture",
    secrets: "forbidden",
    github_cleanup: "preserve",
    cma_state_cleanup: CMA_STATE_CLEANUP_POLICY,
    session_cleanup: SESSION_CLEANUP_POLICY,
  };
  for (const [key, expected] of Object.entries(expectedPolicy)) {
    if (fixture.policy[key] !== expected)
      fail(EVIDENCE_ERROR, `evidence fixture policy ${key} is invalid`);
  }

  evidenceExactKeys(
    raw.action,
    [
      "kind",
      "name",
      "run_marker",
      "fault_injection",
      "provider_resource_policy",
    ],
    "evidence.action",
  );
  if (
    raw.action.kind !== "rollout" ||
    raw.action.name !== "p-01-provider-smoke" ||
    !RUN_MARKER.test(raw.action.run_marker) ||
    raw.action.fault_injection !== "none" ||
    raw.action.provider_resource_policy !== "preserve"
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence action is not the closed P-01 rollout action",
    );
  }

  evidenceExactKeys(
    raw.external_inventory,
    ["before", "after", "delta"],
    "evidence.external_inventory",
  );
  validateInventory(
    raw.external_inventory.before,
    "evidence.external_inventory.before",
  );
  validateInventory(
    raw.external_inventory.after,
    "evidence.external_inventory.after",
  );
  validateInventory(
    raw.external_inventory.delta,
    "evidence.external_inventory.delta",
  );
  const sandboxInventory = validateSandboxInventory(
    raw.sandbox_inventory,
    "evidence.sandbox_inventory",
  );
  const sandboxBefore = sandboxInventory.observations.before_launch;
  const sandboxLive = sandboxInventory.observations.live_session;
  const sandboxAfter = sandboxInventory.observations.after_stop;

  const countKeys = [
    "launch",
    "fork",
    "fork_created_delta",
    "branch",
    "workspace",
    "sandbox",
    "session",
    "initial_owner_message",
    "contribution_commit",
    "pull_request",
  ];
  evidenceExactKeys(raw.counts, countKeys, "evidence.counts");
  for (const key of countKeys)
    evidenceInteger(raw.counts[key], `evidence.counts.${key}`);
  if (
    raw.counts.sandbox !==
    (sandboxLive === null ? 0 : sandboxLive.entitled_objects.length)
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence sandbox count is not derived from the live inventory",
    );
  }

  const resources = raw.resources;
  const resourceKeys = [
    "launch_id",
    "fork_repository_id",
    "fork_owner_account_id",
    "branch_name",
    "workspace_manifest_digest",
    "workspace_epoch",
    "session_id",
    "runtime_generation",
    "delivery_origin_key",
    "diff_artifact_id",
    "contribution_id",
    "commit_id",
    "pull_request_provider_id",
    "pull_request_number",
    "pull_request_url",
  ];
  evidenceExactKeys(resources, resourceKeys, "evidence.resources");
  for (const key of [
    "launch_id",
    "branch_name",
    "session_id",
    "delivery_origin_key",
    "diff_artifact_id",
    "contribution_id",
  ]) {
    nullable(resources[key], (value) =>
      evidenceText(value, `evidence.resources.${key}`, 512),
    );
  }
  nullable(resources.workspace_manifest_digest, (value) =>
    evidenceSha256(value, "evidence.resources.workspace_manifest_digest"),
  );
  nullable(resources.commit_id, (value) =>
    evidenceSha1(value, "evidence.resources.commit_id"),
  );
  for (const key of [
    "fork_repository_id",
    "fork_owner_account_id",
    "workspace_epoch",
    "runtime_generation",
    "pull_request_provider_id",
    "pull_request_number",
  ]) {
    nullable(resources[key], (value) =>
      evidenceInteger(value, `evidence.resources.${key}`, 1),
    );
  }
  nullable(resources.pull_request_url, (value) =>
    evidenceUrl(value, "evidence.resources.pull_request_url", { hash: false }),
  );

  let runtimeAttestation = null;
  nullable(raw.runtime_attestation, (value) => {
    runtimeAttestation = validateRuntimeAttestationEvidence(
      value,
      "evidence.runtime_attestation",
    );
  });
  if (
    runtimeAttestation !== null &&
    (runtimeAttestation.pod.image_reference !==
      deployment.expected_runtime_version ||
      !runtimeImageIds.includes(runtimeAttestation.pod.image_id) ||
      runtimeAttestation.pod.namespace !== sandboxInventory.namespace ||
      runtimeAttestation.pod.session_id !== resources.session_id ||
      runtimeAttestation.pod.runtime_generation !==
        resources.runtime_generation ||
      sandboxLive === null ||
      sandboxLive.entitled_objects.length !== 1 ||
      runtimeAttestation.pod.sandbox_id !==
        sandboxLive.entitled_objects[0].labels.sandbox_id)
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence runtime attestation does not bind the approved Launch sandbox image",
    );
  }

  const providerSettlement = raw.provider_settlement;
  evidenceExactKeys(
    providerSettlement,
    [
      "launch_state",
      "reach_state",
      "session_credential_retirement",
      "verified_at",
    ],
    "evidence.provider_settlement",
  );
  nullable(providerSettlement.launch_state, (value) => {
    if (value !== "completed") {
      fail(
        EVIDENCE_ERROR,
        "evidence.provider_settlement.launch_state must be completed",
      );
    }
  });
  nullable(providerSettlement.reach_state, (value) => {
    if (value !== "revoked") {
      fail(
        EVIDENCE_ERROR,
        "evidence.provider_settlement.reach_state must be revoked",
      );
    }
  });
  nullable(providerSettlement.session_credential_retirement, (retirement) => {
    evidenceExactKeys(
      retirement,
      ["session_id", "settled_at"],
      "evidence.provider_settlement.session_credential_retirement",
    );
    evidenceText(
      retirement.session_id,
      "evidence.provider_settlement.session_credential_retirement.session_id",
      512,
    );
    evidenceTimestamp(
      retirement.settled_at,
      "evidence.provider_settlement.session_credential_retirement.settled_at",
    );
  });
  nullable(providerSettlement.verified_at, (value) =>
    evidenceTimestamp(value, "evidence.provider_settlement.verified_at"),
  );
  const providerSettlementComplete =
    providerSettlement.launch_state === "completed" &&
    providerSettlement.reach_state === "revoked" &&
    providerSettlement.session_credential_retirement !== null &&
    providerSettlement.verified_at !== null;
  const providerSettlementAbsent =
    providerSettlement.launch_state === null &&
    providerSettlement.reach_state === null &&
    providerSettlement.session_credential_retirement === null &&
    providerSettlement.verified_at === null;
  if (!providerSettlementComplete && !providerSettlementAbsent) {
    fail(
      EVIDENCE_ERROR,
      "evidence provider settlement must be wholly absent or positively complete",
    );
  }
  if (
    providerSettlementComplete &&
    providerSettlement.session_credential_retirement.session_id !==
      resources.session_id
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence provider settlement belongs to another session",
    );
  }
  if (
    providerSettlementComplete &&
    Date.parse(providerSettlement.session_credential_retirement.settled_at) >
      Date.parse(providerSettlement.verified_at)
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence observed provider settlement before it was settled",
    );
  }

  evidenceExactKeys(
    raw.correlations,
    ["launch", "provider"],
    "evidence.correlations",
  );
  for (const key of ["launch", "provider"]) {
    const values = raw.correlations[key];
    if (!Array.isArray(values))
      fail(EVIDENCE_ERROR, `evidence correlations ${key} must be an array`);
    values.forEach((value, index) =>
      evidenceText(
        value,
        `evidence.correlations.${key}[${String(index)}]`,
        255,
      ),
    );
    if (new Set(values).size !== values.length)
      fail(EVIDENCE_ERROR, `evidence correlations ${key} must be unique`);
  }

  if (!Array.isArray(raw.stages))
    fail(EVIDENCE_ERROR, "evidence stages must be an array");
  raw.stages.forEach((stage, index) => {
    const path = `evidence.stages[${String(index)}]`;
    evidenceExactKeys(
      stage,
      ["name", "status", "started_at", "finished_at"],
      path,
    );
    evidenceText(stage.name, `${path}.name`, 128);
    if (!["running", "passed", "failed"].includes(stage.status))
      fail(EVIDENCE_ERROR, `${path}.status is invalid`);
    evidenceTimestamp(stage.started_at, `${path}.started_at`);
    nullable(stage.finished_at, (value) =>
      evidenceTimestamp(value, `${path}.finished_at`),
    );
  });
  const sessionCleanupStages = raw.stages.filter(
    (stage) => stage.name === "session_cleanup",
  );
  const launchCompletionStages = raw.stages.filter(
    (stage) => stage.name === "launch_completion",
  );
  const sandboxLiveStages = raw.stages.filter(
    (stage) => stage.name === "sandbox_inventory_live_session",
  );
  const runtimeAttestationStages = raw.stages.filter(
    (stage) => stage.name === "runtime_image_attestation",
  );

  evidenceExactKeys(
    raw.cleanup,
    ["github", "cma_launch", "session", "provider_object_settlement"],
    "evidence.cleanup",
  );
  let providerObjectSettlement = null;
  nullable(raw.cleanup.provider_object_settlement, (value) => {
    providerObjectSettlement = validateProviderObjectSettlement(value, raw);
  });
  if (
    raw.cleanup.github !== "preserved_by_policy" ||
    !["not_created", CMA_LAUNCH_RETAINED, EPHEMERAL_STATE_DESTROYED].includes(
      raw.cleanup.cma_launch,
    ) ||
    ![
      "not_created",
      SESSION_TEARDOWN_PENDING,
      SESSION_IDLE_AWAITING_TEARDOWN,
      EPHEMERAL_STATE_DESTROYED,
    ].includes(raw.cleanup.session)
  ) {
    fail(EVIDENCE_ERROR, "evidence cleanup state is invalid");
  }
  const launchCreated = raw.resources.launch_id !== null;
  const sessionCreated = raw.resources.session_id !== null;
  if (
    (launchCreated && raw.cleanup.cma_launch === "not_created") ||
    (!launchCreated && raw.cleanup.cma_launch !== "not_created") ||
    (sessionCreated && raw.cleanup.session === "not_created") ||
    (!sessionCreated && raw.cleanup.session !== "not_created") ||
    (raw.cleanup.cma_launch === EPHEMERAL_STATE_DESTROYED &&
      !["not_created", EPHEMERAL_STATE_DESTROYED].includes(
        raw.cleanup.session,
      )) ||
    (raw.cleanup.session === EPHEMERAL_STATE_DESTROYED &&
      raw.cleanup.cma_launch !== EPHEMERAL_STATE_DESTROYED) ||
    (providerObjectSettlement !== null &&
      (raw.result !== "pass" ||
        raw.cleanup.cma_launch !== EPHEMERAL_STATE_DESTROYED ||
        raw.cleanup.session !== EPHEMERAL_STATE_DESTROYED)) ||
    (raw.result === "pass" &&
      raw.cleanup.cma_launch === EPHEMERAL_STATE_DESTROYED &&
      providerObjectSettlement === null)
  ) {
    fail(
      EVIDENCE_ERROR,
      "evidence cleanup state contradicts created resources",
    );
  }

  evidenceExactKeys(
    raw.redaction,
    ["status", "audited_at", "checks"],
    "evidence.redaction",
  );
  if (!["pending", "passed"].includes(raw.redaction.status))
    fail(EVIDENCE_ERROR, "evidence redaction status is invalid");
  nullable(raw.redaction.audited_at, (value) =>
    evidenceTimestamp(value, "evidence.redaction.audited_at"),
  );
  const redactionChecks = [
    "closed_schema",
    "environment_secrets",
    "kubernetes_credentials",
    "credential_patterns",
    "forbidden_fields",
  ];
  if (
    JSON.stringify(raw.redaction.checks) !== JSON.stringify(redactionChecks)
  ) {
    fail(EVIDENCE_ERROR, "evidence redaction checks are not closed");
  }

  if (raw.failure !== null) {
    evidenceExactKeys(
      raw.failure,
      ["stage", "kind", "code", "http_status", "api_code", "request_id"],
      "evidence.failure",
    );
    evidenceText(raw.failure.stage, "evidence.failure.stage", 128);
    evidenceText(raw.failure.kind, "evidence.failure.kind", 128);
    evidenceText(raw.failure.code, "evidence.failure.code", 128);
    nullable(raw.failure.http_status, (value) =>
      evidenceInteger(value, "evidence.failure.http_status", 100),
    );
    nullable(raw.failure.api_code, (value) =>
      evidenceText(value, "evidence.failure.api_code", 128),
    );
    nullable(raw.failure.request_id, (value) =>
      evidenceText(value, "evidence.failure.request_id", 255),
    );
  }

  if (raw.result === "running") {
    if (
      raw.actual_result !== "running" ||
      raw.finished_at !== null ||
      raw.failure !== null
    ) {
      fail(EVIDENCE_ERROR, "running evidence has a terminal field");
    }
  } else if (raw.finished_at === null) {
    fail(EVIDENCE_ERROR, "terminal evidence lacks a finish timestamp");
  }

  if (raw.result === "pass") {
    const exactSandboxInventory =
      sandboxBefore !== null &&
      sandboxBefore.total_objects === 0 &&
      sandboxBefore.entitled_objects.length === 0 &&
      sandboxBefore.unexpected_objects.length === 0 &&
      sandboxLive !== null &&
      sandboxLive.total_objects === 1 &&
      sandboxLive.entitled_objects.length === 1 &&
      sandboxLive.unexpected_objects.length === 0 &&
      sandboxLive.expected.session_id === resources.session_id &&
      sandboxLive.expected.runtime_generation ===
        resources.runtime_generation &&
      sandboxLive.expected.runtime_reservation_sha256 !== null &&
      sandboxAfter !== null &&
      sandboxAfter.total_objects === 0 &&
      sandboxAfter.entitled_objects.length === 0 &&
      sandboxAfter.unexpected_objects.length === 0 &&
      JSON.stringify(sandboxAfter.expected) ===
        JSON.stringify(sandboxLive.expected);
    if (
      raw.actual_result !== "matched_expected" ||
      raw.failure !== null ||
      deployment.version === null ||
      deployment.observed_runtime_version !==
        deployment.expected_runtime_version ||
      deployment.observed_kernel !== deployment.expected_kernel ||
      deployment.trigger_conformance !== true ||
      fixture.upstream_owner_account_id === null ||
      fixture.contributor_account_id === null ||
      fixture.launch_creation_mode === null ||
      runtimeAttestation === null ||
      !providerSettlementComplete ||
      !(
        (raw.cleanup.cma_launch === CMA_LAUNCH_RETAINED &&
          raw.cleanup.session === SESSION_IDLE_AWAITING_TEARDOWN) ||
        (raw.cleanup.cma_launch === EPHEMERAL_STATE_DESTROYED &&
          raw.cleanup.session === EPHEMERAL_STATE_DESTROYED)
      ) ||
      !exactSandboxInventory ||
      raw.correlations.launch.length === 0 ||
      raw.correlations.provider.length === 0 ||
      raw.stages.length === 0 ||
      raw.stages.some(
        (stage) => stage.status !== "passed" || stage.finished_at === null,
      ) ||
      sandboxLiveStages.length !== 1 ||
      runtimeAttestationStages.length !== 1 ||
      raw.stages.indexOf(runtimeAttestationStages[0]) !==
        raw.stages.indexOf(sandboxLiveStages[0]) + 1 ||
      Date.parse(runtimeAttestation.verified_at) <
        Date.parse(runtimeAttestationStages[0].started_at) ||
      Date.parse(runtimeAttestation.verified_at) >
        Date.parse(runtimeAttestationStages[0].finished_at) ||
      sessionCleanupStages.length !== 1 ||
      raw.stages[raw.stages.length - 1] !== sessionCleanupStages[0] ||
      sessionCleanupStages[0].status !== "passed" ||
      sessionCleanupStages[0].finished_at === null ||
      launchCompletionStages.length !== 1 ||
      raw.stages[raw.stages.length - 2] !== launchCompletionStages[0] ||
      launchCompletionStages[0].status !== "passed" ||
      launchCompletionStages[0].finished_at === null ||
      Date.parse(providerSettlement.verified_at) <
        Date.parse(launchCompletionStages[0].started_at) ||
      Date.parse(providerSettlement.verified_at) >
        Date.parse(launchCompletionStages[0].finished_at) ||
      Date.parse(launchCompletionStages[0].finished_at) >
        Date.parse(sessionCleanupStages[0].started_at)
    ) {
      fail(EVIDENCE_ERROR, "passing evidence is incomplete or contradictory");
    }
    const expectedCounts = {
      launch: 1,
      fork: 1,
      fork_created_delta: fixture.expected_fork_mode === "new" ? 1 : 0,
      branch: 1,
      workspace: 1,
      sandbox: 1,
      session: 1,
      initial_owner_message: 1,
      contribution_commit: 1,
      pull_request: 1,
    };
    for (const [key, expected] of Object.entries(expectedCounts)) {
      if (raw.counts[key] !== expected)
        fail(EVIDENCE_ERROR, `passing evidence count ${key} is not exact`);
    }
    for (const key of resourceKeys) {
      if (resources[key] === null)
        fail(EVIDENCE_ERROR, `passing evidence resource ${key} is absent`);
    }
    for (const key of INVENTORY_KEYS) {
      const before = raw.external_inventory.before[key];
      const after = raw.external_inventory.after[key];
      const delta = raw.external_inventory.delta[key];
      if (
        before === null ||
        after === null ||
        delta === null ||
        after - before !== delta
      ) {
        fail(EVIDENCE_ERROR, `passing evidence inventory ${key} is incomplete`);
      }
    }
    const expectedDeltas = {
      launches: 1,
      contributor_forks: fixture.expected_fork_mode === "new" ? 1 : 0,
      launch_branches: 1,
      launch_pull_requests: 1,
    };
    for (const [key, expected] of Object.entries(expectedDeltas)) {
      if (raw.external_inventory.delta[key] !== expected)
        fail(
          EVIDENCE_ERROR,
          `passing evidence inventory delta ${key} is not exact`,
        );
    }
  } else if (raw.result !== "running") {
    const expectedActual = {
      fail: "smoke_failed",
      inconclusive: "inconclusive",
      blocked: "blocked",
    }[raw.result];
    if (raw.actual_result !== expectedActual || raw.failure === null) {
      fail(EVIDENCE_ERROR, "non-passing evidence lacks its closed outcome");
    }
  }

  if (
    publishable &&
    (raw.result === "running" ||
      raw.redaction.status !== "passed" ||
      raw.redaction.audited_at === null)
  ) {
    fail(EVIDENCE_ERROR, "evidence is not publishable");
  }
  return raw;
}

function positiveEnvironmentInteger(name) {
  const raw = requiredEnvironment(name, 32);
  if (!/^\d+$/u.test(raw))
    fail("missing_environment", `${name} is not numeric`);
  const value = Number(raw);
  if (!Number.isSafeInteger(value) || value <= 0)
    fail("missing_environment", `${name} is not a safe positive integer`);
  return value;
}

function workflowContext() {
  const repository = requiredEnvironment("GITHUB_REPOSITORY", 201);
  const parts = repository.split("/");
  if (
    parts.length !== 2 ||
    !LOGIN.test(parts[0]) ||
    !REPOSITORY.test(parts[1])
  ) {
    fail("missing_environment", "GITHUB_REPOSITORY is invalid");
  }
  const workflowCommit = requiredEnvironment("GITHUB_SHA", 40);
  if (!SHA1.test(workflowCommit))
    fail("missing_environment", "GITHUB_SHA is not a full Git commit");
  const runId = positiveEnvironmentInteger("GITHUB_RUN_ID");
  const runAttempt = positiveEnvironmentInteger("GITHUB_RUN_ATTEMPT");
  const repositoryId = positiveEnvironmentInteger("GITHUB_REPOSITORY_ID");
  const serverUrl = new URL(requiredEnvironment("GITHUB_SERVER_URL", 512));
  if (
    serverUrl.protocol !== "https:" ||
    serverUrl.username !== "" ||
    serverUrl.password !== "" ||
    serverUrl.pathname !== "/" ||
    serverUrl.search !== "" ||
    serverUrl.hash !== ""
  ) {
    fail("missing_environment", "GITHUB_SERVER_URL must be one HTTPS origin");
  }
  const operator = {
    login: requiredEnvironment("CMA_TRIGGER_SMOKE_OPERATOR_LOGIN", 39),
    account_id: positiveEnvironmentInteger(
      "CMA_TRIGGER_SMOKE_OPERATOR_ACCOUNT_ID",
    ),
  };
  if (!LOGIN.test(operator.login))
    fail("missing_environment", "smoke operator login is invalid");
  let reviewers;
  try {
    reviewers = JSON.parse(
      requiredEnvironment("CMA_TRIGGER_SMOKE_REVIEWERS_JSON", 16384),
    );
  } catch {
    fail("missing_environment", "smoke reviewer provenance is not JSON");
  }
  const runUrl = `${serverUrl.origin}/${repository}/actions/runs/${String(runId)}`;
  return {
    repository,
    repository_id: repositoryId,
    workflow_commit: workflowCommit,
    run_id: runId,
    run_attempt: runAttempt,
    run_url: runUrl,
    artifact_name: `cma-trigger-provider-smoke-${String(runId)}-${String(runAttempt)}`,
    artifact_url: `${runUrl}#artifacts`,
    operator,
    reviewers,
  };
}

class Evidence {
  constructor(path, config, marker, context) {
    this.path = path;
    this.document = {
      schema_version: 6,
      evidence_id: `p-01-gha-${String(context.workflow.run_id)}-${String(context.workflow.run_attempt)}`,
      ledger_id: "P-01",
      result: "running",
      expected_result:
        "one_completed_launch_to_one_reviewed_contribution_and_open_pull_request_with_provider_settlement",
      actual_result: "running",
      started_at: now(),
      finished_at: null,
      provenance: {
        workflow: {
          repository: context.workflow.repository,
          repository_id: context.workflow.repository_id,
          workflow_commit: context.workflow.workflow_commit,
          run_id: context.workflow.run_id,
          run_attempt: context.workflow.run_attempt,
          run_url: context.workflow.run_url,
          artifact_name: context.workflow.artifact_name,
          artifact_url: context.workflow.artifact_url,
        },
        deployment: {
          environment: "cma-trigger-smoke",
          version: null,
          build_commit: config.expected_build_sha,
          runtime_image_digest: config.runtime_image_digest,
          runtime_image_ids: config.runtime_image_ids,
          expected_runtime_version: config.expected_runtime_version,
          observed_runtime_version: null,
          expected_kernel: TRIGGER_KERNEL_ID,
          observed_kernel: null,
          configuration_revision: config.configuration_revision,
          fixture_config_sha256: context.config_sha256,
          trigger_conformance: false,
        },
      },
      actors: {
        operator: context.workflow.operator,
        reviewers: context.workflow.reviewers,
      },
      fixture: {
        upstream_repository_id: config.upstream.repository_id,
        upstream_owner_account_id: null,
        upstream_owner: config.upstream.owner,
        upstream_repository: config.upstream.repository,
        source_sha: config.source_sha,
        public_trigger_id: config.public_id,
        trigger_revision_digest: config.revision_digest,
        contributor_account_id: null,
        contributor_login: config.contributor.login,
        service_id_sha256: context.service_id_sha256,
        expected_fork_mode: config.contributor.expected_fork_mode,
        launch_creation_mode: null,
        policy: config.fixture_policy,
      },
      action: {
        kind: "rollout",
        name: "p-01-provider-smoke",
        run_marker: marker,
        fault_injection: "none",
        provider_resource_policy: "preserve",
      },
      external_inventory: {
        before: Object.fromEntries(INVENTORY_KEYS.map((key) => [key, null])),
        after: Object.fromEntries(INVENTORY_KEYS.map((key) => [key, null])),
        delta: Object.fromEntries(INVENTORY_KEYS.map((key) => [key, null])),
      },
      sandbox_inventory: {
        namespace: config.trigger_sandbox_namespace,
        api_version: BATCH_SANDBOX_API_VERSION,
        kind: BATCH_SANDBOX_KIND,
        resource: BATCH_SANDBOX_RESOURCE,
        label_contract: { ...SANDBOX_LABELS },
        observations: Object.fromEntries(
          SANDBOX_PHASES.map((phase) => [phase, null]),
        ),
      },
      counts: {
        launch: 0,
        fork: 0,
        fork_created_delta: 0,
        branch: 0,
        workspace: 0,
        sandbox: 0,
        session: 0,
        initial_owner_message: 0,
        contribution_commit: 0,
        pull_request: 0,
      },
      resources: {
        launch_id: null,
        fork_repository_id: null,
        fork_owner_account_id: null,
        branch_name: null,
        workspace_manifest_digest: null,
        workspace_epoch: null,
        session_id: null,
        runtime_generation: null,
        delivery_origin_key: null,
        diff_artifact_id: null,
        contribution_id: null,
        commit_id: null,
        pull_request_provider_id: null,
        pull_request_number: null,
        pull_request_url: null,
      },
      correlations: {
        launch: [],
        provider: [],
      },
      stages: [],
      runtime_attestation: null,
      provider_settlement: {
        launch_state: null,
        reach_state: null,
        session_credential_retirement: null,
        verified_at: null,
      },
      cleanup: {
        github: "preserved_by_policy",
        cma_launch: "not_created",
        session: "not_created",
        provider_object_settlement: null,
      },
      redaction: {
        status: "pending",
        audited_at: null,
        checks: [
          "closed_schema",
          "environment_secrets",
          "kubernetes_credentials",
          "credential_patterns",
          "forbidden_fields",
        ],
      },
      failure: null,
    };
  }

  async write() {
    const temporary = `${this.path}.tmp`;
    await writeFile(temporary, `${JSON.stringify(this.document, null, 2)}\n`, {
      mode: 0o600,
    });
    await rename(temporary, this.path);
  }

  async stage(name, operation) {
    const record = {
      name,
      status: "running",
      started_at: now(),
      finished_at: null,
    };
    this.document.stages.push(record);
    await this.write();
    console.log(`[smoke] ${name}`);
    try {
      const result = await operation();
      record.status = "passed";
      record.finished_at = now();
      await this.write();
      return result;
    } catch (cause) {
      record.status = "failed";
      record.finished_at = now();
      await this.write();
      throw cause;
    }
  }
}

function segment(value) {
  return encodeURIComponent(value);
}

async function fetchJson(url, expectedStatuses) {
  const response = await fetch(url, {
    redirect: "error",
    signal: AbortSignal.timeout(30_000),
    headers: {
      Accept: "application/vnd.github+json, application/json",
      "User-Agent": "cma-trigger-provider-smoke",
      "X-GitHub-Api-Version": "2022-11-28",
    },
  });
  if (!expectedStatuses.includes(response.status)) {
    fail(
      "public_http_refused",
      `public HTTP request returned ${response.status}`,
    );
  }
  const providerRequestId = response.headers.get("x-github-request-id");
  if (
    providerRequestId !== null &&
    (providerRequestId.length === 0 ||
      providerRequestId.length > 255 ||
      /[\u0000-\u001f\u007f]/u.test(providerRequestId))
  ) {
    fail("public_http_malformed", "provider request ID was not safe metadata");
  }
  if (response.status === 404) {
    return {
      status: 404,
      body: null,
      provider_request_id: providerRequestId,
    };
  }
  let bytes;
  try {
    bytes = new Uint8Array(await response.arrayBuffer());
  } catch {
    fail(
      "public_http_malformed",
      "public HTTP response body could not be read",
    );
  }
  if (bytes.byteLength > MAX_PUBLIC_JSON_BYTES) {
    fail(
      "public_http_oversized",
      "public HTTP response exceeded the smoke bound",
    );
  }
  let body;
  try {
    body = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes));
  } catch {
    fail("public_http_malformed", "public HTTP response was not JSON");
  }
  return {
    status: response.status,
    body,
    provider_request_id: providerRequestId,
  };
}

function retainedText(value, maximum, pattern) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    /[\u0000-\u001f\u007f]/u.test(value) ||
    !pattern.test(value)
  ) {
    return null;
  }
  return value;
}

function retainedTimestamp(value) {
  if (
    typeof value !== "string" ||
    !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/u.test(value) ||
    Number.isNaN(Date.parse(value))
  ) {
    return null;
  }
  return value;
}

function sandboxRawLabels(item) {
  const labels = item?.metadata?.labels;
  return labels !== null && typeof labels === "object" && !Array.isArray(labels)
    ? labels
    : {};
}

function exactReservationCandidate(item, namespace, expected) {
  const labels = sandboxRawLabels(item);
  const name = item?.metadata?.name;
  return (
    item?.apiVersion === BATCH_SANDBOX_API_VERSION &&
    item?.kind === BATCH_SANDBOX_KIND &&
    item?.metadata?.namespace === namespace &&
    typeof name === "string" &&
    labels[SANDBOX_LABELS.sandbox_id] === name &&
    labels[SANDBOX_LABELS.canonical_kernel] === TRIGGER_KERNEL_ID &&
    labels[SANDBOX_LABELS.legacy_kernel] === TRIGGER_KERNEL_ID &&
    labels[SANDBOX_LABELS.canonical_session] === expected.session_id &&
    labels[SANDBOX_LABELS.legacy_session] === expected.session_id &&
    labels[SANDBOX_LABELS.runtime_generation] ===
      String(expected.runtime_generation) &&
    RUNTIME_RESERVATION.test(labels[SANDBOX_LABELS.runtime_reservation] ?? "")
  );
}

function sandboxObjectRecord(
  item,
  { namespace, phase, expected, expectedReservation, reservationAmbiguous },
) {
  const metadata =
    item.metadata !== null &&
    typeof item.metadata === "object" &&
    !Array.isArray(item.metadata)
      ? item.metadata
      : {};
  const rawLabels = sandboxRawLabels(item);
  const name = retainedText(metadata.name, 253, KUBERNETES_NAME);
  const uid = retainedText(metadata.uid, 255, SAFE_KUBERNETES_METADATA);
  const resourceVersion = retainedText(
    metadata.resourceVersion,
    255,
    SAFE_KUBERNETES_METADATA,
  );
  const objectNamespace = retainedText(metadata.namespace, 63, DNS1123_LABEL);
  const creationTimestamp = retainedTimestamp(metadata.creationTimestamp);
  const rawDeletionTimestamp = metadata.deletionTimestamp;
  const deletionTimestamp =
    rawDeletionTimestamp === undefined || rawDeletionTimestamp === null
      ? null
      : retainedTimestamp(rawDeletionTimestamp);
  const label = (key) =>
    retainedText(rawLabels[key], 63, KUBERNETES_LABEL_VALUE);
  const sandboxId = label(SANDBOX_LABELS.sandbox_id);
  const canonicalKernel = label(SANDBOX_LABELS.canonical_kernel);
  const legacyKernel = label(SANDBOX_LABELS.legacy_kernel);
  const canonicalSession = label(SANDBOX_LABELS.canonical_session);
  const legacySession = label(SANDBOX_LABELS.legacy_session);
  const runtimeGeneration = label(SANDBOX_LABELS.runtime_generation);
  const rawReservation = rawLabels[SANDBOX_LABELS.runtime_reservation];
  const validReservation =
    typeof rawReservation === "string" &&
    RUNTIME_RESERVATION.test(rawReservation)
      ? rawReservation
      : null;
  const issues = [];
  const issue = (condition, name) => {
    if (condition) issues.push(name);
  };

  issue(phase === "before_launch", "object_present_before_launch");
  issue(item.apiVersion !== BATCH_SANDBOX_API_VERSION, "api_version_mismatch");
  issue(item.kind !== BATCH_SANDBOX_KIND, "kind_mismatch");
  issue(objectNamespace !== namespace, "namespace_mismatch");
  issue(name === null, "name_missing_or_invalid");
  issue(uid === null, "uid_missing_or_invalid");
  issue(resourceVersion === null, "resource_version_missing_or_invalid");
  issue(creationTimestamp === null, "creation_timestamp_missing_or_invalid");
  issue(
    rawDeletionTimestamp !== undefined &&
      rawDeletionTimestamp !== null &&
      deletionTimestamp === null,
    "deletion_timestamp_invalid",
  );
  issue(
    rawDeletionTimestamp !== undefined && rawDeletionTimestamp !== null,
    "deletion_in_progress",
  );
  issue(name === null || sandboxId !== name, "sandbox_id_mismatch");
  issue(canonicalKernel !== TRIGGER_KERNEL_ID, "canonical_kernel_mismatch");
  issue(legacyKernel !== TRIGGER_KERNEL_ID, "legacy_kernel_mismatch");
  issue(
    canonicalKernel !== null &&
      legacyKernel !== null &&
      canonicalKernel !== legacyKernel,
    "kernel_labels_ambiguous",
  );

  if (phase !== "before_launch") {
    issue(
      canonicalSession !== expected.session_id,
      "canonical_session_mismatch",
    );
    issue(legacySession !== expected.session_id, "legacy_session_mismatch");
    issue(
      canonicalSession !== null &&
        legacySession !== null &&
        canonicalSession !== legacySession,
      "session_labels_ambiguous",
    );
    issue(
      runtimeGeneration !== String(expected.runtime_generation),
      "runtime_generation_mismatch",
    );
    issue(
      rawReservation === undefined || rawReservation === null,
      "runtime_reservation_missing",
    );
    issue(
      rawReservation !== undefined &&
        rawReservation !== null &&
        validReservation === null,
      "runtime_reservation_invalid",
    );
    issue(reservationAmbiguous, "runtime_reservation_ambiguous");
    issue(
      validReservation !== null &&
        expectedReservation !== null &&
        validReservation !== expectedReservation,
      "runtime_reservation_mismatch",
    );
  }

  const record = {
    api_version: retainedText(item.apiVersion, 128, SAFE_KUBERNETES_METADATA),
    kind: retainedText(item.kind, 128, SAFE_KUBERNETES_METADATA),
    namespace: objectNamespace,
    name,
    uid,
    resource_version: resourceVersion,
    creation_timestamp: creationTimestamp,
    deletion_timestamp: deletionTimestamp,
    labels: {
      sandbox_id: sandboxId,
      canonical_kernel: canonicalKernel,
      legacy_kernel: legacyKernel,
      canonical_session: canonicalSession,
      legacy_session: legacySession,
      runtime_generation: runtimeGeneration,
      runtime_reservation_sha256:
        validReservation === null ? null : sha256(validReservation),
    },
    issues: [...new Set(issues)].sort(),
  };
  return { record, raw_reservation: validReservation };
}

function classifySandboxInventory(list, { namespace, phase, expected }) {
  if (
    list === null ||
    typeof list !== "object" ||
    Array.isArray(list) ||
    list.apiVersion !== BATCH_SANDBOX_API_VERSION ||
    list.kind !== BATCH_SANDBOX_LIST_KIND ||
    !Array.isArray(list.items) ||
    list.items.length > 10_000 ||
    list.metadata === null ||
    typeof list.metadata !== "object" ||
    Array.isArray(list.metadata)
  ) {
    fail(
      "sandbox_inventory_malformed",
      "Kubernetes returned a malformed BatchSandbox inventory",
    );
  }
  if (
    list.items.some(
      (item) =>
        item === null || typeof item !== "object" || Array.isArray(item),
    )
  ) {
    fail(
      "sandbox_inventory_malformed",
      "Kubernetes returned a non-object BatchSandbox item",
    );
  }
  if (
    typeof list.metadata.continue === "string" &&
    list.metadata.continue.length > 0
  ) {
    fail(
      "sandbox_inventory_incomplete",
      "Kubernetes BatchSandbox inventory retained a continuation token",
    );
  }
  const listResourceVersion = retainedText(
    list.metadata.resourceVersion,
    255,
    SAFE_KUBERNETES_METADATA,
  );
  if (listResourceVersion === null) {
    fail(
      "sandbox_inventory_malformed",
      "Kubernetes BatchSandbox inventory lacks a safe resourceVersion",
    );
  }

  let expectedReservation = expected.runtime_reservation ?? null;
  let reservationAmbiguous = false;
  if (phase !== "before_launch" && expectedReservation === null) {
    const candidates = list.items.filter((item) =>
      exactReservationCandidate(item, namespace, expected),
    );
    const reservations = [
      ...new Set(
        candidates.map(
          (item) => sandboxRawLabels(item)[SANDBOX_LABELS.runtime_reservation],
        ),
      ),
    ];
    if (reservations.length === 1) {
      [expectedReservation] = reservations;
    } else if (reservations.length > 1) {
      reservationAmbiguous = true;
    }
  }
  if (
    expectedReservation !== null &&
    !RUNTIME_RESERVATION.test(expectedReservation)
  ) {
    fail(
      "sandbox_identity_invalid",
      "expected runtime reservation is not an exact provider reservation",
    );
  }

  const entitledObjects = [];
  const unexpectedObjects = [];
  for (const item of list.items) {
    const inspected = sandboxObjectRecord(item, {
      namespace,
      phase,
      expected,
      expectedReservation,
      reservationAmbiguous,
    });
    if (inspected.record.issues.length === 0) {
      entitledObjects.push(inspected.record);
    } else {
      unexpectedObjects.push(inspected.record);
    }
  }
  const observation = {
    phase,
    observed_at: now(),
    list_resource_version: listResourceVersion,
    expected: {
      session_id: expected.session_id,
      runtime_generation: expected.runtime_generation,
      runtime_reservation_sha256:
        expectedReservation === null ? null : sha256(expectedReservation),
    },
    total_objects: list.items.length,
    entitled_objects: entitledObjects,
    unexpected_objects: unexpectedObjects,
  };
  return { observation, runtime_reservation: expectedReservation };
}

async function kubernetesSandboxInventory({
  kubeconfig,
  namespace,
  phase,
  expected,
}) {
  const childEnvironment = {};
  for (const name of [
    "PATH",
    "LANG",
    "LC_ALL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
  ]) {
    if (typeof process.env[name] === "string") {
      childEnvironment[name] = process.env[name];
    }
  }
  let stdout;
  try {
    ({ stdout } = await execFileAsync(
      "kubectl",
      [
        "--kubeconfig",
        kubeconfig,
        "get",
        BATCH_SANDBOX_RESOURCE,
        "--namespace",
        namespace,
        "--output=json",
        "--request-timeout=30s",
        "--chunk-size=0",
      ],
      {
        encoding: "utf8",
        env: childEnvironment,
        timeout: 45_000,
        maxBuffer: MAX_PUBLIC_JSON_BYTES,
      },
    ));
  } catch {
    fail(
      "sandbox_inventory_unavailable",
      "the exact Trigger BatchSandbox inventory could not be read",
    );
  }
  let list;
  try {
    list = JSON.parse(stdout);
  } catch {
    fail(
      "sandbox_inventory_malformed",
      "kubectl did not return a JSON BatchSandbox inventory",
    );
  }
  return classifySandboxInventory(list, { namespace, phase, expected });
}

function retainedAttestationText(value, path, maximum, pattern) {
  const checked = text(value, path, maximum, "runtime_attestation_invalid");
  if (!pattern.test(checked)) {
    fail("runtime_attestation_invalid", `${path} is invalid`);
  }
  return checked;
}

function classifyRuntimeAttestation(
  raw,
  { config, sessionId, runtimeGeneration, sandboxId },
) {
  exactKeys(
    raw,
    ["api", "observed_at", "runtime", "schema_version"],
    "runtime attestation",
    "runtime_attestation_invalid",
  );
  if (
    raw.schema_version !== 1 ||
    !Array.isArray(raw.api) ||
    raw.api.length !== 1 ||
    !Array.isArray(raw.runtime) ||
    raw.runtime.length !== 1 ||
    !isExactUtcTimestamp(raw.observed_at)
  ) {
    fail(
      "runtime_attestation_invalid",
      "runtime attestation is not one fresh API and runtime observation",
    );
  }
  const observed = Date.parse(raw.observed_at);
  if (observed > Date.now() + 5_000 || observed < Date.now() - 60_000) {
    fail("runtime_attestation_stale", "runtime attestation is not fresh");
  }

  const apiPod = raw.api[0];
  exactKeys(
    apiPod,
    [
      "containers",
      "init_containers",
      "labels",
      "name",
      "namespace",
      "phase",
      "resource_version",
      "uid",
    ],
    "runtime attestation API pod",
    "runtime_attestation_invalid",
  );
  exactKeys(
    apiPod.labels,
    [
      "canonical_kernel",
      "canonical_session",
      "runtime_generation",
      "sandbox_id",
    ],
    "runtime attestation API pod labels",
    "runtime_attestation_invalid",
  );
  if (
    apiPod.namespace !== API_ATTESTATION_NAMESPACE ||
    apiPod.phase !== "Running" ||
    Object.values(apiPod.labels).some((value) => value !== null) ||
    !Array.isArray(apiPod.containers) ||
    apiPod.containers.length !== 1 ||
    !Array.isArray(apiPod.init_containers) ||
    apiPod.init_containers.length !== 0
  ) {
    fail(
      "runtime_attestation_mismatch",
      "runtime attestation does not retain the exact staging API Pod",
    );
  }
  retainedAttestationText(
    apiPod.name,
    "runtime attestation API pod name",
    253,
    /^cma-api-[a-z0-9]+-[a-z0-9]+$/u,
  );
  retainedAttestationText(
    apiPod.uid,
    "runtime attestation API pod UID",
    64,
    UUID,
  );
  retainedAttestationText(
    apiPod.resource_version,
    "runtime attestation API pod resource version",
    255,
    SAFE_KUBERNETES_METADATA,
  );
  const apiContainer = apiPod.containers[0];
  exactKeys(
    apiContainer,
    ["image", "image_id", "name", "ready"],
    "runtime attestation API container",
    "runtime_attestation_invalid",
  );
  if (
    apiContainer.name !== "api" ||
    apiContainer.image !== API_ATTESTATION_IMAGE ||
    apiContainer.ready !== true
  ) {
    fail(
      "runtime_attestation_mismatch",
      "runtime attestation API container differs from the staged API",
    );
  }
  runtimeImageIdentity(
    apiContainer.image_id,
    "runtime attestation API image ID",
    "runtime_attestation_invalid",
  );

  const pod = raw.runtime[0];
  exactKeys(
    pod,
    [
      "containers",
      "init_containers",
      "labels",
      "name",
      "namespace",
      "phase",
      "resource_version",
      "uid",
    ],
    "runtime attestation pod",
    "runtime_attestation_invalid",
  );
  exactKeys(
    pod.labels,
    [
      "canonical_kernel",
      "canonical_session",
      "runtime_generation",
      "sandbox_id",
    ],
    "runtime attestation pod labels",
    "runtime_attestation_invalid",
  );
  if (
    pod.namespace !== config.trigger_sandbox_namespace ||
    pod.phase !== "Running" ||
    pod.labels.canonical_kernel !== TRIGGER_KERNEL_ID ||
    pod.labels.canonical_session !== sessionId ||
    pod.labels.runtime_generation !== String(runtimeGeneration) ||
    pod.labels.sandbox_id !== sandboxId ||
    !Array.isArray(pod.containers) ||
    pod.containers.length !== 1 ||
    !Array.isArray(pod.init_containers) ||
    pod.init_containers.length !== 1
  ) {
    fail(
      "runtime_attestation_mismatch",
      "runtime attestation does not bind the exact Launch sandbox",
    );
  }
  retainedAttestationText(
    pod.name,
    "runtime attestation pod name",
    253,
    KUBERNETES_NAME,
  );
  retainedAttestationText(pod.uid, "runtime attestation pod UID", 64, UUID);
  retainedAttestationText(
    pod.resource_version,
    "runtime attestation pod resource version",
    255,
    SAFE_KUBERNETES_METADATA,
  );

  const container = pod.containers[0];
  const initContainer = pod.init_containers[0];
  for (const [name, value] of [
    ["runtime", container],
    ["init", initContainer],
  ]) {
    exactKeys(
      value,
      ["image", "image_id", "name", "ready"],
      `runtime attestation ${name} container`,
      "runtime_attestation_invalid",
    );
    if (value.ready !== true) {
      fail(
        "runtime_attestation_mismatch",
        `runtime attestation ${name} container is not ready`,
      );
    }
  }
  const observedImageId = runtimeImageIdentity(
    container.image_id,
    "runtime attestation image ID",
    "runtime_attestation_invalid",
  );
  if (
    container.name !== "sandbox" ||
    container.image !== config.expected_runtime_version ||
    !config.runtime_image_ids.includes(observedImageId) ||
    initContainer.name !== "execd-installer" ||
    initContainer.image !== EXECD_ATTESTATION_IMAGE
  ) {
    fail(
      "runtime_attestation_mismatch",
      "running container images differ from the approved staging identities",
    );
  }
  const observedInitImageId = runtimeImageIdentity(
    initContainer.image_id,
    "runtime attestation init image ID",
    "runtime_attestation_invalid",
  );

  return {
    configmap_name: RUNTIME_ATTESTATION_CONFIGMAP,
    configmap_namespace: RUNTIME_ATTESTATION_NAMESPACE,
    observed_at: raw.observed_at,
    pod: {
      image_id: observedImageId,
      image_reference: container.image,
      image_ready: container.ready,
      init_image_id: observedInitImageId,
      init_image_reference: initContainer.image,
      init_ready: initContainer.ready,
      name: pod.name,
      namespace: pod.namespace,
      resource_version: pod.resource_version,
      runtime_generation: Number(pod.labels.runtime_generation),
      sandbox_id: pod.labels.sandbox_id,
      session_id: pod.labels.canonical_session,
      uid: pod.uid,
    },
    verified_at: now(),
  };
}

async function kubernetesRuntimeAttestation({
  kubeconfig,
  config,
  sessionId,
  runtimeGeneration,
  sandboxId,
}) {
  const childEnvironment = {};
  for (const name of [
    "PATH",
    "LANG",
    "LC_ALL",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTPS_PROXY",
    "HTTP_PROXY",
    "NO_PROXY",
  ]) {
    if (typeof process.env[name] === "string") {
      childEnvironment[name] = process.env[name];
    }
  }
  let stdout;
  try {
    ({ stdout } = await execFileAsync(
      "kubectl",
      [
        "--kubeconfig",
        kubeconfig,
        "get",
        `configmap/${RUNTIME_ATTESTATION_CONFIGMAP}`,
        "--namespace",
        RUNTIME_ATTESTATION_NAMESPACE,
        "--output=json",
        "--request-timeout=30s",
      ],
      {
        encoding: "utf8",
        env: childEnvironment,
        timeout: 45_000,
        maxBuffer: 1024 * 1024,
      },
    ));
  } catch {
    fail(
      "runtime_attestation_unavailable",
      "the exact redacted runtime attestation could not be read",
    );
  }
  let configmap;
  try {
    configmap = JSON.parse(stdout);
  } catch {
    fail(
      "runtime_attestation_invalid",
      "kubectl did not return a JSON runtime attestation ConfigMap",
    );
  }
  if (
    configmap?.apiVersion !== "v1" ||
    configmap?.kind !== "ConfigMap" ||
    configmap?.metadata?.name !== RUNTIME_ATTESTATION_CONFIGMAP ||
    configmap?.metadata?.namespace !== RUNTIME_ATTESTATION_NAMESPACE ||
    configmap?.data === null ||
    typeof configmap?.data !== "object" ||
    Array.isArray(configmap.data) ||
    JSON.stringify(Object.keys(configmap.data).sort()) !==
      JSON.stringify(["attestation.json"])
  ) {
    fail(
      "runtime_attestation_invalid",
      "runtime attestation ConfigMap escaped its closed identity",
    );
  }
  let attestation;
  try {
    attestation = JSON.parse(configmap.data["attestation.json"]);
  } catch {
    fail(
      "runtime_attestation_invalid",
      "runtime attestation payload is not JSON",
    );
  }
  return classifyRuntimeAttestation(attestation, {
    config,
    sessionId,
    runtimeGeneration,
    sandboxId,
  });
}

async function waitForRuntimeAttestation(
  arguments_,
  timeoutSeconds,
  pollSeconds,
) {
  const deadline = Date.now() + timeoutSeconds * 1000;
  while (true) {
    try {
      return await kubernetesRuntimeAttestation(arguments_);
    } catch (cause) {
      if (
        !(cause instanceof SmokeFailure) ||
        !cause.code.startsWith("runtime_attestation_") ||
        Date.now() >= deadline
      ) {
        throw cause;
      }
    }
    await sleep(pollSeconds * 1000);
  }
}

function exactSandboxExpectation(
  sessionId,
  runtimeGeneration,
  reservation = null,
) {
  if (
    typeof sessionId !== "string" ||
    sessionId.length === 0 ||
    sessionId.length > 63 ||
    !KUBERNETES_LABEL_VALUE.test(sessionId) ||
    !Number.isSafeInteger(runtimeGeneration) ||
    runtimeGeneration <= 0 ||
    (reservation !== null && !RUNTIME_RESERVATION.test(reservation))
  ) {
    fail(
      "sandbox_identity_invalid",
      "Launch session identity cannot fence a BatchSandbox inventory",
    );
  }
  return {
    session_id: sessionId,
    runtime_generation: runtimeGeneration,
    runtime_reservation: reservation,
  };
}

function githubUrl(config, owner, repository, suffix = "") {
  return `${config.github_api_url}/repos/${segment(owner)}/${segment(repository)}${suffix}`;
}

function validateFork(repository, config, upstreamId, accountId) {
  assertEqual(
    upstreamId,
    config.upstream.repository_id,
    "fork_network_mismatch",
    "fork check used another upstream identity",
  );
  assert(
    Number.isSafeInteger(repository.id) && repository.id > 0,
    "fork_identity_mismatch",
    "fork response lacked numeric identity",
  );
  assertEqual(
    repository.owner?.id,
    accountId,
    "fork_identity_mismatch",
    "fork owner numeric identity changed",
  );
  assertEqual(
    repository.owner?.login?.toLowerCase(),
    config.contributor.login.toLowerCase(),
    "fork_identity_mismatch",
    "fork owner login binding changed",
  );
  assertEqual(
    repository.fork,
    true,
    "fork_network_mismatch",
    "contributor repository is not a fork",
  );
  assertEqual(
    repository.source?.id,
    upstreamId,
    "fork_network_mismatch",
    "fork source numeric identity changed",
  );
  assertEqual(
    repository.parent?.id,
    upstreamId,
    "fork_network_mismatch",
    "fork parent numeric identity changed",
  );
}

async function allLaunches(client, publicId) {
  const launches = [];
  let cursor;
  for (let pageNumber = 0; pageNumber < 20; pageNumber += 1) {
    const page = await client.listLaunches({
      trigger_public_id: publicId,
      limit: 100,
      ...(cursor === undefined ? {} : { cursor }),
    });
    launches.push(...page.items);
    if (page.next_cursor === null) return launches;
    cursor = page.next_cursor;
  }
  fail(
    "launch_inventory_unbounded",
    "launch inventory exceeded the smoke bound",
  );
}

function retryable(cause, ApiError, TransportError) {
  return (
    cause instanceof TransportError ||
    (cause instanceof ApiError && cause.isRetryable)
  );
}

async function replayingCommand(key, command, ApiError, TransportError) {
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    try {
      return await command(key);
    } catch (cause) {
      if (attempt === 3 || !retryable(cause, ApiError, TransportError))
        throw cause;
      await sleep(attempt * 1000);
    }
  }
  fail("command_retry_exhausted", "command retry exhausted");
}

async function createAndReclickLaunch({
  client,
  publicId,
  priorLaunchCount,
  createKey,
  reclickKey,
  execute,
}) {
  const creationMode =
    priorLaunchCount === 0 ? "create_or_resume" : "new_after_settled";
  const create = (idempotencyKey) =>
    creationMode === "create_or_resume"
      ? client.createOrResumeLaunch(publicId, { idempotencyKey })
      : client.createNewLaunch(publicId, { idempotencyKey });
  const launch = await execute(createKey, create);
  const replay = await create(createKey);
  const reclick = await client.createOrResumeLaunch(publicId, {
    idempotencyKey: reclickKey,
  });
  assertEqual(
    replay.id,
    launch.id,
    "launch_replay_duplicated",
    "same idempotency key returned another Launch",
  );
  assertEqual(
    reclick.id,
    launch.id,
    "readme_reclick_duplicated",
    "README re-click returned another Launch",
  );
  return { launch, creationMode };
}

function settledPriorLaunch(launch) {
  const retirement = launch?.session_credential_retirement;
  if (
    !SETTLED_PRIOR_LAUNCH_STATES.has(launch?.state) ||
    retirement === null ||
    typeof retirement !== "object" ||
    typeof retirement.session_id !== "string" ||
    retirement.session_id.length === 0 ||
    !isExactUtcTimestamp(retirement.settled_at) ||
    (launch.session !== null &&
      launch.session?.session_id !== retirement.session_id)
  ) {
    return false;
  }
  return (
    launch.reach?.state === "revoked" ||
    (launch.reach === null && launch.state !== "completed")
  );
}

async function selfTestLaunchChoice() {
  function fakeClient(currentId) {
    const calls = [];
    const operations = new Map();
    let current = currentId === undefined ? undefined : { id: currentId };
    const command = (kind, key, make) => {
      calls.push(kind);
      const operation = `${kind}:${key}`;
      if (!operations.has(operation)) operations.set(operation, make());
      current = operations.get(operation);
      return Promise.resolve(current);
    };
    return {
      calls,
      createOrResumeLaunch(_publicId, { idempotencyKey }) {
        return command(
          "resume",
          idempotencyKey,
          () => current ?? { id: "first" },
        );
      },
      createNewLaunch(_publicId, { idempotencyKey }) {
        return command("new", idempotencyKey, () => ({ id: "replacement" }));
      },
    };
  }

  const execute = (_key, command) => command(_key);
  const first = fakeClient();
  const firstResult = await createAndReclickLaunch({
    client: first,
    publicId: "fixture",
    priorLaunchCount: 0,
    createKey: "create",
    reclickKey: "reclick",
    execute,
  });
  assertEqual(
    firstResult.creationMode,
    "create_or_resume",
    "self_test_failed",
    "first launch selected the wrong route",
  );
  assertEqual(
    firstResult.launch.id,
    "first",
    "self_test_failed",
    "first launch identity changed",
  );
  assertEqual(
    first.calls.join(","),
    "resume,resume,resume",
    "self_test_failed",
    "first launch call sequence changed",
  );

  const rerun = fakeClient("open-pr");
  const rerunResult = await createAndReclickLaunch({
    client: rerun,
    publicId: "fixture",
    priorLaunchCount: 1,
    createKey: "create",
    reclickKey: "reclick",
    execute,
  });
  assertEqual(
    rerunResult.creationMode,
    "new_after_settled",
    "self_test_failed",
    "preserved rerun selected the wrong route",
  );
  assertEqual(
    rerunResult.launch.id,
    "replacement",
    "self_test_failed",
    "preserved rerun did not create a new Launch",
  );
  assertEqual(
    rerun.calls.join(","),
    "new,new,resume",
    "self_test_failed",
    "preserved rerun call sequence changed",
  );
  const retiredSession = {
    session_id: "sesn_prior",
    settled_at: "2026-08-28T01:02:45Z",
  };
  assert(
    settledPriorLaunch({
      state: "completed",
      session: { session_id: "sesn_prior" },
      reach: { state: "revoked" },
      session_credential_retirement: retiredSession,
    }) &&
      settledPriorLaunch({
        state: "failed_terminal",
        session: null,
        reach: null,
        session_credential_retirement: retiredSession,
      }),
    "self_test_failed",
    "terminal provider-settled Launch was not treated as settled",
  );
  assert(
    !settledPriorLaunch({
      state: "pr_open",
      session: { session_id: "sesn_prior" },
      reach: { state: "revoked" },
      session_credential_retirement: retiredSession,
    }) &&
      !settledPriorLaunch({ state: "ready" }) &&
      !settledPriorLaunch({ state: "contributing" }) &&
      !settledPriorLaunch({ state: "failed_retryable" }) &&
      !settledPriorLaunch({
        state: "completed",
        session: { session_id: "sesn_prior" },
        reach: { state: "revocation_pending" },
        session_credential_retirement: retiredSession,
      }) &&
      !settledPriorLaunch({
        state: "cancelled",
        session: { session_id: "sesn_prior" },
        reach: null,
        session_credential_retirement: null,
      }) &&
      !settledPriorLaunch({
        state: "cancelled",
        session: { session_id: "sesn_prior" },
        reach: null,
        session_credential_retirement: {
          ...retiredSession,
          session_id: "sesn_other",
        },
      }),
    "self_test_failed",
    "an unsettled Launch was allowed to overlap the smoke",
  );
  console.log("[smoke] launch selection self-test passed");
}

function selfTestFixtureConfig() {
  const runtimeImageDigest = `sha256:${"2".repeat(64)}`;
  return {
    schema_version: 3,
    base_url: "https://cma.example.invalid",
    expected_build_sha: "1".repeat(40),
    runtime_image_digest: runtimeImageDigest,
    runtime_image_ids: [runtimeImageDigest],
    expected_runtime_version: "cma-runtime:ci",
    configuration_revision: "deployment-42",
    trigger_sandbox_namespace: "trigger-workloads",
    public_id: "00000000-0000-4000-8000-000000000001",
    revision_digest: "4".repeat(64),
    source_sha: "3".repeat(40),
    upstream: {
      repository_id: 101,
      owner: "ChronoAIProject",
      repository: "cma-trigger-fixture",
      default_branch: "main",
    },
    contributor: {
      login: "cma-trigger-smoke-user",
      fork_repository: "cma-trigger-fixture",
      expected_fork_mode: "new",
    },
    task: {
      marker_input: "smoke_marker",
      expected_path_template: "cma-trigger-smoke/{marker}.txt",
      expected_content_template: "CMA Trigger provider smoke {marker}\n",
    },
    fixture_policy: {
      repository_owner: "ChronoAIProject",
      lifecycle: "persistent_public_fixture",
      secrets: "forbidden",
      github_cleanup: "preserve",
      cma_state_cleanup: CMA_STATE_CLEANUP_POLICY,
      session_cleanup: SESSION_CLEANUP_POLICY,
    },
    timeouts: {
      launch_seconds: 2400,
      effect_seconds: 600,
      poll_seconds: 5,
    },
    github_api_url: "https://api.github.com",
  };
}

function selfTestConfigValidation() {
  const base = selfTestFixtureConfig();
  validateConfig(JSON.parse(JSON.stringify(base)));
  const mutations = [
    ["extra kubeconfig field", (value) => (value.kubeconfig = "forbidden")],
    [
      "missing Trigger namespace",
      (value) => delete value.trigger_sandbox_namespace,
    ],
    [
      "invalid Trigger namespace",
      (value) => (value.trigger_sandbox_namespace = "Other/Namespace"),
    ],
    [
      "missing runtime image identities",
      (value) => delete value.runtime_image_ids,
    ],
    [
      "runtime image identities omit approved digest",
      (value) => (value.runtime_image_ids = [`sha256:${"a".repeat(64)}`]),
    ],
    [
      "unordered runtime image identities",
      (value) =>
        (value.runtime_image_ids = [
          value.runtime_image_digest,
          `cma-runtime:ci@sha256:${"3".repeat(64)}`,
        ]),
    ],
    [
      "credential-bearing task content",
      (value) =>
        (value.task.expected_content_template =
          "Bearer github_pat_forbidden {marker}"),
    ],
    [
      "missing CMA-state lifecycle",
      (value) => delete value.fixture_policy.cma_state_cleanup,
    ],
    [
      "obsolete session-preservation claim",
      (value) =>
        (value.fixture_policy.session_cleanup =
          "stop_runtime_" + "preserve_session"),
    ],
  ];
  for (const [name, mutate] of mutations) {
    const candidate = JSON.parse(JSON.stringify(base));
    mutate(candidate);
    let refused = false;
    try {
      validateConfig(candidate);
    } catch (cause) {
      refused =
        cause instanceof SmokeFailure && cause.code === "invalid_config";
    }
    assert(
      refused,
      "self_test_failed",
      `config mutation was accepted: ${name}`,
    );
  }
  let hiddenCredentialRefused = false;
  try {
    validateConfigRedaction(
      `{"expected_runtime_version":"Bearer github_pat_hidden","expected_runtime_version":"safe"}`,
    );
  } catch (cause) {
    hiddenCredentialRefused =
      cause instanceof SmokeFailure && cause.code === "invalid_config";
  }
  assert(
    hiddenCredentialRefused,
    "self_test_failed",
    "serialized duplicate-field credential was accepted",
  );
  console.log(
    `[smoke] config schema and redaction self-test passed (${String(mutations.length + 1)} rejected mutations)`,
  );
}

function selfTestRuntimeAttestationDocument(config = selfTestFixtureConfig()) {
  return {
    api: [
      {
        containers: [
          {
            image: API_ATTESTATION_IMAGE,
            image_id: `sha256:${"a".repeat(64)}`,
            name: "api",
            ready: true,
          },
        ],
        init_containers: [],
        labels: {
          canonical_kernel: null,
          canonical_session: null,
          runtime_generation: null,
          sandbox_id: null,
        },
        name: "cma-api-abc123-def45",
        namespace: API_ATTESTATION_NAMESPACE,
        phase: "Running",
        resource_version: "81",
        uid: "00000000-0000-4000-8000-000000000080",
      },
    ],
    observed_at: now(),
    runtime: [
      {
        containers: [
          {
            image: config.expected_runtime_version,
            image_id: config.runtime_image_digest,
            name: "sandbox",
            ready: true,
          },
        ],
        init_containers: [
          {
            image: EXECD_ATTESTATION_IMAGE,
            image_id: EXECD_ATTESTATION_IMAGE,
            name: "execd-installer",
            ready: true,
          },
        ],
        labels: {
          canonical_kernel: TRIGGER_KERNEL_ID,
          canonical_session: "sesn_provider_smoke",
          runtime_generation: "1",
          sandbox_id: "sandbox-provider-smoke",
        },
        name: "sandbox-provider-smoke-runtime",
        namespace: config.trigger_sandbox_namespace,
        phase: "Running",
        resource_version: "82",
        uid: "00000000-0000-4000-8000-000000000081",
      },
    ],
    schema_version: 1,
  };
}

function selfTestRuntimeAttestation() {
  const config = selfTestFixtureConfig();
  const base = selfTestRuntimeAttestationDocument(config);
  const classify = (value) =>
    classifyRuntimeAttestation(value, {
      config,
      sessionId: "sesn_provider_smoke",
      runtimeGeneration: 1,
      sandboxId: "sandbox-provider-smoke",
    });
  const result = classify(JSON.parse(JSON.stringify(base)));
  assertEqual(
    result.pod.image_id,
    config.runtime_image_digest,
    "self_test_failed",
    "runtime attestation did not retain the immutable image ID",
  );
  assertEqual(
    result.pod.session_id,
    "sesn_provider_smoke",
    "self_test_failed",
    "runtime attestation did not retain the exact session",
  );
  const mutations = [
    ["top-level extension", (value) => (value.secret = "forbidden")],
    [
      "stale observation",
      (value) => (value.observed_at = "2026-01-01T00:00:00Z"),
    ],
    ["missing API pod", (value) => (value.api = [])],
    ["wrong API namespace", (value) => (value.api[0].namespace = "other")],
    [
      "API inherited session label",
      (value) => (value.api[0].labels.canonical_session = "sesn_other"),
    ],
    ["missing runtime pod", (value) => (value.runtime = [])],
    [
      "additional runtime pod",
      (value) =>
        value.runtime.push(JSON.parse(JSON.stringify(value.runtime[0]))),
    ],
    [
      "wrong runtime session",
      (value) => (value.runtime[0].labels.canonical_session = "sesn_other"),
    ],
    [
      "wrong runtime generation",
      (value) => (value.runtime[0].labels.runtime_generation = "2"),
    ],
    [
      "wrong sandbox identity",
      (value) => (value.runtime[0].labels.sandbox_id = "sandbox-other"),
    ],
    [
      "unapproved runtime image",
      (value) =>
        (value.runtime[0].containers[0].image_id = `sha256:${"b".repeat(64)}`),
    ],
    [
      "floating runtime reference drift",
      (value) => (value.runtime[0].containers[0].image = "cma-runtime:other"),
    ],
    [
      "runtime not ready",
      (value) => (value.runtime[0].containers[0].ready = false),
    ],
    [
      "init container not ready",
      (value) => (value.runtime[0].init_containers[0].ready = false),
    ],
  ];
  for (const [name, mutate] of mutations) {
    const candidate = JSON.parse(JSON.stringify(base));
    mutate(candidate);
    let refused = false;
    try {
      classify(candidate);
    } catch (cause) {
      refused =
        cause instanceof SmokeFailure &&
        cause.code.startsWith("runtime_attestation_");
    }
    assert(
      refused,
      "self_test_failed",
      `runtime attestation mutation was accepted: ${name}`,
    );
  }
  console.log(
    `[smoke] runtime image attestation self-test passed (${String(mutations.length)} rejected mutations)`,
  );
}

function selfTestBatchSandbox({
  name = "sandbox-provider-smoke",
  uid = "00000000-0000-4000-8000-000000000020",
  resourceVersion = "17",
  session = "sesn_provider_smoke",
  generation = "1",
  reservation = `rres_${"a".repeat(32)}`,
} = {}) {
  return {
    apiVersion: BATCH_SANDBOX_API_VERSION,
    kind: BATCH_SANDBOX_KIND,
    metadata: {
      namespace: "trigger-workloads",
      name,
      uid,
      resourceVersion,
      creationTimestamp: "2026-08-28T01:02:20Z",
      labels: {
        [SANDBOX_LABELS.sandbox_id]: name,
        [SANDBOX_LABELS.canonical_kernel]: TRIGGER_KERNEL_ID,
        [SANDBOX_LABELS.legacy_kernel]: TRIGGER_KERNEL_ID,
        [SANDBOX_LABELS.canonical_session]: session,
        [SANDBOX_LABELS.legacy_session]: session,
        [SANDBOX_LABELS.runtime_generation]: generation,
        [SANDBOX_LABELS.runtime_reservation]: reservation,
      },
    },
  };
}

function selfTestBatchSandboxList(items, resourceVersion) {
  return {
    apiVersion: BATCH_SANDBOX_API_VERSION,
    kind: BATCH_SANDBOX_LIST_KIND,
    metadata: { resourceVersion },
    items,
  };
}

function selfTestSandboxInventory() {
  const expected = exactSandboxExpectation("sesn_provider_smoke", 1);
  const entitled = selfTestBatchSandbox();
  const stale = selfTestBatchSandbox({
    name: "sandbox-stale",
    uid: "00000000-0000-4000-8000-000000000021",
    resourceVersion: "18",
    session: "sesn_stale",
  });
  const extra = classifySandboxInventory(
    selfTestBatchSandboxList([entitled, stale], "19"),
    {
      namespace: "trigger-workloads",
      phase: "live_session",
      expected,
    },
  );
  assert(
    extra.observation.entitled_objects.length === 1 &&
      extra.observation.unexpected_objects.length === 1,
    "self_test_failed",
    "extra or stale live BatchSandbox was not classified as unexpected",
  );

  const conflicting = selfTestBatchSandbox({
    name: "sandbox-conflicting",
    uid: "00000000-0000-4000-8000-000000000022",
    resourceVersion: "20",
    reservation: `rres_${"b".repeat(32)}`,
  });
  const ambiguous = classifySandboxInventory(
    selfTestBatchSandboxList([entitled, conflicting], "21"),
    {
      namespace: "trigger-workloads",
      phase: "live_session",
      expected,
    },
  );
  assert(
    ambiguous.observation.entitled_objects.length === 0 &&
      ambiguous.observation.unexpected_objects.length === 2 &&
      ambiguous.observation.unexpected_objects.every((object) =>
        object.issues.includes("runtime_reservation_ambiguous"),
      ),
    "self_test_failed",
    "ambiguous live reservations were accepted",
  );

  const missing = classifySandboxInventory(selfTestBatchSandboxList([], "22"), {
    namespace: "trigger-workloads",
    phase: "live_session",
    expected,
  });
  assertEqual(
    missing.observation.total_objects,
    0,
    "self_test_failed",
    "missing-live inventory invented a BatchSandbox",
  );

  const leaked = classifySandboxInventory(
    selfTestBatchSandboxList([entitled], "23"),
    {
      namespace: "trigger-workloads",
      phase: "after_stop",
      expected: exactSandboxExpectation(
        "sesn_provider_smoke",
        1,
        `rres_${"a".repeat(32)}`,
      ),
    },
  );
  assertEqual(
    leaked.observation.entitled_objects.length,
    1,
    "self_test_failed",
    "leaked post-stop BatchSandbox was not retained as exact evidence",
  );
  console.log("[smoke] BatchSandbox inventory classifier self-test passed");
}

function terminalProviderSettlement(launch, expectedSessionId) {
  const retirement = launch?.session_credential_retirement;
  if (
    typeof expectedSessionId !== "string" ||
    expectedSessionId.length === 0 ||
    launch?.state !== "completed" ||
    launch?.reach?.state !== "revoked" ||
    retirement === null ||
    typeof retirement !== "object" ||
    retirement.session_id !== expectedSessionId ||
    !isExactUtcTimestamp(retirement.settled_at)
  ) {
    return null;
  }
  return {
    launch_state: "completed",
    reach_state: "revoked",
    session_credential_retirement: {
      session_id: retirement.session_id,
      settled_at: retirement.settled_at,
    },
  };
}

function selfTestTerminalProviderSettlement() {
  const sessionId = "sesn_provider_smoke";
  const launch = {
    state: "completed",
    reach: { state: "revoked" },
    session_credential_retirement: {
      session_id: sessionId,
      settled_at: "2026-08-28T01:02:45Z",
    },
  };
  const settlement = terminalProviderSettlement(launch, sessionId);
  assert(
    settlement !== null &&
      settlement.launch_state === "completed" &&
      settlement.reach_state === "revoked" &&
      settlement.session_credential_retirement.session_id === sessionId &&
      settlement.session_credential_retirement.settled_at ===
        "2026-08-28T01:02:45Z",
    "self_test_failed",
    "positive terminal provider settlement was not recognized",
  );
  assertEqual(
    terminalProviderSettlement(launch, ""),
    null,
    "self_test_failed",
    "terminal provider barrier accepted a missing expected session",
  );

  const mutations = [
    ["nonterminal Launch", (value) => (value.state = "pr_open")],
    ["missing Reach", (value) => (value.reach = null)],
    ["active Reach", (value) => (value.reach.state = "active")],
    [
      "pending Reach revocation",
      (value) => (value.reach.state = "revocation_pending"),
    ],
    ["expired Reach", (value) => (value.reach.state = "expired")],
    [
      "missing credential retirement",
      (value) => (value.session_credential_retirement = null),
    ],
    [
      "wrong retirement session",
      (value) =>
        (value.session_credential_retirement.session_id = "sesn_other"),
    ],
    [
      "invalid retirement timestamp",
      (value) =>
        (value.session_credential_retirement.settled_at = "not-a-timestamp"),
    ],
    [
      "impossible retirement timestamp",
      (value) =>
        (value.session_credential_retirement.settled_at =
          "2026-02-31T01:02:45Z"),
    ],
  ];
  for (const [name, mutate] of mutations) {
    const candidate = JSON.parse(JSON.stringify(launch));
    mutate(candidate);
    assertEqual(
      terminalProviderSettlement(candidate, sessionId),
      null,
      "self_test_failed",
      `terminal provider barrier accepted ${name}`,
    );
  }
  console.log(
    `[smoke] terminal provider settlement self-test passed (${String(mutations.length + 1)} rejected cases)`,
  );
}

function selfTestEvidenceValidation() {
  const config = selfTestFixtureConfig();
  const workflow = {
    repository: "ChronoAIProject/cma",
    repository_id: 1330597394,
    workflow_commit: "5".repeat(40),
    run_id: 42,
    run_attempt: 3,
    run_url: "https://github.com/ChronoAIProject/cma/actions/runs/42",
    artifact_name: "cma-trigger-provider-smoke-42-3",
    artifact_url:
      "https://github.com/ChronoAIProject/cma/actions/runs/42#artifacts",
    operator: { login: "operator-user", account_id: 201 },
    reviewers: [
      {
        login: "reviewer-user",
        account_id: 202,
        approval_observed_at: "2026-08-28T01:01:01Z",
      },
    ],
  };
  const document = new Evidence("unused", config, "gha-42-3", {
    workflow,
    config_sha256: "6".repeat(64),
    service_id_sha256: "7".repeat(64),
  }).document;
  document.result = "pass";
  document.actual_result = "matched_expected";
  document.started_at = "2026-08-28T01:02:00Z";
  document.finished_at = "2026-08-28T01:03:00Z";
  document.provenance.deployment.version = "v1.2.3";
  document.provenance.deployment.observed_runtime_version =
    config.expected_runtime_version;
  document.provenance.deployment.observed_kernel = TRIGGER_KERNEL_ID;
  document.provenance.deployment.trigger_conformance = true;
  document.fixture.upstream_owner_account_id = 301;
  document.fixture.contributor_account_id = 302;
  document.fixture.launch_creation_mode = "create_or_resume";
  document.external_inventory = {
    before: {
      launches: 2,
      contributor_forks: 0,
      launch_branches: 0,
      launch_pull_requests: 0,
    },
    after: {
      launches: 3,
      contributor_forks: 1,
      launch_branches: 1,
      launch_pull_requests: 1,
    },
    delta: {
      launches: 1,
      contributor_forks: 1,
      launch_branches: 1,
      launch_pull_requests: 1,
    },
  };
  const sandboxBefore = classifySandboxInventory(
    selfTestBatchSandboxList([], "16"),
    {
      namespace: config.trigger_sandbox_namespace,
      phase: "before_launch",
      expected: {
        session_id: null,
        runtime_generation: null,
        runtime_reservation: null,
      },
    },
  );
  const sandboxLive = classifySandboxInventory(
    selfTestBatchSandboxList([selfTestBatchSandbox()], "17"),
    {
      namespace: config.trigger_sandbox_namespace,
      phase: "live_session",
      expected: exactSandboxExpectation("sesn_provider_smoke", 1),
    },
  );
  const sandboxAfter = classifySandboxInventory(
    selfTestBatchSandboxList([], "18"),
    {
      namespace: config.trigger_sandbox_namespace,
      phase: "after_stop",
      expected: exactSandboxExpectation(
        "sesn_provider_smoke",
        1,
        sandboxLive.runtime_reservation,
      ),
    },
  );
  sandboxBefore.observation.observed_at = "2026-08-28T01:02:10Z";
  sandboxLive.observation.observed_at = "2026-08-28T01:02:30Z";
  sandboxAfter.observation.observed_at = "2026-08-28T01:02:59Z";
  document.sandbox_inventory.observations = {
    before_launch: sandboxBefore.observation,
    live_session: sandboxLive.observation,
    after_stop: sandboxAfter.observation,
  };
  document.counts = {
    launch: 1,
    fork: 1,
    fork_created_delta: 1,
    branch: 1,
    workspace: 1,
    sandbox: 0,
    session: 1,
    initial_owner_message: 1,
    contribution_commit: 1,
    pull_request: 1,
  };
  document.counts.sandbox =
    document.sandbox_inventory.observations.live_session.entitled_objects.length;
  document.resources = {
    launch_id: "00000000-0000-4000-8000-000000000010",
    fork_repository_id: 401,
    fork_owner_account_id: 302,
    branch_name: "cma/00000000-0000-4000-8000-000000000010",
    workspace_manifest_digest: "8".repeat(64),
    workspace_epoch: 1,
    session_id: "sesn_provider_smoke",
    runtime_generation: 1,
    delivery_origin_key: "trigger:launch:delivery:1",
    diff_artifact_id: "diff_provider_smoke",
    contribution_id: "contribution_provider_smoke",
    commit_id: "9".repeat(40),
    pull_request_provider_id: 501,
    pull_request_number: 7,
    pull_request_url:
      "https://github.com/ChronoAIProject/cma-trigger-fixture/pull/7",
  };
  document.runtime_attestation = {
    configmap_name: RUNTIME_ATTESTATION_CONFIGMAP,
    configmap_namespace: RUNTIME_ATTESTATION_NAMESPACE,
    observed_at: "2026-08-28T01:02:31Z",
    pod: {
      image_id: config.runtime_image_digest,
      image_reference: config.expected_runtime_version,
      image_ready: true,
      init_image_id: EXECD_ATTESTATION_IMAGE,
      init_image_reference: EXECD_ATTESTATION_IMAGE,
      init_ready: true,
      name: "sandbox-provider-smoke-runtime",
      namespace: config.trigger_sandbox_namespace,
      resource_version: "82",
      runtime_generation: document.resources.runtime_generation,
      sandbox_id:
        document.sandbox_inventory.observations.live_session.entitled_objects[0]
          .labels.sandbox_id,
      session_id: document.resources.session_id,
      uid: "00000000-0000-4000-8000-000000000081",
    },
    verified_at: "2026-08-28T01:02:32Z",
  };
  const providerSettlement = terminalProviderSettlement(
    {
      state: "completed",
      reach: { state: "revoked" },
      session_credential_retirement: {
        session_id: document.resources.session_id,
        settled_at: "2026-08-28T01:02:54Z",
      },
    },
    document.resources.session_id,
  );
  assert(
    providerSettlement !== null,
    "self_test_failed",
    "evidence fixture lacks a terminal provider settlement",
  );
  document.provider_settlement = {
    ...providerSettlement,
    verified_at: "2026-08-28T01:02:55Z",
  };
  document.correlations = {
    launch: ["launch-correlation-1"],
    provider: ["github-request-1"],
  };
  document.stages = [
    {
      name: "provider_path",
      status: "passed",
      started_at: "2026-08-28T01:02:00Z",
      finished_at: "2026-08-28T01:02:29Z",
    },
    {
      name: "sandbox_inventory_live_session",
      status: "passed",
      started_at: "2026-08-28T01:02:29Z",
      finished_at: "2026-08-28T01:02:30Z",
    },
    {
      name: "runtime_image_attestation",
      status: "passed",
      started_at: "2026-08-28T01:02:30Z",
      finished_at: "2026-08-28T01:02:33Z",
    },
    {
      name: "launch_completion",
      status: "passed",
      started_at: "2026-08-28T01:02:53Z",
      finished_at: "2026-08-28T01:02:56Z",
    },
    {
      name: "session_cleanup",
      status: "passed",
      started_at: "2026-08-28T01:02:56Z",
      finished_at: "2026-08-28T01:03:00Z",
    },
  ];
  document.cleanup.cma_launch = CMA_LAUNCH_RETAINED;
  document.cleanup.session = SESSION_IDLE_AWAITING_TEARDOWN;
  document.redaction.status = "passed";
  document.redaction.audited_at = "2026-08-28T01:04:00Z";
  validateEvidence(document, { publishable: true });

  const clone = () => JSON.parse(JSON.stringify(document));
  const mutations = [
    ["top-level extension", (value) => (value.access_token = "forbidden")],
    [
      "missing runtime digest",
      (value) => delete value.provenance.deployment.runtime_image_digest,
    ],
    [
      "floating runtime digest",
      (value) => (value.provenance.deployment.runtime_image_digest = "latest"),
    ],
    [
      "runtime observation drift",
      (value) =>
        (value.provenance.deployment.observed_runtime_version = "other"),
    ],
    [
      "missing approved runtime identity set",
      (value) => delete value.provenance.deployment.runtime_image_ids,
    ],
    [
      "approved runtime identity set omits digest",
      (value) =>
        (value.provenance.deployment.runtime_image_ids = [
          `sha256:${"a".repeat(64)}`,
        ]),
    ],
    [
      "missing runtime image attestation",
      (value) => (value.runtime_attestation = null),
    ],
    [
      "stale runtime image attestation",
      (value) =>
        (value.runtime_attestation.observed_at = "2026-08-28T01:01:00Z"),
    ],
    [
      "runtime attestation for another session",
      (value) => (value.runtime_attestation.pod.session_id = "sesn_other"),
    ],
    [
      "runtime attestation for another generation",
      (value) => (value.runtime_attestation.pod.runtime_generation = 2),
    ],
    [
      "runtime attestation for another sandbox",
      (value) => (value.runtime_attestation.pod.sandbox_id = "sandbox-other"),
    ],
    [
      "runtime attestation for an unapproved image",
      (value) =>
        (value.runtime_attestation.pod.image_id = `sha256:${"a".repeat(64)}`),
    ],
    [
      "runtime attestation with non-ready image",
      (value) => (value.runtime_attestation.pod.image_ready = false),
    ],
    [
      "runtime attestation with non-ready init image",
      (value) => (value.runtime_attestation.pod.init_ready = false),
    ],
    [
      "runtime attestation stage detached from inventory",
      (value) => {
        const stage = value.stages.splice(2, 1)[0];
        value.stages.unshift(stage);
      },
    ],
    [
      "wrong expected kernel",
      (value) => (value.provenance.deployment.expected_kernel = "other"),
    ],
    [
      "kernel observation drift",
      (value) => (value.provenance.deployment.observed_kernel = "other"),
    ],
    [
      "missing launch completion stage",
      (value) =>
        value.stages.splice(
          value.stages.findIndex((stage) => stage.name === "launch_completion"),
          1,
        ),
    ],
    [
      "launch completion after session cleanup",
      (value) => value.stages.reverse(),
    ],
    [
      "unproven provider settlement",
      (value) =>
        (value.provider_settlement = {
          launch_state: null,
          reach_state: null,
          session_credential_retirement: null,
          verified_at: null,
        }),
    ],
    [
      "nonterminal provider settlement",
      (value) => (value.provider_settlement.launch_state = "pr_open"),
    ],
    [
      "pending provider reach revocation",
      (value) => (value.provider_settlement.reach_state = "revocation_pending"),
    ],
    [
      "missing session credential retirement",
      (value) =>
        (value.provider_settlement.session_credential_retirement = null),
    ],
    [
      "retirement for another session",
      (value) =>
        (value.provider_settlement.session_credential_retirement.session_id =
          "sesn_other"),
    ],
    [
      "invalid retirement settlement timestamp",
      (value) =>
        (value.provider_settlement.session_credential_retirement.settled_at =
          "not-a-timestamp"),
    ],
    [
      "missing provider settlement observation",
      (value) => (value.provider_settlement.verified_at = null),
    ],
    [
      "provider observation before settlement",
      (value) =>
        (value.provider_settlement.verified_at = "2026-08-28T01:02:53Z"),
    ],
    [
      "session cleanup timestamp before provider barrier",
      (value) =>
        (value.stages[value.stages.length - 1].started_at =
          "2026-08-28T01:02:55Z"),
    ],
    ["missing session cleanup stage", (value) => value.stages.pop()],
    [
      "incomplete session cleanup stage",
      (value) => {
        value.stages[value.stages.length - 1].status = "failed";
        value.stages[value.stages.length - 1].finished_at = null;
      },
    ],
    [
      "cleanup state drift",
      (value) => (value.cleanup.session = SESSION_TEARDOWN_PENDING),
    ],
    [
      "self review",
      (value) =>
        (value.actors.reviewers[0].account_id =
          value.actors.operator.account_id),
    ],
    [
      "late approval observation",
      (value) =>
        (value.actors.reviewers[0].approval_observed_at =
          "2026-08-28T01:02:01Z"),
    ],
    [
      "missing numeric contributor",
      (value) => (value.fixture.contributor_account_id = null),
    ],
    [
      "raw service identity extension",
      (value) => (value.fixture.service_id = "must-not-appear"),
    ],
    [
      "extra live BatchSandbox object",
      (value) => {
        const observation = value.sandbox_inventory.observations.live_session;
        const extra = JSON.parse(
          JSON.stringify(observation.entitled_objects[0]),
        );
        extra.name = "sandbox-extra";
        extra.uid = "00000000-0000-4000-8000-000000000023";
        extra.resource_version = "24";
        extra.labels.sandbox_id = extra.name;
        extra.labels.canonical_session = "sesn_stale";
        extra.labels.legacy_session = "sesn_stale";
        extra.issues = [
          "canonical_session_mismatch",
          "legacy_session_mismatch",
        ];
        observation.unexpected_objects.push(extra);
        observation.total_objects += 1;
      },
    ],
    [
      "stale or ambiguous live BatchSandbox object",
      (value) => {
        const observation = value.sandbox_inventory.observations.live_session;
        const stale = observation.entitled_objects.pop();
        stale.labels.canonical_session = "sesn_stale";
        stale.issues = [
          "canonical_session_mismatch",
          "session_labels_ambiguous",
        ];
        observation.unexpected_objects.push(stale);
        value.counts.sandbox = observation.entitled_objects.length;
      },
    ],
    [
      "missing live BatchSandbox object",
      (value) => {
        value.sandbox_inventory.observations.live_session = null;
        value.counts.sandbox = 0;
      },
    ],
    [
      "leaked post-stop BatchSandbox object",
      (value) => {
        const live =
          value.sandbox_inventory.observations.live_session.entitled_objects[0];
        const after = value.sandbox_inventory.observations.after_stop;
        after.entitled_objects.push(JSON.parse(JSON.stringify(live)));
        after.total_objects = after.entitled_objects.length;
      },
    ],
    [
      "raw sandbox reservation extension",
      (value) =>
        (value.sandbox_inventory.observations.live_session.entitled_objects[0].labels.runtime_reservation = `rres_${"f".repeat(32)}`),
    ],
    [
      "inventory mismatch",
      (value) => (value.external_inventory.after.launches += 1),
    ],
    ["count mismatch", (value) => (value.counts.workspace = 0)],
    ["fabricated sandbox count", (value) => (value.counts.sandbox = 0)],
    [
      "missing provider correlation",
      (value) => (value.correlations.provider = []),
    ],
    [
      "unbound artifact URL",
      (value) =>
        (value.provenance.workflow.artifact_url = `${value.provenance.workflow.run_url}#other`),
    ],
    ["missing resource", (value) => (value.resources.session_id = null)],
    [
      "pending redaction",
      (value) => {
        value.redaction.status = "pending";
        value.redaction.audited_at = null;
      },
    ],
    [
      "running artifact",
      (value) => {
        value.result = "running";
        value.actual_result = "running";
        value.finished_at = null;
        value.redaction.status = "pending";
        value.redaction.audited_at = null;
      },
    ],
  ];
  for (const [name, mutate] of mutations) {
    const candidate = clone();
    mutate(candidate);
    let refused = false;
    try {
      validateEvidence(candidate, { publishable: true });
    } catch (cause) {
      refused = cause instanceof SmokeFailure && cause.code === EVIDENCE_ERROR;
    }
    assert(
      refused,
      "self_test_failed",
      `evidence mutation was accepted: ${name}`,
    );
  }

  const failure = clone();
  failure.result = "fail";
  failure.actual_result = "smoke_failed";
  failure.failure = {
    stage: "provider_path",
    kind: "SmokeFailure",
    code: "provider_refused",
    http_status: 503,
    api_code: null,
    request_id: "request-safe",
  };
  validateEvidence(failure, { publishable: true });

  const providerObjectSettlement = {
    schema_version: 1,
    kind: "cma-trigger-provider-object-settlement",
    evidence_id: document.evidence_id,
    workflow: {
      repository_id: document.provenance.workflow.repository_id,
      run_id: document.provenance.workflow.run_id,
      run_attempt: document.provenance.workflow.run_attempt,
      source_sha: document.provenance.workflow.workflow_commit,
    },
    storage_contract: {
      repository: STORAGE_CONTRACT_REPOSITORY,
      source_sha: STORAGE_CONTRACT_SOURCE_SHA,
    },
    owner_service_account_id_sha256: sha256(
      "66666666-6666-4666-8666-666666666666",
    ),
    launch_id_sha256: sha256(document.resources.launch_id),
    session_id_sha256: sha256(document.resources.session_id),
    settled_at: "2026-08-28T01:05:00Z",
    buckets: {
      context: {
        bucket_name_sha256: sha256("cma-trigger-context"),
        selected_objects: 1,
        selected_bytes_observed: 1024,
        selected_objects_without_size: 0,
        legacy_deletions_acknowledged: 1,
        fenced_deletions_acknowledged: 0,
        unrelated_live_objects_observed: 0,
        current_run_live_objects_after: 0,
        tombstone_retention: STORAGE_TOMBSTONE_RETENTION,
      },
      trigger_workspace: {
        bucket_name_sha256: sha256("cma-trigger-workspace"),
        selected_objects: 2,
        selected_bytes_observed: 2048,
        selected_objects_without_size: 1,
        legacy_deletions_acknowledged: 1,
        fenced_deletions_acknowledged: 1,
        unrelated_live_objects_observed: 0,
        current_run_live_objects_after: 0,
        tombstone_retention: STORAGE_TOMBSTONE_RETENTION,
      },
    },
  };
  const receipt = {
    schema_version: 2,
    cluster: "cma-trigger-smoke",
    run_id: "42",
    run_attempt: "3",
    cluster_destroyed: true,
    credentials_purged: true,
    provider_object_settlement: providerObjectSettlement,
  };
  const beforeFinalization = clone();
  const providerSettlementBeforeFinalization = JSON.stringify(
    beforeFinalization.provider_settlement,
  );
  const finalized = finalizeEphemeralCleanupEvidence(
    beforeFinalization,
    receipt,
    "42",
    "3",
  );
  assertEqual(
    finalized.cleanup.cma_launch,
    EPHEMERAL_STATE_DESTROYED,
    "self_test_failed",
    "terminal cleanup did not close CMA Launch state",
  );
  assertEqual(
    finalized.cleanup.session,
    EPHEMERAL_STATE_DESTROYED,
    "self_test_failed",
    "terminal cleanup did not close session state",
  );
  assertEqual(
    JSON.stringify(finalized.provider_settlement),
    providerSettlementBeforeFinalization,
    "self_test_failed",
    "terminal local cleanup rewrote provider settlement evidence",
  );
  assertEqual(
    JSON.stringify(finalized.cleanup.provider_object_settlement),
    JSON.stringify(providerObjectSettlement),
    "self_test_failed",
    "terminal cleanup did not retain provider-object settlement evidence",
  );
  validateEvidence(finalized, { publishable: true });
  const unproven = clone();
  unproven.provider_settlement = {
    launch_state: null,
    reach_state: null,
    session_credential_retirement: null,
    verified_at: null,
  };
  assertThrowsCode(
    () => finalizeEphemeralCleanupEvidence(unproven, receipt, "42", "3"),
    EVIDENCE_ERROR,
  );
  for (const [name, mutate] of [
    [
      "missing provider-object settlement",
      (value) => (value.provider_object_settlement = null),
    ],
    [
      "wrong provider workflow repository",
      (value) => (value.provider_object_settlement.workflow.repository_id += 1),
    ],
    [
      "wrong provider storage contract",
      (value) =>
        (value.provider_object_settlement.storage_contract.source_sha =
          "0".repeat(40)),
    ],
    [
      "wrong provider Launch",
      (value) =>
        (value.provider_object_settlement.launch_id_sha256 = "0".repeat(64)),
    ],
    [
      "wrong provider session",
      (value) =>
        (value.provider_object_settlement.session_id_sha256 = "0".repeat(64)),
    ],
    [
      "unrelated provider object",
      (value) =>
        (value.provider_object_settlement.buckets.context.unrelated_live_objects_observed = 1),
    ],
    [
      "residual current-run provider object",
      (value) =>
        (value.provider_object_settlement.buckets.trigger_workspace.current_run_live_objects_after = 1),
    ],
    [
      "provider deletion accounting drift",
      (value) =>
        (value.provider_object_settlement.buckets.trigger_workspace.selected_objects += 1),
    ],
    [
      "provider tombstone retention drift",
      (value) =>
        (value.provider_object_settlement.buckets.context.tombstone_retention =
          "deleted"),
    ],
  ]) {
    const candidate = JSON.parse(JSON.stringify(receipt));
    mutate(candidate);
    let refused = false;
    try {
      finalizeEphemeralCleanupEvidence(clone(), candidate, "42", "3");
    } catch (cause) {
      refused =
        cause instanceof SmokeFailure &&
        [EVIDENCE_ERROR, "cleanup_receipt_invalid"].includes(cause.code);
    }
    assert(
      refused,
      "self_test_failed",
      `provider cleanup mutation was accepted: ${name}`,
    );
  }
  assertThrowsCode(
    () =>
      finalizeEphemeralCleanupEvidence(
        clone(),
        { ...receipt, run_id: "43" },
        "42",
        "3",
      ),
    "cleanup_receipt_invalid",
  );
  assertThrowsCode(
    () =>
      finalizeEphemeralCleanupEvidence(
        clone(),
        { ...receipt, schema_version: 1 },
        "42",
        "3",
      ),
    "cleanup_receipt_invalid",
  );
  assertThrowsCode(
    () =>
      finalizeEphemeralCleanupEvidence(
        clone(),
        { ...receipt, provider_reach_revoked: true },
        "42",
        "3",
      ),
    "cleanup_receipt_invalid",
  );
  console.log(
    `[smoke] evidence schema self-test passed (${String(mutations.length)} rejected mutations)`,
  );
}

function launchFailure(launch) {
  if (FAILURE_STATES.has(launch.state)) return true;
  return launch.delivery?.state === "recovery_required";
}

async function waitForLaunch(
  client,
  launchId,
  timeoutSeconds,
  pollSeconds,
  predicate,
) {
  const deadline = Date.now() + timeoutSeconds * 1000;
  while (Date.now() < deadline) {
    const launch = await client.readLaunch(launchId);
    if (launchFailure(launch)) {
      fail(
        launch.failure?.code ?? "launch_failed",
        "Launch entered a non-automatic recovery state",
      );
    }
    if (predicate(launch)) return launch;
    await sleep(pollSeconds * 1000);
  }
  fail(
    "launch_poll_timeout",
    "Launch did not reach the required durable state",
  );
}

async function ownerMessageCount(client, sessionId, payloadHash) {
  const session = await client.readSession(sessionId);
  assert(
    Number.isSafeInteger(session.lastSeq) && session.lastSeq > 0,
    "session_event_cursor_missing",
    "session had no durable event cursor",
  );
  let count = 0;
  let matching = 0;
  for await (const frame of client.streamEvents(sessionId, {
    startingAfter: 0,
    signal: AbortSignal.timeout(60_000),
    maxEmptyConnections: 1,
    jitterRatio: 0,
  })) {
    if (frame.event.type === "owner_message") {
      count += 1;
      if (sha256(frame.event.content) === payloadHash) matching += 1;
    }
    if (frame.seq >= session.lastSeq) break;
  }
  assertEqual(
    count,
    1,
    "owner_message_count_mismatch",
    "session did not contain exactly one owner message",
  );
  assertEqual(
    matching,
    1,
    "owner_message_digest_mismatch",
    "owner message did not match the Trigger delivery digest",
  );
  return session;
}

async function launchProgress(client, launchId) {
  const events = [];
  let cursor = 0;
  for (let page = 0; page < 50; page += 1) {
    const result = await client.readLaunchProgress(launchId, cursor, 100);
    if (result.events.length === 0) return events;
    events.push(...result.events);
    assert(
      result.next_cursor > cursor,
      "launch_progress_stalled",
      "Launch progress cursor did not advance",
    );
    cursor = result.next_cursor;
  }
  fail("launch_progress_unbounded", "Launch progress exceeded the smoke bound");
}

function safeFailure(cause, stage) {
  const failure = {
    stage,
    kind: cause instanceof Error ? cause.name : "UnknownFailure",
    code: "smoke_failed",
    http_status: null,
    api_code: null,
    request_id: null,
  };
  if (cause instanceof SmokeFailure) failure.code = cause.code;
  if (typeof cause?.status === "number") failure.http_status = cause.status;
  if (typeof cause?.code === "string" && cause.code.length <= 128)
    failure.api_code = cause.code;
  if (typeof cause?.requestId === "string" && cause.requestId.length <= 255)
    failure.request_id = cause.requestId;
  return failure;
}

async function runSmoke(configPath, evidencePath) {
  const loadedConfig = await loadConfig(configPath);
  const config = loadedConfig.value;
  const marker = requiredEnvironment("CMA_TRIGGER_SMOKE_RUN_MARKER", 63);
  if (!RUN_MARKER.test(marker))
    fail("invalid_run_marker", "run marker is invalid");
  const serviceId = requiredEnvironment("CMA_TRIGGER_SMOKE_SERVICE_ID", 255);
  const context = {
    workflow: workflowContext(),
    config_sha256: loadedConfig.sha256,
    service_id_sha256: sha256(serviceId),
  };
  const evidence = new Evidence(evidencePath, config, marker, context);
  validateEvidence(evidence.document);
  await evidence.write();
  let currentStage = "environment";
  let client;
  let sessionId;
  let runtimeReservation;

  try {
    const accessToken = requiredEnvironment("CMA_TRIGGER_SMOKE_ACCESS_TOKEN");
    const kubeconfig = requiredEnvironment(
      "CMA_TRIGGER_SMOKE_KUBECONFIG",
      4096,
    );
    if (!kubeconfig.startsWith("/")) {
      fail(
        "invalid_kubeconfig_path",
        "CMA_TRIGGER_SMOKE_KUBECONFIG must be an absolute runner path",
      );
    }
    await access(kubeconfig, fsConstants.R_OK);
    await access(SDK_ENTRY, fsConstants.R_OK);
    const sdk = await import(SDK_ENTRY);
    let requestSequence = 0;
    client = sdk.createClient({
      baseUrl: config.base_url,
      cookies: false,
      credential: () => accessToken,
      requestTimeoutMs: 30_000,
      requestIdProvider: () =>
        `cma-smoke-${marker}-${String(++requestSequence)}`,
    });

    currentStage = "deployment_preflight";
    await evidence.stage(currentStage, async () => {
      const conformance = await client.readConformance();
      assert(
        conformance.conformsTo.includes(TRIGGER_CONFORMANCE),
        "trigger_conformance_missing",
        "deployment does not advertise Trigger conformance",
      );
      assertEqual(
        conformance.buildSha,
        config.expected_build_sha,
        "deployment_build_mismatch",
        "deployment build does not match the approved smoke target",
      );
      const openapi = (
        await fetchJson(`${config.base_url}/openapi.json`, [200])
      ).body;
      for (const path of REQUIRED_OPENAPI_PATHS) {
        assert(
          openapi?.paths?.[path] !== undefined,
          "trigger_openapi_missing",
          "deployment OpenAPI is missing a required Trigger route",
        );
      }
      evidence.document.provenance.deployment.version = conformance.version;
      evidence.document.provenance.deployment.trigger_conformance = true;
    });

    currentStage = "fixture_preflight";
    let upstream;
    let preFork;
    let connection;
    await evidence.stage(currentStage, async () => {
      const preview = await client.readTriggerPreview(config.public_id);
      assertEqual(
        preview.availability,
        "published",
        "trigger_not_published",
        "fixture Trigger is not published",
      );
      assertEqual(
        preview.repository_id,
        config.upstream.repository_id,
        "trigger_repository_mismatch",
        "Trigger numeric repository identity changed",
      );
      assertEqual(
        preview.revision_digest,
        config.revision_digest,
        "trigger_revision_mismatch",
        "Trigger revision digest changed",
      );
      assertEqual(
        preview.allow_forking,
        true,
        "fixture_forking_disabled",
        "fixture does not allow forking",
      );
      assertEqual(
        preview.visibility,
        "public",
        "fixture_not_public",
        "fixture is not public",
      );

      upstream = (
        await fetchJson(
          githubUrl(config, config.upstream.owner, config.upstream.repository),
          [200],
        )
      ).body;
      assertEqual(
        upstream.id,
        config.upstream.repository_id,
        "upstream_identity_mismatch",
        "upstream numeric repository identity changed",
      );
      assert(
        Number.isSafeInteger(upstream.owner?.id) && upstream.owner.id > 0,
        "upstream_owner_identity_mismatch",
        "upstream owner lacks numeric provider identity",
      );
      evidence.document.fixture.upstream_owner_account_id = upstream.owner.id;
      assertEqual(
        upstream.full_name?.toLowerCase(),
        `${config.upstream.owner}/${config.upstream.repository}`.toLowerCase(),
        "upstream_binding_mismatch",
        "upstream owner/name binding changed",
      );
      assertEqual(
        upstream.default_branch,
        config.upstream.default_branch,
        "upstream_default_branch_mismatch",
        "fixture default branch changed",
      );
      assertEqual(
        upstream.fork,
        false,
        "upstream_network_mismatch",
        "fixture upstream unexpectedly became a fork",
      );
      const sourceRef = (
        await fetchJson(
          githubUrl(
            config,
            config.upstream.owner,
            config.upstream.repository,
            `/git/ref/heads/${segment(config.upstream.default_branch)}`,
          ),
          [200],
        )
      ).body;
      assertEqual(
        sourceRef.object?.sha,
        config.source_sha,
        "fixture_source_mismatch",
        "fixture source ref moved from the pinned smoke SHA",
      );

      connection = await client.preflightTriggerConnection(serviceId);
      assertEqual(
        connection.state,
        "active",
        "connection_inactive",
        "NyxID GitHub connection is not active",
      );
      assertEqual(
        connection.account_login?.toLowerCase(),
        config.contributor.login.toLowerCase(),
        "connection_account_mismatch",
        "NyxID connection account binding changed",
      );
      assert(
        Number.isSafeInteger(connection.account_id) &&
          connection.account_id > 0,
        "connection_account_mismatch",
        "NyxID connection lacks numeric account identity",
      );
      evidence.document.fixture.contributor_account_id = connection.account_id;
      assert(
        connection.granted_scopes.includes("public_repo"),
        "connection_scope_missing",
        "NyxID connection lacks public_repo",
      );
      assert(
        !connection.granted_scopes.includes("repo") &&
          !connection.granted_scopes.includes("workflow"),
        "connection_scope_too_broad",
        "smoke connection exceeds the public-repository scope policy",
      );

      preFork = await fetchJson(
        githubUrl(
          config,
          config.contributor.login,
          config.contributor.fork_repository,
        ),
        [200, 404],
      );
      evidence.document.external_inventory.before.contributor_forks =
        preFork.status === 404 ? 0 : 1;
      if (config.contributor.expected_fork_mode === "new") {
        assertEqual(
          preFork.status,
          404,
          "fork_precondition_mismatch",
          "new-fork smoke requires an absent contributor repository",
        );
      } else {
        assertEqual(
          preFork.status,
          200,
          "fork_precondition_mismatch",
          "reuse smoke requires an existing contributor fork",
        );
        validateFork(
          preFork.body,
          config,
          config.upstream.repository_id,
          connection.account_id,
        );
      }
    });

    currentStage = "sandbox_inventory_before_launch";
    await evidence.stage(currentStage, async () => {
      const inventory = await kubernetesSandboxInventory({
        kubeconfig,
        namespace: config.trigger_sandbox_namespace,
        phase: "before_launch",
        expected: {
          session_id: null,
          runtime_generation: null,
          runtime_reservation: null,
        },
      });
      evidence.document.sandbox_inventory.observations.before_launch =
        inventory.observation;
      assertEqual(
        inventory.observation.total_objects,
        0,
        "sandbox_precondition_mismatch",
        "Trigger namespace was not empty before Launch creation",
      );
      assertEqual(
        inventory.observation.unexpected_objects.length,
        0,
        "sandbox_precondition_mismatch",
        "Trigger namespace contained an unexpected BatchSandbox before Launch",
      );
    });

    currentStage = "launch_and_reclick";
    let launch;
    let launchCountBefore;
    await evidence.stage(currentStage, async () => {
      const before = await allLaunches(client, config.public_id);
      assert(
        before.every(settledPriorLaunch),
        "active_launch_precondition",
        "disposable contributor has an unsettled fixture Launch",
      );
      launchCountBefore = before.length;
      evidence.document.external_inventory.before.launches = before.length;
      const selected = await createAndReclickLaunch({
        client,
        publicId: config.public_id,
        priorLaunchCount: before.length,
        createKey: `cma-smoke-${marker}-launch`,
        reclickKey: `cma-smoke-${marker}-reclick`,
        execute: (key, command) =>
          replayingCommand(key, command, sdk.ApiError, sdk.TransportError),
      });
      launch = selected.launch;
      const after = await allLaunches(client, config.public_id);
      assertEqual(
        after.length - launchCountBefore,
        1,
        "launch_count_mismatch",
        "Launch inventory delta was not exactly one",
      );
      evidence.document.external_inventory.after.launches = after.length;
      evidence.document.external_inventory.delta.launches =
        after.length - launchCountBefore;
      assertEqual(
        after.filter((candidate) => candidate.id === launch.id).length,
        1,
        "launch_count_mismatch",
        "created Launch appeared more than once",
      );
      assertEqual(
        launch.revision_digest,
        config.revision_digest,
        "launch_revision_mismatch",
        "Launch did not capture the pinned revision",
      );
      assertEqual(
        launch.source_sha,
        config.source_sha,
        "launch_source_mismatch",
        "Launch did not capture the pinned source SHA",
      );
      assertEqual(
        launch.revision_content.upstream.repository.repository_id,
        config.upstream.repository_id,
        "launch_repository_mismatch",
        "Launch did not capture the numeric upstream identity",
      );
      evidence.document.counts.launch = 1;
      evidence.document.resources.launch_id = launch.id;
      evidence.document.cleanup.cma_launch = CMA_LAUNCH_RETAINED;
      evidence.document.fixture.launch_creation_mode = selected.creationMode;
      evidence.document.external_inventory.before.launch_branches = 0;

      if (preFork.status === 200) {
        // A preserved pr_open Launch already owns another cma/<launch-id>
        // branch. Prove this new Launch cannot advance that earlier branch.
        const expectedBranch = `cma/${launch.id}`;
        const existingBranch = await fetchJson(
          githubUrl(
            config,
            config.contributor.login,
            config.contributor.fork_repository,
            `/git/ref/heads/${segment(expectedBranch)}`,
          ),
          [200, 404],
        );
        assertEqual(
          existingBranch.status,
          404,
          "branch_precondition_mismatch",
          "new Launch branch already exists in the preserved fork",
        );
      }
    });

    currentStage = "launch_start";
    const expectedPath = config.task.expected_path_template.replace(
      "{marker}",
      marker,
    );
    const expectedContent = config.task.expected_content_template.replace(
      "{marker}",
      marker,
    );
    repositoryPath(expectedPath, "resolved expected task path");
    await evidence.stage(currentStage, async () => {
      const overrides = {
        ...launch.overrides,
        task_inputs: {
          ...launch.overrides.task_inputs,
          [config.task.marker_input]: marker,
        },
      };
      launch = await replayingCommand(
        `cma-smoke-${marker}-overrides`,
        (key) =>
          client.updateLaunchOverrides(
            launch.id,
            launch.revision_number,
            overrides,
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      launch = await replayingCommand(
        `cma-smoke-${marker}-connection`,
        (key) =>
          client.selectLaunchConnection(
            launch.id,
            launch.revision_number,
            serviceId,
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      assertEqual(
        launch.workflow_paths.length,
        0,
        "fixture_workflow_paths_present",
        "provider smoke fixture must not require workflow acknowledgement",
      );
      launch = await replayingCommand(
        `cma-smoke-${marker}-start`,
        (key) =>
          client.startLaunch(
            launch.id,
            launch.revision_number,
            launch.effective.digest,
            false,
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      launch = await waitForLaunch(
        client,
        launch.id,
        config.timeouts.launch_seconds,
        config.timeouts.poll_seconds,
        (candidate) =>
          candidate.state === "ready" &&
          candidate.delivery?.state === "completed",
      );
      assert(
        launch.fork !== null &&
          launch.branch !== null &&
          launch.workspace !== null &&
          launch.session !== null &&
          launch.delivery !== null,
        "launch_resources_incomplete",
        "ready Launch lacks a required resource",
      );
      assert(
        launch.runtime_admission !== null && launch.start_admission !== null,
        "runtime_admission_missing",
        "ready Launch lacks its reviewed and Start-time runtime identity",
      );
      sessionId = launch.session.session_id;
      evidence.document.resources.session_id = sessionId;
      evidence.document.cleanup.session = SESSION_TEARDOWN_PENDING;
      assertEqual(
        launch.runtime_admission.runtime_version,
        config.expected_runtime_version,
        "runtime_version_mismatch",
        "reviewed runtime version differs from the approved smoke target",
      );
      assertEqual(
        launch.start_admission.runtime_version,
        config.expected_runtime_version,
        "runtime_version_mismatch",
        "Start-time runtime version differs from the approved smoke target",
      );
      evidence.document.provenance.deployment.observed_runtime_version =
        launch.start_admission.runtime_version;
      evidence.document.provenance.deployment.observed_kernel =
        launch.runtime_admission.kernel;
      assertEqual(
        launch.runtime_admission.kernel,
        TRIGGER_KERNEL_ID,
        "trigger_kernel_mismatch",
        "reviewed kernel differs from the code-owned Trigger kernel",
      );
      assertEqual(
        launch.fork.reused,
        config.contributor.expected_fork_mode === "reuse",
        "fork_mode_mismatch",
        "Launch fork result did not match the declared precondition",
      );
      assertEqual(
        launch.fork.repository.repository_id > 0,
        true,
        "fork_identity_mismatch",
        "Launch fork lacks numeric identity",
      );
      assertEqual(
        launch.delivery.session_id,
        launch.session.session_id,
        "session_identity_mismatch",
        "delivery and Launch session identities differ",
      );
      const createdSession = await client.readSession(
        launch.session.session_id,
      );
      assertEqual(
        createdSession.id,
        launch.session.session_id,
        "session_identity_mismatch",
        "created session identity differs from the Launch session",
      );
      assertEqual(
        createdSession.kernel,
        TRIGGER_KERNEL_ID,
        "trigger_kernel_mismatch",
        "created session did not retain the code-owned Trigger kernel",
      );
      assertEqual(
        launch.delivery.launch_id,
        launch.id,
        "delivery_identity_mismatch",
        "delivery belongs to another Launch",
      );
      assertEqual(
        launch.delivery.revision_digest,
        config.revision_digest,
        "delivery_revision_mismatch",
        "delivery revision changed",
      );
      assertEqual(
        launch.delivery.recovery_generation,
        0,
        "delivery_recovery_unexpected",
        "happy path used explicit prompt recovery",
      );
      assertEqual(
        launch.branch.name,
        `cma/${launch.id}`,
        "branch_identity_mismatch",
        "Launch branch did not use the fenced Launch identity",
      );
      evidence.document.counts.fork = 1;
      evidence.document.counts.branch = 1;
      evidence.document.counts.workspace = 1;
      evidence.document.counts.session = 1;
      evidence.document.resources.fork_repository_id =
        launch.fork.repository.repository_id;
      evidence.document.resources.fork_owner_account_id =
        launch.fork.repository.owner_id;
      evidence.document.resources.branch_name = launch.branch.name;
      evidence.document.resources.workspace_manifest_digest =
        launch.workspace.baseline.digest;
      evidence.document.resources.workspace_epoch =
        launch.workspace.workspace_epoch;
      evidence.document.resources.runtime_generation =
        launch.session.runtime_generation;
      evidence.document.resources.delivery_origin_key =
        launch.delivery.origin_key;
      evidence.document.cleanup.session = SESSION_TEARDOWN_PENDING;
    });

    currentStage = "sandbox_inventory_live_session";
    await evidence.stage(currentStage, async () => {
      const expected = exactSandboxExpectation(
        sessionId,
        launch.session.runtime_generation,
        typeof launch.session.runtime_reservation === "string"
          ? launch.session.runtime_reservation
          : null,
      );
      const inventory = await kubernetesSandboxInventory({
        kubeconfig,
        namespace: config.trigger_sandbox_namespace,
        phase: "live_session",
        expected,
      });
      evidence.document.sandbox_inventory.observations.live_session =
        inventory.observation;
      runtimeReservation = inventory.runtime_reservation;
      evidence.document.counts.sandbox =
        inventory.observation.entitled_objects.length;
      if (inventory.observation.total_objects === 0) {
        fail(
          "sandbox_live_object_missing",
          "ready Trigger session had no BatchSandbox in its exact namespace",
        );
      }
      assertEqual(
        inventory.observation.unexpected_objects.length,
        0,
        "sandbox_live_inventory_ambiguous",
        "ready Trigger session had an extra, stale, or ambiguous BatchSandbox",
      );
      assertEqual(
        inventory.observation.entitled_objects.length,
        1,
        "sandbox_live_count_mismatch",
        "ready Trigger session did not own exactly one entitled BatchSandbox",
      );
      assert(
        runtimeReservation !== null,
        "sandbox_reservation_missing",
        "entitled BatchSandbox lacked its exact runtime reservation",
      );
    });

    currentStage = "runtime_image_attestation";
    await evidence.stage(currentStage, async () => {
      const liveSandbox =
        evidence.document.sandbox_inventory.observations.live_session
          .entitled_objects[0];
      evidence.document.runtime_attestation = await waitForRuntimeAttestation(
        {
          kubeconfig,
          config,
          sessionId,
          runtimeGeneration: launch.session.runtime_generation,
          sandboxId: liveSandbox.labels.sandbox_id,
        },
        config.timeouts.effect_seconds,
        config.timeouts.poll_seconds,
      );
    });

    currentStage = "initial_message";
    await evidence.stage(currentStage, async () => {
      await ownerMessageCount(client, sessionId, launch.delivery.payload_hash);
      evidence.document.counts.initial_owner_message = 1;
    });

    currentStage = "diff_review";
    await evidence.stage(currentStage, async () => {
      launch = await replayingCommand(
        `cma-smoke-${marker}-diff`,
        (key) =>
          client.requestLaunchDiff(
            launch.id,
            launch.revision_number,
            launch.effective.digest,
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      launch = await waitForLaunch(
        client,
        launch.id,
        config.timeouts.effect_seconds,
        config.timeouts.poll_seconds,
        (candidate) =>
          candidate.state === "contributing" && candidate.diff !== null,
      );
      const diff = launch.diff;
      assertEqual(
        diff.validation,
        "valid",
        "diff_validation_failed",
        "fixture diff was not publishable",
      );
      assertEqual(
        diff.total_changed_files,
        1,
        "diff_count_mismatch",
        "fixture task did not change exactly one file",
      );
      assertEqual(
        diff.files.length,
        1,
        "diff_count_mismatch",
        "diff summary did not contain exactly one file",
      );
      assertEqual(
        diff.files[0].path,
        expectedPath,
        "diff_path_mismatch",
        "fixture task changed an unexpected path",
      );
      assertEqual(
        diff.files[0].change,
        "added",
        "diff_change_mismatch",
        "fixture task did not add the expected file",
      );
      assertEqual(
        diff.files[0].binary,
        false,
        "diff_change_mismatch",
        "fixture evidence file became binary",
      );
      assertEqual(
        diff.files[0].new_sha256,
        sha256(expectedContent),
        "diff_content_mismatch",
        "fixture evidence content digest changed",
      );
      assertEqual(
        diff.workflow_paths.length,
        0,
        "diff_workflow_scope_mismatch",
        "fixture diff included a workflow path",
      );
      const expectedAuthorEmail = `${String(connection.account_id)}+${config.contributor.login}@users.noreply.github.com`;
      assertEqual(
        diff.author.name,
        config.contributor.login,
        "commit_author_mismatch",
        "diff author name changed",
      );
      assertEqual(
        diff.author.email,
        expectedAuthorEmail,
        "commit_author_mismatch",
        "diff author email changed",
      );
      assertEqual(
        diff.committer.name,
        "CMA",
        "commit_committer_mismatch",
        "diff committer name changed",
      );
      assertEqual(
        diff.committer.email,
        "cma@chrono.ai",
        "commit_committer_mismatch",
        "diff committer email changed",
      );
      evidence.document.resources.diff_artifact_id = diff.id;
    });

    currentStage = "branch_publication";
    let contribution;
    let postFork;
    await evidence.stage(currentStage, async () => {
      launch = await replayingCommand(
        `cma-smoke-${marker}-publish`,
        (key) =>
          client.publishLaunchBranch(
            launch.id,
            launch.revision_number,
            launch.diff.digest,
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      launch = await waitForLaunch(
        client,
        launch.id,
        config.timeouts.effect_seconds,
        config.timeouts.poll_seconds,
        (candidate) =>
          candidate.state === "pr_pending" &&
          candidate.contribution?.state === "published",
      );
      contribution = launch.contribution;
      assertEqual(
        contribution.commit_id,
        contribution.resulting_head,
        "contribution_head_mismatch",
        "published ref does not name the reviewed commit",
      );
      assertEqual(
        contribution.branch_name,
        launch.branch.name,
        "contribution_branch_mismatch",
        "published contribution names another branch",
      );
      assertEqual(
        contribution.author.name,
        config.contributor.login,
        "commit_author_mismatch",
        "published author changed",
      );
      assertEqual(
        contribution.committer.name,
        "CMA",
        "commit_committer_mismatch",
        "published committer changed",
      );

      postFork = (
        await fetchJson(
          githubUrl(
            config,
            launch.fork.repository.canonical_owner,
            launch.fork.repository.canonical_name,
          ),
          [200],
        )
      ).body;
      validateFork(
        postFork,
        config,
        config.upstream.repository_id,
        connection.account_id,
      );
      assertEqual(
        postFork.id,
        launch.fork.repository.repository_id,
        "fork_identity_mismatch",
        "GitHub and Launch fork IDs differ",
      );
      evidence.document.counts.fork_created_delta =
        preFork.status === 404 ? 1 : 0;
      evidence.document.external_inventory.after.contributor_forks = 1;
      evidence.document.external_inventory.delta.contributor_forks =
        preFork.status === 404 ? 1 : 0;

      const branch = (
        await fetchJson(
          githubUrl(
            config,
            launch.fork.repository.canonical_owner,
            launch.fork.repository.canonical_name,
            `/git/ref/heads/${segment(launch.branch.name)}`,
          ),
          [200],
        )
      ).body;
      assertEqual(
        branch.object?.sha,
        contribution.commit_id,
        "branch_head_mismatch",
        "public fork branch does not name the contribution commit",
      );
      evidence.document.external_inventory.after.launch_branches = 1;
      evidence.document.external_inventory.delta.launch_branches = 1;
      const commit = (
        await fetchJson(
          githubUrl(
            config,
            launch.fork.repository.canonical_owner,
            launch.fork.repository.canonical_name,
            `/commits/${segment(contribution.commit_id)}`,
          ),
          [200],
        )
      ).body;
      assertEqual(
        commit.sha,
        contribution.commit_id,
        "commit_identity_mismatch",
        "GitHub returned another commit",
      );
      assertEqual(
        commit.parents?.length,
        1,
        "commit_parent_mismatch",
        "contribution commit was not a single-parent commit",
      );
      assertEqual(
        commit.parents?.[0]?.sha,
        contribution.expected_head,
        "commit_parent_mismatch",
        "contribution commit did not continue the guarded head",
      );
      assertEqual(
        commit.commit?.author?.name,
        contribution.author.name,
        "commit_author_mismatch",
        "GitHub commit author name differs",
      );
      assertEqual(
        commit.commit?.author?.email,
        contribution.author.email,
        "commit_author_mismatch",
        "GitHub commit author email differs",
      );
      assertEqual(
        commit.commit?.committer?.name,
        contribution.committer.name,
        "commit_committer_mismatch",
        "GitHub commit committer name differs",
      );
      assertEqual(
        commit.commit?.committer?.email,
        contribution.committer.email,
        "commit_committer_mismatch",
        "GitHub commit committer email differs",
      );
      assertEqual(
        commit.files?.length,
        1,
        "commit_path_count_mismatch",
        "GitHub commit changed more than one path",
      );
      assertEqual(
        commit.files?.[0]?.filename,
        expectedPath,
        "commit_path_mismatch",
        "GitHub commit changed an unexpected path",
      );
      assertEqual(
        commit.files?.[0]?.status,
        "added",
        "commit_path_mismatch",
        "GitHub commit did not add the evidence path",
      );

      const encodedPath = expectedPath.split("/").map(segment).join("/");
      const content = (
        await fetchJson(
          githubUrl(
            config,
            launch.fork.repository.canonical_owner,
            launch.fork.repository.canonical_name,
            `/contents/${encodedPath}?ref=${segment(launch.branch.name)}`,
          ),
          [200],
        )
      ).body;
      assertEqual(
        content.type,
        "file",
        "commit_content_mismatch",
        "GitHub evidence path is not a regular file",
      );
      assertEqual(
        content.encoding,
        "base64",
        "commit_content_mismatch",
        "GitHub evidence path used an unexpected encoding",
      );
      assertEqual(
        content.sha,
        commit.files[0].sha,
        "commit_content_mismatch",
        "GitHub content object differs from the commit",
      );
      assertEqual(
        Buffer.from(content.content, "base64").toString("utf8"),
        expectedContent,
        "commit_content_mismatch",
        "GitHub content bytes differ from the fixture contract",
      );

      evidence.document.counts.contribution_commit = 1;
      evidence.document.resources.contribution_id = contribution.id;
      evidence.document.resources.commit_id = contribution.commit_id;
    });

    currentStage = "pull_request";
    await evidence.stage(currentStage, async () => {
      const pullRequestInventoryQuery = new URLSearchParams({
        state: "all",
        head: `${config.contributor.login}:${launch.branch.name}`,
        base: config.upstream.default_branch,
        per_page: "100",
      });
      const beforePullRequests = (
        await fetchJson(
          githubUrl(
            config,
            config.upstream.owner,
            config.upstream.repository,
            `/pulls?${pullRequestInventoryQuery.toString()}`,
          ),
          [200],
        )
      ).body;
      assert(
        Array.isArray(beforePullRequests),
        "pull_request_inventory_malformed",
        "provider pull request preflight inventory was malformed",
      );
      assertEqual(
        beforePullRequests.length,
        0,
        "pull_request_precondition_mismatch",
        "Launch branch was already linked to a pull request",
      );
      evidence.document.external_inventory.before.launch_pull_requests = 0;

      const proposal = await client.reviewPullRequest(launch.id, "open");
      launch = await replayingCommand(
        `cma-smoke-${marker}-pull-request`,
        (key) =>
          client.confirmPullRequest(
            launch.id,
            "open",
            launch.revision_number,
            proposal.intent_digest,
            {},
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      launch = await waitForLaunch(
        client,
        launch.id,
        config.timeouts.effect_seconds,
        config.timeouts.poll_seconds,
        (candidate) =>
          candidate.state === "pr_open" &&
          candidate.pull_request?.state === "open",
      );
      const pullRequest = launch.pull_request;
      assert(
        Number.isSafeInteger(pullRequest.number) && pullRequest.number > 0,
        "pull_request_identity_missing",
        "Launch lacks a provider pull request number",
      );
      const providerResponse = await fetchJson(
        githubUrl(
          config,
          config.upstream.owner,
          config.upstream.repository,
          `/pulls/${String(pullRequest.number)}`,
        ),
        [200],
      );
      const provider = providerResponse.body;
      assert(
        typeof providerResponse.provider_request_id === "string" &&
          providerResponse.provider_request_id.length > 0,
        "provider_correlation_missing",
        "GitHub pull request read lacked its safe provider request ID",
      );
      evidence.document.correlations.provider = [
        providerResponse.provider_request_id,
      ];
      assertEqual(
        provider.state,
        "open",
        "pull_request_state_mismatch",
        "provider pull request is not open",
      );
      assertEqual(
        provider.base?.repo?.id,
        config.upstream.repository_id,
        "pull_request_target_mismatch",
        "pull request targets another numeric repository",
      );
      assertEqual(
        provider.base?.ref,
        config.upstream.default_branch,
        "pull_request_target_mismatch",
        "pull request targets another base ref",
      );
      assertEqual(
        provider.head?.repo?.id,
        postFork.id,
        "pull_request_head_mismatch",
        "pull request head belongs to another fork",
      );
      assertEqual(
        provider.head?.sha,
        contribution.commit_id,
        "pull_request_head_mismatch",
        "pull request head differs from the reviewed commit",
      );
      assertEqual(
        provider.user?.id,
        connection.account_id,
        "pull_request_actor_mismatch",
        "pull request actor differs from the selected GitHub account",
      );
      assert(
        provider.body?.includes(
          `<!-- cma-trigger:launch:${launch.id}:v1 -->`,
        ) === true,
        "pull_request_marker_missing",
        "pull request lacks the Launch marker",
      );
      assertEqual(
        pullRequest.canonical_url,
        provider.html_url,
        "pull_request_url_mismatch",
        "Launch pull request URL differs from GitHub",
      );
      const providerUrl = new URL(provider.html_url);
      assertEqual(
        providerUrl.protocol,
        "https:",
        "pull_request_url_mismatch",
        "pull request URL is not HTTPS",
      );
      assertEqual(
        providerUrl.hostname,
        "github.com",
        "pull_request_url_mismatch",
        "pull request URL is not on github.com",
      );

      const matches = (
        await fetchJson(
          githubUrl(
            config,
            config.upstream.owner,
            config.upstream.repository,
            `/pulls?${pullRequestInventoryQuery.toString()}`,
          ),
          [200],
        )
      ).body;
      assert(
        Array.isArray(matches),
        "pull_request_inventory_malformed",
        "provider pull request inventory was malformed",
      );
      assertEqual(
        matches.filter((candidate) => candidate.number === pullRequest.number)
          .length,
        1,
        "pull_request_count_mismatch",
        "provider did not expose exactly one matching pull request",
      );
      assertEqual(
        matches.length,
        1,
        "pull_request_count_mismatch",
        "Launch branch is linked to more than one pull request",
      );

      evidence.document.counts.pull_request = 1;
      evidence.document.external_inventory.after.launch_pull_requests =
        matches.length;
      evidence.document.external_inventory.delta.launch_pull_requests =
        matches.length;
      evidence.document.resources.pull_request_provider_id = provider.id;
      evidence.document.resources.pull_request_number = pullRequest.number;
      evidence.document.resources.pull_request_url = provider.html_url;
    });

    currentStage = "durable_audit";
    await evidence.stage(currentStage, async () => {
      const progress = await launchProgress(client, launch.id);
      assertEqual(
        progress.filter((event) => event.kind?.type === "created").length,
        1,
        "launch_event_count_mismatch",
        "Launch log did not contain exactly one creation event",
      );
      assertEqual(
        progress.filter((event) => event.kind?.type === "started").length,
        1,
        "launch_event_count_mismatch",
        "Launch log did not contain exactly one Start event",
      );
      const correlations = [
        ...new Set(progress.map((event) => event.correlation_id)),
      ];
      assert(
        correlations.every(
          (value) =>
            typeof value === "string" &&
            value.length > 0 &&
            value.length <= 255,
        ),
        "correlation_id_invalid",
        "Launch log carried an invalid correlation ID",
      );
      evidence.document.correlations.launch = correlations;
      assert(
        Object.values(evidence.document.counts).every(
          (count) => Number.isSafeInteger(count) && count >= 0,
        ),
        "resource_count_invalid",
        "evidence resource count is invalid",
      );
      for (const resource of [
        "launch",
        "fork",
        "branch",
        "workspace",
        "sandbox",
        "session",
        "initial_owner_message",
        "contribution_commit",
        "pull_request",
      ]) {
        assertEqual(
          evidence.document.counts[resource],
          1,
          "resource_count_mismatch",
          `${resource} count was not exactly one`,
        );
      }
    });

    currentStage = "launch_completion";
    await evidence.stage(currentStage, async () => {
      launch = await replayingCommand(
        `cma-smoke-${marker}-complete`,
        (key) =>
          client.completeLaunch(
            launch.id,
            launch.revision_number,
            launch.effective.digest,
            { idempotencyKey: key },
          ),
        sdk.ApiError,
        sdk.TransportError,
      );
      assertEqual(
        launch.state,
        "completed",
        "launch_completion_failed",
        "Launch completion did not enter the completed state",
      );
      assertEqual(
        launch.pull_request?.state,
        "open",
        "launch_completion_changed_pull_request",
        "Launch completion did not preserve the open pull request",
      );
      launch = await waitForLaunch(
        client,
        launch.id,
        config.timeouts.effect_seconds,
        config.timeouts.poll_seconds,
        (candidate) =>
          terminalProviderSettlement(candidate, sessionId) !== null,
      );
      const providerSettlement = terminalProviderSettlement(launch, sessionId);
      assert(
        providerSettlement !== null,
        "provider_settlement_incomplete",
        "completed Launch lacks positive Reach revocation and session credential retirement",
      );
      evidence.document.provider_settlement = {
        ...providerSettlement,
        verified_at: now(),
      };
    });

    currentStage = "session_cleanup";
    await evidence.stage(currentStage, async () => {
      await client.stopSession(sessionId);
      const stopped = await client.readSession(sessionId);
      assertEqual(
        stopped.id,
        sessionId,
        "session_cleanup_failed",
        "stopped session identity changed",
      );
      assertEqual(
        stopped.kernel,
        TRIGGER_KERNEL_ID,
        "session_cleanup_failed",
        "stopped session did not retain the Trigger kernel",
      );
      assertEqual(
        stopped.lifecycle,
        "idle",
        "session_cleanup_failed",
        "smoke session was not stopped",
      );
      const expected = exactSandboxExpectation(
        sessionId,
        evidence.document.resources.runtime_generation,
        runtimeReservation,
      );
      const deadline = Date.now() + config.timeouts.effect_seconds * 1000;
      while (true) {
        const inventory = await kubernetesSandboxInventory({
          kubeconfig,
          namespace: config.trigger_sandbox_namespace,
          phase: "after_stop",
          expected,
        });
        evidence.document.sandbox_inventory.observations.after_stop =
          inventory.observation;
        if (inventory.observation.total_objects === 0) break;
        if (Date.now() >= deadline) {
          fail(
            "sandbox_leaked_after_stop",
            "Trigger BatchSandbox remained after stop reconciliation",
          );
        }
        await sleep(config.timeouts.poll_seconds * 1000);
      }
      assertEqual(
        evidence.document.sandbox_inventory.observations.after_stop
          .unexpected_objects.length,
        0,
        "sandbox_post_stop_inventory_mismatch",
        "Trigger namespace retained an unexpected BatchSandbox after stop",
      );
      evidence.document.cleanup.session = SESSION_IDLE_AWAITING_TEARDOWN;
    });

    evidence.document.result = "pass";
    evidence.document.actual_result = "matched_expected";
    evidence.document.finished_at = now();
    validateEvidence(evidence.document);
    await evidence.write();
  } catch (cause) {
    evidence.document.result = "fail";
    evidence.document.actual_result = "smoke_failed";
    evidence.document.finished_at = now();
    evidence.document.failure = safeFailure(cause, currentStage);
    if (sessionId !== undefined)
      evidence.document.cleanup.session = SESSION_TEARDOWN_PENDING;
    validateEvidence(evidence.document);
    await evidence.write();
    console.error(
      `[smoke] failed at ${currentStage}: ${evidence.document.failure.code}`,
    );
    process.exitCode = 1;
  }
}

function finalizeEphemeralCleanupEvidence(
  document,
  receipt,
  expectedRunId,
  expectedRunAttempt,
) {
  validateEvidence(document, { publishable: true });
  exactKeys(
    receipt,
    [
      "schema_version",
      "cluster",
      "run_id",
      "run_attempt",
      "cluster_destroyed",
      "credentials_purged",
      "provider_object_settlement",
    ],
    "cleanup receipt",
    "cleanup_receipt_invalid",
  );
  if (
    !/^[1-9][0-9]*$/.test(expectedRunId) ||
    !/^[1-9][0-9]*$/.test(expectedRunAttempt) ||
    receipt.schema_version !== 2 ||
    receipt.cluster !== "cma-trigger-smoke" ||
    receipt.run_id !== expectedRunId ||
    receipt.run_attempt !== expectedRunAttempt ||
    receipt.cluster_destroyed !== true ||
    receipt.credentials_purged !== true ||
    String(document.provenance.workflow.run_id) !== expectedRunId ||
    String(document.provenance.workflow.run_attempt) !== expectedRunAttempt
  ) {
    fail(
      "cleanup_receipt_invalid",
      "cleanup receipt is not bound to the evidence workflow and terminal teardown",
    );
  }
  const providerObjectSettlement = receipt.provider_object_settlement;
  if (document.result === "pass") {
    if (providerObjectSettlement === null) {
      fail(
        "cleanup_receipt_invalid",
        "passing evidence has no provider-object settlement receipt",
      );
    }
    validateProviderObjectSettlement(providerObjectSettlement, document);
  } else if (providerObjectSettlement !== null) {
    fail(
      "cleanup_receipt_invalid",
      "failed evidence cannot adopt a provider-object settlement receipt",
    );
  }
  if (
    document.fixture.policy.github_cleanup !== "preserve" ||
    document.fixture.policy.cma_state_cleanup !== CMA_STATE_CLEANUP_POLICY ||
    document.fixture.policy.session_cleanup !== SESSION_CLEANUP_POLICY
  ) {
    fail(
      "cleanup_policy_mismatch",
      "evidence does not declare the reviewed ephemeral cleanup policy",
    );
  }
  // Provider Reach revocation and credential retirement were established
  // before teardown. Provider-object settlement is accepted only after Kind
  // destruction has quiesced every writer.
  if (document.cleanup.cma_launch === CMA_LAUNCH_RETAINED) {
    document.cleanup.cma_launch = EPHEMERAL_STATE_DESTROYED;
  } else if (
    !["not_created", EPHEMERAL_STATE_DESTROYED].includes(
      document.cleanup.cma_launch,
    )
  ) {
    fail(
      "cleanup_state_invalid",
      "local CMA Launch record state cannot be closed by terminal teardown",
    );
  }
  if (
    [SESSION_TEARDOWN_PENDING, SESSION_IDLE_AWAITING_TEARDOWN].includes(
      document.cleanup.session,
    )
  ) {
    document.cleanup.session = EPHEMERAL_STATE_DESTROYED;
  } else if (
    !["not_created", EPHEMERAL_STATE_DESTROYED].includes(
      document.cleanup.session,
    )
  ) {
    fail(
      "cleanup_state_invalid",
      "local session record state cannot be closed by terminal teardown",
    );
  }
  document.cleanup.provider_object_settlement =
    providerObjectSettlement === null
      ? null
      : JSON.parse(JSON.stringify(providerObjectSettlement));
  validateEvidence(document, { publishable: true });
  return document;
}

async function finalizeEphemeralCleanupEvidenceFile(evidencePath, receiptPath) {
  let serializedEvidence;
  let serializedReceipt;
  try {
    [serializedEvidence, serializedReceipt] = await Promise.all([
      readFile(evidencePath, "utf8"),
      readFile(receiptPath, "utf8"),
    ]);
  } catch {
    fail(
      "cleanup_receipt_missing",
      "evidence or terminal cleanup receipt is absent",
    );
  }
  if (
    serializedEvidence.length > MAX_PUBLIC_JSON_BYTES ||
    serializedReceipt.length > 16 * 1024
  ) {
    fail(
      "cleanup_receipt_invalid",
      "evidence or terminal cleanup receipt exceeds its bound",
    );
  }
  let document;
  let receipt;
  try {
    document = JSON.parse(serializedEvidence);
    receipt = JSON.parse(serializedReceipt);
  } catch {
    fail(
      "cleanup_receipt_invalid",
      "evidence or terminal cleanup receipt is not JSON",
    );
  }
  const finalized = finalizeEphemeralCleanupEvidence(
    document,
    receipt,
    requiredEnvironment("GITHUB_RUN_ID", 32),
    requiredEnvironment("GITHUB_RUN_ATTEMPT", 32),
  );
  const temporary = `${evidencePath}.tmp`;
  await writeFile(temporary, `${JSON.stringify(finalized, null, 2)}\n`, {
    mode: 0o600,
  });
  await rename(temporary, evidencePath);
  console.log("[smoke] evidence finalized against terminal ephemeral cleanup");
}

async function auditEvidence(path) {
  let serialized;
  let document;
  try {
    serialized = await readFile(path, "utf8");
    document = JSON.parse(serialized);
  } catch {
    fail("evidence_missing", "evidence file is missing or not JSON");
  }
  validateEvidence(document);
  const accessToken = process.env.CMA_TRIGGER_SMOKE_ACCESS_TOKEN;
  if (
    typeof accessToken === "string" &&
    accessToken.length > 0 &&
    serialized.includes(accessToken)
  ) {
    fail("evidence_secret_detected", "evidence contains a protected value");
  }
  const serviceId = process.env.CMA_TRIGGER_SMOKE_SERVICE_ID;
  const containsStringValue = (value, expected) => {
    if (typeof value === "string") return value === expected;
    if (Array.isArray(value))
      return value.some((entry) => containsStringValue(entry, expected));
    if (value !== null && typeof value === "object") {
      return Object.values(value).some((entry) =>
        containsStringValue(entry, expected),
      );
    }
    return false;
  };
  if (
    typeof serviceId === "string" &&
    serviceId.length > 0 &&
    containsStringValue(document, serviceId)
  ) {
    fail(
      "evidence_secret_detected",
      "evidence contains the raw service identity",
    );
  }
  const forbidden = [
    ...CONFIG_CREDENTIAL_PATTERNS,
    /"(?:provider_body|rendered_prompt|runtime_reservation|kubernetes_config|cluster_server)"\s*:/iu,
  ];
  if (forbidden.some((pattern) => pattern.test(serialized))) {
    fail(
      "evidence_secret_detected",
      "evidence contains a forbidden credential or response field",
    );
  }
  document.redaction.status = "passed";
  document.redaction.audited_at = now();
  validateEvidence(document, { publishable: true });
  const temporary = `${path}.tmp`;
  await writeFile(temporary, `${JSON.stringify(document, null, 2)}\n`, {
    mode: 0o600,
  });
  await rename(temporary, path);
  console.log("[smoke] closed evidence and redaction audit passed");
}

async function validateEvidenceFile(path) {
  let document;
  try {
    document = JSON.parse(await readFile(path, "utf8"));
  } catch {
    fail("evidence_missing", "evidence file is missing or not JSON");
  }
  validateEvidence(document, { publishable: true });
  console.log("[smoke] publishable evidence schema is valid");
}

async function main() {
  const [mode, first, second, ...rest] = process.argv.slice(2);
  if (
    mode === "--self-test" &&
    first === undefined &&
    second === undefined &&
    rest.length === 0
  ) {
    await selfTestLaunchChoice();
    selfTestConfigValidation();
    selfTestRuntimeAttestation();
    selfTestSandboxInventory();
    selfTestTerminalProviderSettlement();
    selfTestEvidenceValidation();
    return;
  }
  if (rest.length > 0 || first === undefined) {
    fail(
      "invalid_arguments",
      "usage: --self-test | --validate-config FILE | --run CONFIG EVIDENCE | --audit-evidence FILE | --validate-evidence FILE | --finalize-ephemeral-cleanup EVIDENCE RECEIPT",
    );
  }
  if (mode === "--validate-config" && second === undefined) {
    await loadConfig(first);
    console.log("[smoke] fixture configuration is valid");
    return;
  }
  if (mode === "--run" && second !== undefined) {
    await runSmoke(first, second);
    return;
  }
  if (mode === "--audit-evidence" && second === undefined) {
    await auditEvidence(first);
    return;
  }
  if (mode === "--validate-evidence" && second === undefined) {
    await validateEvidenceFile(first);
    return;
  }
  if (mode === "--finalize-ephemeral-cleanup" && second !== undefined) {
    await finalizeEphemeralCleanupEvidenceFile(first, second);
    return;
  }
  fail(
    "invalid_arguments",
    "usage: --self-test | --validate-config FILE | --run CONFIG EVIDENCE | --audit-evidence FILE | --validate-evidence FILE | --finalize-ephemeral-cleanup EVIDENCE RECEIPT",
  );
}

await main().catch((cause) => {
  const code = cause instanceof SmokeFailure ? cause.code : "harness_failed";
  console.error(`[smoke] ${code}`);
  process.exitCode = 1;
});
