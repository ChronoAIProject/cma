#!/usr/bin/env node

import { createHash } from "node:crypto";
import { constants as fsConstants } from "node:fs";
import { lstat, open, unlink } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import { pathToFileURL } from "node:url";

const NYXID_BASE_URL = "https://nyx-api.chrono-ai.fun";
const STORAGE_SERVICE_SLUG = "chrono-storage-service";
const STORAGE_CONTRACT_REPOSITORY = "ChronoAIProject/chrono-bucket";
const STORAGE_CONTRACT_SOURCE_SHA = "4087662e2a65e3747b3203ba619eca7342089272";
const STORAGE_TOMBSTONE_RETENTION =
  "storage_contract_lifetime_not_logically_enumerable";
const RECEIPT_KIND = "cma-trigger-provider-object-settlement";
const EXPECTED_EVIDENCE_RESULT =
  "one_completed_launch_to_one_reviewed_contribution_and_open_pull_request_with_provider_settlement";
const STORAGE_SCOPES = Object.freeze(["proxy", "object:read", "object:delete"]);
const REDACTION_CHECKS = Object.freeze([
  "closed_schema",
  "environment_secrets",
  "kubernetes_credentials",
  "credential_patterns",
  "forbidden_fields",
]);
const EVIDENCE_TOP_LEVEL_KEYS = Object.freeze([
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
]);
const HANDOFF_KEYS = Object.freeze([
  "schema_version",
  "nyxid_base_url",
  "nyxid_client_id",
  "nyxid_client_secret",
  "storage_client_id",
  "storage_client_secret",
  "storage_owner_service_account_id",
  "context_bucket",
  "trigger_workspace_bucket",
  "public_github_service_slug",
  "subject_id",
  "access_token",
  "access_token_expires_at",
]);
const RESOURCE_KEYS = Object.freeze([
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
]);
const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SHA1 = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const SA_CLIENT_ID = /^sa_[0-9a-f]{24}$/u;
const BUCKET = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/u;
const SLUG = /^[a-z0-9](?:[a-z0-9_-]{0,98}[a-z0-9])?$/u;
const LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/u;
const REPOSITORY = /^[A-Za-z0-9_.-]{1,100}$/u;
const SESSION_ID = /^[A-Za-z0-9](?:[A-Za-z0-9._-]{0,61}[A-Za-z0-9])?$/u;
const WORKSPACE_SEGMENT = /^[A-Za-z0-9][A-Za-z0-9._-]{0,255}$/u;
const UTC_TIMESTAMP = /^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{3})?Z$/u;

export const DEFAULT_LIMITS = Object.freeze({
  maxResponseBytes: 8 * 1024 * 1024,
  maxObjectsPerBucket: 4_096,
  maxObjectKeyLength: 1_024,
  maxInventoryBytesPerBucket: 8 * 1024 * 1024 * 1024,
  maxSelectedObjects: 4_096,
  maxSelectedBytes: 8 * 1024 * 1024 * 1024,
  maxDeletes: 4_096,
});

const MAX_HANDOFF_JSON_BYTES = 128 * 1024;
const MAX_EVIDENCE_JSON_BYTES = 8 * 1024 * 1024;
const MIN_STORAGE_TOKEN_LIFETIME_SECONDS = 120;
const REQUEST_TIMEOUT_MS = 30_000;

export class SettlementFailure extends Error {
  constructor(code, message) {
    super(message);
    this.name = "SettlementFailure";
    this.code = code;
  }
}

function fail(code, message) {
  throw new SettlementFailure(code, message);
}

function plainObject(value, path, code = "invalid_input") {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(code, `${path} must be an object`);
  }
  return value;
}

function exactKeys(value, expected, path, code = "invalid_input") {
  const actual = Object.keys(plainObject(value, path, code)).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    fail(code, `${path} does not match its closed schema`);
  }
}

function closedKeys(
  value,
  allowed,
  required,
  path,
  code = "remote_contract_drift",
) {
  const actual = Object.keys(plainObject(value, path, code));
  if (
    actual.some((key) => !allowed.includes(key)) ||
    required.some((key) => !actual.includes(key))
  ) {
    fail(code, `${path} does not match its closed schema`);
  }
}

function boundedText(value, path, maximum, code = "invalid_input") {
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

function safeInteger(
  value,
  path,
  minimum = 0,
  maximum = Number.MAX_SAFE_INTEGER,
  code = "invalid_input",
) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    fail(code, `${path} is outside its integer bound`);
  }
  return value;
}

function isExactTimestamp(value) {
  if (typeof value !== "string" || !UTC_TIMESTAMP.test(value)) return false;
  const parsed = Date.parse(value);
  if (Number.isNaN(parsed)) return false;
  const canonical = new Date(parsed).toISOString();
  return value === canonical || value === canonical.replace(".000Z", "Z");
}

function timestamp(value, path, code = "invalid_input") {
  if (!isExactTimestamp(value)) {
    fail(code, `${path} must be an exact UTC timestamp`);
  }
  return value;
}

function sha256(value) {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function validateTokenSet(value, expected, path) {
  const checked = boundedText(value, path, 2_048, "storage_token_drift");
  const tokens = checked.split(/\s+/u);
  if (
    new Set(tokens).size !== tokens.length ||
    JSON.stringify([...tokens].sort()) !== JSON.stringify([...expected].sort())
  ) {
    fail("storage_token_drift", `${path} does not match the exact scope set`);
  }
}

function normalizeLimits(overrides = {}) {
  exactKeys(
    { ...DEFAULT_LIMITS, ...overrides },
    Object.keys(DEFAULT_LIMITS),
    "limits",
    "invalid_limits",
  );
  const limits = { ...DEFAULT_LIMITS, ...overrides };
  for (const [field, value] of Object.entries(limits)) {
    safeInteger(
      value,
      `limits.${field}`,
      1,
      Number.MAX_SAFE_INTEGER,
      "invalid_limits",
    );
  }
  return limits;
}

export function validateHandoff(raw) {
  exactKeys(raw, HANDOFF_KEYS, "handoff", "unsafe_handoff");
  if (raw.schema_version !== 1 || raw.nyxid_base_url !== NYXID_BASE_URL) {
    fail("unsafe_handoff", "handoff contract identity drifted");
  }
  for (const [field, pattern] of [
    ["nyxid_client_id", UUID],
    ["storage_owner_service_account_id", UUID],
    ["subject_id", UUID],
    ["storage_client_id", SA_CLIENT_ID],
    ["context_bucket", BUCKET],
    ["trigger_workspace_bucket", BUCKET],
    ["public_github_service_slug", SLUG],
  ]) {
    if (typeof raw[field] !== "string" || !pattern.test(raw[field])) {
      fail("unsafe_handoff", `handoff.${field} is invalid`);
    }
  }
  for (const field of [
    "nyxid_client_secret",
    "storage_client_secret",
    "access_token",
  ]) {
    boundedText(raw[field], `handoff.${field}`, 32_768, "unsafe_handoff");
  }
  safeInteger(
    raw.access_token_expires_at,
    "handoff.access_token_expires_at",
    1,
    Number.MAX_SAFE_INTEGER,
    "unsafe_handoff",
  );
  if (raw.context_bucket === raw.trigger_workspace_bucket) {
    fail("unsafe_handoff", "handoff dedicated buckets must be distinct");
  }
  return raw;
}

export function validateEvidence(raw) {
  exactKeys(raw, EVIDENCE_TOP_LEVEL_KEYS, "evidence", "unsafe_evidence");
  if (
    raw.schema_version !== 6 ||
    raw.ledger_id !== "P-01" ||
    raw.result !== "pass" ||
    raw.actual_result !== "matched_expected" ||
    raw.expected_result !== EXPECTED_EVIDENCE_RESULT ||
    raw.failure !== null
  ) {
    fail("unsafe_evidence", "evidence is not one closed passing P-01 run");
  }
  boundedText(raw.evidence_id, "evidence.evidence_id", 255, "unsafe_evidence");
  timestamp(raw.started_at, "evidence.started_at", "unsafe_evidence");
  timestamp(raw.finished_at, "evidence.finished_at", "unsafe_evidence");
  if (Date.parse(raw.finished_at) < Date.parse(raw.started_at)) {
    fail("unsafe_evidence", "evidence interval runs backwards");
  }

  exactKeys(
    raw.provenance,
    ["workflow", "deployment"],
    "evidence.provenance",
    "unsafe_evidence",
  );
  const workflow = raw.provenance.workflow;
  exactKeys(
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
    "unsafe_evidence",
  );
  const repositoryParts = boundedText(
    workflow.repository,
    "evidence.provenance.workflow.repository",
    201,
    "unsafe_evidence",
  ).split("/");
  if (
    repositoryParts.length !== 2 ||
    !LOGIN.test(repositoryParts[0]) ||
    !REPOSITORY.test(repositoryParts[1]) ||
    !SHA1.test(workflow.workflow_commit)
  ) {
    fail("unsafe_evidence", "evidence workflow identity is invalid");
  }
  for (const field of ["repository_id", "run_id", "run_attempt"]) {
    safeInteger(
      workflow[field],
      `evidence.provenance.workflow.${field}`,
      1,
      Number.MAX_SAFE_INTEGER,
      "unsafe_evidence",
    );
  }
  let runUrl;
  let artifactUrl;
  try {
    runUrl = new URL(workflow.run_url);
    artifactUrl = new URL(workflow.artifact_url);
  } catch {
    fail("unsafe_evidence", "evidence workflow URLs are invalid");
  }
  const expectedRunPath = `/${workflow.repository}/actions/runs/${String(workflow.run_id)}`;
  if (
    runUrl.protocol !== "https:" ||
    runUrl.username !== "" ||
    runUrl.password !== "" ||
    runUrl.pathname !== expectedRunPath ||
    runUrl.search !== "" ||
    runUrl.hash !== "" ||
    artifactUrl.href !== `${runUrl.href}#artifacts` ||
    workflow.artifact_name !==
      `cma-trigger-provider-smoke-${String(workflow.run_id)}-${String(workflow.run_attempt)}` ||
    raw.evidence_id !==
      `p-01-gha-${String(workflow.run_id)}-${String(workflow.run_attempt)}`
  ) {
    fail("unsafe_evidence", "evidence does not bind the exact workflow run");
  }

  exactKeys(
    raw.resources,
    RESOURCE_KEYS,
    "evidence.resources",
    "unsafe_evidence",
  );
  if (
    !UUID.test(raw.resources.launch_id) ||
    !SESSION_ID.test(raw.resources.session_id)
  ) {
    fail("unsafe_evidence", "evidence Launch or session identity is invalid");
  }

  exactKeys(
    raw.provider_settlement,
    [
      "launch_state",
      "reach_state",
      "session_credential_retirement",
      "verified_at",
    ],
    "evidence.provider_settlement",
    "unsafe_evidence",
  );
  const retirement = raw.provider_settlement.session_credential_retirement;
  exactKeys(
    retirement,
    ["session_id", "settled_at"],
    "evidence.provider_settlement.session_credential_retirement",
    "unsafe_evidence",
  );
  timestamp(
    retirement.settled_at,
    "evidence.provider_settlement.session_credential_retirement.settled_at",
    "unsafe_evidence",
  );
  timestamp(
    raw.provider_settlement.verified_at,
    "evidence.provider_settlement.verified_at",
    "unsafe_evidence",
  );
  if (
    raw.provider_settlement.launch_state !== "completed" ||
    raw.provider_settlement.reach_state !== "revoked" ||
    retirement.session_id !== raw.resources.session_id ||
    Date.parse(retirement.settled_at) >
      Date.parse(raw.provider_settlement.verified_at) ||
    Date.parse(raw.provider_settlement.verified_at) >
      Date.parse(raw.finished_at)
  ) {
    fail("unsafe_evidence", "evidence provider settlement is incomplete");
  }

  exactKeys(
    raw.cleanup,
    ["github", "cma_launch", "session", "provider_object_settlement"],
    "evidence.cleanup",
    "unsafe_evidence",
  );
  if (
    raw.cleanup.github !== "preserved_by_policy" ||
    raw.cleanup.cma_launch !== "retained_until_terminal_ephemeral_teardown" ||
    raw.cleanup.session !== "idle_awaiting_terminal_ephemeral_teardown" ||
    raw.cleanup.provider_object_settlement !== null
  ) {
    fail(
      "unsafe_evidence",
      "evidence is not awaiting terminal provider-object settlement",
    );
  }

  exactKeys(
    raw.redaction,
    ["status", "audited_at", "checks"],
    "evidence.redaction",
    "unsafe_evidence",
  );
  timestamp(
    raw.redaction.audited_at,
    "evidence.redaction.audited_at",
    "unsafe_evidence",
  );
  if (
    raw.redaction.status !== "passed" ||
    JSON.stringify(raw.redaction.checks) !== JSON.stringify(REDACTION_CHECKS) ||
    Date.parse(raw.redaction.audited_at) < Date.parse(raw.finished_at)
  ) {
    fail(
      "unsafe_evidence",
      "evidence has not passed its closed redaction audit",
    );
  }
  return raw;
}

function validateObjectKey(key, limits, path) {
  boundedText(key, path, limits.maxObjectKeyLength, "inventory_malformed");
  if (
    key.includes("\0") ||
    Buffer.byteLength(key, "utf8") > 4 * limits.maxObjectKeyLength
  ) {
    fail("inventory_malformed", `${path} exceeds the provider key bound`);
  }
}

function normalizeInventoryObject(raw, limits, path) {
  closedKeys(
    raw,
    [
      "key",
      "size",
      "lastModified",
      "etag",
      "generation",
      "digest",
      "fenceVersion",
    ],
    ["key", "generation"],
    path,
    "inventory_malformed",
  );
  validateObjectKey(raw.key, limits, `${path}.key`);
  const generation = safeInteger(
    raw.generation,
    `${path}.generation`,
    0,
    Number.MAX_SAFE_INTEGER - 1,
    "inventory_malformed",
  );
  if (raw.size !== undefined) {
    safeInteger(
      raw.size,
      `${path}.size`,
      0,
      limits.maxInventoryBytesPerBucket,
      "inventory_byte_overflow",
    );
  }
  if (raw.lastModified !== undefined) {
    timestamp(raw.lastModified, `${path}.lastModified`, "inventory_malformed");
  }
  if (raw.etag !== undefined) {
    boundedText(raw.etag, `${path}.etag`, 2_048, "inventory_malformed");
  }
  if (generation === 0) {
    if (raw.digest !== undefined || raw.fenceVersion !== undefined) {
      fail("inventory_malformed", `${path} mixes legacy and fenced metadata`);
    }
  } else if (!SHA256.test(raw.digest) || raw.fenceVersion !== 1) {
    fail("inventory_malformed", `${path} lacks exact fenced metadata`);
  }
  const normalized = { key: raw.key, generation };
  for (const field of [
    "size",
    "lastModified",
    "etag",
    "digest",
    "fenceVersion",
  ]) {
    if (raw[field] !== undefined) normalized[field] = raw[field];
  }
  return normalized;
}

export function validateBucketInventory(
  raw,
  { bucket, ownerServiceAccountId, limits: limitOverrides = {} },
) {
  const limits = normalizeLimits(limitOverrides);
  exactKeys(raw, ["data", "error"], "bucket response", "remote_contract_drift");
  if (raw.error !== null) {
    fail("bucket_authority_drift", "bucket read returned an error envelope");
  }
  exactKeys(
    raw.data,
    ["name", "owner", "createdAt", "metadata", "objects"],
    "bucket response.data",
    "remote_contract_drift",
  );
  timestamp(
    raw.data.createdAt,
    "bucket response.data.createdAt",
    "remote_contract_drift",
  );
  plainObject(
    raw.data.metadata,
    "bucket response.data.metadata",
    "remote_contract_drift",
  );
  if (raw.data.name !== bucket || raw.data.owner !== ownerServiceAccountId) {
    fail("bucket_authority_drift", "bucket identity or exact owner drifted");
  }
  if (!Array.isArray(raw.data.objects)) {
    fail("inventory_malformed", "bucket objects must be an array");
  }
  if (raw.data.objects.length > limits.maxObjectsPerBucket) {
    fail("inventory_count_overflow", "bucket object count exceeds its bound");
  }
  const objects = [];
  const seen = new Set();
  let observedBytes = 0;
  for (const [index, value] of raw.data.objects.entries()) {
    const object = normalizeInventoryObject(
      value,
      limits,
      `bucket response.data.objects[${String(index)}]`,
    );
    if (seen.has(object.key)) {
      fail(
        "duplicate_inventory_object",
        "bucket inventory contains a duplicate logical key",
      );
    }
    seen.add(object.key);
    if (object.size !== undefined) {
      observedBytes += object.size;
      if (
        !Number.isSafeInteger(observedBytes) ||
        observedBytes > limits.maxInventoryBytesPerBucket
      ) {
        fail(
          "inventory_byte_overflow",
          "bucket inventory bytes exceed their bound",
        );
      }
    }
    objects.push(object);
  }
  const ordered = [...objects].sort((left, right) =>
    left.key < right.key ? -1 : left.key > right.key ? 1 : 0,
  );
  if (JSON.stringify(objects) !== JSON.stringify(ordered)) {
    fail(
      "inventory_malformed",
      "bucket inventory is not in canonical logical-key order",
    );
  }
  return { bucket, owner: raw.data.owner, objects, observedBytes };
}

function parseContextKey(key) {
  if (!key.startsWith("sessions/")) return null;
  const parts = key.split("/");
  if (
    parts.length !== 3 ||
    !SESSION_ID.test(parts[1]) ||
    !["context.jsonl", "handover.snapshot", "skill-packs.v1"].includes(parts[2])
  ) {
    fail(
      "inventory_namespace_invalid",
      "context bucket contains a malformed session namespace",
    );
  }
  return { sessionId: parts[1], kind: parts[2] };
}

function parseWorkspaceKey(key) {
  if (!key.startsWith("trigger-workspaces/")) return null;
  const parts = key.split("/");
  if (
    parts.length < 6 ||
    parts[0] !== "trigger-workspaces" ||
    parts[1] !== "subjects" ||
    !SHA256.test(parts[2]) ||
    parts[3] !== "launches" ||
    !WORKSPACE_SEGMENT.test(parts[4]) ||
    parts.slice(5).some((part) => !WORKSPACE_SEGMENT.test(part))
  ) {
    fail(
      "inventory_namespace_invalid",
      "workspace bucket contains a malformed Launch namespace",
    );
  }
  return { subjectHash: parts[2], launchId: parts[4] };
}

export function classifyCurrentRunObjects(
  inventory,
  { kind, sessionId, launchId, subjectHash },
) {
  const selected = [];
  const unrelated = [];
  for (const object of inventory.objects) {
    if (kind === "context") {
      if (object.key.startsWith("trigger-workspaces/")) {
        fail(
          "inventory_namespace_invalid",
          "workspace object appeared in the context bucket",
        );
      }
      const parsed = parseContextKey(object.key);
      if (parsed?.sessionId === sessionId) selected.push(object);
      else unrelated.push(object);
      continue;
    }
    if (kind === "trigger_workspace") {
      if (object.key.startsWith("sessions/")) {
        fail(
          "inventory_namespace_invalid",
          "context object appeared in the workspace bucket",
        );
      }
      const parsed = parseWorkspaceKey(object.key);
      if (parsed?.launchId === launchId) {
        if (parsed.subjectHash !== subjectHash) {
          fail(
            "workspace_subject_mismatch",
            "current Launch appeared under another subject namespace",
          );
        }
        selected.push(object);
      } else unrelated.push(object);
      continue;
    }
    fail("internal_error", "unknown dedicated bucket kind");
  }
  return { selected, unrelated };
}

export function buildDeleteRequest({ baseUrl, bucket, object }) {
  const url = new URL(
    `/api/v1/proxy/s/${STORAGE_SERVICE_SLUG}/api/buckets/${encodeURIComponent(bucket)}/objects`,
    baseUrl,
  );
  url.searchParams.set("key", object.key);
  if (object.generation > 0) {
    url.searchParams.set("mutationGeneration", String(object.generation + 1));
  }
  return { method: "DELETE", url: url.href };
}

export function validateDeleteAcknowledgement(raw, object) {
  exactKeys(
    raw,
    ["data", "error"],
    "delete acknowledgement",
    "acknowledgement_drift",
  );
  if (raw.error !== null) {
    fail("acknowledgement_drift", "delete acknowledgement returned an error");
  }
  if (object.generation === 0) {
    exactKeys(
      raw.data,
      ["deleted"],
      "legacy delete acknowledgement.data",
      "acknowledgement_drift",
    );
    if (raw.data.deleted !== true) {
      fail(
        "acknowledgement_drift",
        "legacy deletion was not positively acknowledged",
      );
    }
    return { kind: "legacy" };
  }
  exactKeys(
    raw.data,
    ["deleted", "generation", "fenceVersion"],
    "fenced delete acknowledgement.data",
    "acknowledgement_drift",
  );
  if (
    raw.data.deleted !== true ||
    raw.data.generation !== object.generation + 1 ||
    raw.data.fenceVersion !== 1
  ) {
    fail("acknowledgement_drift", "fenced deletion acknowledgement drifted");
  }
  return { kind: "fenced" };
}

async function readBoundedJsonResponse(response, limit) {
  const declared = Number(response.headers?.get?.("content-length"));
  if (Number.isFinite(declared) && declared > limit) {
    fail("remote_response_too_large", "remote JSON exceeds the response bound");
  }
  const contentType = response.headers?.get?.("content-type") ?? "";
  if (
    !contentType.toLowerCase().includes("application/json") ||
    response.body === null
  ) {
    fail("remote_contract_drift", "remote response is not bounded JSON");
  }
  const chunks = [];
  let bytes = 0;
  try {
    for await (const chunk of response.body) {
      bytes += chunk.length;
      if (bytes > limit) {
        fail(
          "remote_response_too_large",
          "remote JSON exceeds the response bound",
        );
      }
      chunks.push(chunk);
    }
  } catch (cause) {
    if (cause instanceof SettlementFailure) throw cause;
    fail("remote_transport_error", "remote response body was interrupted");
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    fail("remote_contract_drift", "remote response is malformed JSON");
  }
}

async function requestJson({
  fetchImpl,
  method = "GET",
  url,
  token,
  form,
  limits,
  operation,
}) {
  const headers = {
    accept: "application/json",
    "user-agent": "cma-trigger-provider-object-settlement/1",
  };
  let body;
  if (token !== undefined) headers.authorization = `Bearer ${token}`;
  if (form !== undefined) {
    headers["content-type"] = "application/x-www-form-urlencoded";
    body = new URLSearchParams(form).toString();
  }
  let response;
  try {
    response = await fetchImpl(url, {
      method,
      headers,
      body,
      redirect: "error",
      signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    });
  } catch {
    fail(
      operation === "delete"
        ? "delete_outcome_ambiguous"
        : "remote_transport_error",
      `${operation} did not produce an HTTP response`,
    );
  }
  let responseJson;
  try {
    responseJson = await readBoundedJsonResponse(
      response,
      limits.maxResponseBytes,
    );
  } catch (cause) {
    if (operation === "delete") {
      fail(
        "delete_outcome_ambiguous",
        "delete response did not provide a readable acknowledgement",
      );
    }
    throw cause;
  }
  if (response.status !== 200) {
    fail(
      operation === "delete"
        ? "delete_outcome_ambiguous"
        : "remote_status_error",
      `${operation} returned an unexpected HTTP status`,
    );
  }
  return responseJson;
}

async function mintStorageToken({ handoff, fetchImpl, limits }) {
  const json = await requestJson({
    fetchImpl,
    method: "POST",
    url: `${handoff.nyxid_base_url}/oauth/token`,
    form: {
      grant_type: "client_credentials",
      client_id: handoff.storage_client_id,
      client_secret: handoff.storage_client_secret,
      scope: STORAGE_SCOPES.join(" "),
    },
    limits,
    operation: "token mint",
  });
  closedKeys(
    json,
    [
      "access_token",
      "token_type",
      "expires_in",
      "refresh_token",
      "id_token",
      "scope",
      "resource",
      "binding_id",
      "binding_updated",
      "issued_token_type",
    ],
    [
      "access_token",
      "token_type",
      "expires_in",
      "refresh_token",
      "id_token",
      "scope",
    ],
    "storage token response",
    "storage_token_drift",
  );
  boundedText(
    json.access_token,
    "storage token response.access_token",
    32_768,
    "storage_token_drift",
  );
  validateTokenSet(json.scope, STORAGE_SCOPES, "storage token response.scope");
  if (
    json.token_type !== "Bearer" ||
    !Number.isSafeInteger(json.expires_in) ||
    json.expires_in < MIN_STORAGE_TOKEN_LIFETIME_SECONDS ||
    json.refresh_token !== null ||
    json.id_token !== null
  ) {
    fail("storage_token_drift", "storage token contract drifted");
  }
  return json.access_token;
}

function bucketUrl(baseUrl, bucket) {
  return new URL(
    `/api/v1/proxy/s/${STORAGE_SERVICE_SLUG}/api/buckets/${encodeURIComponent(bucket)}`,
    baseUrl,
  ).href;
}

async function readBucketInventory({
  handoff,
  bucket,
  token,
  fetchImpl,
  limits,
}) {
  const json = await requestJson({
    fetchImpl,
    url: bucketUrl(handoff.nyxid_base_url, bucket),
    token,
    limits,
    operation: "bucket read",
  });
  return validateBucketInventory(json, {
    bucket,
    ownerServiceAccountId: handoff.storage_owner_service_account_id,
    limits,
  });
}

function objectAccounting(objects) {
  let selectedBytesObserved = 0;
  let selectedObjectsWithoutSize = 0;
  for (const object of objects) {
    if (object.size === undefined) selectedObjectsWithoutSize += 1;
    else selectedBytesObserved += object.size;
  }
  return { selectedBytesObserved, selectedObjectsWithoutSize };
}

function exactObjectSnapshot(objects) {
  return JSON.stringify(objects);
}

function settlementTimestamp(clock, evidence) {
  const raw = clock();
  const value = raw instanceof Date ? raw.toISOString() : raw;
  timestamp(value, "settled_at", "invalid_clock");
  if (Date.parse(value) < Date.parse(evidence.finished_at)) {
    fail("invalid_clock", "settlement time predates the closed evidence");
  }
  return value;
}

export function buildReceipt({ handoff, evidence, bucketResults, settledAt }) {
  const bucketReceipt = (result) => ({
    bucket_name_sha256: sha256(result.bucket),
    selected_objects: result.selected.length,
    selected_bytes_observed: result.selectedBytesObserved,
    selected_objects_without_size: result.selectedObjectsWithoutSize,
    legacy_deletions_acknowledged: result.legacyAcknowledged,
    fenced_deletions_acknowledged: result.fencedAcknowledged,
    unrelated_live_objects_observed: result.unrelatedAfter.length,
    current_run_live_objects_after: result.currentAfter.length,
    tombstone_retention: STORAGE_TOMBSTONE_RETENTION,
  });
  const receipt = {
    schema_version: 1,
    kind: RECEIPT_KIND,
    evidence_id: evidence.evidence_id,
    workflow: {
      repository_id: evidence.provenance.workflow.repository_id,
      run_id: evidence.provenance.workflow.run_id,
      run_attempt: evidence.provenance.workflow.run_attempt,
      source_sha: evidence.provenance.workflow.workflow_commit,
    },
    storage_contract: {
      repository: STORAGE_CONTRACT_REPOSITORY,
      source_sha: STORAGE_CONTRACT_SOURCE_SHA,
    },
    owner_service_account_id_sha256: sha256(
      handoff.storage_owner_service_account_id,
    ),
    launch_id_sha256: sha256(evidence.resources.launch_id),
    session_id_sha256: sha256(evidence.resources.session_id),
    settled_at: settledAt,
    buckets: {
      context: bucketReceipt(bucketResults.context),
      trigger_workspace: bucketReceipt(bucketResults.triggerWorkspace),
    },
  };
  validateReceipt(receipt);
  const serialized = JSON.stringify(receipt);
  for (const protectedValue of [
    handoff.nyxid_client_secret,
    handoff.storage_client_secret,
    handoff.access_token,
    handoff.context_bucket,
    handoff.trigger_workspace_bucket,
    handoff.storage_owner_service_account_id,
    evidence.resources.launch_id,
    evidence.resources.session_id,
  ]) {
    if (serialized.includes(protectedValue)) {
      fail(
        "receipt_redaction_failed",
        "receipt retained a protected raw value",
      );
    }
  }
  return receipt;
}

export function validateReceipt(raw) {
  exactKeys(
    raw,
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
    "receipt",
    "receipt_invalid",
  );
  if (raw.schema_version !== 1 || raw.kind !== RECEIPT_KIND) {
    fail("receipt_invalid", "receipt identity drifted");
  }
  boundedText(raw.evidence_id, "receipt.evidence_id", 255, "receipt_invalid");
  exactKeys(
    raw.workflow,
    ["repository_id", "run_id", "run_attempt", "source_sha"],
    "receipt.workflow",
    "receipt_invalid",
  );
  for (const field of ["repository_id", "run_id", "run_attempt"]) {
    safeInteger(
      raw.workflow[field],
      `receipt.workflow.${field}`,
      1,
      Number.MAX_SAFE_INTEGER,
      "receipt_invalid",
    );
  }
  if (!SHA1.test(raw.workflow.source_sha)) {
    fail("receipt_invalid", "receipt workflow source SHA is invalid");
  }
  exactKeys(
    raw.storage_contract,
    ["repository", "source_sha"],
    "receipt.storage_contract",
    "receipt_invalid",
  );
  if (
    raw.storage_contract.repository !== STORAGE_CONTRACT_REPOSITORY ||
    raw.storage_contract.source_sha !== STORAGE_CONTRACT_SOURCE_SHA
  ) {
    fail("receipt_invalid", "receipt storage contract drifted");
  }
  for (const field of [
    "owner_service_account_id_sha256",
    "launch_id_sha256",
    "session_id_sha256",
  ]) {
    if (!SHA256.test(raw[field]))
      fail("receipt_invalid", `receipt.${field} is invalid`);
  }
  timestamp(raw.settled_at, "receipt.settled_at", "receipt_invalid");
  exactKeys(
    raw.buckets,
    ["context", "trigger_workspace"],
    "receipt.buckets",
    "receipt_invalid",
  );
  const bucketHashes = [];
  for (const kind of ["context", "trigger_workspace"]) {
    const bucket = raw.buckets[kind];
    exactKeys(
      bucket,
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
      `receipt.buckets.${kind}`,
      "receipt_invalid",
    );
    if (!SHA256.test(bucket.bucket_name_sha256)) {
      fail("receipt_invalid", `receipt.buckets.${kind} hash is invalid`);
    }
    bucketHashes.push(bucket.bucket_name_sha256);
    for (const field of [
      "selected_objects",
      "selected_bytes_observed",
      "selected_objects_without_size",
      "legacy_deletions_acknowledged",
      "fenced_deletions_acknowledged",
      "unrelated_live_objects_observed",
      "current_run_live_objects_after",
    ]) {
      safeInteger(
        bucket[field],
        `receipt.buckets.${kind}.${field}`,
        0,
        Number.MAX_SAFE_INTEGER,
        "receipt_invalid",
      );
    }
    if (
      bucket.selected_objects !==
        bucket.legacy_deletions_acknowledged +
          bucket.fenced_deletions_acknowledged ||
      bucket.selected_objects_without_size > bucket.selected_objects ||
      bucket.current_run_live_objects_after !== 0 ||
      bucket.tombstone_retention !== STORAGE_TOMBSTONE_RETENTION
    ) {
      fail("receipt_invalid", `receipt.buckets.${kind} accounting drifted`);
    }
  }
  if (bucketHashes[0] === bucketHashes[1]) {
    fail("receipt_invalid", "receipt dedicated bucket hashes collide");
  }
  return raw;
}

export async function settleProviderObjects({
  handoff: rawHandoff,
  evidence: rawEvidence,
  fetchImpl = globalThis.fetch,
  clock = () => new Date(),
  limits: limitOverrides = {},
}) {
  const handoff = validateHandoff(rawHandoff);
  const evidence = validateEvidence(rawEvidence);
  const limits = normalizeLimits(limitOverrides);
  if (typeof fetchImpl !== "function") {
    fail("invalid_transport", "fetch implementation is missing");
  }
  const token = await mintStorageToken({ handoff, fetchImpl, limits });
  const contextInitial = await readBucketInventory({
    handoff,
    bucket: handoff.context_bucket,
    token,
    fetchImpl,
    limits,
  });
  const workspaceInitial = await readBucketInventory({
    handoff,
    bucket: handoff.trigger_workspace_bucket,
    token,
    fetchImpl,
    limits,
  });
  const contextClassified = classifyCurrentRunObjects(contextInitial, {
    kind: "context",
    sessionId: evidence.resources.session_id,
    launchId: evidence.resources.launch_id,
  });
  const workspaceClassified = classifyCurrentRunObjects(workspaceInitial, {
    kind: "trigger_workspace",
    sessionId: evidence.resources.session_id,
    launchId: evidence.resources.launch_id,
    subjectHash: sha256(handoff.subject_id),
  });
  const totalSelected =
    contextClassified.selected.length + workspaceClassified.selected.length;
  if (
    totalSelected > limits.maxSelectedObjects ||
    totalSelected > limits.maxDeletes
  ) {
    fail(
      "selected_object_count_overflow",
      "selected deletion count exceeds its bound",
    );
  }
  const contextAccounting = objectAccounting(contextClassified.selected);
  const workspaceAccounting = objectAccounting(workspaceClassified.selected);
  const totalSelectedBytes =
    contextAccounting.selectedBytesObserved +
    workspaceAccounting.selectedBytesObserved;
  if (
    !Number.isSafeInteger(totalSelectedBytes) ||
    totalSelectedBytes > limits.maxSelectedBytes
  ) {
    fail("selected_byte_overflow", "selected object bytes exceed their bound");
  }

  const deleteSelected = async (bucket, objects) => {
    let legacyAcknowledged = 0;
    let fencedAcknowledged = 0;
    for (const object of objects) {
      const request = buildDeleteRequest({
        baseUrl: handoff.nyxid_base_url,
        bucket,
        object,
      });
      const acknowledgement = await requestJson({
        fetchImpl,
        method: request.method,
        url: request.url,
        token,
        limits,
        operation: "delete",
      });
      const result = validateDeleteAcknowledgement(acknowledgement, object);
      if (result.kind === "legacy") legacyAcknowledged += 1;
      else fencedAcknowledged += 1;
    }
    return { legacyAcknowledged, fencedAcknowledged };
  };

  const contextAcknowledged = await deleteSelected(
    handoff.context_bucket,
    contextClassified.selected,
  );
  const workspaceAcknowledged = await deleteSelected(
    handoff.trigger_workspace_bucket,
    workspaceClassified.selected,
  );

  const contextAfterInventory = await readBucketInventory({
    handoff,
    bucket: handoff.context_bucket,
    token,
    fetchImpl,
    limits,
  });
  const workspaceAfterInventory = await readBucketInventory({
    handoff,
    bucket: handoff.trigger_workspace_bucket,
    token,
    fetchImpl,
    limits,
  });
  const contextAfter = classifyCurrentRunObjects(contextAfterInventory, {
    kind: "context",
    sessionId: evidence.resources.session_id,
    launchId: evidence.resources.launch_id,
  });
  const workspaceAfter = classifyCurrentRunObjects(workspaceAfterInventory, {
    kind: "trigger_workspace",
    sessionId: evidence.resources.session_id,
    launchId: evidence.resources.launch_id,
    subjectHash: sha256(handoff.subject_id),
  });
  if (contextAfter.selected.length > 0 || workspaceAfter.selected.length > 0) {
    fail(
      "residual_current_run_object",
      "current-run live objects remain after deletion",
    );
  }
  if (
    exactObjectSnapshot(contextClassified.unrelated) !==
      exactObjectSnapshot(contextAfter.unrelated) ||
    exactObjectSnapshot(workspaceClassified.unrelated) !==
      exactObjectSnapshot(workspaceAfter.unrelated)
  ) {
    fail(
      "unrelated_object_drift",
      "unrelated live-object inventory changed during settlement",
    );
  }

  const settledAt = settlementTimestamp(clock, evidence);
  return buildReceipt({
    handoff,
    evidence,
    settledAt,
    bucketResults: {
      context: {
        bucket: handoff.context_bucket,
        selected: contextClassified.selected,
        unrelatedAfter: contextAfter.unrelated,
        currentAfter: contextAfter.selected,
        ...contextAccounting,
        ...contextAcknowledged,
      },
      triggerWorkspace: {
        bucket: handoff.trigger_workspace_bucket,
        selected: workspaceClassified.selected,
        unrelatedAfter: workspaceAfter.unrelated,
        currentAfter: workspaceAfter.selected,
        ...workspaceAccounting,
        ...workspaceAcknowledged,
      },
    },
  });
}

async function readJsonInput(path, maximum, label) {
  let handle;
  try {
    handle = await open(path, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const stat = await handle.stat();
    if (!stat.isFile() || stat.size <= 0 || stat.size > maximum) {
      fail("invalid_input_file", `${label} is not a bounded regular file`);
    }
    const serialized = await handle.readFile("utf8");
    if (Buffer.byteLength(serialized, "utf8") > maximum) {
      fail("invalid_input_file", `${label} exceeds its byte bound`);
    }
    return JSON.parse(serialized);
  } catch (cause) {
    if (cause instanceof SettlementFailure) throw cause;
    fail("invalid_input_file", `${label} is missing or not valid JSON`);
  } finally {
    await handle?.close();
  }
}

function parseArguments(argv) {
  if (argv[0] !== "settle") {
    fail(
      "invalid_arguments",
      "usage: settle --handoff FILE --evidence FILE --output FILE",
    );
  }
  const values = new Map();
  for (let index = 1; index < argv.length; index += 2) {
    const flag = argv[index];
    const value = argv[index + 1];
    if (
      !["--handoff", "--evidence", "--output"].includes(flag) ||
      value === undefined ||
      value.length === 0 ||
      values.has(flag)
    ) {
      fail(
        "invalid_arguments",
        "usage: settle --handoff FILE --evidence FILE --output FILE",
      );
    }
    values.set(flag, resolve(value));
  }
  if (argv.length !== 7 || values.size !== 3) {
    fail(
      "invalid_arguments",
      "usage: settle --handoff FILE --evidence FILE --output FILE",
    );
  }
  const handoffPath = values.get("--handoff");
  const evidencePath = values.get("--evidence");
  const outputPath = values.get("--output");
  if (new Set([handoffPath, evidencePath, outputPath]).size !== 3) {
    fail("invalid_arguments", "input and output paths must be distinct");
  }
  return { handoffPath, evidencePath, outputPath };
}

async function reserveOutput(path) {
  try {
    const parent = await lstat(dirname(path));
    if (!parent.isDirectory())
      fail("invalid_output", "output parent is not a directory");
    return await open(
      path,
      fsConstants.O_CREAT |
        fsConstants.O_EXCL |
        fsConstants.O_WRONLY |
        fsConstants.O_NOFOLLOW,
      0o600,
    );
  } catch (cause) {
    if (cause instanceof SettlementFailure) throw cause;
    fail("invalid_output", "output path cannot be reserved exclusively");
  }
}

export async function main(
  argv = process.argv.slice(2),
  { fetchImpl = globalThis.fetch, clock = () => new Date() } = {},
) {
  const { handoffPath, evidencePath, outputPath } = parseArguments(argv);
  const [handoff, evidence] = await Promise.all([
    readJsonInput(handoffPath, MAX_HANDOFF_JSON_BYTES, "handoff"),
    readJsonInput(evidencePath, MAX_EVIDENCE_JSON_BYTES, "evidence"),
  ]);
  validateHandoff(handoff);
  validateEvidence(evidence);
  const output = await reserveOutput(outputPath);
  let completed = false;
  try {
    const receipt = await settleProviderObjects({
      handoff,
      evidence,
      fetchImpl,
      clock,
    });
    await output.writeFile(`${JSON.stringify(receipt, null, 2)}\n`, "utf8");
    await output.sync();
    completed = true;
    return receipt;
  } finally {
    await output.close();
    if (!completed) {
      try {
        await unlink(outputPath);
      } catch {
        // Preserve the primary failure; an invalid reservation is not a receipt.
      }
    }
  }
}

const entryPath =
  process.argv[1] === undefined
    ? null
    : pathToFileURL(resolve(process.argv[1])).href;
if (entryPath === import.meta.url) {
  await main().catch((cause) => {
    const code =
      cause instanceof SettlementFailure
        ? cause.code
        : "provider_object_settlement_failed";
    console.error(`[provider-object-settlement] ${code}`);
    process.exitCode = 1;
  });
}
