#!/usr/bin/env node

import assert from "node:assert/strict";
import {
  createHash,
  createPublicKey,
  randomBytes,
  timingSafeEqual,
  verify as verifySignature,
} from "node:crypto";
import { constants as fsConstants } from "node:fs";
import { spawn } from "node:child_process";
import {
  chmod,
  lstat,
  mkdir,
  mkdtemp,
  open,
  readFile,
  realpath,
  rename,
  rm,
  writeFile,
} from "node:fs/promises";
import {
  basename,
  dirname,
  isAbsolute,
  join,
  relative,
  resolve,
} from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

process.umask(0o077);

const TOOL_ROOT = dirname(fileURLToPath(import.meta.url));
const REPOSITORY_ROOT = resolve(TOOL_ROOT, "../../..");
const SEALER_MANIFEST = join(TOOL_ROOT, "sealed-box", "Cargo.toml");
const NYXID_BASE_URL = "https://nyx-api.chrono-ai.fun";
const NYXID_SOURCE_REVISION = "245600cb15110c43aafed77c8a8ee73f655ac060";
const NYXID_API_VERSION = "0.10.0";
const GITHUB_CATALOG_SERVICE_ID = "86741372-764f-4423-8687-95bebece158c";
const ENVIRONMENT = "cma-trigger-smoke";
const WORKFLOW_PATH = ".github/workflows/cma-trigger-provider-smoke.yml";
const WORKFLOW_FILE = "cma-trigger-provider-smoke.yml";
const EPHEMERAL_HANDOFF_SECRET_PREFIX = "CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_";
const RUNTIME_SERVICE_SECRET = "CMA_TRIGGER_SMOKE_SERVICE_ID";
const KUBECONFIG_SECRET = "CMA_TRIGGER_SMOKE_KUBECONFIG_B64";
const KIND_CA_MATERIAL_NAME = "CMA_TRIGGER_SMOKE_KIND_CA_" + "KEY_B64";
const KUBECONFIG_DIGEST_VARIABLE = "CMA_TRIGGER_SMOKE_KUBECONFIG_SHA256";
const SETUP_CONFIRMATION = "setup-reviewed-cma-trigger-staging";
const RUN_CONFIRMATION = "dispatch-reviewed-cma-trigger-staging";
const CLEANUP_CONFIRMATION = "cleanup-reviewed-cma-trigger-staging";
const CONFIRMATION_ENV = "CMA_TRIGGER_SMOKE_ENV_CONFIRM";
const TOKEN_ENV = "CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN";
const TOKEN_FILE_ENV = "CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE";
const MAX_JSON_BYTES = 4 * 1024 * 1024;
const MAX_SECRET_BYTES = 256 * 1024;
const MAX_GITHUB_SECRET_PLAINTEXT_BYTES = 48 * 1024;
const MAX_TOKEN_BYTES = 64 * 1024;
const PREFLIGHT_LIFETIME_SECONDS = 10 * 60;
const MIN_HANDOFF_LIFETIME_SECONDS = 7_200;
const MAX_HANDOFF_LIFETIME_SECONDS = 28_800;
const HTTP_TIMEOUT_MS = 30_000;
const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SHA1 = /^[0-9a-f]{40}$/u;
const SHA256 = /^[0-9a-f]{64}$/u;
const DNS_LABEL = /^[a-z0-9](?:[-a-z0-9]{0,61}[a-z0-9])?$/u;
const SERVICE_SLUG = /^[a-z0-9](?:[a-z0-9-]{0,78}[a-z0-9])?$/u;
const BUCKET = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/u;
const SA_CLIENT = /^sa_[0-9a-f]{24}$/u;
const DISPATCH_NONCE = /^[0-9a-f]{32}$/u;
const EPHEMERAL_HANDOFF_SECRET_NAME =
  /^CMA_TRIGGER_SMOKE_NYXID_BOOTSTRAP_[0-9A-F]{32}$/u;

const TARGET = Object.freeze({
  repository: Object.freeze({
    owner: "ChronoAIProject",
    name: "cma",
    id: 1_330_597_394,
    node_id: "R_kgDOT09OEg",
    default_branch: "develop",
  }),
  environment: Object.freeze({
    name: ENVIRONMENT,
    id: 20_761_349_259,
    node_id: "EN_kwDOT09OEs8AAAAE1XkMiw",
    reviewer: Object.freeze({
      type: "Team",
      id: 15_748_251,
      node_id: "T_kwDODyHfns4A8Eyb",
      slug: "ai-application",
    }),
  }),
  workflow: Object.freeze({
    id: 344_394_996,
    node_id: "W_kwDOT09OEs4Uhwz0",
    name: "CMA Trigger provider smoke",
    path: WORKFLOW_PATH,
  }),
  account: Object.freeze({
    login: "chronoai-shining",
    id: 250_120_269,
    node_id: "U_kgDODuiITQ",
  }),
});

const STATIC_VARIABLE_NAMES = Object.freeze([
  "CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN",
  "CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE",
  "CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE",
  "CMA_TRIGGER_SMOKE_FORK_REPOSITORY",
  "CMA_TRIGGER_SMOKE_MARKER_INPUT",
  "CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE",
  "CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH",
  "CMA_TRIGGER_SMOKE_UPSTREAM_OWNER",
  "CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY",
  "CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID",
]);
const MANAGED_VARIABLE_NAMES = Object.freeze([
  ...STATIC_VARIABLE_NAMES,
  KUBECONFIG_DIGEST_VARIABLE,
]);
const MANAGED_SECRET_NAMES = Object.freeze([
  KIND_CA_MATERIAL_NAME,
  KUBECONFIG_SECRET,
  RUNTIME_SERVICE_SECRET,
]);
const ACTIVE_RUN_STATUSES = Object.freeze([
  "in_progress",
  "pending",
  "queued",
  "requested",
  "waiting",
]);
const REQUIRED_DEVELOP_CONTEXTS = Object.freeze([
  "Backend",
  "Frontend",
  "Images (api)",
  "Images (runtime)",
  "Images (web)",
  "Manifests",
  "Repository structure",
  "Rust SDK",
  "Secret scanning",
  "Trigger provider smoke contract",
  "Trigger release contract",
  "TypeScript SDK",
]);
const RUNTIME_GITHUB_SCOPES = Object.freeze([
  "public_repo",
  "read:user",
  "user:email",
]);
const PROVISIONING_GITHUB_SCOPES = Object.freeze([
  "repo",
  "read:user",
  "user:email",
]);

class OperatorFailure extends Error {
  constructor(code, message, details = undefined) {
    super(message);
    this.name = "OperatorFailure";
    this.code = code;
    this.details = details;
  }
}

function fail(code, message, details = undefined) {
  throw new OperatorFailure(code, message, details);
}

function object(value, path, code = "contract_drift") {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(code, `${path} must be an object`);
  }
  return value;
}

function exactKeys(value, expected, path, code = "contract_drift") {
  const actual = Object.keys(object(value, path, code)).sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((key, index) => key !== wanted[index])
  ) {
    fail(
      code,
      `${path} has a closed schema (expected ${wanted.join(", ")}; got ${actual.join(", ")})`,
    );
  }
}

function closedKeys(value, allowed, required, path, code = "contract_drift") {
  const keys = Object.keys(object(value, path, code));
  const extras = keys.filter((key) => !allowed.includes(key));
  const missing = required.filter((key) => !keys.includes(key));
  if (extras.length > 0 || missing.length > 0) {
    fail(
      code,
      `${path} has a closed schema (extra: ${extras.join(", ") || "none"}; missing: ${missing.join(", ") || "none"})`,
    );
  }
}

function boundedString(value, path, maximum = 4096, code = "contract_drift") {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    fail(code, `${path} must be one bounded printable string`);
  }
  return value;
}

function integer(value, path, code = "contract_drift") {
  if (!Number.isSafeInteger(value)) fail(code, `${path} must be an integer`);
  return value;
}

function positiveInteger(value, path, code = "contract_drift") {
  integer(value, path, code);
  if (value <= 0) fail(code, `${path} must be positive`);
  return value;
}

function canonicalUuid(value, path, code = "contract_drift") {
  if (typeof value !== "string" || !UUID.test(value)) {
    fail(code, `${path} must be one canonical non-nil UUID`);
  }
  return value;
}

function exactString(value, expected, path, code = "contract_drift") {
  if (value !== expected) fail(code, `${path} must equal ${expected}`);
  return value;
}

function exactSet(value, expected, path, code = "authority_drift") {
  if (
    !Array.isArray(value) ||
    value.some((item) => typeof item !== "string") ||
    new Set(value).size !== value.length
  ) {
    fail(code, `${path} must be an array of unique strings`);
  }
  const actual = [...value].sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((item, index) => item !== wanted[index])
  ) {
    fail(code, `${path} differs from the reviewed closed set`);
  }
  return actual;
}

function canonicalJson(value) {
  if (Array.isArray(value)) {
    return `[${value.map((item) => canonicalJson(item)).join(",")}]`;
  }
  if (value !== null && typeof value === "object") {
    return `{${Object.keys(value)
      .sort()
      .map((key) => `${JSON.stringify(key)}:${canonicalJson(value[key])}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function sha256(value) {
  return createHash("sha256").update(value).digest("hex");
}

function newDispatchNonce() {
  return randomBytes(16).toString("hex");
}

function validateDispatchNonce(value) {
  if (typeof value !== "string" || !DISPATCH_NONCE.test(value)) {
    fail("dispatch_nonce_invalid", "dispatch nonce is not canonical");
  }
  return value;
}

function ephemeralHandoffSecretName(dispatchNonce) {
  return `${EPHEMERAL_HANDOFF_SECRET_PREFIX}${validateDispatchNonce(
    dispatchNonce,
  ).toUpperCase()}`;
}

function dispatchRunTitle(dispatchNonce) {
  return `CMA Trigger provider smoke ${validateDispatchNonce(dispatchNonce)}`;
}

function isManagedSecretName(name) {
  return (
    MANAGED_SECRET_NAMES.includes(name) ||
    (typeof name === "string" && EPHEMERAL_HANDOFF_SECRET_NAME.test(name))
  );
}

function ephemeralHandoffSecretNames(inventory) {
  return Object.keys(inventory).filter((name) =>
    EPHEMERAL_HANDOFF_SECRET_NAME.test(name),
  );
}

function requireEphemeralSecretSet(inventory, expected = []) {
  const observed = ephemeralHandoffSecretNames(inventory).sort();
  const desired = [...expected].sort();
  if (
    observed.length !== desired.length ||
    observed.some((name, index) => name !== desired[index])
  ) {
    fail("ephemeral_secret_conflict", "ephemeral handoff lease set drifted", {
      expected: desired,
      observed,
    });
  }
}

function secretEqual(left, right) {
  const a = Buffer.from(left);
  const b = Buffer.from(right);
  return a.length === b.length && timingSafeEqual(a, b);
}

function nowSeconds() {
  return Math.floor(Date.now() / 1000);
}

function parseTimestamp(value, path, code = "contract_drift") {
  boundedString(value, path, 128, code);
  const milliseconds = Date.parse(value);
  if (!Number.isFinite(milliseconds)) fail(code, `${path} is not RFC 3339`);
  return milliseconds;
}

function validateStaticVariables(raw) {
  exactKeys(raw, STATIC_VARIABLE_NAMES, "static_variables", "invalid_config");
  for (const name of STATIC_VARIABLE_NAMES) {
    if (name === "CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE") {
      if (
        typeof raw[name] !== "string" ||
        raw[name].length === 0 ||
        raw[name].length > 4096 ||
        /[\u0000-\u0009\u000b-\u001f\u007f]/u.test(raw[name])
      ) {
        fail("invalid_config", `static_variables.${name} is not bounded text`);
      }
    } else {
      boundedString(
        raw[name],
        `static_variables.${name}`,
        4096,
        "invalid_config",
      );
    }
  }
  const expected = {
    CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN: TARGET.account.login,
    CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE:
      "CMA Trigger provider smoke {marker}\n",
    CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE: "cma-trigger-smoke/{marker}.txt",
    CMA_TRIGGER_SMOKE_FORK_REPOSITORY: "cma-trigger-fixture",
    CMA_TRIGGER_SMOKE_MARKER_INPUT: "smoke_marker",
    CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE: "cma-trigger-sandboxes",
    CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH: "main",
    CMA_TRIGGER_SMOKE_UPSTREAM_OWNER: "ChronoAIProject",
    CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY: "cma-trigger-fixture",
    CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID: "1349237845",
  };
  for (const [name, value] of Object.entries(expected)) {
    exactString(raw[name], value, `static_variables.${name}`, "invalid_config");
  }
  if (!DNS_LABEL.test(raw.CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE)) {
    fail("invalid_config", "sandbox namespace is not a DNS-1123 label");
  }
  return raw;
}

function validateConfig(raw) {
  exactKeys(
    raw,
    [
      "schema_version",
      "environment",
      "nyxid_base_url",
      "provisioning_user_service_id",
      "dispatch",
      "static_variables",
    ],
    "config",
    "invalid_config",
  );
  if (raw.schema_version !== 1) {
    fail("invalid_config", "config schema_version must be 1");
  }
  exactString(raw.environment, ENVIRONMENT, "environment", "invalid_config");
  exactString(
    raw.nyxid_base_url,
    NYXID_BASE_URL,
    "nyxid_base_url",
    "invalid_config",
  );
  canonicalUuid(
    raw.provisioning_user_service_id,
    "provisioning_user_service_id",
    "invalid_config",
  );
  exactKeys(
    raw.dispatch,
    [
      "ref",
      "expected_fork_mode",
      "preservation_confirmation",
      "poll_seconds",
      "watch_timeout_seconds",
    ],
    "dispatch",
    "invalid_config",
  );
  exactString(raw.dispatch.ref, "develop", "dispatch.ref", "invalid_config");
  if (!["new", "reuse"].includes(raw.dispatch.expected_fork_mode)) {
    fail("invalid_config", "dispatch.expected_fork_mode is invalid");
  }
  exactString(
    raw.dispatch.preservation_confirmation,
    "PRESERVE",
    "dispatch.preservation_confirmation",
    "invalid_config",
  );
  integer(raw.dispatch.poll_seconds, "dispatch.poll_seconds", "invalid_config");
  integer(
    raw.dispatch.watch_timeout_seconds,
    "dispatch.watch_timeout_seconds",
    "invalid_config",
  );
  if (raw.dispatch.poll_seconds < 2 || raw.dispatch.poll_seconds > 30) {
    fail("invalid_config", "dispatch.poll_seconds must be between 2 and 30");
  }
  if (
    raw.dispatch.watch_timeout_seconds < 900 ||
    raw.dispatch.watch_timeout_seconds > 21_600
  ) {
    fail(
      "invalid_config",
      "dispatch.watch_timeout_seconds must be between 900 and 21600",
    );
  }
  validateStaticVariables(raw.static_variables);
  return raw;
}

async function readBoundedFile(path, maximum, label) {
  let handle;
  try {
    handle = await open(path, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const info = await handle.stat();
    if (!info.isFile() || info.size <= 0 || info.size > maximum) {
      fail("unsafe_file", `${label} must be a nonempty bounded regular file`);
    }
    const buffer = Buffer.alloc(info.size);
    const { bytesRead } = await handle.read(buffer, 0, info.size, 0);
    if (bytesRead !== info.size)
      fail("unsafe_file", `${label} changed while read`);
    const after = await handle.stat();
    if (
      after.dev !== info.dev ||
      after.ino !== info.ino ||
      after.size !== info.size ||
      after.mtimeMs !== info.mtimeMs
    ) {
      fail("unsafe_file", `${label} changed while read`);
    }
    return buffer;
  } finally {
    await handle?.close();
  }
}

async function readJsonFile(path, maximum = MAX_JSON_BYTES) {
  const buffer = await readBoundedFile(path, maximum, path);
  try {
    return JSON.parse(buffer.toString("utf8"));
  } catch {
    fail("invalid_json", `${path} is not valid JSON`);
  }
}

function secureAbsolutePath(path, label) {
  if (typeof path !== "string" || !isAbsolute(path) || resolve(path) !== path) {
    fail("unsafe_secret_path", `${label} must be one normalized absolute path`);
  }
  const fromRepository = relative(REPOSITORY_ROOT, path);
  if (
    fromRepository === "" ||
    (!fromRepository.startsWith("..") && !isAbsolute(fromRepository))
  ) {
    fail("unsafe_secret_path", `${label} must be outside the repository`);
  }
  return path;
}

async function requireCanonicalRealPath(path, label) {
  let actual;
  try {
    actual = await realpath(path);
  } catch {
    fail("unsafe_secret_path", `${label} could not be resolved safely`);
  }
  if (actual !== path) {
    fail(
      "unsafe_secret_path",
      `${label} must not traverse any symbolic-link component`,
    );
  }
}

async function verifyPrivateDirectory(path, label = "custody directory") {
  secureAbsolutePath(path, label);
  const info = await lstat(path);
  await requireCanonicalRealPath(path, label);
  if (!info.isDirectory() || info.isSymbolicLink()) {
    fail("unsafe_secret_path", `${label} must be a real directory`);
  }
  if (typeof process.getuid === "function" && info.uid !== process.getuid()) {
    fail("unsafe_secret_path", `${label} is owned by another user`);
  }
  if ((info.mode & 0o777) !== 0o700) {
    fail("unsafe_secret_path", `${label} must have mode 0700`);
  }
}

async function verifySecureFile(path, label) {
  secureAbsolutePath(path, label);
  await verifyPrivateDirectory(dirname(path), `${label} parent`);
  await requireCanonicalRealPath(path, label);
  const info = await lstat(path);
  if (!info.isFile() || info.isSymbolicLink() || info.nlink !== 1) {
    fail(
      "unsafe_secret_file",
      `${label} must be one regular singly linked file`,
    );
  }
  if (typeof process.getuid === "function" && info.uid !== process.getuid()) {
    fail("unsafe_secret_file", `${label} is owned by another user`);
  }
  if ((info.mode & 0o777) !== 0o600) {
    fail("unsafe_secret_file", `${label} must have mode 0600`);
  }
  return info;
}

async function readSecureBoundedFile(path, label, maximum) {
  const expected = await verifySecureFile(path, label);
  let handle;
  try {
    handle = await open(path, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const info = await handle.stat();
    if (
      !info.isFile() ||
      info.dev !== expected.dev ||
      info.ino !== expected.ino ||
      info.nlink !== 1 ||
      info.size <= 0 ||
      info.size > maximum ||
      (typeof process.getuid === "function" && info.uid !== process.getuid()) ||
      (info.mode & 0o777) !== 0o600
    ) {
      fail("unsafe_secret_file", `${label} changed before its secure read`);
    }
    const buffer = Buffer.alloc(info.size);
    const { bytesRead } = await handle.read(buffer, 0, info.size, 0);
    if (bytesRead !== info.size) {
      fail("unsafe_secret_file", `${label} changed during its secure read`);
    }
    const after = await handle.stat();
    if (
      after.dev !== info.dev ||
      after.ino !== info.ino ||
      after.size !== info.size ||
      after.mtimeMs !== info.mtimeMs ||
      after.nlink !== 1
    ) {
      fail("unsafe_secret_file", `${label} changed during its secure read`);
    }
    await verifyPrivateDirectory(dirname(path), `${label} parent`);
    await requireCanonicalRealPath(path, label);
    const final = await lstat(path);
    if (final.dev !== info.dev || final.ino !== info.ino) {
      fail(
        "unsafe_secret_file",
        `${label} was replaced during its secure read`,
      );
    }
    return buffer;
  } finally {
    await handle?.close();
  }
}

async function readSecureJson(path, label, maximum = MAX_JSON_BYTES) {
  const buffer = await readSecureBoundedFile(path, label, maximum);
  try {
    return JSON.parse(buffer.toString("utf8"));
  } catch {
    fail("invalid_json", `${path} is not valid JSON`);
  }
}

async function atomicWrite(path, data, mode = 0o600) {
  const parent = dirname(path);
  await verifyPrivateDirectory(parent, "output parent");
  const temporary = join(
    parent,
    `.${basename(path)}.${process.pid}.${Date.now()}.tmp`,
  );
  const handle = await open(temporary, "wx", mode);
  try {
    await handle.writeFile(data);
    await handle.sync();
  } finally {
    await handle.close();
  }
  await rename(temporary, path);
  await chmod(path, mode);
}

async function readProvisioningToken(environment = process.env) {
  const direct = environment[TOKEN_ENV];
  const file = environment[TOKEN_FILE_ENV];
  if ((direct === undefined) === (file === undefined)) {
    fail(
      "credential_input_invalid",
      `set exactly one of ${TOKEN_ENV} or ${TOKEN_FILE_ENV}`,
    );
  }
  let token;
  if (file !== undefined) {
    secureAbsolutePath(file, TOKEN_FILE_ENV);
    token = (
      await readSecureBoundedFile(file, TOKEN_FILE_ENV, MAX_TOKEN_BYTES)
    ).toString("utf8");
    if (token.endsWith("\n")) token = token.slice(0, -1);
  } else {
    token = direct;
  }
  boundedString(
    token,
    "provisioning NyxID token",
    MAX_TOKEN_BYTES,
    "credential_input_invalid",
  );
  if (/\s/u.test(token)) {
    fail(
      "credential_input_invalid",
      "provisioning NyxID token contains whitespace",
    );
  }
  return token;
}

function validateBootstrapState(raw) {
  exactKeys(
    raw,
    [
      "schema_version",
      "environment",
      "nyxid_base_url",
      "source_revision",
      "api_version",
      "observer",
      "oidc_app",
      "storage_service_account",
      "buckets",
      "caller",
    ],
    "bootstrap state",
    "unsafe_state",
  );
  if (
    raw.schema_version !== 1 ||
    raw.environment !== ENVIRONMENT ||
    raw.nyxid_base_url !== NYXID_BASE_URL ||
    raw.source_revision !== NYXID_SOURCE_REVISION ||
    raw.api_version !== NYXID_API_VERSION
  ) {
    fail(
      "state_authority_mismatch",
      "bootstrap state contract identity drifted",
    );
  }
  if (raw.observer !== null) {
    exactKeys(raw.observer, ["id", "slug"], "state.observer", "unsafe_state");
    canonicalUuid(raw.observer.id, "state.observer.id", "unsafe_state");
    if (!SERVICE_SLUG.test(raw.observer.slug)) {
      fail("unsafe_state", "state observer slug is invalid");
    }
  }
  if (raw.oidc_app !== null) {
    exactKeys(
      raw.oidc_app,
      ["id", "client_secret"],
      "state.oidc_app",
      "unsafe_state",
    );
    canonicalUuid(raw.oidc_app.id, "state.oidc_app.id", "unsafe_state");
    boundedString(
      raw.oidc_app.client_secret,
      "state.oidc_app.client_secret",
      32_768,
      "unsafe_state",
    );
  }
  if (raw.storage_service_account !== null) {
    exactKeys(
      raw.storage_service_account,
      ["id", "client_id", "client_secret"],
      "state.storage_service_account",
      "unsafe_state",
    );
    canonicalUuid(
      raw.storage_service_account.id,
      "state.storage_service_account.id",
      "unsafe_state",
    );
    if (!SA_CLIENT.test(raw.storage_service_account.client_id)) {
      fail("unsafe_state", "state storage client ID is invalid");
    }
    boundedString(
      raw.storage_service_account.client_secret,
      "state.storage_service_account.client_secret",
      32_768,
      "unsafe_state",
    );
  }
  exactKeys(
    raw.buckets,
    ["context_bucket", "trigger_workspace_bucket", "owner_service_account_id"],
    "state.buckets",
    "unsafe_state",
  );
  for (const field of ["context_bucket", "trigger_workspace_bucket"]) {
    if (!BUCKET.test(raw.buckets[field])) {
      fail("unsafe_state", `state.buckets.${field} is invalid`);
    }
  }
  if (raw.buckets.context_bucket === raw.buckets.trigger_workspace_bucket) {
    fail("unsafe_state", "state buckets must be distinct");
  }
  if (raw.buckets.owner_service_account_id !== null) {
    canonicalUuid(
      raw.buckets.owner_service_account_id,
      "state.buckets.owner_service_account_id",
      "unsafe_state",
    );
  }
  if (raw.caller !== null) {
    exactKeys(
      raw.caller,
      ["client_id", "subject_id", "service_id", "refresh_token", "scope"],
      "state.caller",
      "unsafe_state",
    );
    for (const field of ["client_id", "subject_id", "service_id"]) {
      canonicalUuid(raw.caller[field], `state.caller.${field}`, "unsafe_state");
    }
    boundedString(
      raw.caller.refresh_token,
      "state.caller.refresh_token",
      32_768,
      "unsafe_state",
    );
    exactString(
      raw.caller.scope,
      "openid profile email roles proxy offline_access",
      "state.caller.scope",
      "unsafe_state",
    );
    if (raw.oidc_app === null || raw.caller.client_id !== raw.oidc_app.id) {
      fail(
        "state_authority_mismatch",
        "caller is not bound to the exact OIDC app",
      );
    }
  }
  if (
    raw.buckets.owner_service_account_id !== null &&
    raw.storage_service_account !== null &&
    raw.buckets.owner_service_account_id !== raw.storage_service_account.id
  ) {
    fail("state_authority_mismatch", "bucket owner is not the storage account");
  }
  return raw;
}

function validateHandoff(raw, state, provisioningServiceId, at = nowSeconds()) {
  exactKeys(
    raw,
    [
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
    ],
    "NyxID smoke handoff",
    "unsafe_handoff",
  );
  if (state.caller === null) {
    fail(
      "runtime_authority_missing",
      "bootstrap state has no selected caller authority",
    );
  }
  if (
    state.oidc_app === null ||
    state.storage_service_account === null ||
    state.observer === null
  ) {
    fail(
      "runtime_authority_missing",
      "bootstrap state resources are incomplete",
    );
  }
  if (raw.schema_version !== 1 || raw.nyxid_base_url !== NYXID_BASE_URL) {
    fail("unsafe_handoff", "handoff contract identity drifted");
  }
  canonicalUuid(
    raw.nyxid_client_id,
    "handoff NyxID client ID",
    "unsafe_handoff",
  );
  canonicalUuid(raw.subject_id, "handoff subject ID", "unsafe_handoff");
  if (!SA_CLIENT.test(raw.storage_client_id)) {
    fail("unsafe_handoff", "handoff storage client ID is invalid");
  }
  canonicalUuid(
    raw.storage_owner_service_account_id,
    "handoff storage owner service-account ID",
    "unsafe_handoff",
  );
  for (const field of [
    "nyxid_client_secret",
    "storage_client_secret",
    "access_token",
  ]) {
    boundedString(raw[field], `handoff ${field}`, 64 * 1024, "unsafe_handoff");
  }
  for (const field of ["context_bucket", "trigger_workspace_bucket"]) {
    if (!BUCKET.test(raw[field]))
      fail("unsafe_handoff", `handoff ${field} is invalid`);
  }
  if (!SERVICE_SLUG.test(raw.public_github_service_slug)) {
    fail("unsafe_handoff", "handoff observer slug is invalid");
  }
  integer(
    raw.access_token_expires_at,
    "handoff access_token_expires_at",
    "unsafe_handoff",
  );
  if (raw.access_token_expires_at - at < MIN_HANDOFF_LIFETIME_SECONDS) {
    fail(
      "unsafe_handoff",
      "handoff access token is not fresh enough for dispatch",
    );
  }
  const bindings = [
    [raw.nyxid_client_id, state.oidc_app.id, "OIDC client ID"],
    [
      raw.nyxid_client_secret,
      state.oidc_app.client_secret,
      "OIDC client secret",
    ],
    [
      raw.storage_client_id,
      state.storage_service_account.client_id,
      "storage client ID",
    ],
    [
      raw.storage_client_secret,
      state.storage_service_account.client_secret,
      "storage client secret",
    ],
    [
      raw.storage_owner_service_account_id,
      state.storage_service_account.id,
      "storage owner service-account ID",
    ],
    [raw.context_bucket, state.buckets.context_bucket, "context bucket"],
    [
      raw.trigger_workspace_bucket,
      state.buckets.trigger_workspace_bucket,
      "Trigger workspace bucket",
    ],
    [raw.public_github_service_slug, state.observer.slug, "observer slug"],
    [raw.subject_id, state.caller.subject_id, "caller subject"],
  ];
  for (const [actual, expected, label] of bindings) {
    if (!secretEqual(actual, expected)) {
      fail(
        "handoff_state_mismatch",
        `${label} is not bound to bootstrap state`,
      );
    }
  }
  if (state.caller.service_id === provisioningServiceId) {
    fail(
      "runtime_authority_broadened",
      "the staging-administration UserService cannot be accepted by Trigger runtime",
    );
  }
  return raw;
}

function decodeJwtPart(part, path) {
  if (
    typeof part !== "string" ||
    part.length === 0 ||
    part.length > 64 * 1024
  ) {
    fail("caller_token_invalid", `${path} is not bounded`);
  }
  let decoded;
  try {
    decoded = Buffer.from(part, "base64url");
  } catch {
    fail("caller_token_invalid", `${path} is not base64url`);
  }
  try {
    return JSON.parse(decoded.toString("utf8"));
  } catch {
    fail("caller_token_invalid", `${path} is not JSON`);
  }
}

function parseJwt(token) {
  boundedString(
    token,
    "caller access token",
    MAX_TOKEN_BYTES,
    "caller_token_invalid",
  );
  const parts = token.split(".");
  if (parts.length !== 3 || parts.some((part) => part.length === 0)) {
    fail("caller_token_invalid", "caller access token is not a compact JWT");
  }
  const header = decodeJwtPart(parts[0], "caller token header");
  const claims = decodeJwtPart(parts[1], "caller token claims");
  exactKeys(
    header,
    ["alg", "kid", "typ"],
    "caller token header",
    "caller_token_invalid",
  );
  if (
    header.alg !== "RS256" ||
    header.typ !== "JWT" ||
    typeof header.kid !== "string" ||
    header.kid.length === 0 ||
    header.kid.length > 256
  ) {
    fail("caller_token_invalid", "caller token header authority is invalid");
  }
  return { parts, header, claims };
}

function validateAccessClaims(claims, handoff, state, at = nowSeconds()) {
  const allowed = [
    "sub",
    "iss",
    "aud",
    "exp",
    "iat",
    "jti",
    "scope",
    "token_type",
    "client_id",
    "roles",
    "groups",
    "permissions",
    "sid",
    "act",
    "delegated",
    "sa",
    "cnf",
    "relay",
    "relay_api_key_id",
    "relay_api_key_name",
    "relay_allowed_service_ids",
    "relay_allowed_node_ids",
    "relay_allow_all_services",
    "relay_allow_all_nodes",
    "resources",
    "allowed_service_ids",
    "allow_all_services",
    "allowed_node_ids",
    "allow_all_nodes",
    "assistant_forward",
  ];
  closedKeys(
    claims,
    allowed,
    [
      "sub",
      "iss",
      "aud",
      "exp",
      "iat",
      "jti",
      "scope",
      "token_type",
      "client_id",
      "resources",
      "allowed_service_ids",
      "allow_all_services",
      "allowed_node_ids",
      "allow_all_nodes",
    ],
    "caller access claims",
    "caller_token_invalid",
  );
  for (const field of ["sub", "iss", "aud", "client_id", "token_type"]) {
    boundedString(
      claims[field],
      `caller claims.${field}`,
      2048,
      "caller_token_invalid",
    );
  }
  canonicalUuid(claims.sub, "caller claims.sub", "caller_token_invalid");
  canonicalUuid(
    claims.client_id,
    "caller claims.client_id",
    "caller_token_invalid",
  );
  for (const field of ["iat", "exp"]) {
    integer(claims[field], `caller claims.${field}`, "caller_token_invalid");
  }
  boundedString(claims.jti, "caller claims.jti", 2048, "caller_token_invalid");
  boundedString(
    claims.scope,
    "caller claims.scope",
    2048,
    "caller_token_invalid",
  );
  if (/\s{2,}|[^\x20-\x7e]/u.test(claims.scope)) {
    fail("caller_token_invalid", "caller access scope encoding is invalid");
  }
  exactSet(
    claims.scope.split(" "),
    ["openid", "profile", "email", "roles", "proxy", "offline_access"],
    "caller claims.scope",
    "caller_token_invalid",
  );
  if (
    claims.sub !== state.caller.subject_id ||
    claims.sub !== handoff.subject_id ||
    claims.client_id !== state.caller.client_id ||
    claims.client_id !== handoff.nyxid_client_id ||
    claims.iss !== NYXID_BASE_URL ||
    claims.aud !== NYXID_BASE_URL ||
    claims.token_type !== "access" ||
    claims.exp !== handoff.access_token_expires_at ||
    claims.iat > at + 30 ||
    claims.exp - at < MIN_HANDOFF_LIFETIME_SECONDS ||
    claims.exp - claims.iat < MIN_HANDOFF_LIFETIME_SECONDS ||
    claims.exp - claims.iat > MAX_HANDOFF_LIFETIME_SECONDS ||
    !Array.isArray(claims.resources) ||
    claims.resources.length !== 0 ||
    claims.allow_all_services !== false ||
    !Array.isArray(claims.allowed_node_ids) ||
    claims.allowed_node_ids.length !== 0 ||
    claims.allow_all_nodes !== true ||
    claims.delegated === true ||
    claims.sa === true ||
    claims.relay === true
  ) {
    fail("caller_token_invalid", "caller token identity or lifetime drifted");
  }
  exactSet(
    claims.allowed_service_ids,
    [state.caller.service_id],
    "caller claims.allowed_service_ids",
    "caller_service_authority_denied",
  );
  return claims;
}

async function verifyHandoffToken(
  transport,
  handoff,
  state,
  at = nowSeconds(),
) {
  const parsed = parseJwt(handoff.access_token);
  const discovery = await transport.nyxid(
    "GET",
    "/.well-known/openid-configuration",
    {
      token: undefined,
    },
  );
  object(discovery.json, "OIDC discovery");
  exactString(discovery.json.issuer, NYXID_BASE_URL, "OIDC issuer");
  exactString(
    discovery.json.jwks_uri,
    `${NYXID_BASE_URL}/.well-known/jwks.json`,
    "OIDC JWKS URI",
  );
  const jwks = await transport.nyxid("GET", "/.well-known/jwks.json", {
    token: undefined,
  });
  object(jwks.json, "JWKS");
  if (!Array.isArray(jwks.json.keys))
    fail("caller_token_invalid", "JWKS keys are absent");
  const matches = jwks.json.keys.filter(
    (key) => key?.kid === parsed.header.kid,
  );
  if (matches.length !== 1) {
    fail("caller_token_invalid", "caller token signing key is ambiguous");
  }
  let verified = false;
  try {
    const key = createPublicKey({ key: matches[0], format: "jwk" });
    verified = verifySignature(
      "RSA-SHA256",
      Buffer.from(`${parsed.parts[0]}.${parsed.parts[1]}`),
      key,
      Buffer.from(parsed.parts[2], "base64url"),
    );
  } catch {
    fail(
      "caller_token_invalid",
      "caller token signature could not be verified",
    );
  }
  if (!verified)
    fail("caller_token_invalid", "caller token signature is invalid");
  return validateAccessClaims(parsed.claims, handoff, state, at);
}

async function boundedResponse(response) {
  const declared = response.headers?.get?.("content-length");
  if (declared !== null && declared !== undefined) {
    const length = Number(declared);
    if (
      !Number.isSafeInteger(length) ||
      length < 0 ||
      length > MAX_JSON_BYTES
    ) {
      fail(
        "remote_response_oversized",
        "remote response declared an unsafe size",
      );
    }
  }
  if (response.status === 204 || response.status === 205) return null;
  const reader = response.body?.getReader?.();
  let buffer;
  if (reader === undefined) {
    const raw = Buffer.from(await response.arrayBuffer());
    if (raw.length > MAX_JSON_BYTES) {
      fail(
        "remote_response_oversized",
        "remote response exceeded the JSON limit",
      );
    }
    buffer = raw;
  } else {
    const chunks = [];
    let total = 0;
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      total += value.byteLength;
      if (total > MAX_JSON_BYTES) {
        await reader.cancel();
        fail(
          "remote_response_oversized",
          "remote response exceeded the JSON limit",
        );
      }
      chunks.push(Buffer.from(value));
    }
    buffer = Buffer.concat(chunks, total);
  }
  if (buffer.length === 0) return null;
  try {
    return JSON.parse(buffer.toString("utf8"));
  } catch {
    fail("remote_response_invalid", "remote response was not valid JSON");
  }
}

class Transport {
  constructor({
    token,
    provisioningUserServiceId,
    fetchImpl = globalThis.fetch,
    timeoutMs = HTTP_TIMEOUT_MS,
  }) {
    this.token = token;
    this.provisioningUserServiceId = provisioningUserServiceId;
    this.fetchImpl = fetchImpl;
    this.timeoutMs = timeoutMs;
  }

  async request(method, url, { token, json, statuses = [200] } = {}) {
    const parsed = new URL(url);
    if (parsed.origin !== NYXID_BASE_URL) {
      fail(
        "endpoint_not_pinned",
        "every request must terminate at the pinned NyxID origin",
      );
    }
    if (
      json !== undefined &&
      Buffer.byteLength(JSON.stringify(json)) > MAX_JSON_BYTES
    ) {
      fail("request_oversized", "request JSON exceeded the bound");
    }
    const headers = {
      accept: "application/vnd.github+json, application/json",
      "user-agent": "cma-trigger-smoke-environment-operator/1",
    };
    const bearer = token === undefined ? this.token : token;
    if (bearer !== undefined) headers.authorization = `Bearer ${bearer}`;
    let body;
    if (json !== undefined) {
      headers["content-type"] = "application/json";
      body = JSON.stringify(json);
    }
    let response;
    try {
      response = await this.fetchImpl(url, {
        method,
        headers,
        body,
        redirect: "error",
        signal: AbortSignal.timeout(this.timeoutMs),
      });
    } catch {
      fail(
        "remote_transport_ambiguous",
        `${method} request produced no response`,
      );
    }
    const responseJson = await boundedResponse(response);
    if (!statuses.includes(response.status)) {
      fail(
        "remote_status_error",
        `${method} request returned HTTP ${response.status}`,
        { status: response.status, response: responseJson },
      );
    }
    return {
      status: response.status,
      json: responseJson,
      headers: response.headers,
    };
  }

  nyxid(method, path, options = {}) {
    if (
      typeof path !== "string" ||
      !path.startsWith("/") ||
      path.includes("?") ||
      path.includes("#") ||
      path.includes("..")
    ) {
      fail("endpoint_not_pinned", "NyxID request path is invalid");
    }
    return this.request(method, `${NYXID_BASE_URL}${path}`, options);
  }

  github(
    method,
    path,
    {
      query = {},
      json,
      statuses,
      token,
      userServiceId = this.provisioningUserServiceId,
    } = {},
  ) {
    canonicalUuid(
      userServiceId,
      "GitHub routing UserService ID",
      "endpoint_not_pinned",
    );
    if (
      typeof path !== "string" ||
      !path.startsWith("/") ||
      path.includes("?") ||
      path.includes("#") ||
      path.includes("\\") ||
      path.split("/").includes("..")
    ) {
      fail("endpoint_not_pinned", "GitHub request path is invalid");
    }
    const search = new URLSearchParams();
    for (const [name, value] of Object.entries(query)) {
      if (!/^[a-z_]+$/u.test(name) || value === undefined || value === null) {
        fail("endpoint_not_pinned", "GitHub query is invalid");
      }
      const string = String(value);
      if (string.length === 0 || string.length > 1024) {
        fail("endpoint_not_pinned", "GitHub query value is invalid");
      }
      search.set(name, string);
    }
    search.set("_nyxid_via", userServiceId);
    const url =
      `${NYXID_BASE_URL}/api/v1/proxy/${GITHUB_CATALOG_SERVICE_ID}` +
      `${path}?${search.toString()}`;
    return this.request(method, url, { token, json, statuses });
  }
}

function validateNyxidMe(raw, expectedSubject = undefined) {
  object(raw, "NyxID user profile");
  canonicalUuid(raw.id, "NyxID user profile id");
  if (raw.is_active !== true)
    fail("authority_drift", "NyxID subject is inactive");
  if (expectedSubject !== undefined && raw.id !== expectedSubject) {
    fail(
      "authority_drift",
      "NyxID token subject differs from bootstrap custody",
    );
  }
  return { subject_id: raw.id, is_active: true };
}

function validateUserServiceList(raw, serviceId) {
  exactKeys(raw, ["services"], "NyxID UserService list");
  if (!Array.isArray(raw.services) || raw.services.length > 1000) {
    fail("contract_drift", "NyxID UserService list is invalid or unbounded");
  }
  const matches = raw.services.filter((service) => service?.id === serviceId);
  if (matches.length !== 1) {
    fail("authority_drift", "exactly one selected UserService is required");
  }
  const service = object(matches[0], "selected UserService");
  canonicalUuid(service.id, "selected UserService id");
  canonicalUuid(service.catalog_service_id, "selected UserService catalog id");
  if (
    service.catalog_service_id !== GITHUB_CATALOG_SERVICE_ID ||
    service.is_active !== true ||
    !Number.isSafeInteger(service.state_version) ||
    service.state_version <= 0 ||
    service.credential_source?.type !== "personal" ||
    typeof service.slug !== "string" ||
    !SERVICE_SLUG.test(service.slug)
  ) {
    fail(
      "authority_drift",
      "selected UserService is not one active personal GitHub route",
    );
  }
  const expectedResource = `${NYXID_BASE_URL}/api/v1/proxy/s/${service.slug}`;
  if (service.resource_uri !== expectedResource) {
    fail("authority_drift", "selected UserService resource URI drifted");
  }
  canonicalUuid(service.api_key_id, "selected UserService backing key id");
  return {
    id: service.id,
    slug: service.slug,
    catalog_service_id: service.catalog_service_id,
    resource_uri: service.resource_uri,
    state_version: service.state_version,
    api_key_id: service.api_key_id,
    credential_source: "personal",
    is_active: true,
  };
}

function validateAuthorization(raw, service, expectedScopes) {
  object(raw, "UserService authorization evidence");
  if (
    raw.id !== service.id ||
    raw.api_key_id !== service.api_key_id ||
    raw.is_active !== true ||
    raw.status !== "active" ||
    raw.connection_status !== "active" ||
    raw.state_version !== service.state_version
  ) {
    fail("authority_drift", "UserService authorization evidence drifted");
  }
  exactSet(raw.granted_scopes, expectedScopes, "GitHub granted scopes");
  parseTimestamp(raw.last_authorized_at, "GitHub last_authorized_at");
  return {
    state: "active",
    state_version: raw.state_version,
    granted_scopes: [...expectedScopes],
    last_authorized_at: raw.last_authorized_at,
  };
}

function validateGithubAccount(raw) {
  object(raw, "GitHub account");
  if (
    raw.login !== TARGET.account.login ||
    raw.id !== TARGET.account.id ||
    raw.node_id !== TARGET.account.node_id ||
    raw.type !== "User" ||
    raw.site_admin !== false
  ) {
    fail(
      "authority_drift",
      "GitHub account provenance differs from chronoai-shining",
    );
  }
  return { ...TARGET.account, type: "User", site_admin: false };
}

async function inspectAuthority(
  transport,
  { token, serviceId, expectedSubject, expectedScopes },
) {
  const me = validateNyxidMe(
    (await transport.nyxid("GET", "/api/v1/users/me", { token })).json,
    expectedSubject,
  );
  const services = validateUserServiceList(
    (await transport.nyxid("GET", "/api/v1/user-services", { token })).json,
    serviceId,
  );
  const authorization = validateAuthorization(
    (
      await transport.nyxid(
        "GET",
        `/api/v1/keys/${encodeURIComponent(serviceId)}/authorization`,
        { token },
      )
    ).json,
    services,
    expectedScopes,
  );
  const account = validateGithubAccount(
    (
      await transport.github("GET", "/user", {
        token,
        userServiceId: serviceId,
      })
    ).json,
  );
  return { nyxid: me, user_service: services, authorization, github: account };
}

function validateSourceRef(raw) {
  object(raw, "NyxID source ref");
  if (
    raw.ref !== "refs/heads/main" ||
    raw.object?.type !== "commit" ||
    raw.object?.sha !== NYXID_SOURCE_REVISION
  ) {
    fail(
      "nyxid_source_drift",
      "latest remote NyxID main differs from the reviewed source",
    );
  }
  return {
    repository: "ChronoAIProject/NyxID",
    ref: raw.ref,
    sha: raw.object.sha,
  };
}

function validateRepository(raw) {
  object(raw, "GitHub repository");
  if (
    raw.id !== TARGET.repository.id ||
    raw.node_id !== TARGET.repository.node_id ||
    raw.full_name !== `${TARGET.repository.owner}/${TARGET.repository.name}` ||
    raw.name !== TARGET.repository.name ||
    raw.owner?.login !== TARGET.repository.owner ||
    raw.default_branch !== TARGET.repository.default_branch ||
    raw.archived !== false ||
    raw.disabled !== false
  ) {
    fail("target_drift", "GitHub repository identity or state drifted");
  }
  return {
    ...TARGET.repository,
    full_name: raw.full_name,
    archived: false,
    disabled: false,
  };
}

function validateEnvironment(raw) {
  object(raw, "GitHub environment");
  if (
    raw.id !== TARGET.environment.id ||
    raw.node_id !== TARGET.environment.node_id ||
    raw.name !== ENVIRONMENT ||
    raw.can_admins_bypass !== false ||
    raw.deployment_branch_policy?.protected_branches !== true ||
    raw.deployment_branch_policy?.custom_branch_policies !== false ||
    !Array.isArray(raw.protection_rules)
  ) {
    fail(
      "environment_unprotected",
      "GitHub smoke environment protection drifted",
    );
  }
  const reviewerRules = raw.protection_rules.filter(
    (rule) => rule?.type === "required_reviewers",
  );
  const branchRules = raw.protection_rules.filter(
    (rule) => rule?.type === "branch_policy",
  );
  if (
    reviewerRules.length !== 1 ||
    branchRules.length !== 1 ||
    reviewerRules[0].prevent_self_review !== true ||
    !Array.isArray(reviewerRules[0].reviewers) ||
    reviewerRules[0].reviewers.length !== 1
  ) {
    fail("environment_unprotected", "environment protection rule set drifted");
  }
  const reviewer = reviewerRules[0].reviewers[0];
  if (
    reviewer.type !== TARGET.environment.reviewer.type ||
    reviewer.reviewer?.id !== TARGET.environment.reviewer.id ||
    reviewer.reviewer?.node_id !== TARGET.environment.reviewer.node_id ||
    reviewer.reviewer?.slug !== TARGET.environment.reviewer.slug
  ) {
    fail("environment_unprotected", "independent reviewer identity drifted");
  }
  return {
    id: raw.id,
    node_id: raw.node_id,
    name: raw.name,
    can_admins_bypass: false,
    prevent_self_review: true,
    reviewer: { ...TARGET.environment.reviewer },
    deployment_branch_policy: {
      protected_branches: true,
      custom_branch_policies: false,
    },
  };
}

function validateBranchProtection(raw) {
  object(raw, "develop protection");
  const checks = raw.required_status_checks?.checks;
  if (
    raw.required_status_checks?.strict !== true ||
    !Array.isArray(checks) ||
    raw.enforce_admins?.enabled !== true ||
    raw.required_conversation_resolution?.enabled !== true ||
    raw.allow_force_pushes?.enabled !== false ||
    raw.allow_deletions?.enabled !== false ||
    raw.lock_branch?.enabled !== false
  ) {
    fail("branch_unprotected", "develop branch protection drifted");
  }
  const contexts = checks.map((check) => check?.context);
  exactSet(
    contexts,
    REQUIRED_DEVELOP_CONTEXTS,
    "develop required contexts",
    "branch_unprotected",
  );
  return {
    branch: "develop",
    strict_status_checks: true,
    enforce_admins: true,
    required_conversation_resolution: true,
    allow_force_pushes: false,
    allow_deletions: false,
    required_contexts: [...contexts].sort(),
  };
}

function validateWorkflow(raw) {
  object(raw, "provider-smoke workflow");
  if (
    raw.id !== TARGET.workflow.id ||
    raw.node_id !== TARGET.workflow.node_id ||
    raw.name !== TARGET.workflow.name ||
    raw.path !== TARGET.workflow.path ||
    raw.state !== "active"
  ) {
    fail("workflow_drift", "provider-smoke workflow identity or state drifted");
  }
  return { ...TARGET.workflow, state: "active" };
}

function validateVariables(raw) {
  exactKeys(
    raw,
    ["total_count", "variables"],
    "environment variable inventory",
  );
  if (
    !Array.isArray(raw.variables) ||
    raw.variables.length !== raw.total_count ||
    raw.variables.length > 100
  ) {
    fail("inventory_drift", "environment variable inventory is incomplete");
  }
  const values = {};
  for (const item of raw.variables) {
    object(item, "environment variable");
    boundedString(item.name, "environment variable name", 128);
    if (
      !MANAGED_VARIABLE_NAMES.includes(item.name) ||
      Object.hasOwn(values, item.name)
    ) {
      fail(
        "inventory_drift",
        "environment variable inventory contains an unmanaged name",
      );
    }
    if (typeof item.value !== "string" || item.value.length > 16 * 1024) {
      fail("inventory_drift", "environment variable value is invalid");
    }
    values[item.name] = item.value;
  }
  return values;
}

function validateSecrets(raw) {
  exactKeys(raw, ["total_count", "secrets"], "environment secret inventory");
  if (
    !Array.isArray(raw.secrets) ||
    raw.secrets.length !== raw.total_count ||
    raw.secrets.length > 100
  ) {
    fail("inventory_drift", "environment secret inventory is incomplete");
  }
  const values = {};
  for (const item of raw.secrets) {
    object(item, "environment secret");
    boundedString(item.name, "environment secret name", 128);
    if (!isManagedSecretName(item.name) || Object.hasOwn(values, item.name)) {
      fail(
        "inventory_drift",
        "environment secret inventory contains an unmanaged name",
      );
    }
    parseTimestamp(item.created_at, `secret ${item.name} created_at`);
    parseTimestamp(item.updated_at, `secret ${item.name} updated_at`);
    values[item.name] = {
      created_at: item.created_at,
      updated_at: item.updated_at,
    };
  }
  return values;
}

function validateEnvironmentPublicKey(raw) {
  exactKeys(raw, ["key_id", "key"], "environment public key");
  boundedString(raw.key_id, "environment public key id", 256);
  boundedString(raw.key, "environment public key", 128);
  let decoded;
  try {
    decoded = Buffer.from(raw.key, "base64");
  } catch {
    fail("environment_key_invalid", "environment public key is not base64");
  }
  if (decoded.length !== 32 || decoded.toString("base64") !== raw.key) {
    fail(
      "environment_key_invalid",
      "environment public key is not canonical Curve25519",
    );
  }
  return { key_id: raw.key_id, key: raw.key };
}

function validateDevelopRef(raw) {
  object(raw, "develop ref");
  if (raw.ref !== "refs/heads/develop" || raw.object?.type !== "commit") {
    fail("branch_drift", "develop ref identity drifted");
  }
  if (typeof raw.object.sha !== "string" || !SHA1.test(raw.object.sha)) {
    fail("branch_drift", "develop ref does not resolve to one full commit");
  }
  return { ref: raw.ref, sha: raw.object.sha };
}

function validateRunList(raw, status) {
  exactKeys(raw, ["total_count", "workflow_runs"], `run list ${status}`);
  if (
    !Number.isSafeInteger(raw.total_count) ||
    !Array.isArray(raw.workflow_runs) ||
    raw.workflow_runs.length !== raw.total_count ||
    raw.workflow_runs.length > 100
  ) {
    fail(
      "writer_inventory_incomplete",
      `active run list ${status} is incomplete`,
    );
  }
  return raw.workflow_runs.map((run) => {
    object(run, `run ${status}`);
    positiveInteger(run.id, "workflow run id");
    boundedString(run.path, "workflow run path", 512);
    if (run.status !== status)
      fail("contract_drift", "workflow run status drifted");
    return {
      id: run.id,
      path: run.path,
      name: typeof run.name === "string" ? run.name : null,
      status: run.status,
      event: typeof run.event === "string" ? run.event : null,
      head_branch: typeof run.head_branch === "string" ? run.head_branch : null,
      head_sha: typeof run.head_sha === "string" ? run.head_sha : null,
      html_url: typeof run.html_url === "string" ? run.html_url : null,
    };
  });
}

async function inspectConflicts(transport) {
  const relevant = new Set([WORKFLOW_PATH, ".github/workflows/publish.yml"]);
  const conflicts = [];
  for (const status of ACTIVE_RUN_STATUSES) {
    const response = await transport.github(
      "GET",
      `/repos/${TARGET.repository.owner}/${TARGET.repository.name}/actions/runs`,
      { query: { per_page: 100, status } },
    );
    for (const run of validateRunList(response.json, status)) {
      if (relevant.has(run.path)) conflicts.push(run);
    }
  }
  conflicts.sort((left, right) => left.id - right.id);
  return conflicts;
}

function requireNoConflicts(snapshot) {
  if (snapshot.conflicts.length > 0) {
    fail("writer_conflict", "a smoke or package writer run is active", {
      conflicts: snapshot.conflicts,
    });
  }
}

function requireApprovedHead(snapshot, expectedSha) {
  if (snapshot.branch.head.sha !== expectedSha) {
    fail(
      "approved_head_changed",
      "develop moved after the reviewed staging preflight",
      { expected_sha: expectedSha, observed_sha: snapshot.branch.head.sha },
    );
  }
}

async function collectLiveSnapshot({ config, state, transport }) {
  const expectedSubject = state.caller?.subject_id;
  const authority = await inspectAuthority(transport, {
    token: transport.token,
    serviceId: config.provisioning_user_service_id,
    expectedSubject,
    expectedScopes: PROVISIONING_GITHUB_SCOPES,
  });
  const githubPrefix = `/repos/${TARGET.repository.owner}/${TARGET.repository.name}`;
  const [
    source,
    repository,
    environment,
    variables,
    secrets,
    publicKey,
    branchProtection,
    developRef,
    workflow,
  ] = await Promise.all([
    transport.github("GET", "/repos/ChronoAIProject/NyxID/git/ref/heads/main"),
    transport.github("GET", `/repositories/${TARGET.repository.id}`),
    transport.github("GET", `${githubPrefix}/environments/${ENVIRONMENT}`),
    transport.github(
      "GET",
      `${githubPrefix}/environments/${ENVIRONMENT}/variables`,
      { query: { per_page: 100 } },
    ),
    transport.github(
      "GET",
      `${githubPrefix}/environments/${ENVIRONMENT}/secrets`,
      { query: { per_page: 100 } },
    ),
    transport.github(
      "GET",
      `${githubPrefix}/environments/${ENVIRONMENT}/secrets/public-key`,
    ),
    transport.github("GET", `${githubPrefix}/branches/develop/protection`),
    transport.github("GET", `${githubPrefix}/git/ref/heads/develop`),
    transport.github(
      "GET",
      `${githubPrefix}/actions/workflows/${WORKFLOW_FILE}`,
    ),
  ]);
  const conflicts = await inspectConflicts(transport);
  return {
    source: validateSourceRef(source.json),
    target: {
      repository: validateRepository(repository.json),
      environment: validateEnvironment(environment.json),
      workflow: validateWorkflow(workflow.json),
    },
    authority,
    branch: {
      protection: validateBranchProtection(branchProtection.json),
      head: validateDevelopRef(developRef.json),
    },
    inventory: {
      variables: validateVariables(variables.json),
      secrets: validateSecrets(secrets.json),
      public_key: validateEnvironmentPublicKey(publicKey.json),
    },
    conflicts,
  };
}

function desiredVariables(config, kubeconfigDigest = null) {
  const result = { ...config.static_variables };
  if (kubeconfigDigest !== null) {
    if (!SHA256.test(kubeconfigDigest)) {
      fail("internal_error", "derived kubeconfig digest is invalid");
    }
    result[KUBECONFIG_DIGEST_VARIABLE] = kubeconfigDigest;
  }
  return result;
}

function setupActions(snapshot, config) {
  const actions = [];
  for (const [name, value] of Object.entries(config.static_variables).sort()) {
    if (!Object.hasOwn(snapshot.inventory.variables, name)) {
      actions.push({ action: "create_variable", name, value });
    } else if (snapshot.inventory.variables[name] !== value) {
      actions.push({ action: "update_variable", name, value });
    }
  }
  if (
    !Object.hasOwn(snapshot.inventory.variables, KUBECONFIG_DIGEST_VARIABLE)
  ) {
    actions.push({
      action: "derive_and_create_variable",
      name: KUBECONFIG_DIGEST_VARIABLE,
    });
  }
  for (const name of [KIND_CA_MATERIAL_NAME, KUBECONFIG_SECRET]) {
    actions.push({
      action: Object.hasOwn(snapshot.inventory.secrets, name)
        ? "rewrite_stable_secret_from_custody"
        : "create_stable_secret_from_custody",
      name,
    });
  }
  return actions;
}

function snapshotForApproval(snapshot, config) {
  return {
    contract: {
      schema_version: 1,
      kind: "cma-trigger-smoke-environment-plan",
      nyxid_base_url: NYXID_BASE_URL,
      nyxid_api_version: NYXID_API_VERSION,
      nyxid_source_revision: NYXID_SOURCE_REVISION,
      github_catalog_service_id: GITHUB_CATALOG_SERVICE_ID,
      provisioning_user_service_id: config.provisioning_user_service_id,
    },
    source: snapshot.source,
    target: snapshot.target,
    authority: snapshot.authority,
    branch: snapshot.branch,
    inventory: {
      variables: snapshot.inventory.variables,
      secrets: snapshot.inventory.secrets,
      environment_public_key: {
        key_id: snapshot.inventory.public_key.key_id,
        key_sha256: sha256(
          Buffer.from(snapshot.inventory.public_key.key, "base64"),
        ),
      },
    },
    conflicts: snapshot.conflicts,
    intended_setup_actions: setupActions(snapshot, config),
    intended_ephemeral_secret_prefix: EPHEMERAL_HANDOFF_SECRET_PREFIX,
    intended_runtime_service_secret: RUNTIME_SERVICE_SECRET,
    runtime_service_id: configRuntimeServiceId(config, snapshot),
    dispatch: { ...config.dispatch, workflow_id: TARGET.workflow.id },
  };
}

function configRuntimeServiceId(_config, snapshot) {
  // The provisioning snapshot intentionally cannot infer or auto-select the
  // Trigger runtime UserService. The caller-owned bootstrap state supplies it.
  return snapshot.runtime_service_id ?? null;
}

function buildPlan(snapshot, config, state, generatedAt = new Date()) {
  const observed = snapshotForApproval(
    { ...snapshot, runtime_service_id: state.caller?.service_id ?? null },
    config,
  );
  observed.runtime_service_id = state.caller?.service_id ?? null;
  const generated = generatedAt.toISOString();
  const expires = new Date(
    generatedAt.getTime() + PREFLIGHT_LIFETIME_SECONDS * 1000,
  ).toISOString();
  return {
    schema_version: 1,
    kind: "cma-trigger-smoke-environment-plan",
    generated_at: generated,
    expires_at: expires,
    approval_fingerprint: sha256(
      canonicalJson({ generated_at: generated, expires_at: expires, observed }),
    ),
    observed,
  };
}

function validatePlan(raw) {
  exactKeys(
    raw,
    [
      "schema_version",
      "kind",
      "generated_at",
      "expires_at",
      "approval_fingerprint",
      "observed",
    ],
    "approved plan",
    "invalid_plan",
  );
  if (
    raw.schema_version !== 1 ||
    raw.kind !== "cma-trigger-smoke-environment-plan" ||
    typeof raw.approval_fingerprint !== "string" ||
    !SHA256.test(raw.approval_fingerprint)
  ) {
    fail("invalid_plan", "approved plan contract identity is invalid");
  }
  const generated = parseTimestamp(
    raw.generated_at,
    "plan generated_at",
    "invalid_plan",
  );
  const expires = parseTimestamp(
    raw.expires_at,
    "plan expires_at",
    "invalid_plan",
  );
  if (expires - generated !== PREFLIGHT_LIFETIME_SECONDS * 1000) {
    fail("invalid_plan", "approved plan lifetime drifted");
  }
  if (
    raw.approval_fingerprint !==
    sha256(
      canonicalJson({
        generated_at: raw.generated_at,
        expires_at: raw.expires_at,
        observed: raw.observed,
      }),
    )
  ) {
    fail("invalid_plan", "approved plan fingerprint is invalid");
  }
  return raw;
}

function requireCurrentApprovedPlan(approved, current, at = Date.now()) {
  validatePlan(approved);
  validatePlan(current);
  if (Date.parse(approved.generated_at) > at) {
    fail("plan_not_yet_valid", "approved staging preflight is future-dated");
  }
  if (Date.parse(approved.expires_at) <= at) {
    fail("plan_expired", "approved staging preflight has expired");
  }
  if (
    sha256(canonicalJson(approved.observed)) !==
    sha256(canonicalJson(current.observed))
  ) {
    fail(
      "preflight_changed",
      "live staging preflight differs from the approved plan",
    );
  }
  return current;
}

async function planEnvironment({ config, state, transport, generatedAt }) {
  const snapshot = await collectLiveSnapshot({ config, state, transport });
  return { plan: buildPlan(snapshot, config, state, generatedAt), snapshot };
}

function sanitizedCommandEnvironment(extra = {}) {
  const allowed = [
    "HOME",
    "PATH",
    "CARGO_HOME",
    "RUSTUP_HOME",
    "SSL_CERT_FILE",
    "SSL_CERT_DIR",
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "NO_PROXY",
    "http_proxy",
    "https_proxy",
    "no_proxy",
  ];
  const environment = {};
  for (const name of allowed) {
    if (process.env[name] !== undefined) environment[name] = process.env[name];
  }
  return { ...environment, ...extra };
}

async function runCommand(
  command,
  args,
  { cwd, env = sanitizedCommandEnvironment(), input, timeoutMs = 120_000 } = {},
) {
  return new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd,
      env,
      shell: false,
      stdio: ["pipe", "pipe", "pipe"],
    });
    const stdout = [];
    const stderr = [];
    let stdoutBytes = 0;
    let stderrBytes = 0;
    let killedForSize = false;
    const timer = setTimeout(() => child.kill("SIGKILL"), timeoutMs);
    child.stdout.on("data", (chunk) => {
      stdoutBytes += chunk.length;
      if (stdoutBytes > MAX_JSON_BYTES) {
        killedForSize = true;
        child.kill("SIGKILL");
      } else stdout.push(chunk);
    });
    child.stderr.on("data", (chunk) => {
      stderrBytes += chunk.length;
      if (stderrBytes > MAX_JSON_BYTES) {
        killedForSize = true;
        child.kill("SIGKILL");
      } else stderr.push(chunk);
    });
    child.on("error", () => {
      clearTimeout(timer);
      rejectPromise(
        new OperatorFailure("command_failed", `${command} could not start`),
      );
    });
    child.on("close", (code, signal) => {
      clearTimeout(timer);
      if (killedForSize) {
        rejectPromise(
          new OperatorFailure(
            "command_failed",
            `${command} output exceeded its bound`,
          ),
        );
      } else if (code !== 0) {
        rejectPromise(
          new OperatorFailure(
            "command_failed",
            `${command} failed (${signal ?? `exit ${code}`})`,
          ),
        );
      } else {
        resolvePromise({
          stdout: Buffer.concat(stdout).toString("utf8"),
          stderr: Buffer.concat(stderr).toString("utf8"),
        });
      }
    });
    if (input !== undefined) child.stdin.end(input);
    else child.stdin.end();
  });
}

async function publicKeyDigest(kind, path, temporaryPath) {
  const result =
    kind === "certificate"
      ? await runCommand("openssl", ["x509", "-in", path, "-pubkey", "-noout"])
      : await runCommand("openssl", ["pkey", "-in", path, "-pubout"]);
  await writeFile(temporaryPath, result.stdout, { mode: 0o600, flag: "w" });
  const digest = sha256(await readFile(temporaryPath));
  await rm(temporaryPath, { force: true });
  return digest;
}

async function certificateText(path) {
  try {
    return (
      await runCommand("openssl", ["x509", "-in", path, "-noout", "-text"])
    ).stdout;
  } catch {
    fail("unsafe_authority", "Kind authority certificate could not be read");
  }
}

function hasCertificateExtensionValue(text, extension, expectedValue) {
  const lines = text.split(/\r?\n/u);
  const heading = `X509v3 ${extension}:`;
  const start = lines.findIndex((line) => line.trimStart().startsWith(heading));
  if (start === -1) return false;
  for (const line of lines.slice(start + 1)) {
    const value = line.trim();
    if (value.startsWith("X509v3 ")) return false;
    if (
      value
        .split(",")
        .map((item) => item.trim())
        .includes(expectedValue)
    ) {
      return true;
    }
  }
  return false;
}

async function verifyAuthorityDirectory(path) {
  await verifyPrivateDirectory(path, "Kind authority directory");
  const files = {
    ca_cert: join(path, "ca.crt"),
    ca_key: join(path, "ca.key"),
    observer_cert: join(path, "observer.crt"),
    observer_key: join(path, "observer.key"),
    kubeconfig: join(path, "observer-kubeconfig.yaml"),
    manifest: join(path, "authority.json"),
  };
  for (const [name, file] of Object.entries(files)) {
    await verifySecureFile(file, `Kind authority ${name}`);
  }
  const manifest = await readSecureJson(
    files.manifest,
    "Kind authority manifest",
  );
  exactKeys(
    manifest,
    [
      "schema_version",
      "environment_id",
      "cluster_server",
      "observer_cn",
      "namespace",
      "created_at",
      "certificate_not_after",
      "digests",
    ],
    "Kind authority manifest",
    "unsafe_authority",
  );
  if (
    manifest.schema_version !== 1 ||
    manifest.environment_id !== TARGET.environment.id ||
    manifest.cluster_server !== "https://127.0.0.1:6443" ||
    manifest.observer_cn !== "cma-trigger-smoke-observer" ||
    manifest.namespace !== "cma-trigger-sandboxes"
  ) {
    fail("unsafe_authority", "Kind authority identity drifted");
  }
  parseTimestamp(
    manifest.created_at,
    "authority created_at",
    "unsafe_authority",
  );
  parseTimestamp(
    manifest.certificate_not_after,
    "authority certificate_not_after",
    "unsafe_authority",
  );
  exactKeys(
    manifest.digests,
    ["ca_cert", "ca_key", "observer_cert", "observer_key", "kubeconfig"],
    "authority digests",
    "unsafe_authority",
  );
  for (const [name, file] of Object.entries(files)) {
    if (name === "manifest") continue;
    const digest = sha256(await readFile(file));
    if (manifest.digests[name] !== digest) {
      fail("unsafe_authority", `Kind authority ${name} digest drifted`);
    }
  }
  await runCommand("openssl", [
    "x509",
    "-in",
    files.ca_cert,
    "-checkend",
    "259200",
    "-noout",
  ]);
  await runCommand("openssl", [
    "x509",
    "-in",
    files.observer_cert,
    "-checkend",
    "259200",
    "-noout",
  ]);
  const caCertificateText = await certificateText(files.ca_cert);
  if (
    !hasCertificateExtensionValue(
      caCertificateText,
      "Basic Constraints",
      "CA:TRUE",
    )
  ) {
    fail("unsafe_authority", "Kind CA certificate must contain CA:TRUE");
  }
  const observerCertificateText = await certificateText(files.observer_cert);
  if (
    !hasCertificateExtensionValue(
      observerCertificateText,
      "Basic Constraints",
      "CA:FALSE",
    )
  ) {
    fail("unsafe_authority", "observer certificate must contain CA:FALSE");
  }
  if (
    !hasCertificateExtensionValue(
      observerCertificateText,
      "Extended Key Usage",
      "TLS Web Client Authentication",
    )
  ) {
    fail(
      "unsafe_authority",
      "observer certificate must contain the client-auth EKU",
    );
  }
  await runCommand("openssl", [
    "verify",
    "-purpose",
    "sslclient",
    "-CAfile",
    files.ca_cert,
    files.observer_cert,
  ]);
  const subject = (
    await runCommand("openssl", [
      "x509",
      "-in",
      files.observer_cert,
      "-subject",
      "-nameopt",
      "RFC2253",
      "-noout",
    ])
  ).stdout
    .trim()
    .replace(/^subject=\s*/u, "");
  if (subject !== "CN=cma-trigger-smoke-observer") {
    fail("unsafe_authority", "observer certificate subject drifted");
  }
  const scratch = join(path, `.public-key-${process.pid}`);
  const pairs = [
    [files.ca_cert, "certificate", files.ca_key, "key"],
    [files.observer_cert, "certificate", files.observer_key, "key"],
  ];
  for (const [cert, certKind, key, keyKind] of pairs) {
    const certDigest = await publicKeyDigest(certKind, cert, scratch);
    const keyDigest = await publicKeyDigest(keyKind, key, scratch);
    if (certDigest !== keyDigest) {
      fail(
        "unsafe_authority",
        "Kind authority certificate and private key differ",
      );
    }
  }
  return { path, files, manifest };
}

async function generateAuthority(custodyDirectory) {
  await verifyPrivateDirectory(custodyDirectory, "smoke custody directory");
  const finalPath = join(custodyDirectory, "kind-authority-v1");
  try {
    return await verifyAuthorityDirectory(finalPath);
  } catch (error) {
    if (error?.code !== "ENOENT") throw error;
  }
  const temporary = await mkdtemp(
    join(custodyDirectory, ".kind-authority-v1.tmp-"),
  );
  await chmod(temporary, 0o700);
  const caKey = join(temporary, "ca.key");
  const caCert = join(temporary, "ca.crt");
  const observerKey = join(temporary, "observer.key");
  const observerCsr = join(temporary, "observer.csr");
  const observerCert = join(temporary, "observer.crt");
  const extension = join(temporary, "observer.ext");
  const kubeconfig = join(temporary, "observer-kubeconfig.yaml");
  try {
    await runCommand("openssl", ["genrsa", "-out", caKey, "3072"]);
    await runCommand("openssl", [
      "req",
      "-x509",
      "-new",
      "-key",
      caKey,
      "-sha256",
      "-days",
      "30",
      "-subj",
      "/CN=CMA Trigger smoke Kind CA",
      "-addext",
      "basicConstraints=critical,CA:TRUE",
      "-addext",
      "keyUsage=critical,keyCertSign,cRLSign",
      "-out",
      caCert,
    ]);
    await runCommand("openssl", ["genrsa", "-out", observerKey, "3072"]);
    await runCommand("openssl", [
      "req",
      "-new",
      "-key",
      observerKey,
      "-subj",
      "/CN=cma-trigger-smoke-observer",
      "-out",
      observerCsr,
    ]);
    await writeFile(
      extension,
      [
        "basicConstraints=critical,CA:FALSE",
        "keyUsage=critical,digitalSignature,keyEncipherment",
        "extendedKeyUsage=clientAuth",
        "subjectKeyIdentifier=hash",
        "authorityKeyIdentifier=keyid,issuer",
        "",
      ].join("\n"),
      { mode: 0o600, flag: "wx" },
    );
    await runCommand("openssl", [
      "x509",
      "-req",
      "-in",
      observerCsr,
      "-CA",
      caCert,
      "-CAkey",
      caKey,
      "-CAcreateserial",
      "-days",
      "30",
      "-sha256",
      "-extfile",
      extension,
      "-out",
      observerCert,
    ]);
    await rm(observerCsr, { force: true });
    await rm(extension, { force: true });
    await rm(join(temporary, "ca.srl"), { force: true });
    const encode = async (path) => (await readFile(path)).toString("base64");
    const kubeconfigValue = [
      "apiVersion: v1",
      "kind: Config",
      "preferences: {}",
      "clusters:",
      "  - name: cma-trigger-smoke",
      "    cluster:",
      `      certificate-authority-data: ${await encode(caCert)}`,
      "      server: https://127.0.0.1:6443",
      "users:",
      "  - name: cma-trigger-smoke-observer",
      "    user:",
      `      client-certificate-data: ${await encode(observerCert)}`,
      `      client-key-data: ${await encode(observerKey)}`,
      "contexts:",
      "  - name: cma-trigger-smoke-observer",
      "    context:",
      "      cluster: cma-trigger-smoke",
      "      namespace: cma-trigger-sandboxes",
      "      user: cma-trigger-smoke-observer",
      "current-context: cma-trigger-smoke-observer",
      "",
    ].join("\n");
    await writeFile(kubeconfig, kubeconfigValue, { mode: 0o600, flag: "wx" });
    for (const path of [caKey, caCert, observerKey, observerCert, kubeconfig]) {
      await chmod(path, 0o600);
    }
    const notAfterRaw = (
      await runCommand("openssl", [
        "x509",
        "-in",
        observerCert,
        "-enddate",
        "-noout",
      ])
    ).stdout.trim();
    const notAfter = new Date(notAfterRaw.replace(/^notAfter=/u, ""));
    if (!Number.isFinite(notAfter.getTime())) {
      fail(
        "authority_generation_failed",
        "observer expiry could not be parsed",
      );
    }
    const manifest = {
      schema_version: 1,
      environment_id: TARGET.environment.id,
      cluster_server: "https://127.0.0.1:6443",
      observer_cn: "cma-trigger-smoke-observer",
      namespace: "cma-trigger-sandboxes",
      created_at: new Date().toISOString(),
      certificate_not_after: notAfter.toISOString(),
      digests: {
        ca_cert: sha256(await readFile(caCert)),
        ca_key: sha256(await readFile(caKey)),
        observer_cert: sha256(await readFile(observerCert)),
        observer_key: sha256(await readFile(observerKey)),
        kubeconfig: sha256(await readFile(kubeconfig)),
      },
    };
    await writeFile(
      join(temporary, "authority.json"),
      `${JSON.stringify(manifest, null, 2)}\n`,
      {
        mode: 0o600,
        flag: "wx",
      },
    );
    await rename(temporary, finalPath);
    return await verifyAuthorityDirectory(finalPath);
  } catch (error) {
    await rm(temporary, { recursive: true, force: true });
    throw error;
  }
}

async function buildSealer(custodyDirectory) {
  const target = join(custodyDirectory, "sealed-box-target");
  await mkdir(target, { recursive: true, mode: 0o700 });
  await chmod(target, 0o700);
  await runCommand(
    "cargo",
    [
      "build",
      "--locked",
      "--release",
      "--manifest-path",
      SEALER_MANIFEST,
      "--target-dir",
      target,
    ],
    { timeoutMs: 300_000 },
  );
  const binary = join(target, "release", "cma-trigger-sealed-box");
  const info = await lstat(binary);
  if (!info.isFile() || info.isSymbolicLink()) {
    fail("sealer_build_failed", "sealed-box helper output is unsafe");
  }
  await runCommand(binary, ["--self-test"], { timeoutMs: 30_000 });
  return binary;
}

async function sealValue(binary, publicKey, plaintext) {
  const data = Buffer.isBuffer(plaintext) ? plaintext : Buffer.from(plaintext);
  if (data.length === 0 || data.length > MAX_SECRET_BYTES) {
    fail("secret_input_invalid", "secret plaintext is empty or oversized");
  }
  const result = await runCommand(binary, [], {
    env: sanitizedCommandEnvironment({
      CMA_TRIGGER_SEAL_PUBLIC_KEY_B64: publicKey,
    }),
    input: data,
    timeoutMs: 30_000,
  });
  const encoded = result.stdout.trim();
  let ciphertext;
  try {
    ciphertext = Buffer.from(encoded, "base64");
  } catch {
    fail("sealer_failed", "sealed-box helper returned invalid base64");
  }
  if (
    ciphertext.length !== data.length + 48 ||
    ciphertext.toString("base64") !== encoded
  ) {
    fail("sealer_failed", "sealed-box helper returned incompatible ciphertext");
  }
  return encoded;
}

function requireConfirmation(expected, environment = process.env) {
  if (environment[CONFIRMATION_ENV] !== expected) {
    fail(
      "confirmation_required",
      `${CONFIRMATION_ENV} must be exactly ${expected}`,
    );
  }
}

async function readVariableInventory(transport) {
  const response = await transport.github(
    "GET",
    `/repos/${TARGET.repository.owner}/${TARGET.repository.name}/environments/${ENVIRONMENT}/variables`,
    { query: { per_page: 100 } },
  );
  return validateVariables(response.json);
}

async function readSecretInventory(transport) {
  const response = await transport.github(
    "GET",
    `/repos/${TARGET.repository.owner}/${TARGET.repository.name}/environments/${ENVIRONMENT}/secrets`,
    { query: { per_page: 100 } },
  );
  return validateSecrets(response.json);
}

async function upsertVariable(transport, name, value, exists) {
  if (!MANAGED_VARIABLE_NAMES.includes(name)) {
    fail("mutation_target_invalid", "refusing to mutate an unmanaged variable");
  }
  const prefix =
    `/repos/${TARGET.repository.owner}/${TARGET.repository.name}` +
    `/environments/${ENVIRONMENT}/variables`;
  try {
    if (exists) {
      await transport.github("PATCH", `${prefix}/${encodeURIComponent(name)}`, {
        json: { name, value },
        statuses: [204],
      });
    } else {
      await transport.github("POST", prefix, {
        json: { name, value },
        statuses: [201],
      });
    }
  } catch (error) {
    const inventory = await readVariableInventory(transport);
    if (inventory[name] === value)
      return { status: "reconciled_after_ambiguous_write" };
    throw new OperatorFailure(
      "variable_write_recovery_required",
      `variable ${name} could not be reconciled after a write failure`,
      { cause_code: error?.code ?? "unknown" },
    );
  }
  const inventory = await readVariableInventory(transport);
  if (inventory[name] !== value) {
    fail(
      "variable_write_recovery_required",
      `variable ${name} acknowledgement was not observable`,
    );
  }
  return { status: exists ? "updated" : "created" };
}

async function putEnvironmentSecret(
  transport,
  { name, plaintext, publicKey, sealerBinary, seal = sealValue },
) {
  if (!isManagedSecretName(name)) {
    fail("mutation_target_invalid", "refusing to mutate an unmanaged secret");
  }
  const plaintextBytes = Buffer.isBuffer(plaintext)
    ? plaintext
    : Buffer.from(plaintext);
  if (
    plaintextBytes.length === 0 ||
    plaintextBytes.length > MAX_GITHUB_SECRET_PLAINTEXT_BYTES
  ) {
    fail(
      "secret_input_invalid",
      `secret ${name} plaintext exceeds GitHub's 48 KiB limit`,
    );
  }
  const encrypted = await seal(sealerBinary, publicKey.key, plaintextBytes);
  const path =
    `/repos/${TARGET.repository.owner}/${TARGET.repository.name}` +
    `/environments/${ENVIRONMENT}/secrets/${encodeURIComponent(name)}`;
  try {
    await transport.github("PUT", path, {
      json: { encrypted_value: encrypted, key_id: publicKey.key_id },
      statuses: [201, 204],
    });
  } catch (error) {
    const status = error?.details?.status;
    if (
      error?.code === "remote_status_error" &&
      Number.isSafeInteger(status) &&
      status >= 400 &&
      status < 500
    ) {
      throw new OperatorFailure(
        "secret_write_rejected",
        `secret ${name} was definitively rejected before acknowledgement`,
        { status },
      );
    }
    // Secret-list presence cannot prove which plaintext won an ambiguous PUT.
    // Keep the fence closed and require an explicit, reviewed replay.
    const inventory = await readSecretInventory(transport);
    throw new OperatorFailure(
      "secret_write_recovery_required",
      `secret ${name} write is ambiguous and must be explicitly replayed`,
      {
        cause_code: error?.code ?? "unknown",
        name_present: Object.hasOwn(inventory, name),
      },
    );
  }
  const inventory = await readSecretInventory(transport);
  if (!Object.hasOwn(inventory, name)) {
    fail(
      "secret_write_recovery_required",
      `secret ${name} acknowledgement was not observable`,
    );
  }
  return inventory[name];
}

function validateReceipt(raw, authority) {
  exactKeys(
    raw,
    [
      "schema_version",
      "environment_id",
      "completed_at",
      "environment_public_key_id",
      "authority_digests",
      "variables",
      "stable_secrets",
    ],
    "setup receipt",
    "unsafe_receipt",
  );
  if (
    raw.schema_version !== 1 ||
    raw.environment_id !== TARGET.environment.id
  ) {
    fail("unsafe_receipt", "setup receipt target drifted");
  }
  parseTimestamp(
    raw.completed_at,
    "setup receipt completed_at",
    "unsafe_receipt",
  );
  boundedString(
    raw.environment_public_key_id,
    "setup receipt public key id",
    256,
    "unsafe_receipt",
  );
  exactKeys(
    raw.authority_digests,
    ["ca_key", "kubeconfig"],
    "setup receipt authority digests",
    "unsafe_receipt",
  );
  if (
    raw.authority_digests.ca_key !== authority.manifest.digests.ca_key ||
    raw.authority_digests.kubeconfig !== authority.manifest.digests.kubeconfig
  ) {
    fail(
      "unsafe_receipt",
      "setup receipt is not bound to local authority custody",
    );
  }
  exactKeys(
    raw.variables,
    MANAGED_VARIABLE_NAMES,
    "setup receipt variables",
    "unsafe_receipt",
  );
  exactKeys(
    raw.stable_secrets,
    [KIND_CA_MATERIAL_NAME, KUBECONFIG_SECRET],
    "setup receipt stable secrets",
    "unsafe_receipt",
  );
  for (const name of [KIND_CA_MATERIAL_NAME, KUBECONFIG_SECRET]) {
    exactKeys(
      raw.stable_secrets[name],
      ["updated_at"],
      `setup receipt ${name}`,
      "unsafe_receipt",
    );
    parseTimestamp(
      raw.stable_secrets[name].updated_at,
      `setup receipt ${name}.updated_at`,
      "unsafe_receipt",
    );
  }
  return raw;
}

async function readReceipt(authority) {
  const path = join(authority.path, "environment-receipt.json");
  return validateReceipt(
    await readSecureJson(path, "setup receipt"),
    authority,
  );
}

async function writeReceipt(authority, snapshot, desired, publicKeyId) {
  const receipt = {
    schema_version: 1,
    environment_id: TARGET.environment.id,
    completed_at: new Date().toISOString(),
    environment_public_key_id: publicKeyId,
    authority_digests: {
      ca_key: authority.manifest.digests.ca_key,
      kubeconfig: authority.manifest.digests.kubeconfig,
    },
    variables: desired,
    stable_secrets: {
      [KIND_CA_MATERIAL_NAME]: {
        updated_at:
          snapshot.inventory.secrets[KIND_CA_MATERIAL_NAME].updated_at,
      },
      [KUBECONFIG_SECRET]: {
        updated_at: snapshot.inventory.secrets[KUBECONFIG_SECRET].updated_at,
      },
    },
  };
  const path = join(authority.path, "environment-receipt.json");
  await atomicWrite(path, `${JSON.stringify(receipt, null, 2)}\n`);
  return validateReceipt(receipt, authority);
}

async function setupEnvironment({
  config,
  state,
  transport,
  custodyDirectory,
  approvedPlan,
  environment = process.env,
  generate = generateAuthority,
  build = buildSealer,
  seal = sealValue,
}) {
  requireConfirmation(SETUP_CONFIRMATION, environment);
  await verifyPrivateDirectory(custodyDirectory, "smoke custody directory");
  const { plan: current, snapshot: initial } = await planEnvironment({
    config,
    state,
    transport,
  });
  requireCurrentApprovedPlan(approvedPlan, current);
  requireNoConflicts(initial);
  requireEphemeralSecretSet(initial.inventory.secrets);
  const authorityPath = join(custodyDirectory, "kind-authority-v1");
  let authorityExists = true;
  try {
    await lstat(authorityPath);
  } catch (error) {
    if (error?.code === "ENOENT") authorityExists = false;
    else throw error;
  }
  if (
    !authorityExists &&
    [KIND_CA_MATERIAL_NAME, KUBECONFIG_SECRET].some((name) =>
      Object.hasOwn(initial.inventory.secrets, name),
    )
  ) {
    fail(
      "stable_secret_custody_required",
      "stable environment secrets exist without matching local authority custody",
    );
  }
  const authority = await generate(custodyDirectory);
  const kubeconfig = await readFile(authority.files.kubeconfig);
  const caKey = await readFile(authority.files.ca_key);
  const desired = desiredVariables(config, sha256(kubeconfig));
  let variables = await readVariableInventory(transport);
  for (const [name, value] of Object.entries(desired).sort()) {
    if (variables[name] !== value) {
      await upsertVariable(
        transport,
        name,
        value,
        Object.hasOwn(variables, name),
      );
      variables = await readVariableInventory(transport);
    }
  }
  const beforeSecrets = await collectLiveSnapshot({ config, state, transport });
  requireNoConflicts(beforeSecrets);
  for (const [name, value] of Object.entries(desired)) {
    if (beforeSecrets.inventory.variables[name] !== value) {
      fail(
        "setup_preflight_changed",
        `variable ${name} is not exact before secret setup`,
      );
    }
  }
  const sealer = await build(custodyDirectory);
  await putEnvironmentSecret(transport, {
    name: KUBECONFIG_SECRET,
    plaintext: kubeconfig.toString("base64"),
    publicKey: beforeSecrets.inventory.public_key,
    sealerBinary: sealer,
    seal,
  });
  // Re-read the public key and all protection state before the second secret.
  // If GitHub rotates the key or another writer starts, the next PUT is fenced.
  const middle = await collectLiveSnapshot({ config, state, transport });
  requireNoConflicts(middle);
  await putEnvironmentSecret(transport, {
    name: KIND_CA_MATERIAL_NAME,
    plaintext: caKey.toString("base64"),
    publicKey: middle.inventory.public_key,
    sealerBinary: sealer,
    seal,
  });
  const final = await collectLiveSnapshot({ config, state, transport });
  requireNoConflicts(final);
  for (const [name, value] of Object.entries(desired)) {
    if (final.inventory.variables[name] !== value) {
      fail(
        "setup_verification_failed",
        `variable ${name} changed during setup`,
      );
    }
  }
  for (const name of [KIND_CA_MATERIAL_NAME, KUBECONFIG_SECRET]) {
    if (!Object.hasOwn(final.inventory.secrets, name)) {
      fail(
        "setup_verification_failed",
        `stable secret ${name} is absent after setup`,
      );
    }
  }
  requireEphemeralSecretSet(final.inventory.secrets);
  const receipt = await writeReceipt(
    authority,
    final,
    desired,
    final.inventory.public_key.key_id,
  );
  return {
    status: "staging_environment_ready",
    environment_id: TARGET.environment.id,
    authority_path: authority.path,
    receipt,
  };
}

async function validateSetupReadiness({
  config,
  state,
  transport,
  custodyDirectory,
}) {
  const authority = await verifyAuthorityDirectory(
    join(custodyDirectory, "kind-authority-v1"),
  );
  const receipt = await readReceipt(authority);
  const snapshot = await collectLiveSnapshot({ config, state, transport });
  const desired = desiredVariables(
    config,
    authority.manifest.digests.kubeconfig,
  );
  for (const [name, value] of Object.entries(desired)) {
    if (
      snapshot.inventory.variables[name] !== value ||
      receipt.variables[name] !== value
    ) {
      fail(
        "staging_setup_drift",
        `staging variable ${name} differs from custody`,
      );
    }
  }
  for (const name of [KIND_CA_MATERIAL_NAME, KUBECONFIG_SECRET]) {
    if (
      !Object.hasOwn(snapshot.inventory.secrets, name) ||
      snapshot.inventory.secrets[name].updated_at !==
        receipt.stable_secrets[name].updated_at
    ) {
      fail("staging_setup_drift", `stable secret ${name} changed after setup`);
    }
  }
  if (
    snapshot.inventory.public_key.key_id !== receipt.environment_public_key_id
  ) {
    fail("staging_setup_drift", "environment public key changed after setup");
  }
  return { authority, receipt, snapshot };
}

async function deleteEphemeralHandoff(transport, expectedName = undefined) {
  let name = expectedName;
  if (name === undefined) {
    const before = await readSecretInventory(transport);
    const candidates = ephemeralHandoffSecretNames(before);
    if (candidates.length === 0) {
      return { status: "ephemeral_handoff_absent" };
    }
    if (candidates.length !== 1) {
      fail(
        "ephemeral_cleanup_recovery_required",
        "more than one ephemeral handoff exists; refuse name-only cleanup",
        { candidates: candidates.sort() },
      );
    }
    [name] = candidates;
  }
  if (!EPHEMERAL_HANDOFF_SECRET_NAME.test(name)) {
    fail("mutation_target_invalid", "ephemeral cleanup target is invalid");
  }
  const path =
    `/repos/${TARGET.repository.owner}/${TARGET.repository.name}` +
    `/environments/${ENVIRONMENT}/secrets/${name}`;
  try {
    await transport.github("DELETE", path, { statuses: [204, 404] });
  } catch (error) {
    const inventory = await readSecretInventory(transport);
    if (!Object.hasOwn(inventory, name)) {
      return { status: "absence_reconciled_after_ambiguous_delete" };
    }
    throw new OperatorFailure(
      "ephemeral_cleanup_recovery_required",
      "ephemeral handoff delete was ambiguous and its name remains present",
      { cause_code: error?.code ?? "unknown", name },
    );
  }
  const inventory = await readSecretInventory(transport);
  if (Object.hasOwn(inventory, name)) {
    fail(
      "ephemeral_cleanup_incident",
      "ephemeral handoff remains after deletion acknowledgement",
    );
  }
  return { status: "ephemeral_handoff_absent" };
}

function validateWorkflowRunList(raw) {
  exactKeys(
    raw,
    ["total_count", "workflow_runs"],
    "workflow dispatch run list",
  );
  if (
    !Number.isSafeInteger(raw.total_count) ||
    raw.total_count < 0 ||
    !Array.isArray(raw.workflow_runs) ||
    raw.workflow_runs.length > 100 ||
    raw.total_count < raw.workflow_runs.length
  ) {
    fail("dispatch_inventory_invalid", "workflow dispatch run list is invalid");
  }
  return raw.workflow_runs.map((run) => {
    object(run, "workflow dispatch run");
    positiveInteger(run.id, "workflow dispatch run id");
    return run;
  });
}

async function listWorkflowRuns(transport) {
  const response = await transport.github(
    "GET",
    `/repos/${TARGET.repository.owner}/${TARGET.repository.name}/actions/workflows/${TARGET.workflow.id}/runs`,
    {
      query: {
        branch: "develop",
        event: "workflow_dispatch",
        per_page: 100,
      },
    },
  );
  return validateWorkflowRunList(response.json);
}

function runMatchesDispatch(
  run,
  { baselineIds, headSha, startedAt, dispatchNonce },
) {
  const createdAt = Date.parse(run.created_at);
  return (
    !baselineIds.has(run.id) &&
    run.workflow_id === TARGET.workflow.id &&
    run.path === WORKFLOW_PATH &&
    run.event === "workflow_dispatch" &&
    run.head_branch === "develop" &&
    run.head_sha === headSha &&
    run.display_title === dispatchRunTitle(dispatchNonce) &&
    run.actor?.login === TARGET.account.login &&
    run.triggering_actor?.login === TARGET.account.login &&
    Number.isFinite(createdAt) &&
    createdAt >= startedAt - 5_000
  );
}

async function sleep(milliseconds) {
  await new Promise((resolvePromise) =>
    setTimeout(resolvePromise, milliseconds),
  );
}

async function identifyDispatchedRun(
  transport,
  {
    baselineIds,
    headSha,
    startedAt,
    dispatchNonce,
    interrupted,
    timeoutMs = 120_000,
  },
) {
  validateDispatchNonce(dispatchNonce);
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    if (interrupted?.signal !== null) {
      fail("operator_interrupted", `operator received ${interrupted.signal}`);
    }
    const matching = (await listWorkflowRuns(transport)).filter((run) =>
      runMatchesDispatch(run, {
        baselineIds,
        headSha,
        startedAt,
        dispatchNonce,
      }),
    );
    if (matching.length > 1) {
      fail(
        "dispatch_recovery_required",
        "more than one workflow run matches the dispatch fence",
      );
    }
    if (matching.length === 1) return matching[0];
    await sleep(2_000);
  }
  fail(
    "dispatch_recovery_required",
    "no exact workflow run became observable; the dispatch must not be retried automatically",
  );
}

async function dispatchWorkflow(
  transport,
  {
    config,
    headSha,
    interrupted,
    dispatchNonce = newDispatchNonce(),
    baselineRuns = undefined,
  },
) {
  validateDispatchNonce(dispatchNonce);
  const handoffSecretName = ephemeralHandoffSecretName(dispatchNonce);
  const baseline = baselineRuns ?? (await listWorkflowRuns(transport));
  const baselineIds = new Set(baseline.map((run) => run.id));
  const startedAt = Date.now();
  requireNotInterrupted(interrupted);
  let dispatchError = null;
  try {
    if (interrupted?.signal !== null) {
      fail("operator_interrupted", `operator received ${interrupted.signal}`);
    }
    await transport.github(
      "POST",
      `/repos/${TARGET.repository.owner}/${TARGET.repository.name}/actions/workflows/${TARGET.workflow.id}/dispatches`,
      {
        json: {
          ref: config.dispatch.ref,
          inputs: {
            dispatch_nonce: dispatchNonce,
            expected_head_sha: headSha,
            expected_fork_mode: config.dispatch.expected_fork_mode,
            handoff_secret_name: handoffSecretName,
            preservation_confirmation:
              config.dispatch.preservation_confirmation,
          },
        },
        statuses: [204],
      },
    );
  } catch (error) {
    dispatchError = error;
  }
  try {
    const run = await identifyDispatchedRun(transport, {
      baselineIds,
      headSha,
      startedAt,
      dispatchNonce,
      interrupted,
    });
    return {
      run,
      dispatch_nonce: dispatchNonce,
      handoff_secret_name: handoffSecretName,
      dispatch_acknowledged: dispatchError === null,
    };
  } catch (identifyError) {
    if (dispatchError !== null) {
      throw new OperatorFailure(
        "dispatch_recovery_required",
        "workflow dispatch response and run identity are both unresolved; do not resend",
        {
          dispatch_code: dispatchError?.code ?? "unknown",
          identify_code: identifyError?.code ?? "unknown",
        },
      );
    }
    throw identifyError;
  }
}

function validateExactRun(raw, expected) {
  object(raw, "workflow run");
  if (
    raw.id !== expected.id ||
    raw.workflow_id !== TARGET.workflow.id ||
    raw.path !== WORKFLOW_PATH ||
    raw.event !== "workflow_dispatch" ||
    raw.head_branch !== "develop" ||
    raw.head_sha !== expected.head_sha ||
    raw.display_title !== expected.display_title ||
    raw.actor?.login !== TARGET.account.login ||
    raw.triggering_actor?.login !== TARGET.account.login
  ) {
    fail("run_authority_drift", "watched workflow run identity drifted");
  }
  if (!ACTIVE_RUN_STATUSES.includes(raw.status) && raw.status !== "completed") {
    fail("run_authority_drift", "workflow run status is unknown");
  }
  return raw;
}

async function watchWorkflowRun(transport, run, config, interrupted) {
  const deadline = Date.now() + config.dispatch.watch_timeout_seconds * 1000;
  while (Date.now() < deadline) {
    if (interrupted.signal !== null) {
      fail("operator_interrupted", `operator received ${interrupted.signal}`);
    }
    const response = await transport.github(
      "GET",
      `/repos/${TARGET.repository.owner}/${TARGET.repository.name}/actions/runs/${run.id}`,
    );
    const current = validateExactRun(response.json, run);
    if (current.status === "completed") {
      boundedString(current.conclusion, "workflow conclusion", 64);
      return {
        id: current.id,
        status: current.status,
        conclusion: current.conclusion,
        head_sha: current.head_sha,
        html_url: current.html_url,
      };
    }
    await sleep(config.dispatch.poll_seconds * 1000);
  }
  fail(
    "run_watch_timeout",
    "exact provider-smoke run did not complete within its bound",
  );
}

function installSignalFence(interrupted) {
  const handlers = new Map();
  for (const signal of ["SIGHUP", "SIGINT", "SIGTERM"]) {
    const handler = () => {
      if (interrupted.signal === null) interrupted.signal = signal;
    };
    process.on(signal, handler);
    handlers.set(signal, handler);
  }
  return () => {
    for (const [signal, handler] of handlers) process.off(signal, handler);
  };
}

function requireNotInterrupted(interrupted) {
  if (interrupted?.signal !== null) {
    fail("operator_interrupted", `operator received ${interrupted.signal}`);
  }
}

async function stageRun({
  config,
  state,
  handoff,
  handoffBytes,
  transport,
  custodyDirectory,
  approvedPlan,
  environment = process.env,
  build = buildSealer,
  seal = sealValue,
  watch = watchWorkflowRun,
}) {
  requireConfirmation(RUN_CONFIRMATION, environment);
  if (state.caller === null) {
    fail(
      "runtime_authority_missing",
      "mint the bounded caller before stage-run",
    );
  }
  validateHandoff(handoff, state, config.provisioning_user_service_id);
  const { plan: current, snapshot: initial } = await planEnvironment({
    config,
    state,
    transport,
  });
  requireCurrentApprovedPlan(approvedPlan, current);
  requireNoConflicts(initial);
  const approvedHeadSha = initial.branch.head.sha;
  requireEphemeralSecretSet(initial.inventory.secrets);
  const dispatchNonce = newDispatchNonce();
  const handoffSecretName = ephemeralHandoffSecretName(dispatchNonce);
  const setup = await validateSetupReadiness({
    config,
    state,
    transport,
    custodyDirectory,
  });
  requireApprovedHead(setup.snapshot, approvedHeadSha);
  await verifyHandoffToken(transport, handoff, state);
  const runtimeAuthority = await inspectAuthority(transport, {
    token: handoff.access_token,
    serviceId: state.caller.service_id,
    expectedSubject: state.caller.subject_id,
    expectedScopes: RUNTIME_GITHUB_SCOPES,
  });
  if (
    runtimeAuthority.user_service.id === config.provisioning_user_service_id
  ) {
    fail(
      "runtime_authority_broadened",
      "runtime and staging-administration routes are identical",
    );
  }
  const sealer = await build(custodyDirectory);
  let handoffMayExist = false;
  const interrupted = { signal: null };
  const removeSignals = installSignalFence(interrupted);
  let primaryError = null;
  let result;
  try {
    const beforeService = await collectLiveSnapshot({
      config,
      state,
      transport,
    });
    requireNoConflicts(beforeService);
    requireApprovedHead(beforeService, approvedHeadSha);
    requireEphemeralSecretSet(beforeService.inventory.secrets);
    requireNotInterrupted(interrupted);
    await putEnvironmentSecret(transport, {
      name: RUNTIME_SERVICE_SECRET,
      plaintext: state.caller.service_id,
      publicKey: beforeService.inventory.public_key,
      sealerBinary: sealer,
      seal,
    });
    const beforeHandoff = await collectLiveSnapshot({
      config,
      state,
      transport,
    });
    requireNoConflicts(beforeHandoff);
    requireApprovedHead(beforeHandoff, approvedHeadSha);
    requireEphemeralSecretSet(beforeHandoff.inventory.secrets);
    requireNotInterrupted(interrupted);
    handoffMayExist = true;
    await putEnvironmentSecret(transport, {
      name: handoffSecretName,
      plaintext: handoffBytes.toString("base64"),
      publicKey: beforeHandoff.inventory.public_key,
      sealerBinary: sealer,
      seal,
    });
    const beforeDispatch = await collectLiveSnapshot({
      config,
      state,
      transport,
    });
    requireNoConflicts(beforeDispatch);
    requireApprovedHead(beforeDispatch, approvedHeadSha);
    requireEphemeralSecretSet(beforeDispatch.inventory.secrets, [
      handoffSecretName,
    ]);
    if (
      !Object.hasOwn(beforeDispatch.inventory.secrets, handoffSecretName) ||
      !Object.hasOwn(beforeDispatch.inventory.secrets, RUNTIME_SERVICE_SECRET)
    ) {
      fail(
        "dispatch_preflight_failed",
        "staged secret inventory is incomplete",
      );
    }
    const dispatched = await dispatchWorkflow(transport, {
      config,
      headSha: approvedHeadSha,
      dispatchNonce,
      interrupted,
    });
    const completed = await watch(
      transport,
      dispatched.run,
      config,
      interrupted,
    );
    if (
      completed.status !== "completed" ||
      completed.conclusion !== "success"
    ) {
      fail(
        "provider_smoke_failed",
        "the exact protected provider-smoke run did not succeed",
        {
          run_id: completed.id,
          status: completed.status,
          conclusion: completed.conclusion,
          head_sha: completed.head_sha,
          html_url: completed.html_url,
        },
      );
    }
    result = {
      status: "protected_smoke_completed",
      dispatch_nonce: dispatchNonce,
      dispatch_acknowledged: dispatched.dispatch_acknowledged,
      run: completed,
      runtime_authority: {
        subject_id: runtimeAuthority.nyxid.subject_id,
        service_id: runtimeAuthority.user_service.id,
        github_account_id: runtimeAuthority.github.id,
        github_login: runtimeAuthority.github.login,
        granted_scopes: runtimeAuthority.authorization.granted_scopes,
      },
    };
  } catch (error) {
    primaryError = error;
  } finally {
    let cleanupError = null;
    if (handoffMayExist) {
      try {
        await deleteEphemeralHandoff(transport, handoffSecretName);
      } catch (error) {
        cleanupError = error;
      }
    }
    removeSignals();
    if (cleanupError !== null) throw cleanupError;
  }
  if (primaryError !== null) throw primaryError;
  return result;
}

async function cleanupEnvironment({
  config,
  state,
  transport,
  approvedPlan,
  environment = process.env,
}) {
  requireConfirmation(CLEANUP_CONFIRMATION, environment);
  const { plan: current } = await planEnvironment({ config, state, transport });
  requireCurrentApprovedPlan(approvedPlan, current);
  const cleanup = await deleteEphemeralHandoff(transport);
  const inventory = await readSecretInventory(transport);
  for (const stable of [
    KIND_CA_MATERIAL_NAME,
    KUBECONFIG_SECRET,
    RUNTIME_SERVICE_SECRET,
  ]) {
    if (
      Object.hasOwn(current.observed.inventory.secrets, stable) &&
      !Object.hasOwn(inventory, stable)
    ) {
      fail(
        "cleanup_scope_violation",
        `stable secret ${stable} disappeared during cleanup`,
      );
    }
  }
  return { status: "ephemeral_cleanup_complete", cleanup };
}

function parseArguments(argv) {
  const modes = new Set([
    "--validate-config",
    "--plan",
    "--setup",
    "--stage-run",
    "--cleanup",
    "--self-test",
  ]);
  if (argv.length === 1 && argv[0] === "--self-test") {
    return { mode: "--self-test" };
  }
  if (argv.length < 2 || !modes.has(argv[0]) || argv[0] === "--self-test") {
    fail("usage", "invalid operator command");
  }
  const mode = argv[0];
  const configPath = argv[1];
  const options = {};
  for (let index = 2; index < argv.length; index += 2) {
    const name = argv[index];
    const value = argv[index + 1];
    if (
      value === undefined ||
      ![
        "--state",
        "--output",
        "--approved-plan",
        "--custody",
        "--handoff",
      ].includes(name) ||
      Object.hasOwn(options, name)
    ) {
      fail("usage", "operator options are missing, duplicated, or unknown");
    }
    options[name] = value;
  }
  const required = {
    "--validate-config": [],
    "--plan": ["--state", "--output"],
    "--setup": ["--state", "--approved-plan", "--custody"],
    "--stage-run": ["--state", "--approved-plan", "--custody", "--handoff"],
    "--cleanup": ["--state", "--approved-plan"],
  }[mode];
  const actual = Object.keys(options).sort();
  const wanted = [...required].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((name, index) => name !== wanted[index])
  ) {
    fail(
      "usage",
      `${mode} requires exactly: ${wanted.join(" ") || "no options"}`,
    );
  }
  return { mode, configPath, options };
}

async function loadConfig(path) {
  return validateConfig(await readJsonFile(path, 256 * 1024));
}

async function loadState(path) {
  return validateBootstrapState(await readSecureJson(path, "bootstrap state"));
}

async function loadHandoff(path, state, provisioningServiceId) {
  const bytes = await readSecureBoundedFile(
    path,
    "NyxID handoff",
    MAX_SECRET_BYTES,
  );
  let parsed;
  try {
    parsed = JSON.parse(bytes.toString("utf8"));
  } catch {
    fail("unsafe_handoff", "NyxID handoff is not valid JSON");
  }
  return {
    handoff: validateHandoff(parsed, state, provisioningServiceId),
    bytes,
  };
}

function selfTest() {
  assert.equal(
    sha256(canonicalJson({ b: 2, a: [3, { y: true, x: null }] })),
    sha256('{"a":[3,{"x":null,"y":true}],"b":2}'),
  );
  assert.equal(
    validateConfig({
      schema_version: 1,
      environment: ENVIRONMENT,
      nyxid_base_url: NYXID_BASE_URL,
      provisioning_user_service_id: "ac1adb9a-9056-480d-8a7a-411007164a86",
      dispatch: {
        ref: "develop",
        expected_fork_mode: "reuse",
        preservation_confirmation: "PRESERVE",
        poll_seconds: 5,
        watch_timeout_seconds: 14_400,
      },
      static_variables: {
        CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN: "chronoai-shining",
        CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE:
          "CMA Trigger provider smoke {marker}\n",
        CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE:
          "cma-trigger-smoke/{marker}.txt",
        CMA_TRIGGER_SMOKE_FORK_REPOSITORY: "cma-trigger-fixture",
        CMA_TRIGGER_SMOKE_MARKER_INPUT: "smoke_marker",
        CMA_TRIGGER_SMOKE_SANDBOX_NAMESPACE: "cma-trigger-sandboxes",
        CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH: "main",
        CMA_TRIGGER_SMOKE_UPSTREAM_OWNER: "ChronoAIProject",
        CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY: "cma-trigger-fixture",
        CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID: "1349237845",
      },
    }).environment,
    ENVIRONMENT,
  );
  const observed = { z: 1, a: "x" };
  const generated = new Date("2026-08-29T00:00:00.000Z");
  const expires = new Date(
    generated.getTime() + PREFLIGHT_LIFETIME_SECONDS * 1000,
  ).toISOString();
  const plan = {
    schema_version: 1,
    kind: "cma-trigger-smoke-environment-plan",
    generated_at: generated.toISOString(),
    expires_at: expires,
    approval_fingerprint: sha256(
      canonicalJson({
        generated_at: generated.toISOString(),
        expires_at: expires,
        observed,
      }),
    ),
    observed,
  };
  validatePlan(plan);
  return { status: "operator_self_test_passed" };
}

async function main(argv = process.argv.slice(2)) {
  const args = parseArguments(argv);
  if (args.mode === "--self-test") {
    process.stdout.write(`${JSON.stringify(selfTest())}\n`);
    return;
  }
  const config = await loadConfig(args.configPath);
  if (args.mode === "--validate-config") {
    process.stdout.write(
      `${JSON.stringify({ status: "config_valid", environment: config.environment })}\n`,
    );
    return;
  }
  const state = await loadState(args.options["--state"]);
  const token = await readProvisioningToken();
  const transport = new Transport({
    token,
    provisioningUserServiceId: config.provisioning_user_service_id,
  });
  if (args.mode === "--plan") {
    secureAbsolutePath(args.options["--output"], "plan output");
    const { plan } = await planEnvironment({ config, state, transport });
    await atomicWrite(
      args.options["--output"],
      `${JSON.stringify(plan, null, 2)}\n`,
    );
    process.stdout.write(
      `${JSON.stringify({
        status: "plan_ready",
        output: args.options["--output"],
        approval_fingerprint: plan.approval_fingerprint,
        expires_at: plan.expires_at,
        conflicts: plan.observed.conflicts.length,
      })}\n`,
    );
    return;
  }
  const approvedPlan = validatePlan(
    await readSecureJson(
      args.options["--approved-plan"],
      "approved plan",
      MAX_JSON_BYTES,
    ),
  );
  let result;
  if (args.mode === "--setup") {
    result = await setupEnvironment({
      config,
      state,
      transport,
      custodyDirectory: args.options["--custody"],
      approvedPlan,
    });
  } else if (args.mode === "--stage-run") {
    const loaded = await loadHandoff(
      args.options["--handoff"],
      state,
      config.provisioning_user_service_id,
    );
    result = await stageRun({
      config,
      state,
      handoff: loaded.handoff,
      handoffBytes: loaded.bytes,
      transport,
      custodyDirectory: args.options["--custody"],
      approvedPlan,
    });
  } else if (args.mode === "--cleanup") {
    result = await cleanupEnvironment({
      config,
      state,
      transport,
      approvedPlan,
    });
  } else {
    fail("usage", "unsupported command");
  }
  process.stdout.write(`${JSON.stringify(result)}\n`);
}

const isMain =
  process.argv[1] !== undefined &&
  pathToFileURL(resolve(process.argv[1])).href === import.meta.url;

if (isMain) {
  main().catch((error) => {
    const failure =
      error instanceof OperatorFailure
        ? error
        : new OperatorFailure(
            "unexpected_failure",
            "operator failed unexpectedly",
          );
    process.stderr.write(
      `${JSON.stringify({
        status: "failed",
        code: failure.code,
        message: failure.message,
        ...(failure.details === undefined ? {} : { details: failure.details }),
      })}\n`,
    );
    process.exitCode = 1;
  });
}

export {
  ACTIVE_RUN_STATUSES,
  CLEANUP_CONFIRMATION,
  EPHEMERAL_HANDOFF_SECRET_PREFIX,
  GITHUB_CATALOG_SERVICE_ID,
  KIND_CA_MATERIAL_NAME,
  KUBECONFIG_DIGEST_VARIABLE,
  KUBECONFIG_SECRET,
  MANAGED_SECRET_NAMES,
  MANAGED_VARIABLE_NAMES,
  NYXID_SOURCE_REVISION,
  OperatorFailure,
  PROVISIONING_GITHUB_SCOPES,
  RUNTIME_GITHUB_SCOPES,
  RUNTIME_SERVICE_SECRET,
  RUN_CONFIRMATION,
  SETUP_CONFIRMATION,
  TARGET,
  Transport,
  buildPlan,
  canonicalJson,
  cleanupEnvironment,
  collectLiveSnapshot,
  deleteEphemeralHandoff,
  desiredVariables,
  dispatchWorkflow,
  ephemeralHandoffSecretName,
  generateAuthority,
  identifyDispatchedRun,
  inspectAuthority,
  loadHandoff,
  loadState,
  parseArguments,
  planEnvironment,
  putEnvironmentSecret,
  readProvisioningToken,
  requireCurrentApprovedPlan,
  sealValue,
  selfTest,
  setupEnvironment,
  sha256,
  stageRun,
  upsertVariable,
  validateAccessClaims,
  validateAuthorization,
  validateBootstrapState,
  validateConfig,
  validateEnvironment,
  validateHandoff,
  validatePlan,
  validateSecrets,
  validateSetupReadiness,
  validateUserServiceList,
  validateVariables,
  verifyAuthorityDirectory,
};
