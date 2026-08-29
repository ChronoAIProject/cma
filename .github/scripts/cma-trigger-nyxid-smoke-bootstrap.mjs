#!/usr/bin/env node

import {
  createHash,
  createPublicKey,
  randomBytes,
  verify as verifySignature,
} from "node:crypto";
import { execFile } from "node:child_process";
import { constants as fsConstants, existsSync } from "node:fs";
import { lstat, mkdir, open, readFile, rename } from "node:fs/promises";
import http from "node:http";
import { dirname, isAbsolute, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const NYXID_BASE_URL = "https://nyx-api.chrono-ai.fun";
const NYXID_SOURCE_REPOSITORY = "ChronoAIProject/NyxID";
const NYXID_SOURCE_REVISION = "245600cb15110c43aafed77c8a8ee73f655ac060";
const NYXID_API_VERSION = "0.10.0";
const NYXID_SOURCE_REF_URL =
  "https://api.github.com/repos/ChronoAIProject/NyxID/git/ref/heads/main";
const STORAGE_SERVICE_SLUG = "chrono-storage-service";
const STORAGE_ROLE_SLUG = "chrono-bucket-user";
const GITHUB_CATALOG_SERVICE_SLUG = "api-github";
const STORAGE_API_VERSION = "1.0.0";
const STORAGE_ALLOWED_SCOPES = Object.freeze([
  "proxy",
  "object:create",
  "object:read",
  "object:download",
  "object:update",
  "object:delete",
]);
const STORAGE_ROLE_PERMISSIONS = Object.freeze([
  "bucket:create",
  "bucket:delete",
  "bucket:download",
  "bucket:read",
  "bucket:update",
  "object:create",
  "object:delete",
  "object:download",
  "object:read",
  "object:update",
]);
const CALLER_SCOPES = Object.freeze([
  "openid",
  "profile",
  "email",
  "roles",
  "proxy",
  "offline_access",
]);
const CMA_REQUIRED_PERMISSIONS = Object.freeze([
  "cma:session:create",
  "cma:session:use",
  "cma:session:terminate",
]);
const APPLY_CONFIRMATION = "provision-reviewed-nonproduction-resources";
const CONFIG_ENVIRONMENT = "cma-trigger-smoke";
const MAX_JSON_BYTES = 8 * 1024 * 1024;
const MIN_STORAGE_TOKEN_LIFETIME_SECONDS = 120;
const MIN_CALLER_TOKEN_LIFETIME_SECONDS = 7_200;
const MAX_CALLER_TOKEN_LIFETIME_SECONDS = 28_800;
const CALLBACK_TIMEOUT_MS = 5 * 60 * 1000;
const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/u;
const SA_CLIENT_ID = /^sa_[0-9a-f]{24}$/u;
const BUCKET = /^[a-z0-9][a-z0-9.-]{1,61}[a-z0-9]$/u;
const SLUG = /^[a-z0-9](?:[a-z0-9_-]{0,98}[a-z0-9])?$/u;
const REPOSITORY_ROOT = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../..",
);

const DISCOVERY_KEYS = Object.freeze([
  "issuer",
  "authorization_endpoint",
  "token_endpoint",
  "pushed_authorization_request_endpoint",
  "require_pushed_authorization_requests",
  "request_uri_parameter_supported",
  "userinfo_endpoint",
  "jwks_uri",
  "response_types_supported",
  "grant_types_supported",
  "subject_token_types_supported",
  "nyxid_broker_binding_supported",
  "oauth_broker_binding_revocation_webhook_supported",
  "subject_types_supported",
  "id_token_signing_alg_values_supported",
  "dpop_signing_alg_values_supported",
  "tls_client_certificate_bound_access_tokens",
  "introspection_endpoint",
  "revocation_endpoint",
  "scopes_supported",
  "claims_supported",
  "code_challenge_methods_supported",
  "token_endpoint_auth_methods_supported",
  "userinfo_signing_alg_values_supported",
]);
const CREATE_SERVICE_FIELDS = Object.freeze([
  "anonymous_endpoints",
  "auth_key_name",
  "auth_method",
  "auth_notes",
  "base_url",
  "billing",
  "capabilities",
  "credential",
  "default_request_headers",
  "description",
  "developer_app_ids",
  "examples_url",
  "forward_access_token",
  "homepage_url",
  "issues_url",
  "known_limitations",
  "name",
  "proxy_operation_policy",
  "recommended_skills",
  "repository_url",
  "required_permissions",
  "service_category",
  "service_type",
  "slug",
  "ssh_config",
  "token_exchange_config",
  "visibility",
  "ws_frame_injections",
]);
const SERVICE_RESPONSE_FIELDS = Object.freeze([
  "anonymous_endpoints",
  "api_spec_url",
  "asyncapi_spec_url",
  "auth_key_name",
  "auth_method",
  "auth_notes",
  "auth_type",
  "base_url",
  "billing",
  "capabilities",
  "created_at",
  "created_by",
  "custom_user_agent",
  "default_request_headers",
  "delegation_token_scope",
  "description",
  "developer_app_ids",
  "effective_platform_metric",
  "examples_url",
  "forward_access_token",
  "homepage_url",
  "id",
  "identity_include_email",
  "identity_include_name",
  "identity_include_user_id",
  "identity_jwt_audience",
  "identity_propagation_mode",
  "inject_delegation_token",
  "is_active",
  "issues_url",
  "known_limitations",
  "name",
  "node_id",
  "oauth_client_id",
  "openapi_spec_url",
  "proxy_operation_policy",
  "recommended_skills",
  "repository_url",
  "required_permissions",
  "requires_user_credential",
  "service_category",
  "service_type",
  "slug",
  "ssh_config",
  "streaming_supported",
  "updated_at",
  "visibility",
  "ws_frame_injections",
  "your_binding_count",
  "your_user_service_id",
]);
const SERVICE_REQUIRED_FIELDS = Object.freeze([
  "id",
  "name",
  "slug",
  "description",
  "base_url",
  "service_type",
  "visibility",
  "auth_method",
  "auth_type",
  "auth_key_name",
  "is_active",
  "oauth_client_id",
  "openapi_spec_url",
  "api_spec_url",
  "asyncapi_spec_url",
  "streaming_supported",
  "ssh_config",
  "service_category",
  "requires_user_credential",
  "identity_propagation_mode",
  "identity_include_user_id",
  "identity_include_email",
  "identity_include_name",
  "identity_jwt_audience",
  "forward_access_token",
  "inject_delegation_token",
  "delegation_token_scope",
  "effective_platform_metric",
  "ws_frame_injections",
  "anonymous_endpoints",
  "created_by",
  "created_at",
  "updated_at",
  "your_binding_count",
]);
const APP_FIELDS = Object.freeze([
  "id",
  "client_name",
  "client_type",
  "redirect_uris",
  "allowed_scopes",
  "delegation_scopes",
  "broker_capability_enabled",
  "revocation_webhook_url",
  "connection_webhook_url",
  "connection_webhook_enabled",
  "is_active",
  "default_service_catalog_slugs",
  "client_secret",
  "created_at",
  "updated_at",
]);
const APP_REQUIRED_FIELDS = APP_FIELDS.filter((key) => key !== "client_secret");
const SA_ITEM_FIELDS = Object.freeze([
  "id",
  "name",
  "description",
  "client_id",
  "secret_prefix",
  "allowed_scopes",
  "role_ids",
  "is_active",
  "rate_limit_override",
  "created_by",
  "created_at",
  "updated_at",
  "last_authenticated_at",
]);

class BootstrapFailure extends Error {
  constructor(code, message) {
    super(message);
    this.name = "BootstrapFailure";
    this.code = code;
  }
}

function fail(code, message) {
  throw new BootstrapFailure(code, message);
}

function object(value, path, code = "contract_drift") {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail(code, `${path} must be an object`);
  }
  return value;
}

function closedKeys(
  value,
  allowed,
  path,
  required = allowed,
  code = "contract_drift",
) {
  const actual = Object.keys(object(value, path, code));
  const extras = actual.filter((key) => !allowed.includes(key));
  const missing = required.filter((key) => !actual.includes(key));
  if (extras.length > 0 || missing.length > 0) {
    fail(
      code,
      `${path} has a closed schema (extra: ${extras.join(", ") || "none"}; missing: ${missing.join(", ") || "none"})`,
    );
  }
}

function exactKeys(value, expected, path, code = "invalid_config") {
  closedKeys(value, expected, path, expected, code);
}

function boundedString(value, path, maximum = 2048, code = "contract_drift") {
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

function exactArray(value, expected, path, { ordered = true } = {}) {
  if (!Array.isArray(value)) {
    fail("contract_drift", `${path} must be an array`);
  }
  const actual = ordered ? value : [...value].sort();
  const wanted = ordered ? [...expected] : [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    fail("contract_drift", `${path} does not match the pinned contract`);
  }
}

function uuid(value, path, code = "contract_drift") {
  if (typeof value !== "string" || !UUID.test(value)) {
    fail(code, `${path} must be a UUID`);
  }
  return value;
}

function tokenSet(value, expected, path, code = "contract_drift") {
  const checked = boundedString(value, path, 2048, code);
  const tokens = checked.split(/\s+/u);
  if (new Set(tokens).size !== tokens.length) {
    fail(code, `${path} contains duplicate scope tokens`);
  }
  const actual = [...tokens].sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    fail(code, `${path} does not match the exact scope set`);
  }
  return checked;
}

function stageMarked(value, path) {
  const lower = boundedString(value, path, 200, "invalid_config").toLowerCase();
  if (!/(?:^|[- _])(staging|smoke)(?:$|[- _])/u.test(lower)) {
    fail(
      "production_config_rejected",
      `${path} must visibly identify staging or smoke`,
    );
  }
  if (/(?:^|[- _])prod(?:uction)?(?:$|[- _])/u.test(lower)) {
    fail("production_config_rejected", `${path} must not identify production`);
  }
}

function validateRedirects(oidc) {
  if (!Array.isArray(oidc.redirect_uris) || oidc.redirect_uris.length !== 2) {
    fail(
      "invalid_config",
      "oidc_app.redirect_uris must contain exactly two URIs",
    );
  }
  if (new Set(oidc.redirect_uris).size !== oidc.redirect_uris.length) {
    fail(
      "invalid_config",
      "oidc_app.redirect_uris must not contain duplicates",
    );
  }
  if (!oidc.redirect_uris.includes(oidc.smoke_redirect_uri)) {
    fail("invalid_config", "oidc_app.smoke_redirect_uri must be registered");
  }
  const smoke = new URL(oidc.smoke_redirect_uri);
  if (
    smoke.protocol !== "http:" ||
    smoke.hostname !== "127.0.0.1" ||
    smoke.port === "" ||
    smoke.pathname === "/" ||
    smoke.search !== "" ||
    smoke.hash !== "" ||
    smoke.username !== "" ||
    smoke.password !== ""
  ) {
    fail(
      "invalid_config",
      "oidc_app.smoke_redirect_uri must be a fixed 127.0.0.1 HTTP callback",
    );
  }
  const port = Number(smoke.port);
  if (!Number.isInteger(port) || port < 1024 || port > 65535) {
    fail("invalid_config", "oidc_app.smoke_redirect_uri uses an unsafe port");
  }

  const cmaUris = oidc.redirect_uris.filter(
    (uri) => uri !== oidc.smoke_redirect_uri,
  );
  const cma = new URL(cmaUris[0]);
  const allowedKindCallback =
    cma.href === "https://127.0.0.1:8443/auth/callback";
  const visiblyStagingHost = /(?:^|[.-])(staging|smoke)(?:[.-]|$)/u.test(
    cma.hostname,
  );
  if (
    cma.protocol !== "https:" ||
    cma.pathname !== "/auth/callback" ||
    cma.search !== "" ||
    cma.hash !== "" ||
    cma.username !== "" ||
    cma.password !== "" ||
    (!allowedKindCallback && !visiblyStagingHost)
  ) {
    fail(
      "production_config_rejected",
      "the CMA callback must be the reviewed Kind callback or a visibly staging/smoke HTTPS host",
    );
  }
  if (cma.href === "https://bot.chrono-ai.fun/auth/callback") {
    fail(
      "production_config_rejected",
      "the production CMA callback is forbidden",
    );
  }
}

function validateConfig(raw) {
  exactKeys(
    raw,
    [
      "schema_version",
      "environment",
      "nyxid_base_url",
      "observer",
      "oidc_app",
      "storage",
    ],
    "config",
  );
  if (raw.schema_version !== 1 || raw.environment !== CONFIG_ENVIRONMENT) {
    fail(
      "invalid_config",
      "config schema_version/environment are not supported",
    );
  }
  if (raw.nyxid_base_url !== NYXID_BASE_URL) {
    fail(
      "production_config_rejected",
      "nyxid_base_url is not the pinned authority",
    );
  }

  exactKeys(
    raw.observer,
    ["name", "slug", "rule_id", "daily_quota"],
    "observer",
  );
  stageMarked(raw.observer.name, "observer.name");
  stageMarked(raw.observer.slug, "observer.slug");
  if (!SLUG.test(raw.observer.slug)) {
    fail("invalid_config", "observer.slug is invalid");
  }
  uuid(raw.observer.rule_id, "observer.rule_id", "invalid_config");
  if (
    !Number.isSafeInteger(raw.observer.daily_quota) ||
    raw.observer.daily_quota < 1 ||
    raw.observer.daily_quota > 10_000
  ) {
    fail("invalid_config", "observer.daily_quota must be from 1 through 10000");
  }

  exactKeys(
    raw.oidc_app,
    ["name", "redirect_uris", "smoke_redirect_uri"],
    "oidc_app",
  );
  stageMarked(raw.oidc_app.name, "oidc_app.name");
  if (!Array.isArray(raw.oidc_app.redirect_uris)) {
    fail("invalid_config", "oidc_app.redirect_uris must be an array");
  }
  boundedString(
    raw.oidc_app.smoke_redirect_uri,
    "oidc_app.smoke_redirect_uri",
    2048,
    "invalid_config",
  );
  for (const [index, uri] of raw.oidc_app.redirect_uris.entries()) {
    boundedString(
      uri,
      `oidc_app.redirect_uris[${index}]`,
      2048,
      "invalid_config",
    );
    try {
      new URL(uri);
    } catch {
      fail(
        "invalid_config",
        `oidc_app.redirect_uris[${index}] is not an absolute URL`,
      );
    }
  }
  validateRedirects(raw.oidc_app);

  exactKeys(
    raw.storage,
    [
      "service_account_name",
      "context_bucket",
      "trigger_workspace_bucket",
      "service_slug",
      "role_slug",
    ],
    "storage",
  );
  stageMarked(raw.storage.service_account_name, "storage.service_account_name");
  if (
    raw.storage.service_slug !== STORAGE_SERVICE_SLUG ||
    raw.storage.role_slug !== STORAGE_ROLE_SLUG
  ) {
    fail(
      "invalid_config",
      "storage service and role must match the pinned contracts",
    );
  }
  for (const key of ["context_bucket", "trigger_workspace_bucket"]) {
    stageMarked(raw.storage[key], `storage.${key}`);
    if (!BUCKET.test(raw.storage[key])) {
      fail("invalid_config", `storage.${key} is not a valid bucket name`);
    }
    if (raw.storage[key] === "chrono-platform-bot") {
      fail(
        "production_config_rejected",
        "the production CMA bucket is forbidden",
      );
    }
  }
  if (raw.storage.context_bucket === raw.storage.trigger_workspace_bucket) {
    fail(
      "invalid_config",
      "context and Trigger workspace buckets must be distinct",
    );
  }
  return raw;
}

function defaultState(config) {
  return {
    schema_version: 1,
    environment: config.environment,
    nyxid_base_url: config.nyxid_base_url,
    source_revision: NYXID_SOURCE_REVISION,
    api_version: NYXID_API_VERSION,
    observer: null,
    oidc_app: null,
    storage_service_account: null,
    buckets: {
      context_bucket: config.storage.context_bucket,
      trigger_workspace_bucket: config.storage.trigger_workspace_bucket,
      owner_service_account_id: null,
    },
    caller: null,
  };
}

function validateState(raw, config) {
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
    "state",
  );
  if (
    raw.schema_version !== 1 ||
    raw.environment !== config.environment ||
    raw.nyxid_base_url !== config.nyxid_base_url ||
    raw.source_revision !== NYXID_SOURCE_REVISION ||
    raw.api_version !== NYXID_API_VERSION
  ) {
    fail("state_authority_mismatch", "state is bound to a different contract");
  }
  if (raw.observer !== null) {
    exactKeys(raw.observer, ["id", "slug"], "state.observer", "unsafe_state");
    uuid(raw.observer.id, "state.observer.id", "unsafe_state");
    if (raw.observer.slug !== config.observer.slug) {
      fail(
        "state_authority_mismatch",
        "state observer slug does not match config",
      );
    }
  }
  if (raw.oidc_app !== null) {
    exactKeys(
      raw.oidc_app,
      ["id", "client_secret"],
      "state.oidc_app",
      "unsafe_state",
    );
    uuid(raw.oidc_app.id, "state.oidc_app.id", "unsafe_state");
    boundedString(
      raw.oidc_app.client_secret,
      "state.oidc_app.client_secret",
      4096,
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
    uuid(
      raw.storage_service_account.id,
      "state.storage_service_account.id",
      "unsafe_state",
    );
    if (!SA_CLIENT_ID.test(raw.storage_service_account.client_id)) {
      fail("unsafe_state", "state storage client_id is invalid");
    }
    boundedString(
      raw.storage_service_account.client_secret,
      "state.storage_service_account.client_secret",
      4096,
      "unsafe_state",
    );
  }
  exactKeys(
    raw.buckets,
    ["context_bucket", "trigger_workspace_bucket", "owner_service_account_id"],
    "state.buckets",
    "unsafe_state",
  );
  if (
    raw.buckets.context_bucket !== config.storage.context_bucket ||
    raw.buckets.trigger_workspace_bucket !==
      config.storage.trigger_workspace_bucket
  ) {
    fail("state_authority_mismatch", "state bucket names do not match config");
  }
  if (raw.buckets.owner_service_account_id !== null) {
    uuid(
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
    uuid(raw.caller.client_id, "state.caller.client_id", "unsafe_state");
    uuid(raw.caller.subject_id, "state.caller.subject_id", "unsafe_state");
    uuid(raw.caller.service_id, "state.caller.service_id", "unsafe_state");
    boundedString(
      raw.caller.refresh_token,
      "state.caller.refresh_token",
      16_384,
      "unsafe_state",
    );
    tokenSet(
      raw.caller.scope,
      CALLER_SCOPES,
      "state.caller.scope",
      "unsafe_state",
    );
    if (raw.oidc_app === null || raw.caller.client_id !== raw.oidc_app.id) {
      fail(
        "state_authority_mismatch",
        "caller refresh authority is not the exact OIDC app",
      );
    }
  }
  return raw;
}

function secureAbsolutePath(path, label) {
  if (typeof path !== "string" || !isAbsolute(path) || resolve(path) !== path) {
    fail("unsafe_secret_path", `${label} must be a normalized absolute path`);
  }
  const fromRepo = relative(REPOSITORY_ROOT, path);
  if (
    fromRepo === "" ||
    (!fromRepo.startsWith("..") && !isAbsolute(fromRepo))
  ) {
    fail("unsafe_secret_path", `${label} must be outside the repository`);
  }
  return path;
}

async function verifyPrivateDirectory(path) {
  const stat = await lstat(path);
  if (!stat.isDirectory() || stat.isSymbolicLink()) {
    fail("unsafe_secret_path", "secret parent must be a real directory");
  }
  if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
    fail("unsafe_secret_path", "secret parent is owned by another user");
  }
  if ((stat.mode & 0o777) !== 0o700) {
    fail("unsafe_secret_path", "secret parent must have mode 0700");
  }
}

async function ensurePrivateDirectory(path) {
  await mkdir(path, { recursive: true, mode: 0o700 });
  await verifyPrivateDirectory(path);
}

async function verifySecureFile(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.nlink !== 1) {
    fail(
      "unsafe_secret_file",
      "secret file must be a singly-linked regular file",
    );
  }
  if (typeof process.getuid === "function" && stat.uid !== process.getuid()) {
    fail("unsafe_secret_file", "secret file is owned by another user");
  }
  if ((stat.mode & 0o777) !== 0o600) {
    fail("unsafe_secret_file", "secret file must have mode 0600");
  }
  return stat;
}

async function readSecureJson(path, { optional = false } = {}) {
  secureAbsolutePath(path, "secret file path");
  const parent = dirname(path);
  if (!existsSync(parent)) {
    if (optional) return null;
    fail("unsafe_secret_path", "secret parent does not exist");
  }
  await verifyPrivateDirectory(parent);
  if (!existsSync(path)) {
    if (optional) return null;
    fail("unsafe_secret_file", "secret file does not exist");
  }
  const stat = await verifySecureFile(path);
  if (stat.size > MAX_JSON_BYTES) {
    fail("unsafe_secret_file", "secret file exceeds the size limit");
  }
  let parsed;
  try {
    parsed = JSON.parse(await readFile(path, "utf8"));
  } catch {
    fail("unsafe_secret_file", "secret file is not valid JSON");
  }
  return parsed;
}

async function atomicWriteSecureJson(path, value) {
  secureAbsolutePath(path, "secret file path");
  const parent = dirname(path);
  await ensurePrivateDirectory(parent);
  if (existsSync(path)) await verifySecureFile(path);
  const temp = `${path}.tmp-${process.pid}-${randomBytes(8).toString("hex")}`;
  const handle = await open(
    temp,
    fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL,
    0o600,
  );
  try {
    await handle.writeFile(`${JSON.stringify(value, null, 2)}\n`, "utf8");
    await handle.sync();
  } finally {
    await handle.close();
  }
  await rename(temp, path);
  const directory = await open(parent, fsConstants.O_RDONLY);
  try {
    await directory.sync();
  } finally {
    await directory.close();
  }
  await verifySecureFile(path);
}

async function readJsonFile(path) {
  const stat = await lstat(path);
  if (!stat.isFile() || stat.size > MAX_JSON_BYTES) {
    fail("invalid_config", "config must be a bounded regular file");
  }
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch {
    fail("invalid_config", "config is not valid JSON");
  }
}

async function readBoundedResponse(response) {
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > MAX_JSON_BYTES) {
    fail("remote_response_too_large", "remote JSON exceeds the response limit");
  }
  if (response.body === null) {
    fail("remote_contract_error", "remote JSON body is missing");
  }
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.length;
    if (size > MAX_JSON_BYTES) {
      fail(
        "remote_response_too_large",
        "remote JSON exceeds the response limit",
      );
    }
    chunks.push(chunk);
  }
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.toLowerCase().includes("application/json")) {
    fail("remote_contract_error", "remote response is not JSON");
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString("utf8"));
  } catch {
    fail("remote_contract_error", "remote response is malformed JSON");
  }
}

class FetchTransport {
  constructor({ timeoutMs = 20_000 } = {}) {
    this.timeoutMs = timeoutMs;
  }

  async request({ method = "GET", url, token, json, form, statuses = [200] }) {
    const allowed =
      url === NYXID_SOURCE_REF_URL || url.startsWith(`${NYXID_BASE_URL}/`);
    if (!allowed) fail("endpoint_not_pinned", "request endpoint is not pinned");
    const headers = {
      accept: "application/json",
      "user-agent": "cma-trigger-nyxid-smoke-bootstrap/1",
    };
    if (token !== undefined) headers.authorization = `Bearer ${token}`;
    let body;
    if (json !== undefined && form !== undefined) {
      fail("internal_error", "request cannot contain JSON and form bodies");
    }
    if (json !== undefined) {
      headers["content-type"] = "application/json";
      body = JSON.stringify(json);
    }
    if (form !== undefined) {
      headers["content-type"] = "application/x-www-form-urlencoded";
      body = new URLSearchParams(form).toString();
    }
    let response;
    try {
      response = await fetch(url, {
        method,
        headers,
        body,
        redirect: "error",
        signal: AbortSignal.timeout(this.timeoutMs),
      });
    } catch {
      fail(
        "remote_transport_error",
        `${method} request did not produce a response`,
      );
    }
    const responseJson = await readBoundedResponse(response);
    if (!statuses.includes(response.status)) {
      fail(
        "remote_status_error",
        `${method} request returned HTTP ${response.status}`,
      );
    }
    return { status: response.status, json: responseJson };
  }
}

function nyx(path) {
  return `${NYXID_BASE_URL}${path}`;
}

function validateSourceRef(raw) {
  exactKeys(raw, ["ref", "node_id", "url", "object"], "NyxID source ref");
  exactKeys(raw.object, ["sha", "type", "url"], "NyxID source ref object");
  if (
    raw.ref !== "refs/heads/main" ||
    raw.object.type !== "commit" ||
    raw.object.sha !== NYXID_SOURCE_REVISION
  ) {
    fail(
      "source_revision_drift",
      "NyxID main no longer matches the reviewed revision",
    );
  }
}

function validateDiscovery(raw) {
  exactKeys(raw, DISCOVERY_KEYS, "OIDC discovery", "contract_drift");
  const endpointPairs = {
    issuer: NYXID_BASE_URL,
    authorization_endpoint: nyx("/oauth/authorize"),
    token_endpoint: nyx("/oauth/token"),
    pushed_authorization_request_endpoint: nyx("/oauth/par"),
    introspection_endpoint: nyx("/oauth/introspect"),
    jwks_uri: nyx("/.well-known/jwks.json"),
  };
  for (const [field, expected] of Object.entries(endpointPairs)) {
    if (raw[field] !== expected) fail("contract_drift", `${field} drifted`);
  }
  exactArray(
    raw.response_types_supported,
    ["code"],
    "response_types_supported",
  );
  exactArray(
    raw.grant_types_supported,
    [
      "authorization_code",
      "refresh_token",
      "client_credentials",
      "urn:ietf:params:oauth:grant-type:token-exchange",
    ],
    "grant_types_supported",
  );
  exactArray(raw.code_challenge_methods_supported, ["S256"], "PKCE methods");
  if (
    raw.require_pushed_authorization_requests !== false ||
    raw.request_uri_parameter_supported !== true
  ) {
    fail(
      "contract_drift",
      "direct authorization is not permitted by the pinned PAR policy",
    );
  }
  exactArray(
    raw.token_endpoint_auth_methods_supported,
    ["client_secret_basic", "client_secret_post", "none"],
    "token auth methods",
  );
  if (!raw.scopes_supported.includes("offline_access")) {
    fail("contract_drift", "OIDC discovery does not advertise offline_access");
  }
  return raw;
}

function schemaProperties(openapi, name) {
  return Object.keys(
    object(
      object(
        object(openapi.components, "OpenAPI components").schemas,
        "OpenAPI schemas",
      )[name],
      `OpenAPI schema ${name}`,
    ).properties ?? {},
  ).sort();
}

function validateNyxidOpenapi(raw) {
  object(raw.info, "OpenAPI info");
  if (raw.info.version !== NYXID_API_VERSION) {
    fail("api_version_drift", "deployed NyxID API version drifted");
  }
  exactArray(
    schemaProperties(raw, "CreateServiceRequest"),
    [...CREATE_SERVICE_FIELDS].sort(),
    "CreateServiceRequest fields",
  );
  exactArray(
    schemaProperties(raw, "ServiceResponse"),
    [...SERVICE_RESPONSE_FIELDS].sort(),
    "ServiceResponse fields",
  );
}

function validateMe(raw) {
  const fields = [
    "id",
    "email",
    "display_name",
    "avatar_url",
    "email_verified",
    "mfa_enabled",
    "is_admin",
    "is_operator",
    "role",
    "is_active",
    "social_provider",
    "created_at",
    "updated_at",
    "last_login_at",
    "profile_config",
    "capabilities",
  ];
  exactKeys(raw, fields, "admin profile", "contract_drift");
  uuid(raw.id, "admin profile id");
  if (raw.is_admin !== true || raw.role !== "admin" || raw.is_active !== true) {
    fail("admin_required", "the NyxID bearer is not an active platform admin");
  }
}

function validateAnonymousRule(raw, config, path) {
  exactKeys(
    raw,
    ["id", "enabled", "method", "path_pattern", "daily_quota"],
    path,
    "contract_drift",
  );
  if (
    raw.id !== config.observer.rule_id ||
    raw.enabled !== true ||
    raw.method !== "GET" ||
    raw.path_pattern !== "/repositories/**" ||
    raw.daily_quota !== config.observer.daily_quota
  ) {
    fail(
      "observer_contract_drift",
      `${path} is broader than the reviewed rule`,
    );
  }
}

function validateServiceItem(raw, path) {
  closedKeys(raw, SERVICE_RESPONSE_FIELDS, path, SERVICE_REQUIRED_FIELDS);
  uuid(raw.id, `${path}.id`);
  boundedString(raw.created_by, `${path}.created_by`);
  boundedString(raw.name, `${path}.name`);
  boundedString(raw.slug, `${path}.slug`);
  if (
    !Array.isArray(raw.ws_frame_injections) ||
    !Array.isArray(raw.anonymous_endpoints)
  ) {
    fail("contract_drift", `${path} rule collections must be arrays`);
  }
  return raw;
}

function validateObserver(raw, config, path = "observer service") {
  validateServiceItem(raw, path);
  const absentOrEmpty = (field) =>
    raw[field] === undefined ||
    raw[field] === null ||
    (Array.isArray(raw[field]) && raw[field].length === 0);
  if (
    raw.name !== config.observer.name ||
    raw.slug !== config.observer.slug ||
    raw.base_url !== "https://api.github.com" ||
    raw.service_type !== "http" ||
    raw.visibility !== "public" ||
    raw.auth_method !== "none" ||
    raw.auth_key_name !== "" ||
    raw.is_active !== true ||
    raw.service_category !== "internal" ||
    raw.requires_user_credential !== false ||
    raw.identity_propagation_mode !== "none" ||
    raw.identity_include_user_id !== false ||
    raw.identity_include_email !== false ||
    raw.identity_include_name !== false ||
    raw.identity_jwt_audience !== null ||
    raw.forward_access_token !== false ||
    raw.inject_delegation_token !== false ||
    raw.ws_frame_injections.length !== 0 ||
    !absentOrEmpty("billing") ||
    !absentOrEmpty("default_request_headers") ||
    !absentOrEmpty("proxy_operation_policy") ||
    !absentOrEmpty("developer_app_ids") ||
    raw.anonymous_endpoints.length !== 1
  ) {
    fail(
      "observer_contract_drift",
      "public GitHub observer security fields drifted",
    );
  }
  validateAnonymousRule(
    raw.anonymous_endpoints[0],
    config,
    `${path}.anonymous_endpoints[0]`,
  );
  return raw;
}

function validateStorageService(raw, config) {
  validateServiceItem(raw, "storage catalog service");
  const billing = raw.billing;
  if (
    raw.slug !== config.storage.service_slug ||
    raw.is_active !== true ||
    raw.service_type !== "http" ||
    raw.visibility !== "public" ||
    raw.auth_method !== "none" ||
    raw.service_category !== "internal" ||
    raw.requires_user_credential !== false ||
    raw.identity_propagation_mode !== "jwt" ||
    raw.identity_jwt_audience !== "chrono-bucket" ||
    raw.identity_include_user_id !== false ||
    raw.identity_include_email !== false ||
    raw.identity_include_name !== false ||
    raw.forward_access_token !== false ||
    raw.inject_delegation_token !== false ||
    raw.anonymous_endpoints.length !== 0 ||
    raw.ws_frame_injections.length !== 0 ||
    !(billing === undefined || billing === null)
  ) {
    fail(
      "storage_contract_drift",
      "chrono-storage service security fields drifted",
    );
  }
  return raw;
}

function validateRole(raw, path) {
  exactKeys(
    raw,
    [
      "id",
      "name",
      "slug",
      "description",
      "permissions",
      "is_default",
      "is_system",
      "client_id",
      "created_at",
      "updated_at",
    ],
    path,
    "contract_drift",
  );
  uuid(raw.id, `${path}.id`);
  if (
    !Array.isArray(raw.permissions) ||
    raw.permissions.some((item) => typeof item !== "string")
  ) {
    fail("contract_drift", `${path}.permissions must be strings`);
  }
  return raw;
}

function validateStorageRole(raw, config) {
  validateRole(raw, "storage role");
  if (
    raw.name !== "Chrono Bucket User" ||
    raw.slug !== config.storage.role_slug ||
    raw.is_default !== false ||
    raw.is_system !== false ||
    raw.client_id !== null
  ) {
    fail("storage_role_drift", "chrono-bucket-user role shape drifted");
  }
  exactArray(
    raw.permissions,
    STORAGE_ROLE_PERMISSIONS,
    "storage role permissions",
  );
  return raw;
}

function validateApp(raw, config, { secretRequired = false } = {}) {
  closedKeys(raw, APP_FIELDS, "OIDC app", APP_REQUIRED_FIELDS);
  uuid(raw.id, "OIDC app id");
  if (!Array.isArray(raw.redirect_uris))
    fail("contract_drift", "OIDC redirects must be an array");
  if (
    raw.client_name !== config.oidc_app.name ||
    raw.client_type !== "confidential" ||
    raw.delegation_scopes !== "" ||
    raw.broker_capability_enabled !== false ||
    raw.revocation_webhook_url !== null ||
    raw.connection_webhook_url !== null ||
    raw.connection_webhook_enabled !== false ||
    raw.is_active !== true
  ) {
    fail("oidc_app_drift", "OIDC app fields drifted");
  }
  exactArray(
    raw.redirect_uris,
    config.oidc_app.redirect_uris,
    "OIDC redirects",
    { ordered: false },
  );
  exactArray(
    raw.default_service_catalog_slugs,
    [GITHUB_CATALOG_SERVICE_SLUG],
    "OIDC default services",
  );
  tokenSet(raw.allowed_scopes, CALLER_SCOPES, "OIDC allowed scopes");
  if (secretRequired)
    boundedString(raw.client_secret, "OIDC client secret", 4096);
  if (!secretRequired && raw.client_secret !== undefined) {
    fail("secret_exposure", "OIDC list unexpectedly returned a client secret");
  }
  return raw;
}

function validateServiceAccountItem(raw, config, roleId, ownerId) {
  exactKeys(raw, SA_ITEM_FIELDS, "storage service account", "contract_drift");
  uuid(raw.id, "storage service account id");
  uuid(raw.created_by, "storage service account creator");
  if (!SA_CLIENT_ID.test(raw.client_id))
    fail("contract_drift", "storage client_id is invalid");
  if (
    raw.name !== config.storage.service_account_name ||
    raw.is_active !== true ||
    raw.rate_limit_override !== null ||
    raw.created_by !== ownerId
  ) {
    fail("storage_account_drift", "storage service account fields drifted");
  }
  tokenSet(
    raw.allowed_scopes,
    STORAGE_ALLOWED_SCOPES,
    "storage allowed scopes",
  );
  exactArray(raw.role_ids, [roleId], "storage role IDs");
  return raw;
}

function validateCreatedServiceAccount(raw, config, roleId) {
  exactKeys(
    raw,
    [
      "id",
      "name",
      "client_id",
      "client_secret",
      "allowed_scopes",
      "role_ids",
      "is_active",
      "created_at",
      "message",
    ],
    "created storage service account",
    "contract_drift",
  );
  uuid(raw.id, "created storage service account id");
  if (
    raw.name !== config.storage.service_account_name ||
    !SA_CLIENT_ID.test(raw.client_id) ||
    raw.is_active !== true
  ) {
    fail("storage_account_drift", "created storage service account drifted");
  }
  boundedString(raw.client_secret, "created storage client secret", 4096);
  tokenSet(
    raw.allowed_scopes,
    STORAGE_ALLOWED_SCOPES,
    "created storage scopes",
  );
  exactArray(raw.role_ids, [roleId], "created storage role IDs");
  return raw;
}

function validateStorageOpenapi(raw) {
  if (
    raw.info?.version !== STORAGE_API_VERSION ||
    raw.info?.title !== "chrono-bucket API"
  ) {
    fail("storage_api_drift", "chrono-storage OpenAPI identity drifted");
  }
  const requirements = {
    "/api/buckets": ["get", "post"],
    "/api/buckets/{bucket}": ["get"],
    "/api/buckets/{bucket}/objects": ["get", "post", "patch", "delete"],
    "/api/buckets/{bucket}/objects/download": ["get"],
  };
  for (const [path, methods] of Object.entries(requirements)) {
    const operations = object(raw.paths?.[path], `storage OpenAPI ${path}`);
    for (const method of methods) {
      if (!Object.hasOwn(operations, method)) {
        fail(
          "storage_api_drift",
          `storage OpenAPI lacks ${method.toUpperCase()} ${path}`,
        );
      }
    }
  }
}

function reconcileCustody(snapshot, state) {
  if (snapshot.app !== null) {
    if (state.oidc_app === null) {
      fail(
        "oidc_secret_rotation_required",
        "the OIDC app exists without matching local secret custody; rotate manually",
      );
    }
    if (state.oidc_app.id !== snapshot.app.id) {
      fail(
        "state_authority_mismatch",
        "OIDC state ID does not match the live app",
      );
    }
  } else if (state.oidc_app !== null) {
    fail("state_authority_mismatch", "OIDC state refers to a missing live app");
  }
  if (snapshot.serviceAccount !== null) {
    if (state.storage_service_account === null) {
      fail(
        "storage_secret_rotation_required",
        "the storage service account exists without matching local secret custody; rotate manually",
      );
    }
    if (
      state.storage_service_account.id !== snapshot.serviceAccount.id ||
      state.storage_service_account.client_id !==
        snapshot.serviceAccount.client_id
    ) {
      fail(
        "state_authority_mismatch",
        "storage state does not match the live account",
      );
    }
  } else if (state.storage_service_account !== null) {
    fail(
      "state_authority_mismatch",
      "storage state refers to a missing live account",
    );
  }
  if (
    snapshot.observer !== null &&
    state.observer !== null &&
    state.observer.id !== snapshot.observer.id
  ) {
    fail(
      "state_authority_mismatch",
      "observer state ID does not match the live service",
    );
  }
  if (snapshot.observer === null && state.observer !== null) {
    fail(
      "state_authority_mismatch",
      "observer state refers to a missing live service",
    );
  }
  if (
    state.buckets.owner_service_account_id !== null &&
    state.storage_service_account !== null &&
    state.buckets.owner_service_account_id !== state.storage_service_account.id
  ) {
    fail(
      "state_authority_mismatch",
      "bucket ownership is bound to another service account",
    );
  }
}

async function inspectResources({ config, state, transport, adminToken }) {
  const source = await transport.request({ url: NYXID_SOURCE_REF_URL });
  validateSourceRef(source.json);
  const discoveryResponse = await transport.request({
    url: nyx("/.well-known/openid-configuration"),
  });
  const discovery = validateDiscovery(discoveryResponse.json);
  const openapi = await transport.request({
    url: nyx("/api/v1/docs/openapi.json"),
    token: adminToken,
  });
  validateNyxidOpenapi(openapi.json);
  const me = await transport.request({
    url: nyx("/api/v1/users/me"),
    token: adminToken,
  });
  validateMe(me.json);

  const servicesResponse = await transport.request({
    url: nyx("/api/v1/services"),
    token: adminToken,
  });
  exactKeys(
    servicesResponse.json,
    ["services"],
    "service list",
    "contract_drift",
  );
  if (!Array.isArray(servicesResponse.json.services)) {
    fail("contract_drift", "service list must be an array");
  }
  const services = servicesResponse.json.services.map((item, index) =>
    validateServiceItem(item, `services[${index}]`),
  );
  const observerMatches = services.filter(
    (item) => item.slug === config.observer.slug,
  );
  if (observerMatches.length > 1)
    fail("duplicate_resource", "observer slug is duplicated");
  const observer =
    observerMatches.length === 1
      ? validateObserver(observerMatches[0], config)
      : null;
  if (observer !== null && observer.created_by !== me.json.id) {
    fail(
      "observer_contract_drift",
      "observer is not owned by the exact administrator",
    );
  }
  const storageMatches = services.filter(
    (item) => item.slug === config.storage.service_slug,
  );
  if (storageMatches.length !== 1) {
    fail(
      "storage_contract_drift",
      "exactly one active chrono-storage service is required",
    );
  }
  const storageService = validateStorageService(storageMatches[0], config);
  const githubCatalogMatches = services.filter(
    (item) =>
      item.slug === GITHUB_CATALOG_SERVICE_SLUG && item.is_active === true,
  );
  if (githubCatalogMatches.length !== 1) {
    fail(
      "github_catalog_drift",
      "exactly one active api-github catalog service is required",
    );
  }
  const githubCatalogService = githubCatalogMatches[0];

  if (observer !== null) {
    const anonymous = await transport.request({
      url: nyx(`/api/v1/services/${observer.id}/anonymous-endpoints`),
      token: adminToken,
    });
    exactKeys(
      anonymous.json,
      ["endpoints"],
      "anonymous endpoint list",
      "contract_drift",
    );
    if (
      !Array.isArray(anonymous.json.endpoints) ||
      anonymous.json.endpoints.length !== 1
    ) {
      fail(
        "observer_contract_drift",
        "observer must expose exactly one anonymous rule",
      );
    }
    validateAnonymousRule(
      anonymous.json.endpoints[0],
      config,
      "anonymous endpoint list[0]",
    );
  }

  const rolesResponse = await transport.request({
    url: nyx("/api/v1/admin/roles"),
    token: adminToken,
  });
  exactKeys(rolesResponse.json, ["roles"], "role list", "contract_drift");
  if (!Array.isArray(rolesResponse.json.roles))
    fail("contract_drift", "roles must be an array");
  const roles = rolesResponse.json.roles.map((item, index) =>
    validateRole(item, `roles[${index}]`),
  );
  const roleMatches = roles.filter(
    (item) => item.slug === config.storage.role_slug,
  );
  if (roleMatches.length !== 1) {
    fail(
      "storage_role_drift",
      "exactly one chrono-bucket-user role is required",
    );
  }
  const storageRole = validateStorageRole(roleMatches[0], config);

  const appsResponse = await transport.request({
    url: nyx("/api/v1/developer/oauth-clients"),
    token: adminToken,
  });
  exactKeys(appsResponse.json, ["clients"], "OIDC app list", "contract_drift");
  if (!Array.isArray(appsResponse.json.clients))
    fail("contract_drift", "OIDC clients must be an array");
  const appMatches = appsResponse.json.clients.filter(
    (item) => item.client_name === config.oidc_app.name,
  );
  if (appMatches.length > 1)
    fail("duplicate_resource", "OIDC app name is duplicated");
  const app =
    appMatches.length === 1 ? validateApp(appMatches[0], config) : null;

  const accountSearch = encodeURIComponent(config.storage.service_account_name);
  const accountsResponse = await transport.request({
    url: nyx(
      `/api/v1/admin/service-accounts?search=${accountSearch}&per_page=100`,
    ),
    token: adminToken,
  });
  exactKeys(
    accountsResponse.json,
    ["service_accounts", "total", "page", "per_page"],
    "service account list",
    "contract_drift",
  );
  if (!Array.isArray(accountsResponse.json.service_accounts)) {
    fail("contract_drift", "service_accounts must be an array");
  }
  if (
    !Number.isSafeInteger(accountsResponse.json.total) ||
    accountsResponse.json.total !==
      accountsResponse.json.service_accounts.length ||
    accountsResponse.json.page !== 1 ||
    accountsResponse.json.per_page !== 100
  ) {
    fail(
      "contract_drift",
      "service-account search must return one complete unpaginated result set",
    );
  }
  for (const item of accountsResponse.json.service_accounts) {
    closedKeys(item, SA_ITEM_FIELDS, "service account item", SA_ITEM_FIELDS);
  }
  const accountMatches = accountsResponse.json.service_accounts.filter(
    (item) => item.name === config.storage.service_account_name,
  );
  if (accountMatches.length > 1) {
    fail("duplicate_resource", "storage service account name is duplicated");
  }
  const serviceAccount =
    accountMatches.length === 1
      ? validateServiceAccountItem(
          accountMatches[0],
          config,
          storageRole.id,
          me.json.id,
        )
      : null;

  const storageOpenapi = await transport.request({
    url: nyx(`/api/v1/proxy/s/${config.storage.service_slug}/openapi.json`),
    token: adminToken,
  });
  validateStorageOpenapi(storageOpenapi.json);

  const snapshot = {
    discovery,
    observer,
    app,
    serviceAccount,
    storageRole,
    storageService,
    githubCatalogService,
    adminSubjectId: me.json.id,
  };
  reconcileCustody(snapshot, state);
  return snapshot;
}

function provisioningPlan(snapshot, config) {
  const actions = [];
  if (snapshot.observer === null) actions.push("create_public_github_observer");
  if (snapshot.app === null) actions.push("create_confidential_oidc_app");
  if (snapshot.serviceAccount === null)
    actions.push("create_storage_service_account");
  actions.push(`verify_or_create_bucket:${config.storage.context_bucket}`);
  actions.push(
    `verify_or_create_bucket:${config.storage.trigger_workspace_bucket}`,
  );
  return actions;
}

function planEvidence(snapshot, config) {
  return {
    contract: {
      nyxid_base_url: NYXID_BASE_URL,
      nyxid_api_version: NYXID_API_VERSION,
      nyxid_source_repository: NYXID_SOURCE_REPOSITORY,
      nyxid_source_revision: NYXID_SOURCE_REVISION,
    },
    administrator: {
      subject_id: snapshot.adminSubjectId,
      authority: "active_platform_admin",
    },
    public_github_observer:
      snapshot.observer === null
        ? {
            status: "absent",
            target_slug: config.observer.slug,
            target_owner_subject_id: snapshot.adminSubjectId,
          }
        : {
            status: "present",
            id: snapshot.observer.id,
            slug: snapshot.observer.slug,
            owner_subject_id: snapshot.observer.created_by,
          },
    oidc_app:
      snapshot.app === null
        ? {
            status: "absent",
            target_name: config.oidc_app.name,
            target_owner_subject_id: snapshot.adminSubjectId,
          }
        : {
            status: "present",
            id: snapshot.app.id,
            name: snapshot.app.client_name,
            owner_subject_id: snapshot.adminSubjectId,
            owner_evidence: "administrator_scoped_developer_list",
          },
    storage_service_account:
      snapshot.serviceAccount === null
        ? {
            status: "absent",
            target_name: config.storage.service_account_name,
            target_owner_subject_id: snapshot.adminSubjectId,
          }
        : {
            status: "present",
            id: snapshot.serviceAccount.id,
            client_id: snapshot.serviceAccount.client_id,
            name: snapshot.serviceAccount.name,
            owner_subject_id: snapshot.serviceAccount.created_by,
          },
    storage_authority: {
      service_id: snapshot.storageService.id,
      service_slug: snapshot.storageService.slug,
      role_id: snapshot.storageRole.id,
      role_slug: snapshot.storageRole.slug,
      bucket_targets: [
        config.storage.context_bucket,
        config.storage.trigger_workspace_bucket,
      ],
      bucket_observation: "deferred_until_exact_service_account_custody",
    },
    github_authority: {
      catalog_service_id: snapshot.githubCatalogService.id,
      catalog_service_slug: snapshot.githubCatalogService.slug,
      consent_default: "preselected_but_user_reviewed",
      selected_user_service: "CMA_TRIGGER_SMOKE_SERVICE_ID",
    },
  };
}

function observerCreateBody(config) {
  return {
    name: config.observer.name,
    slug: config.observer.slug,
    description:
      "Credential-free CMA Trigger smoke repository metadata observer",
    service_type: "http",
    base_url: "https://api.github.com",
    auth_method: "none",
    service_category: "internal",
    visibility: "public",
    forward_access_token: false,
    anonymous_endpoints: [
      {
        id: config.observer.rule_id,
        enabled: true,
        method: "GET",
        path_pattern: "/repositories/**",
        daily_quota: config.observer.daily_quota,
      },
    ],
  };
}

function appCreateBody(config) {
  return {
    name: config.oidc_app.name,
    redirect_uris: config.oidc_app.redirect_uris,
    client_type: "confidential",
    allowed_scopes: [...CALLER_SCOPES],
    delegation_scopes: "",
    broker_capability_enabled: false,
    default_service_catalog_slugs: [GITHUB_CATALOG_SERVICE_SLUG],
  };
}

function serviceAccountCreateBody(config, roleId) {
  return {
    name: config.storage.service_account_name,
    description: "Dedicated non-production CMA Trigger smoke storage identity",
    allowed_scopes: STORAGE_ALLOWED_SCOPES.join(" "),
    role_ids: [roleId],
  };
}

async function mintStorageAccessToken({ transport, state }) {
  const account = state.storage_service_account;
  if (account === null)
    fail("storage_not_provisioned", "storage account is missing");
  const response = await transport.request({
    method: "POST",
    url: nyx("/oauth/token"),
    form: {
      grant_type: "client_credentials",
      client_id: account.client_id,
      client_secret: account.client_secret,
      scope: STORAGE_ALLOWED_SCOPES.join(" "),
    },
  });
  closedKeys(
    response.json,
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
    "storage token response",
    [
      "access_token",
      "token_type",
      "expires_in",
      "refresh_token",
      "id_token",
      "scope",
    ],
  );
  boundedString(response.json.access_token, "storage access token", 32_768);
  tokenSet(response.json.scope, STORAGE_ALLOWED_SCOPES, "storage token scope");
  if (
    response.json.token_type !== "Bearer" ||
    !Number.isSafeInteger(response.json.expires_in) ||
    response.json.expires_in < MIN_STORAGE_TOKEN_LIFETIME_SECONDS ||
    response.json.refresh_token !== null ||
    response.json.id_token !== null
  ) {
    fail("storage_token_drift", "storage token response drifted");
  }
  return response.json.access_token;
}

function validateErrorEnvelope(raw, path) {
  exactKeys(raw, ["data", "error"], path, "contract_drift");
  if (raw.data !== null) fail("contract_drift", `${path}.data must be null`);
  exactKeys(raw.error, ["code", "message"], `${path}.error`, "contract_drift");
  boundedString(raw.error.code, `${path}.error.code`, 128);
  boundedString(raw.error.message, `${path}.error.message`, 2048);
  return raw.error.code;
}

function validateBucketDetail(raw, bucket, ownerId) {
  exactKeys(raw, ["data", "error"], `bucket ${bucket}`, "contract_drift");
  if (raw.error !== null)
    fail("bucket_authority_drift", `bucket ${bucket} returned an error`);
  exactKeys(
    raw.data,
    ["name", "owner", "createdAt", "metadata", "objects"],
    `bucket ${bucket}.data`,
    "contract_drift",
  );
  if (
    raw.data.name !== bucket ||
    raw.data.owner !== ownerId ||
    typeof raw.data.createdAt !== "string" ||
    raw.data.metadata === null ||
    typeof raw.data.metadata !== "object" ||
    Array.isArray(raw.data.metadata) ||
    !Array.isArray(raw.data.objects)
  ) {
    fail(
      "bucket_authority_drift",
      `bucket ${bucket} is not owned by the exact service account`,
    );
  }
}

function validateBucketCreate(raw, bucket) {
  exactKeys(
    raw,
    ["data", "error"],
    `create bucket ${bucket}`,
    "contract_drift",
  );
  exactKeys(
    raw.data,
    ["name", "created"],
    `create bucket ${bucket}.data`,
    "contract_drift",
  );
  if (
    raw.error !== null ||
    raw.data.name !== bucket ||
    raw.data.created !== true
  ) {
    fail("bucket_contract_drift", `bucket ${bucket} create response drifted`);
  }
}

async function readBucket({ transport, config, token, bucket, ownerId }) {
  const response = await transport.request({
    url: nyx(
      `/api/v1/proxy/s/${config.storage.service_slug}/api/buckets/${bucket}`,
    ),
    token,
    statuses: [200, 401, 403, 404],
  });
  let errorCode = null;
  if (response.status === 200)
    validateBucketDetail(response.json, bucket, ownerId);
  else
    errorCode = validateErrorEnvelope(response.json, `bucket ${bucket} error`);
  if (response.status === 404 && errorCode !== "BUCKET_NOT_FOUND") {
    fail("contract_drift", `bucket ${bucket} returned an unknown absence code`);
  }
  return { ...response, errorCode };
}

async function verifyBucket(args) {
  const response = await readBucket(args);
  if (response.status !== 200) {
    fail(
      "bucket_not_ready",
      `bucket ${args.bucket} is not readable by the exact owner`,
    );
  }
}

async function ensureBucket(args) {
  // chrono-storage intentionally returns NOT_BUCKET_OWNER to an ordinary
  // caller for both an absent bucket and a foreign bucket. Use the reviewed
  // administrator only to establish absence or exact ownership, then require
  // every actual bucket operation to succeed as the dedicated service account.
  const observed = await readBucket({
    ...args,
    token: args.adminToken,
  });
  if (observed.status === 200) {
    const owned = await readBucket(args);
    if (owned.status !== 200) {
      fail(
        "bucket_authority_drift",
        `bucket ${args.bucket} is not readable by the exact owner`,
      );
    }
    return "existing";
  }
  if (observed.status === 401 || observed.status === 403) {
    fail(
      "bucket_authority_drift",
      `bucket ${args.bucket} could not be observed by the reviewed administrator`,
    );
  }
  let createResponse;
  let transportAmbiguous = false;
  try {
    createResponse = await args.transport.request({
      method: "POST",
      url: nyx(
        `/api/v1/proxy/s/${args.config.storage.service_slug}/api/buckets`,
      ),
      token: args.token,
      json: { name: args.bucket },
      statuses: [201, 400, 401, 403, 409, 500, 502, 503, 504],
    });
  } catch (error) {
    if (
      !(error instanceof BootstrapFailure) ||
      error.code !== "remote_transport_error"
    ) {
      throw error;
    }
    transportAmbiguous = true;
  }
  let createErrorCode = null;
  if (createResponse !== undefined && createResponse.status !== 201) {
    createErrorCode = validateErrorEnvelope(
      createResponse.json,
      `create bucket ${args.bucket} error`,
    );
  }
  if (
    createResponse?.status === 403 &&
    createErrorCode === "S3_ACCESS_DENIED"
  ) {
    fail(
      "bucket_provider_provisioning_required",
      `bucket ${args.bucket} is absent, but the backing storage role cannot provision it; create the exact non-production bucket through an authorized provider path, then assign it to the recorded service-account owner through the reviewed storage ownership procedure`,
    );
  }
  if (createResponse?.status === 401 || createResponse?.status === 403) {
    fail(
      "bucket_authority_drift",
      `bucket ${args.bucket} creation was forbidden`,
    );
  }
  if (createResponse?.status === 201)
    validateBucketCreate(createResponse.json, args.bucket);
  const reconciled = await readBucket(args);
  if (reconciled.status === 200) return "created_or_reconciled";
  fail(
    "bucket_creation_recovery_required",
    `bucket ${args.bucket} has no exact owner proof after ${transportAmbiguous ? "an ambiguous transport" : "creation"}`,
  );
}

async function applyProvisioning({
  config,
  state,
  transport,
  adminToken,
  persistState,
}) {
  const snapshot = await inspectResources({
    config,
    state,
    transport,
    adminToken,
  });
  const applied = [];
  let observer = snapshot.observer;
  if (observer === null) {
    const response = await transport.request({
      method: "POST",
      url: nyx("/api/v1/services"),
      token: adminToken,
      json: observerCreateBody(config),
    });
    observer = validateObserver(
      response.json,
      config,
      "created observer service",
    );
    if (observer.created_by !== snapshot.adminSubjectId) {
      fail("observer_contract_drift", "created observer has the wrong owner");
    }
    applied.push("created_public_github_observer");
  }
  if (state.observer === null) {
    state.observer = { id: observer.id, slug: observer.slug };
    await persistState(state);
  }

  let app = snapshot.app;
  if (app === null) {
    try {
      const response = await transport.request({
        method: "POST",
        url: nyx("/api/v1/developer/oauth-clients"),
        token: adminToken,
        json: appCreateBody(config),
      });
      app = validateApp(response.json, config, { secretRequired: true });
    } catch {
      fail(
        "oidc_secret_creation_recovery_required",
        "OIDC app creation did not return a proven one-time secret; do not retry automatically",
      );
    }
    state.oidc_app = { id: app.id, client_secret: app.client_secret };
    await persistState(state);
    applied.push("created_confidential_oidc_app");
  }

  let account = snapshot.serviceAccount;
  if (account === null) {
    try {
      const response = await transport.request({
        method: "POST",
        url: nyx("/api/v1/admin/service-accounts"),
        token: adminToken,
        json: serviceAccountCreateBody(config, snapshot.storageRole.id),
      });
      account = validateCreatedServiceAccount(
        response.json,
        config,
        snapshot.storageRole.id,
      );
    } catch {
      fail(
        "storage_secret_creation_recovery_required",
        "service-account creation did not return a proven one-time secret; do not retry automatically",
      );
    }
    state.storage_service_account = {
      id: account.id,
      client_id: account.client_id,
      client_secret: account.client_secret,
    };
    state.buckets.owner_service_account_id = account.id;
    await persistState(state);
    applied.push("created_storage_service_account");
  }
  if (state.buckets.owner_service_account_id === null) {
    state.buckets.owner_service_account_id = account.id;
    await persistState(state);
  }

  const storageToken = await mintStorageAccessToken({ transport, state });
  for (const bucket of [
    config.storage.context_bucket,
    config.storage.trigger_workspace_bucket,
  ]) {
    const result = await ensureBucket({
      transport,
      config,
      token: storageToken,
      adminToken,
      bucket,
      ownerId: state.storage_service_account.id,
    });
    if (result !== "existing")
      applied.push(`created_or_reconciled_bucket:${bucket}`);
  }
  return { state, applied };
}

function base64url(bytes) {
  return Buffer.from(bytes).toString("base64url");
}

function parseCallbackRequest(requestUrl, expected) {
  let url;
  try {
    url = new URL(requestUrl, `http://127.0.0.1:${expected.port}`);
  } catch {
    fail("oauth_callback_rejected", "OAuth callback URL is malformed");
  }
  if (url.pathname !== expected.path) {
    fail("oauth_callback_rejected", "OAuth callback path does not match");
  }
  if (
    url.protocol !== "http:" ||
    url.hostname !== "127.0.0.1" ||
    Number(url.port) !== expected.port
  ) {
    fail("oauth_callback_rejected", "OAuth callback authority does not match");
  }
  const keys = [...url.searchParams.keys()];
  if (new Set(keys).size !== keys.length) {
    fail(
      "oauth_callback_rejected",
      "OAuth callback contains duplicate parameters",
    );
  }
  const providerError = url.searchParams.get("error");
  if (providerError !== null) {
    const allowed = ["error", "error_description", "state"];
    if (keys.some((key) => !allowed.includes(key))) {
      fail(
        "oauth_callback_rejected",
        "OAuth error callback contains unknown parameters",
      );
    }
    if (url.searchParams.get("state") !== expected.state) {
      fail(
        "oauth_callback_rejected",
        "OAuth error callback state does not match",
      );
    }
    boundedString(providerError, "OAuth provider error", 128);
    fail(
      "oauth_provider_error",
      "NyxID rejected the smoke-caller authorization",
    );
  }
  if (keys.some((key) => !["code", "state"].includes(key))) {
    fail(
      "oauth_callback_rejected",
      "OAuth callback contains unknown parameters",
    );
  }
  if (url.searchParams.get("state") !== expected.state) {
    fail("oauth_callback_rejected", "OAuth callback state does not match");
  }
  return boundedString(
    url.searchParams.get("code"),
    "OAuth authorization code",
    4096,
  );
}

async function waitForAuthorizationCode(redirectUri, state) {
  const redirect = new URL(redirectUri);
  const port = Number(redirect.port);
  let settle;
  const result = new Promise((resolvePromise, rejectPromise) => {
    settle = { resolve: resolvePromise, reject: rejectPromise };
  });
  const server = http.createServer((request, response) => {
    try {
      const code = parseCallbackRequest(request.url ?? "", {
        port,
        path: redirect.pathname,
        state,
      });
      response.writeHead(200, {
        "content-type": "text/plain; charset=utf-8",
        "cache-control": "no-store",
        "referrer-policy": "no-referrer",
      });
      response.end("NyxID authorization received. You can close this tab.\n");
      settle.resolve(code);
      server.close();
    } catch (error) {
      const terminal =
        error instanceof BootstrapFailure &&
        error.code === "oauth_provider_error";
      response.writeHead(terminal ? 400 : 404, {
        "content-type": "text/plain; charset=utf-8",
        "cache-control": "no-store",
      });
      response.end("OAuth callback rejected.\n");
      if (terminal) {
        settle.reject(error);
        server.close();
      }
    }
  });
  await new Promise((resolvePromise, rejectPromise) => {
    server.once("error", rejectPromise);
    server.listen(port, "127.0.0.1", resolvePromise);
  });
  const timeout = setTimeout(() => {
    settle.reject(
      new BootstrapFailure(
        "oauth_callback_timeout",
        "OAuth callback timed out",
      ),
    );
    server.close();
  }, CALLBACK_TIMEOUT_MS);
  try {
    return await result;
  } finally {
    clearTimeout(timeout);
    server.close();
  }
}

async function openBrowser(url) {
  const choices =
    process.platform === "darwin"
      ? [["open", [url]]]
      : process.platform === "win32"
        ? [["cmd", ["/c", "start", "", url]]]
        : [["xdg-open", [url]]];
  for (const [command, args] of choices) {
    const opened = await new Promise((resolvePromise) => {
      const child = execFile(command, args, { windowsHide: true }, (error) =>
        resolvePromise(error === null),
      );
      child.unref();
    });
    if (opened) return true;
  }
  return false;
}

function validateTokenResponse(raw, { requireIdToken }) {
  const allowed = [
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
  ];
  closedKeys(raw, allowed, "caller token response", [
    "access_token",
    "token_type",
    "expires_in",
    "refresh_token",
    "id_token",
    "scope",
  ]);
  boundedString(raw.access_token, "caller access token", 32_768);
  boundedString(raw.refresh_token, "caller refresh token", 32_768);
  if (
    raw.token_type !== "Bearer" ||
    !Number.isSafeInteger(raw.expires_in) ||
    raw.expires_in < MIN_CALLER_TOKEN_LIFETIME_SECONDS ||
    raw.expires_in > MAX_CALLER_TOKEN_LIFETIME_SECONDS
  ) {
    fail("caller_token_drift", "caller token type or lifetime drifted");
  }
  if (requireIdToken) boundedString(raw.id_token, "caller ID token", 32_768);
  else if (raw.id_token !== null)
    fail("caller_token_drift", "refresh unexpectedly returned an ID token");
  if (raw.scope !== null && raw.scope !== undefined) {
    tokenSet(raw.scope, CALLER_SCOPES, "caller token scope");
  }
  return raw;
}

function parseJwt(token, path) {
  const parts = token.split(".");
  if (parts.length !== 3 || parts.some((part) => part.length === 0)) {
    fail("caller_token_invalid", `${path} is not a compact JWT`);
  }
  let header;
  let claims;
  try {
    header = JSON.parse(Buffer.from(parts[0], "base64url").toString("utf8"));
    claims = JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"));
  } catch {
    fail("caller_token_invalid", `${path} JWT JSON is malformed`);
  }
  exactKeys(
    header,
    ["typ", "alg", "kid"],
    `${path} header`,
    "caller_token_invalid",
  );
  if (header.typ !== "JWT" || header.alg !== "RS256") {
    fail("caller_token_invalid", `${path} JWT algorithm drifted`);
  }
  boundedString(header.kid, `${path} key id`, 256, "caller_token_invalid");
  return { parts, header, claims };
}

function validateJwks(raw) {
  exactKeys(raw, ["keys"], "JWKS", "contract_drift");
  if (!Array.isArray(raw.keys) || raw.keys.length === 0)
    fail("contract_drift", "JWKS is empty");
  for (const [index, key] of raw.keys.entries()) {
    exactKeys(
      key,
      ["kty", "use", "alg", "kid", "n", "e"],
      `JWKS key ${index}`,
      "contract_drift",
    );
    if (key.kty !== "RSA" || key.use !== "sig" || key.alg !== "RS256") {
      fail("contract_drift", "JWKS contains an unsupported signing key");
    }
  }
  return raw;
}

function verifyJwtWithJwks(token, jwks, path) {
  const parsed = parseJwt(token, path);
  const matches = jwks.keys.filter((key) => key.kid === parsed.header.kid);
  if (matches.length !== 1)
    fail("caller_token_invalid", `${path} key authority is ambiguous`);
  let valid = false;
  try {
    const key = createPublicKey({ key: matches[0], format: "jwk" });
    valid = verifySignature(
      "RSA-SHA256",
      Buffer.from(`${parsed.parts[0]}.${parsed.parts[1]}`),
      key,
      Buffer.from(parsed.parts[2], "base64url"),
    );
  } catch {
    fail("caller_token_invalid", `${path} signature could not be verified`);
  }
  if (!valid) fail("caller_token_invalid", `${path} signature is invalid`);
  return parsed.claims;
}

function validateAccessClaims(claims, appId, serviceId, nowSeconds) {
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
    "caller access claims",
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
    "caller_token_invalid",
  );
  uuid(claims.sub, "caller subject", "caller_token_invalid");
  uuid(claims.client_id, "caller client_id", "caller_token_invalid");
  uuid(serviceId, "selected GitHub UserService ID", "caller_token_invalid");
  if (
    !Array.isArray(claims.resources) ||
    claims.resources.length !== 0 ||
    !Array.isArray(claims.allowed_service_ids) ||
    claims.allowed_service_ids.length === 0 ||
    new Set(claims.allowed_service_ids).size !==
      claims.allowed_service_ids.length ||
    claims.allowed_service_ids.some(
      (id) => typeof id !== "string" || !UUID.test(id),
    ) ||
    !claims.allowed_service_ids.includes(serviceId) ||
    claims.allow_all_services !== false ||
    !Array.isArray(claims.allowed_node_ids) ||
    claims.allowed_node_ids.length !== 0 ||
    claims.allow_all_nodes !== true
  ) {
    fail(
      "caller_service_authority_denied",
      "caller token does not authorize the exact selected GitHub UserService",
    );
  }
  tokenSet(
    claims.scope,
    CALLER_SCOPES,
    "caller access scope",
    "caller_token_invalid",
  );
  if (
    claims.iss !== NYXID_BASE_URL ||
    claims.aud !== NYXID_BASE_URL ||
    claims.client_id !== appId ||
    claims.token_type !== "access" ||
    claims.delegated === true ||
    claims.sa === true ||
    claims.relay === true ||
    claims.assistant_forward === true ||
    !Number.isSafeInteger(claims.exp) ||
    !Number.isSafeInteger(claims.iat) ||
    claims.exp - nowSeconds < MIN_CALLER_TOKEN_LIFETIME_SECONDS ||
    claims.exp - claims.iat < MIN_CALLER_TOKEN_LIFETIME_SECONDS ||
    claims.exp - claims.iat > MAX_CALLER_TOKEN_LIFETIME_SECONDS ||
    claims.iat > nowSeconds + 30
  ) {
    fail(
      "caller_token_invalid",
      "caller access-token authority or lifetime drifted",
    );
  }
  return claims;
}

function validateIdClaims(
  claims,
  { appId, nonce, accessToken, subjectId, nowSeconds },
) {
  const allowed = [
    "sub",
    "iss",
    "aud",
    "exp",
    "iat",
    "email",
    "email_verified",
    "name",
    "picture",
    "nonce",
    "at_hash",
    "roles",
    "groups",
    "acr",
    "amr",
    "auth_time",
    "sid",
  ];
  closedKeys(
    claims,
    allowed,
    "caller ID claims",
    [
      "sub",
      "iss",
      "aud",
      "exp",
      "iat",
      "email",
      "email_verified",
      "name",
      "picture",
      "nonce",
      "at_hash",
    ],
    "caller_token_invalid",
  );
  const expectedAtHash = base64url(
    createHash("sha256").update(accessToken).digest().subarray(0, 16),
  );
  if (
    claims.sub !== subjectId ||
    claims.iss !== NYXID_BASE_URL ||
    claims.aud !== appId ||
    claims.nonce !== nonce ||
    claims.at_hash !== expectedAtHash ||
    !Number.isSafeInteger(claims.exp) ||
    !Number.isSafeInteger(claims.iat) ||
    claims.exp <= nowSeconds ||
    claims.iat > nowSeconds + 30
  ) {
    fail(
      "caller_token_invalid",
      "caller ID-token authority, nonce, or at_hash drifted",
    );
  }
}

function validateIntrospection(raw, claims, appId, nowSeconds) {
  const allowed = [
    "active",
    "scope",
    "client_id",
    "username",
    "token_type",
    "exp",
    "iat",
    "sub",
    "iss",
    "jti",
    "roles",
    "groups",
    "permissions",
    "resource",
  ];
  closedKeys(raw, allowed, "caller introspection", [
    "active",
    "scope",
    "token_type",
    "exp",
    "iat",
    "sub",
    "iss",
    "jti",
    "roles",
    "groups",
    "permissions",
  ]);
  tokenSet(raw.scope, CALLER_SCOPES, "introspected caller scope");
  if (
    raw.active !== true ||
    raw.token_type !== "access" ||
    raw.sub !== claims.sub ||
    raw.iss !== claims.iss ||
    raw.exp !== claims.exp ||
    raw.iat !== claims.iat ||
    raw.jti !== claims.jti ||
    (raw.client_id !== undefined &&
      raw.client_id !== null &&
      raw.client_id !== appId) ||
    raw.exp - nowSeconds < MIN_CALLER_TOKEN_LIFETIME_SECONDS
  ) {
    fail(
      "caller_introspection_failed",
      "caller token is inactive or has mismatched authority",
    );
  }
  if (
    !Array.isArray(raw.permissions) ||
    raw.permissions.length === 0 ||
    raw.permissions.some(
      (permission) => typeof permission !== "string" || permission === "",
    ) ||
    new Set(raw.permissions).size !== raw.permissions.length
  ) {
    fail(
      "caller_introspection_failed",
      "introspection permissions are incomplete",
    );
  }
  const permissions = new Set(raw.permissions);
  const sufficient =
    permissions.has("*") ||
    permissions.has("cma:*") ||
    CMA_REQUIRED_PERMISSIONS.every((permission) => permissions.has(permission));
  if (!sufficient) {
    fail(
      "caller_permission_denied",
      "smoke caller lacks complete own-session CMA authority",
    );
  }
  return {
    subjectId: raw.sub,
    expiresAt: raw.exp,
    permissions: raw.permissions,
  };
}

async function verifyCallerToken({
  transport,
  discovery,
  app,
  tokenResponse,
  nonce,
  serviceId,
}) {
  const jwksResponse = await transport.request({ url: discovery.jwks_uri });
  const jwks = validateJwks(jwksResponse.json);
  const nowSeconds = Math.floor(Date.now() / 1000);
  const accessClaims = validateAccessClaims(
    verifyJwtWithJwks(tokenResponse.access_token, jwks, "caller access token"),
    app.id,
    serviceId,
    nowSeconds,
  );
  if (nonce !== null) {
    const idClaims = verifyJwtWithJwks(
      tokenResponse.id_token,
      jwks,
      "caller ID token",
    );
    validateIdClaims(idClaims, {
      appId: app.id,
      nonce,
      accessToken: tokenResponse.access_token,
      subjectId: accessClaims.sub,
      nowSeconds,
    });
  }
  const introspection = await transport.request({
    method: "POST",
    url: discovery.introspection_endpoint,
    form: {
      token: tokenResponse.access_token,
      token_type_hint: "access_token",
      client_id: app.id,
      client_secret: app.client_secret,
    },
  });
  const verified = validateIntrospection(
    introspection.json,
    accessClaims,
    app.id,
    nowSeconds,
  );
  return {
    ...verified,
    allowedServiceIds: accessClaims.allowed_service_ids,
  };
}

async function mintCallerToken({
  config,
  state,
  snapshot,
  transport,
  persistState,
  outputPath,
  serviceId,
  noBrowser = false,
  authorize = null,
  verifier = verifyCallerToken,
}) {
  if (
    snapshot.observer === null ||
    snapshot.app === null ||
    snapshot.serviceAccount === null ||
    state.oidc_app === null ||
    state.storage_service_account === null
  ) {
    fail(
      "bootstrap_incomplete",
      "all NyxID smoke resources must exist before caller minting",
    );
  }
  if (state.caller !== null && state.caller.service_id !== serviceId) {
    fail(
      "caller_authority_drift",
      "stored refresh custody is bound to another selected UserService",
    );
  }
  const storageToken = await mintStorageAccessToken({ transport, state });
  for (const bucket of [
    config.storage.context_bucket,
    config.storage.trigger_workspace_bucket,
  ]) {
    await verifyBucket({
      transport,
      config,
      token: storageToken,
      bucket,
      ownerId: state.storage_service_account.id,
    });
  }

  const app = {
    id: state.oidc_app.id,
    client_secret: state.oidc_app.client_secret,
  };
  let tokenResponse;
  let nonce = null;
  if (state.caller !== null) {
    const refreshed = await transport.request({
      method: "POST",
      url: snapshot.discovery.token_endpoint,
      form: {
        grant_type: "refresh_token",
        refresh_token: state.caller.refresh_token,
        client_id: app.id,
        client_secret: app.client_secret,
      },
    });
    tokenResponse = validateTokenResponse(refreshed.json, {
      requireIdToken: false,
    });
  } else {
    const redirect = new URL(config.oidc_app.smoke_redirect_uri);
    const stateValue = base64url(randomBytes(32));
    nonce = base64url(randomBytes(32));
    const pkceVerifier = base64url(randomBytes(64));
    const challenge = base64url(
      createHash("sha256").update(pkceVerifier).digest(),
    );
    const authorizationUrl = new URL(snapshot.discovery.authorization_endpoint);
    authorizationUrl.search = new URLSearchParams({
      response_type: "code",
      client_id: app.id,
      redirect_uri: config.oidc_app.smoke_redirect_uri,
      scope: CALLER_SCOPES.join(" "),
      state: stateValue,
      nonce,
      code_challenge: challenge,
      code_challenge_method: "S256",
      prompt: "consent",
    }).toString();
    const codePromise = authorize
      ? authorize({
          authorizationUrl: authorizationUrl.href,
          state: stateValue,
          nonce,
        })
      : waitForAuthorizationCode(
          config.oidc_app.smoke_redirect_uri,
          stateValue,
        );
    if (!authorize && !noBrowser) {
      const opened = await openBrowser(authorizationUrl.href);
      if (!opened)
        process.stderr.write(
          `Open this NyxID authorization URL:\n${authorizationUrl.href}\n`,
        );
    } else if (!authorize) {
      process.stderr.write(
        `Open this NyxID authorization URL:\n${authorizationUrl.href}\n`,
      );
    }
    const code = await codePromise;
    const exchanged = await transport.request({
      method: "POST",
      url: snapshot.discovery.token_endpoint,
      form: {
        grant_type: "authorization_code",
        code,
        redirect_uri: redirect.href,
        client_id: app.id,
        client_secret: app.client_secret,
        code_verifier: pkceVerifier,
      },
    });
    tokenResponse = validateTokenResponse(exchanged.json, {
      requireIdToken: true,
    });
  }

  const verified = await verifier({
    transport,
    discovery: snapshot.discovery,
    app,
    tokenResponse,
    nonce,
    serviceId,
  });
  if (
    !Array.isArray(verified.allowedServiceIds) ||
    !verified.allowedServiceIds.includes(serviceId)
  ) {
    fail(
      "caller_service_authority_denied",
      "verified caller lacks the exact selected GitHub UserService authority",
    );
  }
  if (verified.subjectId !== snapshot.adminSubjectId) {
    fail(
      "caller_authority_drift",
      "smoke caller is not the exact administrator who reviewed provisioning",
    );
  }
  if (state.caller !== null && state.caller.subject_id !== verified.subjectId) {
    fail(
      "caller_authority_drift",
      "refreshed token changed the smoke caller subject",
    );
  }
  state.caller = {
    client_id: app.id,
    subject_id: verified.subjectId,
    service_id: serviceId,
    refresh_token: tokenResponse.refresh_token,
    scope: CALLER_SCOPES.join(" "),
  };
  await persistState(state);

  const handoff = {
    schema_version: 1,
    nyxid_base_url: config.nyxid_base_url,
    nyxid_client_id: app.id,
    nyxid_client_secret: app.client_secret,
    storage_client_id: state.storage_service_account.client_id,
    storage_client_secret: state.storage_service_account.client_secret,
    storage_owner_service_account_id: state.storage_service_account.id,
    context_bucket: config.storage.context_bucket,
    trigger_workspace_bucket: config.storage.trigger_workspace_bucket,
    public_github_service_slug: config.observer.slug,
    subject_id: verified.subjectId,
    access_token: tokenResponse.access_token,
    access_token_expires_at: verified.expiresAt,
  };
  validateHandoff(handoff);
  await atomicWriteSecureJson(outputPath, handoff);
  return {
    subjectId: verified.subjectId,
    expiresAt: verified.expiresAt,
    handoff,
  };
}

function validateHandoff(raw) {
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
  if (raw.schema_version !== 1 || raw.nyxid_base_url !== NYXID_BASE_URL) {
    fail("unsafe_handoff", "handoff contract identity drifted");
  }
  uuid(raw.nyxid_client_id, "handoff NyxID client ID", "unsafe_handoff");
  uuid(raw.subject_id, "handoff subject ID", "unsafe_handoff");
  if (!SA_CLIENT_ID.test(raw.storage_client_id)) {
    fail("unsafe_handoff", "handoff storage client ID is invalid");
  }
  uuid(
    raw.storage_owner_service_account_id,
    "handoff storage owner service-account ID",
    "unsafe_handoff",
  );
  for (const field of [
    "nyxid_client_secret",
    "storage_client_secret",
    "access_token",
  ]) {
    boundedString(raw[field], `handoff ${field}`, 32_768, "unsafe_handoff");
  }
  if (
    !Number.isSafeInteger(raw.access_token_expires_at) ||
    raw.access_token_expires_at - Math.floor(Date.now() / 1000) <
      MIN_CALLER_TOKEN_LIFETIME_SECONDS
  ) {
    fail(
      "unsafe_handoff",
      "handoff access token is not fresh enough for the smoke budget",
    );
  }
  for (const field of ["context_bucket", "trigger_workspace_bucket"]) {
    if (!BUCKET.test(raw[field]))
      fail("unsafe_handoff", `handoff ${field} is invalid`);
  }
  if (!SLUG.test(raw.public_github_service_slug)) {
    fail("unsafe_handoff", "handoff observer slug is invalid");
  }
  return raw;
}

function parseArguments(argv) {
  const modes = new Set([
    "--validate-config",
    "--plan",
    "--apply",
    "--mint-caller-token",
  ]);
  if (!modes.has(argv[0]) || argv.length < 2) {
    fail("usage", "expected a mode and config path");
  }
  const result = {
    mode: argv[0],
    configPath: resolve(argv[1]),
    statePath: null,
    outputPath: null,
    noBrowser: false,
  };
  for (let index = 2; index < argv.length; index += 1) {
    const argument = argv[index];
    if (argument === "--no-browser") {
      if (result.noBrowser) fail("usage", "--no-browser was repeated");
      result.noBrowser = true;
      continue;
    }
    if (
      !new Set(["--state", "--output"]).has(argument) ||
      index + 1 >= argv.length
    ) {
      fail("usage", `unknown or incomplete argument: ${argument}`);
    }
    const field = argument === "--state" ? "statePath" : "outputPath";
    if (result[field] !== null) fail("usage", `${argument} was repeated`);
    result[field] = argv[index + 1];
    index += 1;
  }
  if (result.mode === "--validate-config") {
    if (
      result.statePath !== null ||
      result.outputPath !== null ||
      result.noBrowser
    ) {
      fail("usage", "--validate-config accepts only a config path");
    }
  } else {
    if (result.statePath === null)
      fail("usage", `${result.mode} requires --state`);
    secureAbsolutePath(result.statePath, "--state");
  }
  if (result.mode === "--mint-caller-token") {
    if (result.outputPath === null)
      fail("usage", "--mint-caller-token requires --output");
    secureAbsolutePath(result.outputPath, "--output");
    if (result.outputPath === result.statePath)
      fail("usage", "state and handoff paths must differ");
  } else if (result.outputPath !== null || result.noBrowser) {
    fail(
      "usage",
      "--output/--no-browser are only valid for caller token minting",
    );
  }
  return result;
}

function adminTokenFromEnvironment() {
  return boundedString(
    process.env.CMA_TRIGGER_NYXID_ADMIN_TOKEN,
    "CMA_TRIGGER_NYXID_ADMIN_TOKEN",
    32_768,
    "admin_token_missing",
  );
}

function serviceIdFromEnvironment() {
  return uuid(
    process.env.CMA_TRIGGER_SMOKE_SERVICE_ID,
    "CMA_TRIGGER_SMOKE_SERVICE_ID",
    "service_id_missing",
  );
}

async function main(argv = process.argv.slice(2)) {
  const args = parseArguments(argv);
  const config = validateConfig(await readJsonFile(args.configPath));
  if (args.mode === "--validate-config") {
    process.stdout.write(
      `${JSON.stringify({ status: "valid", schema_version: 1, environment: config.environment })}\n`,
    );
    return;
  }
  const rawState = await readSecureJson(args.statePath, { optional: true });
  const state =
    rawState === null ? defaultState(config) : validateState(rawState, config);
  const transport = new FetchTransport();
  const adminToken = adminTokenFromEnvironment();
  const persistState = async (next) => {
    validateState(next, config);
    await atomicWriteSecureJson(args.statePath, next);
  };
  if (args.mode === "--plan") {
    const snapshot = await inspectResources({
      config,
      state,
      transport,
      adminToken,
    });
    process.stdout.write(
      `${JSON.stringify({ status: "planned", actions: provisioningPlan(snapshot, config), evidence: planEvidence(snapshot, config), remote_mutations: 0 })}\n`,
    );
    return;
  }
  if (args.mode === "--apply") {
    if (
      process.env.CMA_TRIGGER_NYXID_SMOKE_BOOTSTRAP_CONFIRM !==
      APPLY_CONFIRMATION
    ) {
      fail(
        "confirmation_required",
        "non-production apply confirmation is missing",
      );
    }
    const result = await applyProvisioning({
      config,
      state,
      transport,
      adminToken,
      persistState,
    });
    process.stdout.write(
      `${JSON.stringify({ status: "applied", resources: result.applied, state_path: args.statePath })}\n`,
    );
    return;
  }
  const snapshot = await inspectResources({
    config,
    state,
    transport,
    adminToken,
  });
  const result = await mintCallerToken({
    config,
    state,
    snapshot,
    transport,
    persistState,
    outputPath: args.outputPath,
    serviceId: serviceIdFromEnvironment(),
    noBrowser: args.noBrowser,
  });
  process.stdout.write(
    `${JSON.stringify({ status: "caller_token_ready", subject_id: result.subjectId, expires_at: result.expiresAt, handoff_path: args.outputPath })}\n`,
  );
}

const isMain =
  process.argv[1] !== undefined &&
  pathToFileURL(resolve(process.argv[1])).href === import.meta.url;
if (isMain) {
  main().catch((error) => {
    const code =
      error instanceof BootstrapFailure ? error.code : "unexpected_failure";
    const message =
      error instanceof BootstrapFailure
        ? error.message
        : "bootstrap failed without a safe diagnostic";
    process.stderr.write(
      `${JSON.stringify({ status: "failed", code, message })}\n`,
    );
    process.exitCode = 1;
  });
}

export {
  APPLY_CONFIRMATION,
  BootstrapFailure,
  CALLER_SCOPES,
  FetchTransport,
  GITHUB_CATALOG_SERVICE_SLUG,
  NYXID_API_VERSION,
  NYXID_BASE_URL,
  NYXID_SOURCE_REF_URL,
  NYXID_SOURCE_REVISION,
  SA_ITEM_FIELDS,
  SERVICE_RESPONSE_FIELDS,
  STORAGE_ALLOWED_SCOPES,
  STORAGE_ROLE_PERMISSIONS,
  applyProvisioning,
  atomicWriteSecureJson,
  defaultState,
  ensureBucket,
  inspectResources,
  mintCallerToken,
  parseArguments,
  parseCallbackRequest,
  planEvidence,
  provisioningPlan,
  readSecureJson,
  secureAbsolutePath,
  tokenSet,
  validateAccessClaims,
  validateConfig,
  validateHandoff,
  validateIntrospection,
  validateObserver,
  validateState,
};
