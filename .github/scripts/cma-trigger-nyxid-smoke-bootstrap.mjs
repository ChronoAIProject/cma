#!/usr/bin/env node

import {
  createHash,
  createPublicKey,
  randomBytes,
  verify as verifySignature,
} from "node:crypto";
import { execFile } from "node:child_process";
import { constants as fsConstants, existsSync } from "node:fs";
import {
  link,
  lstat,
  mkdir,
  open,
  readFile,
  realpath,
  rename,
  unlink,
} from "node:fs/promises";
import http from "node:http";
import { hostname } from "node:os";
import { basename, dirname, isAbsolute, join, relative, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const NYXID_BASE_URL = "https://nyx-api.chrono-ai.fun";
const NYXID_SOURCE_REPOSITORY = "ChronoAIProject/NyxID";
const NYXID_SOURCE_REVISION = "0243b5478b5d7b93220b1a5ef495776ee8d7dc41";
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
const BUCKET_ADOPTION_CONFIRMATION =
  "adopt-reviewed-empty-nonproduction-provider-buckets";
const OIDC_RETIREMENT_CONFIRMATION =
  "retire-reviewed-ambiguous-nonproduction-oidc-app";
const RESOURCE_CREATION_RECOVERY_CONFIRMATION =
  "rotate-reviewed-ambiguous-nonproduction-resource-secret";
const CONFIG_ENVIRONMENT = "cma-trigger-smoke";
const MAX_JSON_BYTES = 8 * 1024 * 1024;
const MAX_GITHUB_SECRET_PLAINTEXT_BYTES = 48 * 1024;
const MIN_STORAGE_TOKEN_LIFETIME_SECONDS = 120;
const MIN_CALLER_TOKEN_LIFETIME_SECONDS = 7_200;
const MAX_CALLER_TOKEN_LIFETIME_SECONDS = 28_800;
const CALLBACK_TIMEOUT_MS = 5 * 60 * 1000;
const ISSUER_TTL_WITNESS_MAX_AGE_SECONDS = 15 * 60;
const CALLER_STATE_LOCK_SUFFIX = ".caller-authority.lock";
const REFRESH_RETRYABLE_STATUSES = Object.freeze([409, 500, 502, 503, 504]);
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
    caller_exchange: null,
    resource_creation: null,
    handoff_binding: null,
  };
}

function validateState(raw, config) {
  closedKeys(
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
      "caller_exchange",
      "resource_creation",
      "handoff_binding",
    ],
    "state",
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
    "unsafe_state",
  );
  raw.caller_exchange ??= null;
  raw.resource_creation ??= null;
  raw.handoff_binding ??= null;
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
    closedKeys(
      raw.caller,
      [
        "client_id",
        "subject_id",
        "service_id",
        "refresh_token",
        "scope",
        "verification_state",
      ],
      "state.caller",
      ["client_id", "subject_id", "service_id", "refresh_token", "scope"],
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
    if (
      raw.caller.verification_state !== undefined &&
      !new Set(["pending", "verified"]).has(raw.caller.verification_state)
    ) {
      fail(
        "unsafe_state",
        "state.caller.verification_state is not a closed custody state",
      );
    }
    if (raw.oidc_app === null || raw.caller.client_id !== raw.oidc_app.id) {
      fail(
        "state_authority_mismatch",
        "caller refresh authority is not the exact OIDC app",
      );
    }
  }
  if (raw.caller_exchange !== null) {
    exactKeys(
      raw.caller_exchange,
      [
        "app_id",
        "exchange_started_at",
        "issuer_token_issued_at",
        "issuer_access_token_ttl_seconds",
        "issuer_token_sha256",
        "issuer_commit",
      ],
      "state.caller_exchange",
      "unsafe_state",
    );
    uuid(
      raw.caller_exchange.app_id,
      "state.caller_exchange.app_id",
      "unsafe_state",
    );
    for (const field of ["exchange_started_at", "issuer_token_issued_at"]) {
      if (
        !Number.isSafeInteger(raw.caller_exchange[field]) ||
        raw.caller_exchange[field] <= 0
      ) {
        fail("unsafe_state", `state.caller_exchange.${field} is invalid`);
      }
    }
    if (
      !Number.isSafeInteger(
        raw.caller_exchange.issuer_access_token_ttl_seconds,
      ) ||
      raw.caller_exchange.issuer_access_token_ttl_seconds <
        MIN_CALLER_TOKEN_LIFETIME_SECONDS ||
      raw.caller_exchange.issuer_access_token_ttl_seconds >
        MAX_CALLER_TOKEN_LIFETIME_SECONDS ||
      raw.caller_exchange.issuer_token_issued_at >
        raw.caller_exchange.exchange_started_at + 30 ||
      raw.caller_exchange.exchange_started_at -
        raw.caller_exchange.issuer_token_issued_at >
        ISSUER_TTL_WITNESS_MAX_AGE_SECONDS ||
      !/^[0-9a-f]{64}$/u.test(raw.caller_exchange.issuer_token_sha256) ||
      raw.caller_exchange.issuer_commit !== NYXID_SOURCE_REVISION.slice(0, 12)
    ) {
      fail("unsafe_state", "state caller exchange evidence is invalid");
    }
    if (
      raw.oidc_app === null ||
      raw.caller_exchange.app_id !== raw.oidc_app.id ||
      raw.caller !== null ||
      raw.handoff_binding !== null
    ) {
      fail(
        "state_authority_mismatch",
        "caller exchange evidence is not isolated to the exact OIDC app",
      );
    }
  }
  if (raw.resource_creation !== null) {
    raw.resource_creation.recovery_rotation ??= null;
    exactKeys(
      raw.resource_creation,
      [
        "kind",
        "name",
        "administrator_subject_id",
        "attempt_started_at",
        "attempt_nonce",
        "request_sha256",
        "recovery_rotation",
      ],
      "state.resource_creation",
      "unsafe_state",
    );
    const expectedName = {
      oidc_app: config.oidc_app.name,
      storage_service_account: config.storage.service_account_name,
    }[raw.resource_creation.kind];
    if (expectedName === undefined || raw.resource_creation.name !== expectedName) {
      fail(
        "state_authority_mismatch",
        "resource creation marker is not bound to an exact configured resource",
      );
    }
    uuid(
      raw.resource_creation.administrator_subject_id,
      "state.resource_creation.administrator_subject_id",
      "unsafe_state",
    );
    if (
      !Number.isSafeInteger(raw.resource_creation.attempt_started_at) ||
      raw.resource_creation.attempt_started_at <= 0 ||
      !/^[0-9a-f]{64}$/u.test(raw.resource_creation.attempt_nonce) ||
      !/^[0-9a-f]{64}$/u.test(raw.resource_creation.request_sha256)
    ) {
      fail("unsafe_state", "resource creation marker evidence is invalid");
    }
    if (raw.resource_creation.recovery_rotation !== null) {
      exactKeys(
        raw.resource_creation.recovery_rotation,
        ["resource_id", "rotation_started_at", "rotation_nonce"],
        "state.resource_creation.recovery_rotation",
        "unsafe_state",
      );
      uuid(
        raw.resource_creation.recovery_rotation.resource_id,
        "state.resource_creation.recovery_rotation.resource_id",
        "unsafe_state",
      );
      if (
        !Number.isSafeInteger(
          raw.resource_creation.recovery_rotation.rotation_started_at,
        ) ||
        raw.resource_creation.recovery_rotation.rotation_started_at <= 0 ||
        !/^[0-9a-f]{64}$/u.test(
          raw.resource_creation.recovery_rotation.rotation_nonce,
        )
      ) {
        fail("unsafe_state", "resource recovery rotation evidence is invalid");
      }
    }
    if (
      raw.handoff_binding !== null ||
      (raw.resource_creation.kind === "oidc_app" && raw.oidc_app !== null) ||
      (raw.resource_creation.kind === "storage_service_account" &&
        (raw.storage_service_account !== null ||
          raw.buckets.owner_service_account_id !== null))
    ) {
      fail(
        "state_authority_mismatch",
        "resource creation marker is not isolated from completed custody",
      );
    }
  }
  if (raw.handoff_binding !== null) {
    exactKeys(
      raw.handoff_binding,
      ["sha256"],
      "state.handoff_binding",
      "unsafe_state",
    );
    if (!/^[0-9a-f]{64}$/u.test(raw.handoff_binding.sha256)) {
      fail("unsafe_state", "state handoff binding digest is invalid");
    }
    if (
      raw.caller === null ||
      (raw.caller.verification_state ?? "verified") !== "verified"
    ) {
      fail(
        "unsafe_state",
        "state handoff binding requires verified caller custody",
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
  secureAbsolutePath(path, "secret parent");
  const stat = await lstat(path);
  let actual;
  try {
    actual = await realpath(path);
  } catch {
    fail("unsafe_secret_path", "secret parent could not be resolved safely");
  }
  if (actual !== path) {
    fail(
      "unsafe_secret_path",
      "secret parent must not traverse any symbolic-link component",
    );
  }
  let ancestor = path;
  while (true) {
    const ancestorStat = await lstat(ancestor);
    if (!ancestorStat.isDirectory() || ancestorStat.isSymbolicLink()) {
      fail("unsafe_secret_path", "secret path ancestry is not all directories");
    }
    if ((ancestorStat.mode & 0o022) !== 0) {
      const ownerTrusted =
        typeof process.getuid !== "function" ||
        ancestorStat.uid === 0 ||
        ancestorStat.uid === process.getuid();
      if ((ancestorStat.mode & 0o1000) === 0 || !ownerTrusted) {
        fail(
          "unsafe_secret_path",
          "secret path ancestry contains an untrusted writable directory",
        );
      }
    }
    const parent = dirname(ancestor);
    if (parent === ancestor) break;
    ancestor = parent;
  }
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

async function canonicalAuthorityTarget(path, label) {
  secureAbsolutePath(path, label);
  const parent = dirname(path);
  await verifyPrivateDirectory(parent);
  return join(await realpath(parent), basename(path));
}

async function existingAuthorityIdentity(path, label) {
  try {
    const stat = await lstat(path);
    if (stat.isSymbolicLink()) {
      fail("unsafe_secret_path", `${label} must not be a symbolic link`);
    }
    return {
      dev: stat.dev,
      ino: stat.ino,
      resolved: await realpath(path),
    };
  } catch (error) {
    if (error?.code === "ENOENT") return null;
    throw error;
  }
}

function authorityPathKey(path) {
  return path.normalize("NFC").toLowerCase();
}

async function requireDistinctAuthorityPaths(statePath, outputPath) {
  const stateTarget = await canonicalAuthorityTarget(statePath, "--state");
  const outputTarget = await canonicalAuthorityTarget(outputPath, "--output");
  const lockTarget = `${stateTarget}${CALLER_STATE_LOCK_SUFFIX}`;
  const outputKey = authorityPathKey(outputTarget);
  if (
    outputKey === authorityPathKey(stateTarget) ||
    outputKey === authorityPathKey(lockTarget)
  ) {
    fail("usage", "output path must differ from state and lock paths");
  }
  const outputIdentity = await existingAuthorityIdentity(
    outputTarget,
    "--output",
  );
  if (outputIdentity !== null) {
    for (const [target, label] of [
      [stateTarget, "--state"],
      [lockTarget, "caller state lock"],
    ]) {
      const identity = await existingAuthorityIdentity(target, label);
      if (
        identity !== null &&
        (identity.resolved === outputIdentity.resolved ||
          (identity.dev === outputIdentity.dev &&
            identity.ino === outputIdentity.ino))
      ) {
        fail("usage", "output path aliases state or lock authority");
      }
    }
  }
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

async function readSecureBytes(path, { optional = false } = {}) {
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
  let handle;
  try {
    handle = await open(path, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const opened = await handle.stat();
    if (
      !opened.isFile() ||
      opened.dev !== stat.dev ||
      opened.ino !== stat.ino ||
      opened.nlink !== 1 ||
      opened.size !== stat.size ||
      (typeof process.getuid === "function" &&
        opened.uid !== process.getuid()) ||
      (opened.mode & 0o777) !== 0o600
    ) {
      fail("unsafe_secret_file", "secret file changed before its secure read");
    }
    const bytes = Buffer.alloc(opened.size);
    const { bytesRead } = await handle.read(bytes, 0, opened.size, 0);
    if (bytesRead !== opened.size) {
      fail("unsafe_secret_file", "secret file changed during its secure read");
    }
    const after = await handle.stat();
    if (
      after.dev !== opened.dev ||
      after.ino !== opened.ino ||
      after.size !== opened.size ||
      after.mtimeMs !== opened.mtimeMs ||
      after.nlink !== 1
    ) {
      fail("unsafe_secret_file", "secret file changed during its secure read");
    }
    await verifyPrivateDirectory(parent);
    const final = await lstat(path);
    if (final.dev !== opened.dev || final.ino !== opened.ino) {
      fail("unsafe_secret_file", "secret file was replaced during its secure read");
    }
    return bytes;
  } finally {
    await handle?.close();
  }
}

async function readSecureJson(path, options = {}) {
  const bytes = await readSecureBytes(path, options);
  if (bytes === null) return null;
  let parsed;
  try {
    parsed = JSON.parse(bytes.toString("utf8"));
  } catch {
    fail("unsafe_secret_file", "secret file is not valid JSON");
  }
  return parsed;
}

async function atomicWriteSecureBytes(path, bytes) {
  secureAbsolutePath(path, "secret file path");
  if (!Buffer.isBuffer(bytes) || bytes.length > MAX_JSON_BYTES) {
    fail("unsafe_secret_file", "secret output exceeds the size limit");
  }
  const parent = dirname(path);
  await ensurePrivateDirectory(parent);
  if (existsSync(path)) await verifySecureFile(path);
  const temp = `${path}.tmp-${process.pid}-${randomBytes(8).toString("hex")}`;
  let handle;
  let renamed = false;
  try {
    handle = await open(
      temp,
      fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL,
      0o600,
    );
    await handle.writeFile(bytes);
    await handle.sync();
    await handle.close();
    handle = undefined;
    await rename(temp, path);
    renamed = true;
    const directory = await open(parent, fsConstants.O_RDONLY);
    try {
      await directory.sync();
    } finally {
      await directory.close();
    }
    await verifySecureFile(path);
  } finally {
    await handle?.close().catch(() => {});
    if (!renamed) await unlink(temp).catch(() => {});
  }
}

function secureJsonBytes(value) {
  return Buffer.from(`${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function requireHandoffUploadBound(handoffBytes) {
  if (!Buffer.isBuffer(handoffBytes) || handoffBytes.length === 0) {
    fail("unsafe_handoff", "handoff artifact bytes are invalid");
  }
  if (
    Buffer.byteLength(handoffBytes.toString("base64"), "utf8") >
    MAX_GITHUB_SECRET_PLAINTEXT_BYTES
  ) {
    fail(
      "handoff_upload_too_large",
      "base64-encoded handoff exceeds GitHub's 48 KiB secret plaintext limit",
    );
  }
  return handoffBytes;
}

async function atomicWriteSecureJson(path, value) {
  await atomicWriteSecureBytes(path, secureJsonBytes(value));
}

function validateCallerStateLock(raw) {
  exactKeys(
    raw,
    ["schema_version", "pid", "hostname", "nonce"],
    "caller state lock",
    "caller_state_lock_invalid",
  );
  if (
    raw.schema_version !== 1 ||
    !Number.isSafeInteger(raw.pid) ||
    raw.pid <= 0
  ) {
    fail("caller_state_lock_invalid", "caller state lock identity is invalid");
  }
  boundedString(
    raw.hostname,
    "caller state lock hostname",
    1024,
    "caller_state_lock_invalid",
  );
  if (!/^[0-9a-f]{64}$/u.test(raw.nonce)) {
    fail("caller_state_lock_invalid", "caller state lock nonce is invalid");
  }
  return raw;
}

async function readCallerStateLock(path) {
  const expected = await lstat(path);
  let handle;
  try {
    handle = await open(path, fsConstants.O_RDONLY | fsConstants.O_NOFOLLOW);
    const opened = await handle.stat();
    if (
      !expected.isFile() ||
      expected.isSymbolicLink() ||
      !opened.isFile() ||
      opened.dev !== expected.dev ||
      opened.ino !== expected.ino ||
      opened.nlink < 1 ||
      opened.nlink > 2 ||
      opened.size <= 0 ||
      opened.size > MAX_JSON_BYTES ||
      (typeof process.getuid === "function" &&
        opened.uid !== process.getuid()) ||
      (opened.mode & 0o777) !== 0o600
    ) {
      fail("caller_state_lock_invalid", "caller state lock file is invalid");
    }
    const bytes = Buffer.alloc(opened.size);
    const { bytesRead } = await handle.read(bytes, 0, opened.size, 0);
    const after = await handle.stat();
    const final = await lstat(path);
    if (
      bytesRead !== opened.size ||
      after.dev !== opened.dev ||
      after.ino !== opened.ino ||
      after.size !== opened.size ||
      after.mtimeMs !== opened.mtimeMs ||
      final.dev !== opened.dev ||
      final.ino !== opened.ino
    ) {
      fail(
        "caller_state_lock_invalid",
        "caller state lock changed during its secure read",
      );
    }
    let parsed;
    try {
      parsed = JSON.parse(bytes.toString("utf8"));
    } catch {
      fail("caller_state_lock_invalid", "caller state lock payload is invalid");
    }
    return {
      value: validateCallerStateLock(parsed),
      identity: { dev: opened.dev, ino: opened.ino },
    };
  } catch {
    throw new BootstrapFailure(
      "caller_state_lock_invalid",
      "caller state lock could not be read through a stable file handle",
    );
  } finally {
    await handle?.close().catch(() => {});
  }
}

function processIsActive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error?.code !== "ESRCH";
  }
}

async function withCallerStateLock(statePath, action) {
  secureAbsolutePath(statePath, "--state");
  const parent = dirname(statePath);
  await ensurePrivateDirectory(parent);
  const lockPath = `${statePath}${CALLER_STATE_LOCK_SUFFIX}`;
  secureAbsolutePath(lockPath, "caller state lock");
  const localHostname = hostname();
  const candidatePath = `${lockPath}.candidate-${process.pid}-${randomBytes(8).toString("hex")}`;
  let handle;
  const lock = {
    schema_version: 1,
    pid: process.pid,
    hostname: localHostname,
    nonce: randomBytes(32).toString("hex"),
  };
  let published = false;
  let publishedIdentity;
  try {
    handle = await open(
      candidatePath,
      fsConstants.O_WRONLY | fsConstants.O_CREAT | fsConstants.O_EXCL,
      0o600,
    );
    await handle.writeFile(`${JSON.stringify(lock)}\n`, "utf8");
    await handle.sync();
    await link(candidatePath, lockPath);
    published = true;
    await unlink(candidatePath);
    const acquired = await readCallerStateLock(lockPath);
    const held = await handle.stat();
    if (
      acquired.value.pid !== lock.pid ||
      acquired.value.hostname !== lock.hostname ||
      acquired.value.nonce !== lock.nonce ||
      acquired.identity.dev !== held.dev ||
      acquired.identity.ino !== held.ino
    ) {
      fail(
        "caller_state_lock_invalid",
        "published caller state lock identity drifted before the action",
      );
    }
    publishedIdentity = acquired.identity;
    const directory = await open(parent, fsConstants.O_RDONLY);
    try {
      await directory.sync();
    } finally {
      await directory.close();
    }
  } catch (error) {
    let heldIdentity;
    try {
      const held = await handle?.stat();
      if (held !== undefined) {
        heldIdentity = { dev: held.dev, ino: held.ino };
      }
    } catch {
      heldIdentity = undefined;
    }
    await handle?.close().catch(() => {});
    handle = undefined;
    await unlink(candidatePath).catch(() => {});
    if (published && heldIdentity !== undefined) {
      try {
        const current = await lstat(lockPath);
        if (
          current.dev === heldIdentity.dev &&
          current.ino === heldIdentity.ino
        ) {
          await unlink(lockPath);
        }
      } catch {
        // Retaining an uncertain lock is safer than unlinking another owner.
      }
    }
    if (error?.code !== "EEXIST") throw error;
    const { value: existing } = await readCallerStateLock(lockPath);
    if (
      existing.hostname === localHostname &&
      !processIsActive(existing.pid)
    ) {
      fail(
        "caller_state_lock_stale",
        "a dead-owner caller state lock requires reviewed exact-file removal",
      );
    }
    fail(
      "caller_state_locked",
      "another caller-authority operation owns the state file",
    );
  }

  let result;
  let primaryError;
  try {
    result = await action();
  } catch (error) {
    primaryError = error;
  }
  let releaseError;
  try {
    const held = await handle.stat();
    const current = await readCallerStateLock(lockPath);
    if (
      current.value.pid !== lock.pid ||
      current.value.hostname !== lock.hostname ||
      current.value.nonce !== lock.nonce ||
      current.identity.dev !== publishedIdentity.dev ||
      current.identity.ino !== publishedIdentity.ino ||
      held.dev !== publishedIdentity.dev ||
      held.ino !== publishedIdentity.ino
    ) {
      fail(
        "caller_state_lock_release_failed",
        "caller state lock ownership changed before release",
      );
    }
    await handle.close();
    handle = undefined;
    const final = await lstat(lockPath);
    if (
      final.dev !== publishedIdentity.dev ||
      final.ino !== publishedIdentity.ino
    ) {
      fail(
        "caller_state_lock_release_failed",
        "caller state lock path changed before exact release",
      );
    }
    await unlink(lockPath);
    const directory = await open(parent, fsConstants.O_RDONLY);
    try {
      await directory.sync();
    } finally {
      await directory.close();
    }
  } catch (error) {
    await handle?.close().catch(() => {});
    releaseError = error;
  }
  if (releaseError !== undefined) {
    if (releaseError instanceof BootstrapFailure) throw releaseError;
    fail(
      "caller_state_lock_release_failed",
      "caller state lock could not be released safely",
    );
  }
  if (primaryError !== undefined) throw primaryError;
  return result;
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

async function readBoundedBody(response, { allowMissing = false } = {}) {
  const declared = Number(response.headers.get("content-length"));
  if (Number.isFinite(declared) && declared > MAX_JSON_BYTES) {
    fail("remote_response_too_large", "remote JSON exceeds the response limit");
  }
  if (response.body === null) {
    if (allowMissing) return Buffer.alloc(0);
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
  return Buffer.concat(chunks);
}

async function readBoundedResponse(response) {
  const body = await readBoundedBody(response);
  const contentType = response.headers.get("content-type") ?? "";
  if (!contentType.toLowerCase().includes("application/json")) {
    fail("remote_contract_error", "remote response is not JSON");
  }
  try {
    return JSON.parse(body.toString("utf8"));
  } catch {
    fail("remote_contract_error", "remote response is malformed JSON");
  }
}

async function readEmptyResponse(response) {
  const body = await readBoundedBody(response, { allowMissing: true });
  if (body.length !== 0) {
    fail("remote_contract_error", "remote empty response carried a body");
  }
}

class FetchTransport {
  constructor({ timeoutMs = 20_000 } = {}) {
    this.timeoutMs = timeoutMs;
  }

  async request({
    method = "GET",
    url,
    token,
    json,
    form,
    statuses = [200],
    responseKind = "json",
  }) {
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
    if (!new Set(["json", "empty"]).has(responseKind)) {
      fail("internal_error", "request response kind is invalid");
    }
    const responseJson =
      responseKind === "json"
        ? await readBoundedResponse(response)
        : (await readEmptyResponse(response), null);
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
    revocation_endpoint: nyx("/oauth/revoke"),
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

function validateApp(
  raw,
  config,
  { secretRequired = false, expectedActive = true } = {},
) {
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
    typeof expectedActive !== "boolean" ||
    raw.is_active !== expectedActive
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

function validateHealth(raw) {
  closedKeys(
    raw,
    ["status", "version", "commit", "encryption"],
    "NyxID health",
    ["status", "version", "commit", "encryption"],
  );
  if (
    raw.status !== "ok" ||
    raw.version !== NYXID_API_VERSION ||
    raw.commit !== NYXID_SOURCE_REVISION.slice(0, 12) ||
    raw.encryption === null ||
    typeof raw.encryption !== "object" ||
    Array.isArray(raw.encryption)
  ) {
    fail("nyxid_deployment_drift", "live NyxID deployment is not the pinned build");
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
    "/api/buckets/{bucket}/ownership": ["put"],
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

function reconcileCustody(snapshot, state, config) {
  const creation = state.resource_creation;
  if (creation !== null) {
    if (creation.administrator_subject_id !== snapshot.adminSubjectId) {
      fail(
        "state_authority_mismatch",
        "resource creation marker administrator differs from live authority",
      );
    }
    const expectedRequest =
      creation.kind === "oidc_app"
        ? appCreateBody(config)
        : serviceAccountCreateBody(config, snapshot.storageRole.id);
    if (creation.request_sha256 !== sha256(canonicalJson(expectedRequest))) {
      fail(
        "state_authority_mismatch",
        "resource creation marker request contract drifted",
      );
    }
  }
  if (snapshot.app !== null) {
    if (state.oidc_app === null) {
      if (creation?.kind !== "oidc_app") {
        fail(
          "oidc_secret_rotation_required",
          "the OIDC app exists without marker-bound local secret custody; quarantine it and obtain provider final-outcome evidence before a separately reviewed recovery",
        );
      }
    } else if (state.oidc_app.id !== snapshot.app.id) {
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
      if (creation?.kind !== "storage_service_account") {
        fail(
          "storage_secret_rotation_required",
          "the storage service account exists without marker-bound local secret custody; quarantine it and obtain provider final-outcome evidence before a separately reviewed recovery",
        );
      }
    } else if (
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
  const health = await transport.request({ url: nyx("/health") });
  validateHealth(health.json);
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
  const validatedAppMatches = appMatches.map((item) =>
    validateApp(item, config, { expectedActive: item.is_active }),
  );
  const activeAppMatches = validatedAppMatches.filter(
    (item) => item.is_active === true,
  );
  if (activeAppMatches.length > 1)
    fail("duplicate_resource", "OIDC app name is duplicated");
  const app =
    activeAppMatches.length === 1 ? activeAppMatches[0] : null;

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
    liveCommit: health.json.commit,
    adminSubjectId: me.json.id,
    resourceCreation: state.resource_creation,
  };
  reconcileCustody(snapshot, state, config);
  return snapshot;
}

function provisioningPlan(snapshot, config) {
  if (snapshot.resourceCreation !== null) {
    return [
      `recover_ambiguous_resource_creation:${snapshot.resourceCreation.kind}:${snapshot.resourceCreation.name}`,
    ];
  }
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
    resource_creation_incident:
      snapshot.resourceCreation === null
        ? null
        : {
            kind: snapshot.resourceCreation.kind,
            name: snapshot.resourceCreation.name,
            administrator_subject_id:
              snapshot.resourceCreation.administrator_subject_id,
            attempt_started_at: snapshot.resourceCreation.attempt_started_at,
            attempt_nonce: snapshot.resourceCreation.attempt_nonce,
            request_sha256: snapshot.resourceCreation.request_sha256,
            recovery_rotation: snapshot.resourceCreation.recovery_rotation,
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

function resourceCreationRecoveryCode(kind) {
  return kind === "oidc_app"
    ? "oidc_secret_creation_recovery_required"
    : "storage_secret_creation_recovery_required";
}

function requireNoPendingResourceCreation(state) {
  if (state.resource_creation !== null) {
    fail(
      resourceCreationRecoveryCode(state.resource_creation.kind),
      `the durable ${state.resource_creation.kind} create-attempt marker is unresolved; do not send another create request`,
    );
  }
}

async function beginResourceCreation({
  state,
  persistState,
  kind,
  name,
  administratorSubjectId,
  request,
}) {
  requireNoPendingResourceCreation(state);
  const marker = {
    kind,
    name,
    administrator_subject_id: administratorSubjectId,
    attempt_started_at: Math.floor(Date.now() / 1000),
    attempt_nonce: randomBytes(32).toString("hex"),
    request_sha256: sha256(canonicalJson(request)),
    recovery_rotation: null,
  };
  state.resource_creation = marker;
  try {
    await persistState(state);
  } catch {
    state.resource_creation = null;
    fail(
      "resource_creation_fence_persistence_failed",
      `the ${kind} create request was not sent because its durable attempt fence could not be committed`,
    );
  }
  return marker;
}

function validateRotatedOidcSecret(raw, resourceId) {
  exactKeys(
    raw,
    ["id", "client_secret"],
    "rotated OIDC client secret",
    "contract_drift",
  );
  if (raw.id !== resourceId) {
    fail("state_authority_mismatch", "rotated OIDC client ID drifted");
  }
  boundedString(
    raw.client_secret,
    "rotated OIDC client secret",
    4096,
    "contract_drift",
  );
  return raw.client_secret;
}

function validateRotatedStorageSecret(raw, account) {
  exactKeys(
    raw,
    ["client_id", "client_secret", "secret_prefix", "message"],
    "rotated storage service-account secret",
    "contract_drift",
  );
  if (
    raw.client_id !== account.client_id ||
    raw.message !==
      "Secret rotated. All existing tokens have been revoked. Save the new secret now."
  ) {
    fail(
      "state_authority_mismatch",
      "rotated storage service-account identity drifted",
    );
  }
  boundedString(
    raw.client_secret,
    "rotated storage client secret",
    4096,
    "contract_drift",
  );
  boundedString(
    raw.secret_prefix,
    "rotated storage secret prefix",
    256,
    "contract_drift",
  );
  return raw.client_secret;
}

async function readExactResourceCreationTarget({
  config,
  marker,
  snapshot,
  transport,
  adminToken,
  resourceId,
}) {
  const listed =
    marker.kind === "oidc_app" ? snapshot.app : snapshot.serviceAccount;
  if (listed === null) {
    fail(
      "resource_creation_observation_pending",
      "the ambiguous create target is not yet observable; retain the marker and do not rotate or recreate it",
    );
  }
  if (listed.id !== resourceId) {
    fail(
      "resource_creation_recovery_target_mismatch",
      "the independently reviewed provider resource ID does not match the exact observed target",
    );
  }
  const path =
    marker.kind === "oidc_app"
      ? `/api/v1/developer/oauth-clients/${resourceId}`
      : `/api/v1/admin/service-accounts/${resourceId}`;
  const response = await transport.request({
    url: nyx(path),
    token: adminToken,
    statuses: [200, 401, 403, 404],
  });
  if (response.status !== 200) {
    fail(
      "resource_creation_recovery_target_unavailable",
      "the exact reviewed provider resource ID could not be read through its owner authority",
    );
  }
  const exact =
    marker.kind === "oidc_app"
      ? validateApp(response.json, config, { expectedActive: true })
      : validateServiceAccountItem(
          response.json,
          config,
          snapshot.storageRole.id,
          snapshot.adminSubjectId,
        );
  if (exact.id !== resourceId) {
    fail(
      "resource_creation_recovery_target_mismatch",
      "exact provider read returned another resource identity",
    );
  }
  return exact;
}

async function recoverResourceCreation({
  config,
  state,
  snapshot,
  transport,
  adminToken,
  persistState,
  resourceId,
  attemptNonce,
}) {
  const marker = state.resource_creation;
  if (marker === null) {
    fail(
      "resource_creation_recovery_not_required",
      "bootstrap state has no ambiguous one-time-secret resource creation",
    );
  }
  if (marker.attempt_nonce !== attemptNonce) {
    fail(
      "resource_creation_recovery_confirmation_mismatch",
      "the reviewed create-attempt nonce does not match authoritative state",
    );
  }
  if (marker.recovery_rotation !== null) {
    fail(
      resourceCreationRecoveryCode(marker.kind),
      "an exact secret-rotation attempt is already fenced; inspect provider evidence and never send a second automatic rotation",
    );
  }
  reconcileCustody(snapshot, state, config);
  const target = await readExactResourceCreationTarget({
    config,
    marker,
    snapshot,
    transport,
    adminToken,
    resourceId,
  });
  marker.recovery_rotation = {
    resource_id: resourceId,
    rotation_started_at: Math.floor(Date.now() / 1000),
    rotation_nonce: randomBytes(32).toString("hex"),
  };
  try {
    await persistState(state);
  } catch {
    marker.recovery_rotation = null;
    fail(
      "resource_creation_recovery_fence_persistence_failed",
      "the exact secret rotation was not sent because its durable recovery fence could not be committed",
    );
  }

  let secret;
  try {
    const response = await transport.request({
      method: "POST",
      url: nyx(
        marker.kind === "oidc_app"
          ? `/api/v1/developer/oauth-clients/${resourceId}/rotate-secret`
          : `/api/v1/admin/service-accounts/${resourceId}/rotate-secret`,
      ),
      token: adminToken,
    });
    secret =
      marker.kind === "oidc_app"
        ? validateRotatedOidcSecret(response.json, resourceId)
        : validateRotatedStorageSecret(response.json, target);
  } catch {
    fail(
      resourceCreationRecoveryCode(marker.kind),
      "the exact secret-rotation attempt did not return proven custody; retain the recovery fence and never retry automatically",
    );
  }

  try {
    await readExactResourceCreationTarget({
      config,
      marker,
      snapshot,
      transport,
      adminToken,
      resourceId,
    });
  } catch {
    fail(
      resourceCreationRecoveryCode(marker.kind),
      "the exact resource did not remain active and owner-bound after rotation; retain the recovery fence and never retry automatically",
    );
  }

  if (marker.kind === "oidc_app") {
    state.oidc_app = { id: resourceId, client_secret: secret };
  } else {
    state.storage_service_account = {
      id: resourceId,
      client_id: target.client_id,
      client_secret: secret,
    };
    state.buckets.owner_service_account_id = resourceId;
  }
  state.resource_creation = null;
  try {
    await persistState(state);
  } catch {
    if (marker.kind === "oidc_app") {
      state.oidc_app = null;
    } else {
      state.storage_service_account = null;
      state.buckets.owner_service_account_id = null;
    }
    state.resource_creation = marker;
    fail(
      resourceCreationRecoveryCode(marker.kind),
      "rotated one-time secret custody was not durably acknowledged; retain the recovery fence and never retry automatically",
    );
  }
  return {
    status: "resource_creation_recovered",
    kind: marker.kind,
    resource_id: resourceId,
    custody_persisted: true,
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

function validateBucketOwnership(raw, bucket, ownerId) {
  exactKeys(
    raw,
    ["data", "error"],
    `adopt bucket ${bucket}`,
    "contract_drift",
  );
  exactKeys(
    raw.data,
    ["name", "owner", "updatedAt"],
    `adopt bucket ${bucket}.data`,
    "contract_drift",
  );
  if (
    raw.error !== null ||
    raw.data.name !== bucket ||
    raw.data.owner !== ownerId ||
    typeof raw.data.updatedAt !== "string"
  ) {
    fail(
      "bucket_adoption_contract_drift",
      `bucket ${bucket} ownership response drifted`,
    );
  }
}

function requireEmptyAdoptedBucket(response, bucket) {
  if (
    Object.keys(response.json.data.metadata).length !== 0 ||
    response.json.data.objects.length !== 0
  ) {
    fail(
      "bucket_adoption_not_empty",
      `provider-adopted bucket ${bucket} is not empty; quarantine it and require provider-owner recovery`,
    );
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

async function adoptProviderBucket(args) {
  const observed = await readBucket({ ...args, token: args.adminToken });
  if (observed.status === 200) {
    const owned = await readBucket(args);
    if (owned.status !== 200) {
      fail(
        "bucket_authority_drift",
        `bucket ${args.bucket} is not readable by the exact recorded owner`,
      );
    }
    requireEmptyAdoptedBucket(owned, args.bucket);
    return "existing";
  }
  if (observed.status !== 404) {
    fail(
      "bucket_authority_drift",
      `bucket ${args.bucket} could not be proven unowned by the reviewed administrator`,
    );
  }

  let response;
  let transportAmbiguous = false;
  try {
    response = await args.transport.request({
      method: "PUT",
      url: nyx(
        `/api/v1/proxy/s/${args.config.storage.service_slug}/api/buckets/${args.bucket}/ownership`,
      ),
      token: args.adminToken,
      json: { owner: args.ownerId },
      statuses: [200, 400, 401, 403, 404, 409, 500, 502, 503, 504],
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

  let responseError = null;
  if (response?.status === 200) {
    validateBucketOwnership(response.json, args.bucket, args.ownerId);
  } else if (response !== undefined) {
    responseError = validateErrorEnvelope(
      response.json,
      `adopt bucket ${args.bucket} error`,
    );
  }

  const owned = await readBucket(args);
  if (owned.status === 200) {
    requireEmptyAdoptedBucket(owned, args.bucket);
    return transportAmbiguous
      ? "adopted_after_ambiguous_response"
      : "adopted";
  }
  if (response?.status === 404 && responseError === "BUCKET_NOT_FOUND") {
    fail(
      "bucket_provider_provisioning_required",
      `exact provider bucket ${args.bucket} is still absent`,
    );
  }
  if (response?.status === 401 || response?.status === 403) {
    fail(
      "bucket_adoption_forbidden",
      `administrator authority cannot adopt exact bucket ${args.bucket}`,
    );
  }
  fail(
    "bucket_adoption_recovery_required",
    `bucket ${args.bucket} ownership could not be reconciled after ${transportAmbiguous ? "an ambiguous transfer" : "the reviewed transfer"}`,
  );
}

async function adoptProviderBuckets({
  config,
  state,
  transport,
  adminToken,
  persistState,
}) {
  requireSettledCallerExchange(state);
  requireNoPendingResourceCreation(state);
  await invalidateHandoffBinding({ state, persistState });
  const snapshot = await inspectResources({
    config,
    state,
    transport,
    adminToken,
  });
  if (
    snapshot.serviceAccount === null ||
    state.storage_service_account === null ||
    state.buckets.owner_service_account_id !== state.storage_service_account.id
  ) {
    fail(
      "storage_not_provisioned",
      "exact storage service-account custody must exist before provider-bucket adoption",
    );
  }
  const storageToken = await mintStorageAccessToken({ transport, state });
  const adopted = [];
  for (const bucket of [
    config.storage.context_bucket,
    config.storage.trigger_workspace_bucket,
  ]) {
    const result = await adoptProviderBucket({
      transport,
      config,
      token: storageToken,
      adminToken,
      bucket,
      ownerId: state.storage_service_account.id,
    });
    adopted.push({ bucket, result });
  }
  return { state, adopted };
}

async function applyProvisioning({
  config,
  state,
  transport,
  adminToken,
  persistState,
}) {
  requireSettledCallerExchange(state);
  requireNoPendingResourceCreation(state);
  await invalidateHandoffBinding({ state, persistState });
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
    const request = appCreateBody(config);
    const marker = await beginResourceCreation({
      state,
      persistState,
      kind: "oidc_app",
      name: config.oidc_app.name,
      administratorSubjectId: snapshot.adminSubjectId,
      request,
    });
    try {
      const response = await transport.request({
        method: "POST",
        url: nyx("/api/v1/developer/oauth-clients"),
        token: adminToken,
        json: request,
      });
      app = validateApp(response.json, config, { secretRequired: true });
    } catch {
      fail(
        "oidc_secret_creation_recovery_required",
        "OIDC app creation did not return a proven one-time secret; do not retry automatically",
      );
    }
    state.oidc_app = { id: app.id, client_secret: app.client_secret };
    state.resource_creation = null;
    try {
      await persistState(state);
    } catch {
      state.oidc_app = null;
      state.resource_creation = marker;
      fail(
        "oidc_secret_creation_recovery_required",
        "OIDC app creation returned a one-time secret, but exact custody was not durably acknowledged; retain the create-attempt marker and do not retry automatically",
      );
    }
    applied.push("created_confidential_oidc_app");
  }

  let account = snapshot.serviceAccount;
  if (account === null) {
    const request = serviceAccountCreateBody(config, snapshot.storageRole.id);
    const marker = await beginResourceCreation({
      state,
      persistState,
      kind: "storage_service_account",
      name: config.storage.service_account_name,
      administratorSubjectId: snapshot.adminSubjectId,
      request,
    });
    try {
      const response = await transport.request({
        method: "POST",
        url: nyx("/api/v1/admin/service-accounts"),
        token: adminToken,
        json: request,
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
    state.resource_creation = null;
    try {
      await persistState(state);
    } catch {
      state.storage_service_account = null;
      state.buckets.owner_service_account_id = null;
      state.resource_creation = marker;
      fail(
        "storage_secret_creation_recovery_required",
        "service-account creation returned a one-time secret, but exact custody was not durably acknowledged; retain the create-attempt marker and do not retry automatically",
      );
    }
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

function validateIssuerTtlWitness(raw, at = Math.floor(Date.now() / 1000)) {
  exactKeys(
    raw,
    [
      "issuer_token_issued_at",
      "issuer_access_token_ttl_seconds",
      "issuer_token_sha256",
      "issuer_commit",
    ],
    "issuer TTL witness",
    "issuer_ttl_witness_invalid",
  );
  if (
    !Number.isSafeInteger(raw.issuer_token_issued_at) ||
    raw.issuer_token_issued_at <= 0 ||
    !Number.isSafeInteger(raw.issuer_access_token_ttl_seconds) ||
    raw.issuer_access_token_ttl_seconds < MIN_CALLER_TOKEN_LIFETIME_SECONDS ||
    raw.issuer_access_token_ttl_seconds > MAX_CALLER_TOKEN_LIFETIME_SECONDS ||
    !/^[0-9a-f]{64}$/u.test(raw.issuer_token_sha256) ||
    raw.issuer_commit !== NYXID_SOURCE_REVISION.slice(0, 12) ||
    raw.issuer_token_issued_at > at + 30 ||
    at - raw.issuer_token_issued_at > ISSUER_TTL_WITNESS_MAX_AGE_SECONDS
  ) {
    fail(
      "issuer_ttl_witness_invalid",
      "a fresh accepted issuer token with a bounded deployment TTL is required before code exchange",
    );
  }
  return raw;
}

function captureIssuerTtlWitness(
  token,
  expectedSubjectId,
  at = Math.floor(Date.now() / 1000),
) {
  boundedString(token, "NyxID administrator token", 32_768, "issuer_ttl_witness_invalid");
  uuid(
    expectedSubjectId,
    "issuer TTL witness administrator subject",
    "issuer_ttl_witness_invalid",
  );
  const parts = token.split(".");
  let claims;
  try {
    if (parts.length !== 3 || parts.some((part) => part.length === 0)) throw new Error();
    claims = JSON.parse(Buffer.from(parts[1], "base64url").toString("utf8"));
  } catch {
    fail("issuer_ttl_witness_invalid", "administrator token is not a compact JWT");
  }
  if (
    claims === null ||
    typeof claims !== "object" ||
    Array.isArray(claims) ||
    claims.sub !== expectedSubjectId ||
    claims.iss !== NYXID_BASE_URL ||
    claims.aud !== NYXID_BASE_URL ||
    claims.token_type !== "access" ||
    Object.hasOwn(claims, "client_id") ||
    claims.act !== undefined ||
    claims.delegated !== undefined ||
    claims.sa !== undefined ||
    claims.relay !== undefined ||
    claims.assistant_forward !== undefined ||
    claims.relay_api_key_id !== undefined ||
    claims.relay_api_key_name !== undefined ||
    claims.relay_allowed_service_ids !== undefined ||
    claims.relay_allowed_node_ids !== undefined ||
    claims.relay_allow_all_services !== undefined ||
    claims.relay_allow_all_nodes !== undefined ||
    !Number.isSafeInteger(claims.iat) ||
    !Number.isSafeInteger(claims.exp) ||
    claims.iat > at + 30 ||
    claims.exp <= at
  ) {
    fail(
      "issuer_ttl_witness_invalid",
      "administrator token does not carry current first-party access-token evidence",
    );
  }
  return validateIssuerTtlWitness(
    {
      issuer_token_issued_at: claims.iat,
      issuer_access_token_ttl_seconds: claims.exp - claims.iat,
      issuer_token_sha256: sha256(token),
      issuer_commit: NYXID_SOURCE_REVISION.slice(0, 12),
    },
    at,
  );
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
    claims.allowed_service_ids.length !== 1 ||
    new Set(claims.allowed_service_ids).size !==
      claims.allowed_service_ids.length ||
    claims.allowed_service_ids.some(
      (id) => typeof id !== "string" || !UUID.test(id),
    ) ||
    claims.allowed_service_ids[0] !== serviceId ||
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

function assertCallerBootstrap(state, snapshot, serviceId) {
  if (
    snapshot.observer === null ||
    snapshot.app === null ||
    snapshot.serviceAccount === null ||
    state.oidc_app === null ||
    state.storage_service_account === null
  ) {
    fail(
      "bootstrap_incomplete",
      "all NyxID smoke resources must exist before caller authorization or handoff minting",
    );
  }
  if (state.caller !== null && state.caller.service_id !== serviceId) {
    fail(
      "caller_authority_drift",
      "stored refresh custody is bound to another selected UserService",
    );
  }
  if (
    state.caller !== null &&
    (state.caller.client_id !== state.oidc_app.id ||
      state.caller.subject_id !== snapshot.adminSubjectId)
  ) {
    fail(
      "caller_authority_drift",
      "stored refresh custody is not bound to the exact reviewed caller and application",
    );
  }
}

async function invalidateHandoffBinding({ state, persistState }) {
  if (state.handoff_binding === null) return;
  const retained = state.handoff_binding;
  state.handoff_binding = null;
  try {
    await persistState(state);
  } catch {
    state.handoff_binding = retained;
    fail(
      "handoff_invalidation_failed",
      "the prior caller handoff binding could not be invalidated; no authority work was attempted",
    );
  }
}

function retryableRefreshFailure(error) {
  return (
    error instanceof BootstrapFailure &&
    new Set([
      "caller_refresh_ambiguous",
      "caller_token_drift",
      "contract_drift",
      "remote_contract_error",
      "remote_response_too_large",
      "remote_status_error",
      "remote_transport_error",
    ]).has(error.code)
  );
}

function validateOAuthError(raw, label) {
  exactKeys(
    raw,
    ["error", "error_description"],
    label,
    "remote_contract_error",
  );
  boundedString(raw.error, `${label}.error`, 128, "remote_contract_error");
  boundedString(
    raw.error_description,
    `${label}.error_description`,
    2048,
    "remote_contract_error",
  );
  if (!/^[a-z][a-z0-9_]*$/u.test(raw.error)) {
    fail("remote_contract_error", `${label}.error is not canonical`);
  }
  return raw;
}

function extractRefreshReplacement(raw) {
  object(raw, "caller refresh response", "remote_contract_error");
  return boundedString(
    raw.refresh_token,
    "caller refresh replacement custody",
    16_384,
    "remote_contract_error",
  );
}

async function refreshCallerToken({
  transport,
  discovery,
  app,
  refreshToken,
  retainReplacement = null,
}) {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    try {
      const response = await transport.request({
        method: "POST",
        url: discovery.token_endpoint,
        form: {
          grant_type: "refresh_token",
          refresh_token: refreshToken,
          client_id: app.id,
          client_secret: app.client_secret,
        },
        statuses: [200, 400, 401, ...REFRESH_RETRYABLE_STATUSES],
      });
      if (response.status === 200) {
        let replacementRetained = false;
        let replacement;
        try {
          replacement = extractRefreshReplacement(response.json);
          if (retainReplacement !== null) {
            await retainReplacement(replacement);
            replacementRetained = true;
          }
          return validateTokenResponse(response.json, { requireIdToken: false });
        } catch (error) {
          if (replacementRetained) {
            fail(
              "caller_refresh_response_recovery_required",
              "NyxID rotated caller custody but the remaining token response drifted; replacement custody was retained as pending and no predecessor retry was attempted",
            );
          }
          throw error;
        }
      }
      if (REFRESH_RETRYABLE_STATUSES.includes(response.status)) {
        fail(
          "caller_refresh_ambiguous",
          "caller refresh returned a retryable or ambiguous response",
        );
      }
      const oauthError = validateOAuthError(
        response.json,
        "caller refresh OAuth error",
      );
      if (response.status === 401 && oauthError.error === "invalid_client") {
        fail(
          "caller_refresh_inactive",
          "stored caller refresh custody is no longer accepted",
        );
      }
      fail(
        "caller_refresh_ambiguous",
        "caller refresh did not match the pinned inactive-error contract",
      );
    } catch (error) {
      if (!retryableRefreshFailure(error)) throw error;
      if (attempt === 0) continue;
      fail(
        "caller_refresh_recovery_required",
        "caller refresh remained ambiguous after one bounded automatic retry; retry authorization immediately within NyxID's replacement-chain grace",
      );
    }
  }
  fail("internal_error", "caller refresh retry loop did not terminate");
}

async function persistPendingCallerReplacement({
  state,
  persistState,
  retained,
  refreshToken,
}) {
  state.caller = {
    ...retained,
    refresh_token: refreshToken,
    verification_state: "pending",
  };
  try {
    await persistState(state);
  } catch {
    state.caller = retained;
    fail(
      "caller_custody_persistence_failed",
      "NyxID issued replacement caller custody but local persistence failed; retry immediately within the bounded replacement-chain grace; no handoff was written",
    );
  }
}

async function revokeAndClearCallerCustody({
  state,
  snapshot,
  transport,
  persistState,
  app,
}) {
  if (state.caller === null) return;
  const retained = { ...state.caller };
  let replacement;
  let replacementRetained = false;
  try {
    await transport.request({
      method: "POST",
      url: snapshot.discovery.revocation_endpoint,
      form: {
        token: retained.refresh_token,
        token_type_hint: "refresh_token",
        client_id: app.id,
        client_secret: app.client_secret,
      },
      responseKind: "empty",
    });
    try {
      replacement = await refreshCallerToken({
        transport,
        discovery: snapshot.discovery,
        app,
        refreshToken: retained.refresh_token,
        retainReplacement: async (refreshToken) => {
          await persistPendingCallerReplacement({
            state,
            persistState,
            retained,
            refreshToken,
          });
          replacementRetained = true;
        },
      });
    } catch (error) {
      if (
        error instanceof BootstrapFailure &&
        error.code === "caller_refresh_inactive"
      ) {
        replacement = null;
      } else {
        throw error;
      }
    }
  } catch (error) {
    if (
      error instanceof BootstrapFailure &&
      new Set([
        "caller_custody_persistence_failed",
        "caller_reauthorization_recovery_required",
        "caller_refresh_response_recovery_required",
      ]).has(error.code)
    ) {
      throw error;
    }
    fail(
      "caller_reauthorization_recovery_required",
      "caller refresh custody could not be proven revoked; pending state was retained",
    );
  }
  if (replacement !== null) {
    if (!replacementRetained) {
      await persistPendingCallerReplacement({
        state,
        persistState,
        retained,
        refreshToken: replacement.refresh_token,
      });
    }
    fail(
      "caller_reauthorization_recovery_required",
      "caller revocation did not take effect; replacement custody was retained as pending and no fresh consent was opened",
    );
  }
  state.caller = null;
  try {
    await persistState(state);
  } catch {
    state.caller = retained;
    fail(
      "caller_reauthorization_recovery_required",
      "revoked caller custody could not be cleared from local state",
    );
  }
}

async function authorizeCallerCustody({
  config,
  state,
  snapshot,
  transport,
  persistState,
  serviceId,
  noBrowser = false,
  authorize = null,
  verifier = verifyCallerToken,
  issuerTtlWitness = null,
  reauthorizationAttempt = false,
  handoffInvalidated = false,
}) {
  requireSettledCallerExchange(state);
  requireNoPendingResourceCreation(state);
  if (!handoffInvalidated) {
    await invalidateHandoffBinding({ state, persistState });
    handoffInvalidated = true;
  }
  assertCallerBootstrap(state, snapshot, serviceId);
  const app = {
    id: state.oidc_app.id,
    client_secret: state.oidc_app.client_secret,
  };
  const hadCallerCustody = state.caller !== null;
  let refreshCustodyPersisted = false;
  let tokenResponse;
  let nonce = null;
  if (state.caller !== null) {
    try {
      const retained = { ...state.caller };
      tokenResponse = await refreshCallerToken({
        transport,
        discovery: snapshot.discovery,
        app,
        refreshToken: retained.refresh_token,
        retainReplacement: async (refreshToken) => {
          await persistPendingCallerReplacement({
            state,
            persistState,
            retained,
            refreshToken,
          });
          refreshCustodyPersisted = true;
        },
      });
    } catch (error) {
      if (
        !(error instanceof BootstrapFailure) ||
        error.code !== "caller_refresh_inactive"
      ) {
        throw error;
      }
      await revokeAndClearCallerCustody({
        state,
        snapshot,
        transport,
        persistState,
        app,
      });
      if (reauthorizationAttempt) {
        fail(
          "caller_reauthorization_required",
          "caller authority remained invalid after one automatic reauthorization",
        );
      }
      return authorizeCallerCustody({
        config,
        state,
        snapshot,
        transport,
        persistState,
        serviceId,
        noBrowser,
        authorize,
        verifier,
        issuerTtlWitness,
        reauthorizationAttempt: true,
        handoffInvalidated,
      });
    }
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
    const exchangeStartedAt = Math.floor(Date.now() / 1000);
    const witness = validateIssuerTtlWitness(
      typeof issuerTtlWitness === "function"
        ? await issuerTtlWitness()
        : issuerTtlWitness,
      exchangeStartedAt,
    );
    state.caller_exchange = {
      app_id: app.id,
      exchange_started_at: exchangeStartedAt,
      ...witness,
    };
    try {
      await persistState(state);
    } catch {
      state.caller_exchange = null;
      fail(
        "caller_exchange_fence_persistence_failed",
        "authorization code was not exchanged because its durable incident fence could not be committed",
      );
    }
    try {
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
    } catch {
      fail(
        "caller_authorization_exchange_recovery_required",
        "authorization-code exchange did not produce safely retained custody; retire the exact staging OIDC app and retain the durable exchange evidence until the provider proves an issuance fence",
      );
    }
  }

  // NyxID rotates refresh tokens before the caller can verify the returned
  // access token. Retain the replacement first so a transient JWKS or
  // introspection failure cannot strand local custody on the revoked token.
  const expectedCaller = state.caller ?? {
    client_id: app.id,
    subject_id: snapshot.adminSubjectId,
    service_id: serviceId,
    scope: CALLER_SCOPES.join(" "),
  };
  if (!refreshCustodyPersisted) {
    const exchangeEvidence = state.caller_exchange;
    state.caller = {
      ...expectedCaller,
      refresh_token: tokenResponse.refresh_token,
      verification_state: "pending",
    };
    state.caller_exchange = null;
    try {
      await persistState(state);
    } catch {
      state.caller = null;
      state.caller_exchange = exchangeEvidence;
      if (!hadCallerCustody) {
        fail(
          "caller_authorization_exchange_recovery_required",
          "authorization-code exchange may have issued unretained custody; retire the exact staging OIDC app and retain the durable exchange evidence until the provider proves an issuance fence",
        );
      }
      fail(
        "caller_custody_persistence_failed",
        "NyxID issued replacement caller custody but local persistence failed; retry immediately within the bounded replacement-chain grace; no handoff was written",
      );
    }
  }

  let verified;
  try {
    verified = await verifier({
      transport,
      discovery: snapshot.discovery,
      app,
      tokenResponse,
      nonce,
      serviceId,
    });
    if (
      !Array.isArray(verified.allowedServiceIds) ||
      verified.allowedServiceIds.length !== 1 ||
      verified.allowedServiceIds[0] !== serviceId
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
    if (state.caller.subject_id !== verified.subjectId) {
      fail(
        "caller_authority_drift",
        "refreshed token changed the smoke caller subject",
      );
    }
  } catch (error) {
    if (
      !(error instanceof BootstrapFailure) ||
      !new Set([
        "caller_authority_drift",
        "caller_service_authority_denied",
      ]).has(error.code)
    ) {
      throw error;
    }
    await revokeAndClearCallerCustody({
      state,
      snapshot,
      transport,
      persistState,
      app,
    });
    if (reauthorizationAttempt) {
      fail(
        "caller_reauthorization_required",
        "caller authority remained invalid after one automatic reauthorization",
      );
    }
    return authorizeCallerCustody({
      config,
      state,
      snapshot,
      transport,
      persistState,
      serviceId,
      noBrowser,
      authorize,
      verifier,
      issuerTtlWitness,
      reauthorizationAttempt: true,
      handoffInvalidated,
    });
  }
  state.caller = {
    client_id: app.id,
    subject_id: verified.subjectId,
    service_id: serviceId,
    refresh_token: tokenResponse.refresh_token,
    scope: CALLER_SCOPES.join(" "),
    verification_state: "verified",
  };
  try {
    await persistState(state);
  } catch {
    fail(
      "caller_verification_persistence_failed",
      "verified caller custody remains persisted as pending; rerun authorization to complete recovery; no handoff was written",
    );
  }

  return {
    subjectId: verified.subjectId,
    expiresAt: verified.expiresAt,
    accessToken: tokenResponse.access_token,
  };
}

async function mintCallerToken(args) {
  const {
    config,
    state,
    snapshot,
    transport,
    persistState,
    outputPath,
    serviceId,
  } = args;
  requireSettledCallerExchange(state);
  requireNoPendingResourceCreation(state);
  await invalidateHandoffBinding({ state, persistState });
  await atomicWriteSecureJson(outputPath, {
    schema_version: 1,
    kind: "cma-trigger-handoff-invalid",
    status: "mint_in_progress",
  });
  assertCallerBootstrap(state, snapshot, serviceId);
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

  const authorized = await authorizeCallerCustody({
    ...args,
    handoffInvalidated: true,
  });

  const handoff = {
    schema_version: 1,
    nyxid_base_url: config.nyxid_base_url,
    nyxid_client_id: state.oidc_app.id,
    nyxid_client_secret: state.oidc_app.client_secret,
    storage_client_id: state.storage_service_account.client_id,
    storage_client_secret: state.storage_service_account.client_secret,
    storage_owner_service_account_id: state.storage_service_account.id,
    context_bucket: config.storage.context_bucket,
    trigger_workspace_bucket: config.storage.trigger_workspace_bucket,
    public_github_service_slug: config.observer.slug,
    subject_id: authorized.subjectId,
    access_token: authorized.accessToken,
    access_token_expires_at: authorized.expiresAt,
  };
  validateHandoff(handoff);
  const handoffBytes = requireHandoffUploadBound(secureJsonBytes(handoff));
  state.handoff_binding = { sha256: sha256(handoffBytes) };
  try {
    await persistState(state);
  } catch {
    state.handoff_binding = null;
    fail(
      "handoff_binding_persistence_failed",
      "the fresh handoff binding could not be committed; the invalid output marker remains unusable",
    );
  }
  try {
    await atomicWriteSecureBytes(outputPath, handoffBytes);
  } catch {
    let published = false;
    try {
      const observedBytes = await readSecureBytes(outputPath);
      validateHandoff(JSON.parse(observedBytes.toString("utf8")));
      published = sha256(observedBytes) === state.handoff_binding.sha256;
    } catch {
      published = false;
    }
    if (!published) {
      state.handoff_binding = null;
      try {
        await persistState(state);
      } catch {
        fail(
          "handoff_publication_recovery_required",
          "the exact handoff was not published and its authoritative binding could not be cleared; the invalid output remains operator-rejected",
        );
      }
      fail(
        "handoff_publication_failed",
        "the bound handoff was not published exactly; the operator will reject the output",
      );
    }
  }
  return {
    subjectId: authorized.subjectId,
    expiresAt: authorized.expiresAt,
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

function requireSettledCallerExchange(state) {
  if (state.caller_exchange !== null) {
    fail(
      "caller_authorization_exchange_recovery_required",
      "an earlier authorization-code exchange remains ambiguous; retire the exact staging app and preserve its incident marker",
    );
  }
}

async function readRetirementApp({ config, state, transport, adminToken }) {
  const appId = state.caller_exchange.app_id;
  const response = await transport.request({
    url: nyx(`/api/v1/developer/oauth-clients/${appId}`),
    token: adminToken,
    statuses: [200, 401, 403, 404],
  });
  if (response.status !== 200) {
    fail(
      "caller_app_retirement_evidence_required",
      "the exact ambiguous OIDC app could not be read through its owner authority",
    );
  }
  const app = validateApp(response.json, config, {
    expectedActive: response.json?.is_active,
  });
  if (app.id !== appId || state.oidc_app?.id !== appId) {
    fail(
      "state_authority_mismatch",
      "retirement observation did not resolve the exact incident OIDC app",
    );
  }
  return app;
}

async function retireAmbiguousCallerApp({
  config,
  state,
  transport,
  adminToken,
}) {
  if (state.caller_exchange === null) {
    fail(
      "caller_app_retirement_not_required",
      "bootstrap state has no ambiguous authorization-code exchange",
    );
  }
  const health = await transport.request({ url: nyx("/health") });
  validateHealth(health.json);
  const before = await readRetirementApp({
    config,
    state,
    transport,
    adminToken,
  });
  let deleteResponse;
  let transportAmbiguous = false;
  if (before.is_active) {
    try {
      deleteResponse = await transport.request({
        method: "DELETE",
        url: nyx(`/api/v1/developer/oauth-clients/${before.id}`),
        token: adminToken,
        statuses: [200, 401, 403, 404, 409, 500, 502, 503, 504],
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
    if (deleteResponse?.status === 200) {
      exactKeys(
        deleteResponse.json,
        ["message"],
        "OIDC app retirement response",
        "contract_drift",
      );
      if (deleteResponse.json.message !== "OAuth client deactivated") {
        fail("contract_drift", "OIDC app retirement acknowledgement drifted");
      }
    } else if (
      deleteResponse?.status === 401 ||
      deleteResponse?.status === 403
    ) {
      fail(
        "caller_app_retirement_forbidden",
        "owner authority could not retire the exact ambiguous OIDC app",
      );
    }
  }
  let after;
  try {
    after = await readRetirementApp({
      config,
      state,
      transport,
      adminToken,
    });
  } catch (error) {
    if (
      error instanceof BootstrapFailure &&
      error.code === "caller_app_retirement_evidence_required"
    ) {
      fail(
        "caller_app_retirement_recovery_required",
        "retirement outcome lacks an exact inactive-app acknowledgement; retain the incident marker and do not retry automatically",
      );
    }
    throw error;
  }
  if (after.is_active) {
    fail(
      "caller_app_retirement_recovery_required",
      `the exact incident app remains active after ${transportAmbiguous ? "an ambiguous retirement response" : "the reviewed retirement attempt"}; retain the marker and do not retry automatically`,
    );
  }
  return {
    status: before.is_active ? "caller_app_retired" : "caller_app_already_retired",
    app_id: after.id,
    app_active: false,
    incident_marker_retained: true,
    issuance_fence_proven: false,
  };
}

function parseArguments(argv) {
  const modes = new Set([
    "--validate-config",
    "--plan",
    "--apply",
    "--adopt-provider-buckets",
    "--recover-resource-creation",
    "--retire-ambiguous-caller-app",
    "--authorize-caller",
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
    if (
      result.outputPath === result.statePath ||
      result.outputPath === `${result.statePath}${CALLER_STATE_LOCK_SUFFIX}`
    ) {
      fail("usage", "handoff path must differ from state and lock paths");
    }
  } else if (
    result.outputPath !== null ||
    (result.noBrowser && result.mode !== "--authorize-caller")
  ) {
    fail(
      "usage",
      "--output is only valid for caller token minting; --no-browser is valid for caller authorization or minting",
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

function resourceRecoveryAttemptNonceFromEnvironment() {
  const attemptNonce = boundedString(
    process.env.CMA_TRIGGER_NYXID_SMOKE_RECOVERY_ATTEMPT_NONCE,
    "CMA_TRIGGER_NYXID_SMOKE_RECOVERY_ATTEMPT_NONCE",
    64,
    "resource_creation_recovery_confirmation_mismatch",
  );
  if (!/^[0-9a-f]{64}$/u.test(attemptNonce)) {
    fail(
      "resource_creation_recovery_confirmation_mismatch",
      "the reviewed create-attempt nonce is invalid",
    );
  }
  return attemptNonce;
}

function resourceRecoveryTargetFromEnvironment() {
  const resourceId = uuid(
    process.env.CMA_TRIGGER_NYXID_SMOKE_RECOVERY_RESOURCE_ID,
    "CMA_TRIGGER_NYXID_SMOKE_RECOVERY_RESOURCE_ID",
    "resource_creation_recovery_target_missing",
  );
  const attemptNonce = resourceRecoveryAttemptNonceFromEnvironment();
  return { resourceId, attemptNonce };
}

async function runStatefulMode(args, config) {
  const rawState = await readSecureJson(args.statePath, { optional: true });
  const state =
    rawState === null ? defaultState(config) : validateState(rawState, config);
  if (
    state.resource_creation !== null &&
    !new Set([
      "--plan",
      "--recover-resource-creation",
    ]).has(args.mode)
  ) {
    requireNoPendingResourceCreation(state);
  }
  if (
    state.caller_exchange !== null &&
    !new Set(["--plan", "--retire-ambiguous-caller-app"]).has(args.mode)
  ) {
    requireSettledCallerExchange(state);
  }
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
  if (args.mode === "--recover-resource-creation") {
    if (
      process.env.CMA_TRIGGER_NYXID_SMOKE_RESOURCE_RECOVERY_CONFIRM !==
      RESOURCE_CREATION_RECOVERY_CONFIRMATION
    ) {
      fail(
        "confirmation_required",
        "reviewed ambiguous resource-creation recovery confirmation is missing",
      );
    }
    const target = resourceRecoveryTargetFromEnvironment();
    const snapshot = await inspectResources({
      config,
      state,
      transport,
      adminToken,
    });
    const result = await recoverResourceCreation({
      config,
      state,
      snapshot,
      transport,
      adminToken,
      persistState,
      ...target,
    });
    process.stdout.write(
      `${JSON.stringify({ ...result, state_path: args.statePath })}\n`,
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
  if (args.mode === "--adopt-provider-buckets") {
    if (
      process.env.CMA_TRIGGER_NYXID_SMOKE_BUCKET_ADOPTION_CONFIRM !==
      BUCKET_ADOPTION_CONFIRMATION
    ) {
      fail(
        "confirmation_required",
        "reviewed empty non-production provider-bucket adoption confirmation is missing",
      );
    }
    const result = await adoptProviderBuckets({
      config,
      state,
      transport,
      adminToken,
      persistState,
    });
    process.stdout.write(
      `${JSON.stringify({ status: "provider_buckets_adopted", buckets: result.adopted, state_path: args.statePath })}\n`,
    );
    return;
  }
  if (args.mode === "--retire-ambiguous-caller-app") {
    if (
      process.env.CMA_TRIGGER_NYXID_SMOKE_OIDC_RETIRE_CONFIRM !==
      OIDC_RETIREMENT_CONFIRMATION
    ) {
      fail(
        "confirmation_required",
        "reviewed non-production OIDC app retirement confirmation is missing",
      );
    }
    const result = await retireAmbiguousCallerApp({
      config,
      state,
      transport,
      adminToken,
    });
    process.stdout.write(`${JSON.stringify(result)}\n`);
    return;
  }
  const snapshot = await inspectResources({
    config,
    state,
    transport,
    adminToken,
  });
  const issuerTtlWitness = async () => {
    const health = await transport.request({ url: nyx("/health") });
    validateHealth(health.json);
    const me = await transport.request({
      url: nyx("/api/v1/users/me"),
      token: adminToken,
    });
    validateMe(me.json);
    if (me.json.id !== snapshot.adminSubjectId) {
      fail(
        "issuer_ttl_witness_invalid",
        "administrator authority changed before authorization-code exchange",
      );
    }
    return captureIssuerTtlWitness(adminToken, snapshot.adminSubjectId);
  };
  if (args.mode === "--authorize-caller") {
    const result = await authorizeCallerCustody({
      config,
      state,
      snapshot,
      transport,
      persistState,
      serviceId: serviceIdFromEnvironment(),
      noBrowser: args.noBrowser,
      issuerTtlWitness,
    });
    process.stdout.write(
      `${JSON.stringify({ status: "caller_authority_ready", subject_id: result.subjectId, expires_at: result.expiresAt, state_path: args.statePath })}\n`,
    );
    return;
  }
  const result = await mintCallerToken({
    config,
    state,
    snapshot,
    transport,
    persistState,
    outputPath: args.outputPath,
    serviceId: serviceIdFromEnvironment(),
    noBrowser: args.noBrowser,
    issuerTtlWitness,
  });
  process.stdout.write(
    `${JSON.stringify({ status: "caller_token_ready", subject_id: result.subjectId, expires_at: result.expiresAt, handoff_path: args.outputPath })}\n`,
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
  if (args.outputPath !== null) {
    await requireDistinctAuthorityPaths(args.statePath, args.outputPath);
  }
  if (
    new Set([
      "--apply",
      "--adopt-provider-buckets",
      "--recover-resource-creation",
      "--retire-ambiguous-caller-app",
      "--authorize-caller",
      "--mint-caller-token",
    ]).has(args.mode)
  ) {
    return withCallerStateLock(args.statePath, async () => {
      // The lock now exists, so identity checks catch filesystem-specific
      // aliases that cannot be modeled reliably from non-existent path names.
      if (args.outputPath !== null) {
        await requireDistinctAuthorityPaths(args.statePath, args.outputPath);
      }
      return runStatefulMode(args, config);
    });
  }
  return runStatefulMode(args, config);
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
  BUCKET_ADOPTION_CONFIRMATION,
  BootstrapFailure,
  CALLER_SCOPES,
  FetchTransport,
  GITHUB_CATALOG_SERVICE_SLUG,
  MAX_GITHUB_SECRET_PLAINTEXT_BYTES,
  NYXID_API_VERSION,
  NYXID_BASE_URL,
  NYXID_SOURCE_REF_URL,
  NYXID_SOURCE_REVISION,
  OIDC_RETIREMENT_CONFIRMATION,
  RESOURCE_CREATION_RECOVERY_CONFIRMATION,
  SA_ITEM_FIELDS,
  SERVICE_RESPONSE_FIELDS,
  STORAGE_ALLOWED_SCOPES,
  STORAGE_ROLE_PERMISSIONS,
  applyProvisioning,
  adoptProviderBucket,
  adoptProviderBuckets,
  authorizeCallerCustody,
  atomicWriteSecureJson,
  canonicalAuthorityTarget,
  captureIssuerTtlWitness,
  defaultState,
  ensureBucket,
  inspectResources,
  mintCallerToken,
  parseArguments,
  parseCallbackRequest,
  planEvidence,
  provisioningPlan,
  readSecureJson,
  recoverResourceCreation,
  requireHandoffUploadBound,
  retireAmbiguousCallerApp,
  requireDistinctAuthorityPaths,
  secureAbsolutePath,
  tokenSet,
  validateAccessClaims,
  validateConfig,
  validateHandoff,
  validateHealth,
  validateIntrospection,
  validateObserver,
  validateIssuerTtlWitness,
  validateState,
  withCallerStateLock,
};
