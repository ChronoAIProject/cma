#!/usr/bin/env node

import assert from "node:assert/strict";
import {
  chmod,
  link,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  symlink,
  unlink,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import { join } from "node:path";

import {
  BootstrapFailure,
  CALLER_SCOPES,
  GITHUB_CATALOG_SERVICE_SLUG,
  MAX_GITHUB_SECRET_PLAINTEXT_BYTES,
  NYXID_API_VERSION,
  NYXID_BASE_URL,
  NYXID_SOURCE_REF_URL,
  NYXID_SOURCE_REVISION,
  SA_ITEM_FIELDS,
  SERVICE_RESPONSE_FIELDS,
  STORAGE_ALLOWED_SCOPES,
  STORAGE_ROLE_PERMISSIONS,
  adoptProviderBuckets,
  applyProvisioning,
  authorizeCallerCustody,
  captureIssuerTtlWitness,
  defaultState,
  inspectResources,
  mintCallerToken,
  parseArguments,
  parseCallbackRequest,
  planEvidence,
  provisioningPlan,
  readSecureJson,
  recoverResourceCreation,
  requireDistinctAuthorityPaths,
  requireHandoffUploadBound,
  retireAmbiguousCallerApp,
  validateAccessClaims,
  validateConfig,
  validateHandoff,
  validateIntrospection,
  validateIssuerTtlWitness,
  validateObserver,
  validateState,
  withCallerStateLock,
} from "./cma-trigger-nyxid-smoke-bootstrap.mjs";

const ADMIN_ID = "11111111-1111-4111-8111-111111111111";
const STORAGE_SERVICE_ID = "22222222-2222-4222-8222-222222222222";
const GITHUB_CATALOG_ID = "2a222222-2222-4222-8222-222222222222";
const ROLE_ID = "33333333-3333-4333-8333-333333333333";
const OBSERVER_ID = "44444444-4444-4444-8444-444444444444";
const APP_ID = "55555555-5555-4555-8555-555555555555";
const SA_ID = "66666666-6666-4666-8666-666666666666";
const SUBJECT_ID = "77777777-7777-4777-8777-777777777777";
const OTHER_ID = "88888888-8888-4888-8888-888888888888";
const GITHUB_USER_SERVICE_ID = "8a888888-8888-4888-8888-888888888888";
const SA_CLIENT = `sa_${"a".repeat(24)}`;
const ADMIN_TOKEN = "fixture-admin-bearer";
const NOW = Math.floor(Date.now() / 1000);

function issuerTtlWitness() {
  return {
    issuer_token_issued_at: NOW,
    issuer_access_token_ttl_seconds: 8_000,
    issuer_token_sha256: "d".repeat(64),
    issuer_commit: NYXID_SOURCE_REVISION.slice(0, 12),
  };
}

function issuerAccessToken(overrides = {}) {
  const header = Buffer.from(
    JSON.stringify({ alg: "RS256", typ: "JWT", kid: "fixture" }),
  ).toString("base64url");
  const claims = Buffer.from(
    JSON.stringify({
      sub: ADMIN_ID,
      iss: NYXID_BASE_URL,
      aud: NYXID_BASE_URL,
      exp: NOW + 8_000,
      iat: NOW,
      token_type: "access",
      ...overrides,
    }),
  ).toString("base64url");
  return `${header}.${claims}.fixture-signature`;
}

function config() {
  return {
    schema_version: 1,
    environment: "cma-trigger-smoke",
    nyxid_base_url: NYXID_BASE_URL,
    observer: {
      name: "CMA Trigger staging public GitHub observer",
      slug: "cma-trigger-github-observer-staging",
      rule_id: "2d96f527-9cb8-4ad6-a09c-e14c5c0cfac7",
      daily_quota: 500,
    },
    oidc_app: {
      name: "CMA Trigger staging",
      redirect_uris: [
        "https://127.0.0.1:8443/auth/callback",
        "http://127.0.0.1:8765/callback",
      ],
      smoke_redirect_uri: "http://127.0.0.1:8765/callback",
    },
    storage: {
      service_account_name: "CMA Trigger staging storage",
      context_bucket: "cma-trigger-staging-context-tests",
      trigger_workspace_bucket: "cma-trigger-staging-workspace-tests",
      service_slug: "chrono-storage-service",
      role_slug: "chrono-bucket-user",
    },
  };
}

function discovery() {
  return {
    issuer: NYXID_BASE_URL,
    authorization_endpoint: `${NYXID_BASE_URL}/oauth/authorize`,
    token_endpoint: `${NYXID_BASE_URL}/oauth/token`,
    pushed_authorization_request_endpoint: `${NYXID_BASE_URL}/oauth/par`,
    require_pushed_authorization_requests: false,
    request_uri_parameter_supported: true,
    userinfo_endpoint: `${NYXID_BASE_URL}/oauth/userinfo`,
    jwks_uri: `${NYXID_BASE_URL}/.well-known/jwks.json`,
    response_types_supported: ["code"],
    grant_types_supported: [
      "authorization_code",
      "refresh_token",
      "client_credentials",
      "urn:ietf:params:oauth:grant-type:token-exchange",
    ],
    subject_token_types_supported: [
      "urn:ietf:params:oauth:token-type:access_token",
      "urn:nyxid:params:oauth:token-type:broker-binding",
    ],
    nyxid_broker_binding_supported: true,
    oauth_broker_binding_revocation_webhook_supported: true,
    subject_types_supported: ["public"],
    id_token_signing_alg_values_supported: ["RS256"],
    dpop_signing_alg_values_supported: ["ES256"],
    tls_client_certificate_bound_access_tokens: true,
    introspection_endpoint: `${NYXID_BASE_URL}/oauth/introspect`,
    revocation_endpoint: `${NYXID_BASE_URL}/oauth/revoke`,
    scopes_supported: [
      "openid",
      "profile",
      "email",
      "roles",
      "groups",
      "offline_access",
    ],
    claims_supported: ["sub", "iss", "aud", "exp", "iat"],
    code_challenge_methods_supported: ["S256"],
    token_endpoint_auth_methods_supported: [
      "client_secret_basic",
      "client_secret_post",
      "none",
    ],
    userinfo_signing_alg_values_supported: ["none"],
  };
}

function serviceBase(id, name, slug) {
  return {
    id,
    name,
    slug,
    description: null,
    base_url: "https://service.example.test",
    service_type: "http",
    visibility: "public",
    auth_method: "none",
    auth_type: "none",
    auth_key_name: "",
    is_active: true,
    oauth_client_id: null,
    openapi_spec_url: null,
    api_spec_url: null,
    asyncapi_spec_url: null,
    streaming_supported: false,
    ssh_config: null,
    service_category: "internal",
    requires_user_credential: false,
    identity_propagation_mode: "none",
    identity_include_user_id: false,
    identity_include_email: false,
    identity_include_name: false,
    identity_jwt_audience: null,
    forward_access_token: false,
    inject_delegation_token: false,
    delegation_token_scope: "llm:proxy",
    effective_platform_metric: "requests",
    ws_frame_injections: [],
    anonymous_endpoints: [],
    created_by: ADMIN_ID,
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
    your_binding_count: 0,
  };
}

function observer(cfg = config()) {
  return {
    ...serviceBase(OBSERVER_ID, cfg.observer.name, cfg.observer.slug),
    base_url: "https://api.github.com",
    anonymous_endpoints: [
      {
        id: cfg.observer.rule_id,
        enabled: true,
        method: "GET",
        path_pattern: "/repositories/**",
        daily_quota: cfg.observer.daily_quota,
      },
    ],
  };
}

function storageService(cfg = config()) {
  return {
    ...serviceBase(
      STORAGE_SERVICE_ID,
      "Chrono Storage Service",
      cfg.storage.service_slug,
    ),
    base_url: "http://chrono-storage.example.test:3805",
    identity_propagation_mode: "jwt",
    identity_jwt_audience: "chrono-bucket",
  };
}

function githubCatalogService() {
  return {
    ...serviceBase(
      GITHUB_CATALOG_ID,
      "GitHub OAuth",
      GITHUB_CATALOG_SERVICE_SLUG,
    ),
  };
}

function role(cfg = config()) {
  return {
    id: ROLE_ID,
    name: "Chrono Bucket User",
    slug: cfg.storage.role_slug,
    description: "chrono-bucket-user",
    permissions: [...STORAGE_ROLE_PERMISSIONS],
    is_default: false,
    is_system: false,
    client_id: null,
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
  };
}

function app(cfg = config(), { secret = false } = {}) {
  const value = {
    id: APP_ID,
    client_name: cfg.oidc_app.name,
    client_type: "confidential",
    redirect_uris: [...cfg.oidc_app.redirect_uris],
    allowed_scopes: CALLER_SCOPES.join(" "),
    delegation_scopes: "",
    broker_capability_enabled: false,
    revocation_webhook_url: null,
    connection_webhook_url: null,
    connection_webhook_enabled: false,
    is_active: true,
    default_service_catalog_slugs: [GITHUB_CATALOG_SERVICE_SLUG],
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
  };
  if (secret) value.client_secret = "fixture-oidc-one-time-value";
  return value;
}

function serviceAccount(cfg = config()) {
  return {
    id: SA_ID,
    name: cfg.storage.service_account_name,
    description: "Dedicated non-production CMA Trigger smoke storage identity",
    client_id: SA_CLIENT,
    secret_prefix: "fixture-prefix",
    allowed_scopes: STORAGE_ALLOWED_SCOPES.join(" "),
    role_ids: [ROLE_ID],
    is_active: true,
    rate_limit_override: null,
    created_by: ADMIN_ID,
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
    last_authenticated_at: null,
  };
}

function createdServiceAccount(cfg = config()) {
  return {
    id: SA_ID,
    name: cfg.storage.service_account_name,
    client_id: SA_CLIENT,
    client_secret: "fixture-storage-one-time-value",
    allowed_scopes: STORAGE_ALLOWED_SCOPES.join(" "),
    role_ids: [ROLE_ID],
    is_active: true,
    created_at: "2026-08-29T00:00:00Z",
    message: "Save this value",
  };
}

function adminProfile() {
  return {
    id: ADMIN_ID,
    email: "operator@example.test",
    display_name: null,
    avatar_url: null,
    email_verified: true,
    mfa_enabled: true,
    is_admin: true,
    is_operator: false,
    role: "admin",
    is_active: true,
    social_provider: null,
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
    last_login_at: "2026-08-29T00:00:00Z",
    profile_config: { onboarding: { ai_services_completed_at: null } },
    capabilities: { billing_available: false, enabled_features: [] },
  };
}

function nyxidOpenapi(version = NYXID_API_VERSION) {
  const createFields = [
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
  ];
  return {
    info: { version },
    components: {
      schemas: {
        CreateServiceRequest: {
          properties: Object.fromEntries(createFields.map((key) => [key, {}])),
        },
        ServiceResponse: {
          properties: Object.fromEntries(
            SERVICE_RESPONSE_FIELDS.map((key) => [key, {}]),
          ),
        },
      },
    },
  };
}

function storageOpenapi() {
  return {
    info: { title: "chrono-bucket API", version: "1.0.0" },
    paths: {
      "/api/buckets": { get: {}, post: {} },
      "/api/buckets/{bucket}": { get: {} },
      "/api/buckets/{bucket}/ownership": { put: {} },
      "/api/buckets/{bucket}/objects": {
        get: {},
        post: {},
        patch: {},
        delete: {},
      },
      "/api/buckets/{bucket}/objects/download": { get: {} },
    },
  };
}

function storageToken() {
  return {
    access_token: "fixture-storage-access-value",
    token_type: "Bearer",
    expires_in: 600,
    refresh_token: null,
    id_token: null,
    scope: STORAGE_ALLOWED_SCOPES.join(" "),
  };
}

function bucketDetail(bucket, owner = SA_ID) {
  return {
    data: {
      name: bucket,
      owner,
      createdAt: "2026-08-29T00:00:00Z",
      metadata: {},
      objects: [],
    },
    error: null,
  };
}

function errorEnvelope(code = "NOT_FOUND") {
  return { data: null, error: { code, message: "fixture error" } };
}

function stateWithResources(cfg = config()) {
  const state = defaultState(cfg);
  state.observer = { id: OBSERVER_ID, slug: cfg.observer.slug };
  state.oidc_app = { id: APP_ID, client_secret: "fixture-oidc-custody-value" };
  state.storage_service_account = {
    id: SA_ID,
    client_id: SA_CLIENT,
    client_secret: "fixture-storage-custody-value",
  };
  state.buckets.owner_service_account_id = SA_ID;
  return state;
}

function stateWithCallerExchange(cfg = config()) {
  const state = stateWithResources(cfg);
  state.caller_exchange = {
    app_id: APP_ID,
    exchange_started_at: NOW,
    issuer_token_issued_at: NOW,
    issuer_access_token_ttl_seconds: 8_000,
    issuer_token_sha256: "d".repeat(64),
    issuer_commit: NYXID_SOURCE_REVISION.slice(0, 12),
  };
  return validateState(state, cfg);
}

class FixtureTransport {
  constructor(options = {}) {
    this.options = {
      observerPresent: true,
      appPresent: true,
      accountPresent: true,
      duplicateApps: false,
      duplicateAccounts: false,
      serviceMutator: null,
      openapiVersion: NYXID_API_VERSION,
      handler: null,
      ...options,
    };
    this.calls = [];
  }

  async request(request) {
    const call = { method: request.method ?? "GET", ...request };
    this.calls.push(call);
    if (this.options.handler !== null) {
      const handled = await this.options.handler(call, this);
      if (handled !== undefined) return handled;
    }
    const cfg = config();
    if (call.method !== "GET") {
      throw new Error(`unhandled mutation ${call.method} ${call.url}`);
    }
    if (call.url === NYXID_SOURCE_REF_URL) {
      return {
        status: 200,
        json: {
          ref: "refs/heads/main",
          node_id: "fixture-node",
          url: "https://api.github.com/fixture",
          object: {
            sha: NYXID_SOURCE_REVISION,
            type: "commit",
            url: "https://api.github.com/fixture-commit",
          },
        },
      };
    }
    if (call.url.endsWith("/health")) {
      return {
        status: 200,
        json: {
          status: "ok",
          version: NYXID_API_VERSION,
          commit: NYXID_SOURCE_REVISION.slice(0, 12),
          encryption: {},
        },
      };
    }
    if (call.url.endsWith("/.well-known/openid-configuration")) {
      return { status: 200, json: discovery() };
    }
    if (call.url.endsWith("/api/v1/docs/openapi.json")) {
      return { status: 200, json: nyxidOpenapi(this.options.openapiVersion) };
    }
    if (call.url.endsWith("/api/v1/users/me")) {
      return { status: 200, json: adminProfile() };
    }
    if (call.url.endsWith("/api/v1/services")) {
      let items = [storageService(cfg), githubCatalogService()];
      if (this.options.observerPresent) items.push(observer(cfg));
      if (this.options.serviceMutator !== null) {
        items = this.options.serviceMutator(structuredClone(items));
      }
      return { status: 200, json: { services: items } };
    }
    if (call.url.includes("/anonymous-endpoints")) {
      return {
        status: 200,
        json: { endpoints: structuredClone(observer(cfg).anonymous_endpoints) },
      };
    }
    if (call.url.endsWith("/api/v1/admin/roles")) {
      return { status: 200, json: { roles: [role(cfg)] } };
    }
    if (call.url.endsWith("/api/v1/developer/oauth-clients")) {
      const clients = this.options.appPresent ? [app(cfg)] : [];
      if (this.options.duplicateApps) clients.push(app(cfg));
      return { status: 200, json: { clients } };
    }
    if (call.url.includes("/api/v1/admin/service-accounts?")) {
      const accounts = this.options.accountPresent ? [serviceAccount(cfg)] : [];
      if (this.options.duplicateAccounts) accounts.push(serviceAccount(cfg));
      return {
        status: 200,
        json: {
          service_accounts: accounts,
          total: accounts.length,
          page: 1,
          per_page: 100,
        },
      };
    }
    if (call.url.endsWith("/openapi.json")) {
      return { status: 200, json: storageOpenapi() };
    }
    throw new Error(`unhandled GET ${call.url}`);
  }
}

async function expectFailure(action, code) {
  let caught;
  try {
    await action();
  } catch (error) {
    caught = error;
  }
  assert.ok(
    caught instanceof BootstrapFailure,
    `expected BootstrapFailure, got ${caught}`,
  );
  assert.equal(caught.code, code);
  return caught;
}

const tests = [];
function test(name, run) {
  tests.push({ name, run });
}

test("plan performs no mutating request", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  const transport = new FixtureTransport();
  const snapshot = await inspectResources({
    config: cfg,
    state,
    transport,
    adminToken: ADMIN_TOKEN,
  });
  assert.deepEqual(provisioningPlan(snapshot, cfg), [
    `verify_or_create_bucket:${cfg.storage.context_bucket}`,
    `verify_or_create_bucket:${cfg.storage.trigger_workspace_bucket}`,
  ]);
  const evidence = planEvidence(snapshot, cfg);
  assert.equal(evidence.administrator.subject_id, ADMIN_ID);
  assert.equal(evidence.public_github_observer.owner_subject_id, ADMIN_ID);
  assert.equal(evidence.storage_service_account.owner_subject_id, ADMIN_ID);
  assert.deepEqual(evidence.storage_authority.bucket_targets, [
    cfg.storage.context_bucket,
    cfg.storage.trigger_workspace_bucket,
  ]);
  assert.equal(
    evidence.github_authority.catalog_service_slug,
    GITHUB_CATALOG_SERVICE_SLUG,
  );
  assert.equal(JSON.stringify(evidence).includes("secret"), false);
  assert.ok(transport.calls.every((call) => call.method === "GET"));
});

test("exact existing resources produce no provisioning mutation", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-existing-refresh",
    scope: CALLER_SCOPES.join(" "),
    verification_state: "verified",
  };
  state.handoff_binding = { sha256: "c".repeat(64) };
  const transport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        return { status: 200, json: storageToken() };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        const bucket = call.url.split("/").at(-1);
        return { status: 200, json: bucketDetail(bucket) };
      }
    },
  });
  const persisted = [];
  const result = await applyProvisioning({
    config: cfg,
    state,
    transport,
    adminToken: ADMIN_TOKEN,
    persistState: async (next) => persisted.push(structuredClone(next)),
  });
  assert.deepEqual(result.applied, []);
  assert.equal(persisted.length, 1);
  assert.equal(persisted[0].handoff_binding, null);
  const provisioningMutations = transport.calls.filter(
    (call) => call.method !== "GET" && !call.url.endsWith("/oauth/token"),
  );
  assert.deepEqual(provisioningMutations, []);
});

test("fully absent resources are created in the closed order and buckets reconcile", async () => {
  const cfg = validateConfig(config());
  const state = defaultState(cfg);
  const liveBuckets = new Set();
  const transport = new FixtureTransport({
    observerPresent: false,
    appPresent: false,
    accountPresent: false,
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/api/v1/services")) {
        assert.equal(call.json.forward_access_token, false);
        assert.equal(
          call.json.anonymous_endpoints[0].path_pattern,
          "/repositories/**",
        );
        assert.equal(call.json.credential, undefined);
        assert.equal(call.json.billing, undefined);
        return { status: 200, json: observer(cfg) };
      }
      if (
        call.method === "POST" &&
        call.url.endsWith("/api/v1/developer/oauth-clients")
      ) {
        assert.equal(call.json.client_type, "confidential");
        assert.equal(call.json.broker_capability_enabled, false);
        assert.deepEqual(call.json.allowed_scopes, CALLER_SCOPES);
        assert.deepEqual(call.json.default_service_catalog_slugs, [
          GITHUB_CATALOG_SERVICE_SLUG,
        ]);
        return { status: 200, json: app(cfg, { secret: true }) };
      }
      if (
        call.method === "POST" &&
        call.url.endsWith("/api/v1/admin/service-accounts")
      ) {
        assert.deepEqual(call.json.role_ids, [ROLE_ID]);
        assert.equal(call.json.rate_limit_override, undefined);
        return { status: 200, json: createdServiceAccount(cfg) };
      }
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        return { status: 200, json: storageToken() };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        const bucket = call.url.split("/").at(-1);
        return liveBuckets.has(bucket)
          ? { status: 200, json: bucketDetail(bucket) }
          : { status: 404, json: errorEnvelope("BUCKET_NOT_FOUND") };
      }
      if (call.method === "POST" && call.url.endsWith("/api/buckets")) {
        liveBuckets.add(call.json.name);
        return {
          status: 201,
          json: { data: { name: call.json.name, created: true }, error: null },
        };
      }
    },
  });
  const persisted = [];
  const result = await applyProvisioning({
    config: cfg,
    state,
    transport,
    adminToken: ADMIN_TOKEN,
    persistState: async (next) => persisted.push(structuredClone(next)),
  });
  assert.deepEqual(result.applied, [
    "created_public_github_observer",
    "created_confidential_oidc_app",
    "created_storage_service_account",
    `created_or_reconciled_bucket:${cfg.storage.context_bucket}`,
    `created_or_reconciled_bucket:${cfg.storage.trigger_workspace_bucket}`,
  ]);
  const creationOrder = transport.calls
    .filter(
      (call) => call.method === "POST" && !call.url.endsWith("/oauth/token"),
    )
    .map((call) => new URL(call.url).pathname);
  assert.deepEqual(creationOrder, [
    "/api/v1/services",
    "/api/v1/developer/oauth-clients",
    "/api/v1/admin/service-accounts",
    "/api/v1/proxy/s/chrono-storage-service/api/buckets",
    "/api/v1/proxy/s/chrono-storage-service/api/buckets",
  ]);
  assert.deepEqual(
    persisted
      .filter((entry) => entry.resource_creation !== null)
      .map((entry) => entry.resource_creation.kind),
    ["oidc_app", "storage_service_account"],
  );
  assert.equal(persisted.at(-1).resource_creation, null);
  assert.equal(persisted.at(-1).buckets.owner_service_account_id, SA_ID);
  assert.equal(liveBuckets.size, 2);
});

test("observer security drift fails before mutation", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  for (const mutate of [
    (item) => (item.created_by = OTHER_ID),
    (item) => (item.identity_propagation_mode = "jwt"),
    (item) => (item.forward_access_token = true),
    (item) => (item.inject_delegation_token = true),
    (item) => (item.billing = { resale_billable: true }),
    (item) => (item.anonymous_endpoints[0].method = "POST"),
    (item) => (item.anonymous_endpoints[0].path_pattern = "/**"),
    (item) =>
      item.anonymous_endpoints.push(
        structuredClone(item.anonymous_endpoints[0]),
      ),
  ]) {
    const transport = new FixtureTransport({
      serviceMutator(items) {
        const target = items.find((item) => item.slug === cfg.observer.slug);
        mutate(target);
        return items;
      },
    });
    await expectFailure(
      () =>
        inspectResources({
          config: cfg,
          state,
          transport,
          adminToken: ADMIN_TOKEN,
        }),
      "observer_contract_drift",
    );
    assert.ok(transport.calls.every((call) => call.method === "GET"));
  }
});

test("unknown response fields and API version drift fail closed", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  const unknown = new FixtureTransport({
    serviceMutator(items) {
      items[0].future_security_switch = false;
      return items;
    },
  });
  await expectFailure(
    () =>
      inspectResources({
        config: cfg,
        state,
        transport: unknown,
        adminToken: ADMIN_TOKEN,
      }),
    "contract_drift",
  );
  const version = new FixtureTransport({ openapiVersion: "0.10.1" });
  await expectFailure(
    () =>
      inspectResources({
        config: cfg,
        state,
        transport: version,
        adminToken: ADMIN_TOKEN,
      }),
    "api_version_drift",
  );
});

test("duplicate app and service-account names fail closed", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  for (const options of [
    { duplicateApps: true },
    { duplicateAccounts: true },
  ]) {
    const transport = new FixtureTransport(options);
    await expectFailure(
      () =>
        inspectResources({
          config: cfg,
          state,
          transport,
          adminToken: ADMIN_TOKEN,
        }),
      "duplicate_resource",
    );
  }
});

test("storage service account must be owned by the exact administrator", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  const transport = new FixtureTransport({
    handler(call) {
      if (
        call.method === "GET" &&
        call.url.includes("/api/v1/admin/service-accounts?")
      ) {
        const account = serviceAccount(cfg);
        account.created_by = OTHER_ID;
        return {
          status: 200,
          json: {
            service_accounts: [account],
            total: 1,
            page: 1,
            per_page: 100,
          },
        };
      }
    },
  });
  await expectFailure(
    () =>
      inspectResources({
        config: cfg,
        state,
        transport,
        adminToken: ADMIN_TOKEN,
      }),
    "storage_account_drift",
  );
  assert.ok(transport.calls.every((call) => call.method === "GET"));
});

test("existing secret resources without custody make no mutation", async () => {
  const cfg = validateConfig(config());
  const appOnly = new FixtureTransport({ accountPresent: false });
  await expectFailure(
    () =>
      inspectResources({
        config: cfg,
        state: defaultState(cfg),
        transport: appOnly,
        adminToken: ADMIN_TOKEN,
      }),
    "oidc_secret_rotation_required",
  );
  const saState = defaultState(cfg);
  const saOnly = new FixtureTransport({ appPresent: false });
  await expectFailure(
    () =>
      inspectResources({
        config: cfg,
        state: saState,
        transport: saOnly,
        adminToken: ADMIN_TOKEN,
      }),
    "storage_secret_rotation_required",
  );
  assert.ok(
    [...appOnly.calls, ...saOnly.calls].every((call) => call.method === "GET"),
  );
});

test("ambiguous one-time secret creates durably fence later invocations", async () => {
  const cfg = validateConfig(config());
  const state = defaultState(cfg);
  const transport = new FixtureTransport({
    appPresent: false,
    accountPresent: false,
    handler(call) {
      if (
        call.method === "POST" &&
        call.url.endsWith("/api/v1/developer/oauth-clients")
      ) {
        throw new BootstrapFailure(
          "remote_transport_error",
          "fixture ambiguous response",
        );
      }
    },
  });
  await expectFailure(
    () =>
      applyProvisioning({
        config: cfg,
        state,
        transport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.equal(state.resource_creation.kind, "oidc_app");
  assert.equal(state.resource_creation.name, cfg.oidc_app.name);
  assert.equal(state.resource_creation.administrator_subject_id, ADMIN_ID);
  assert.match(state.resource_creation.attempt_nonce, /^[0-9a-f]{64}$/u);
  assert.match(state.resource_creation.request_sha256, /^[0-9a-f]{64}$/u);
  validateState(state, cfg);
  const absentSnapshot = await inspectResources({
    config: cfg,
    state,
    transport,
    adminToken: ADMIN_TOKEN,
  });
  assert.deepEqual(provisioningPlan(absentSnapshot, cfg), [
    `recover_ambiguous_resource_creation:oidc_app:${cfg.oidc_app.name}`,
  ]);
  assert.equal(absentSnapshot.app, null);
  assert.equal(
    transport.calls.filter(
      (call) =>
        call.method === "POST" &&
        call.url.endsWith("/api/v1/developer/oauth-clients"),
    ).length,
    1,
    "exactly one create POST is expected",
  );
  const callsAfterFirstAttempt = transport.calls.length;
  await expectFailure(
    () =>
      applyProvisioning({
        config: cfg,
        state,
        transport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.equal(transport.calls.length, callsAfterFirstAttempt);
});

test("one-time resource creation fences persistence ambiguity before any retry", async () => {
  const cfg = validateConfig(config());

  const preRequestState = defaultState(cfg);
  preRequestState.observer = { id: OBSERVER_ID, slug: cfg.observer.slug };
  const preRequestTransport = new FixtureTransport({
    appPresent: false,
    accountPresent: false,
  });
  await expectFailure(
    () =>
      applyProvisioning({
        config: cfg,
        state: preRequestState,
        transport: preRequestTransport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {
          throw new Error("fixture marker persistence failure");
        },
      }),
    "resource_creation_fence_persistence_failed",
  );
  assert.equal(preRequestState.resource_creation, null);
  assert.equal(
    preRequestTransport.calls.filter((call) => call.method === "POST").length,
    0,
  );

  const scenarios = [
    {
      kind: "oidc_app",
      code: "oidc_secret_creation_recovery_required",
      state: (() => {
        const value = defaultState(cfg);
        value.observer = { id: OBSERVER_ID, slug: cfg.observer.slug };
        return value;
      })(),
      transport: new FixtureTransport({
        appPresent: false,
        accountPresent: false,
        handler(call) {
          if (
            call.method === "POST" &&
            call.url.endsWith("/api/v1/developer/oauth-clients")
          ) {
            return { status: 200, json: app(cfg, { secret: true }) };
          }
        },
      }),
    },
    {
      kind: "storage_service_account",
      code: "storage_secret_creation_recovery_required",
      state: (() => {
        const value = stateWithResources(cfg);
        value.storage_service_account = null;
        value.buckets.owner_service_account_id = null;
        return value;
      })(),
      transport: new FixtureTransport({
        accountPresent: false,
        handler(call) {
          if (
            call.method === "POST" &&
            call.url.endsWith("/api/v1/admin/service-accounts")
          ) {
            return { status: 200, json: createdServiceAccount(cfg) };
          }
        },
      }),
    },
  ];
  for (const scenario of scenarios) {
    let persistenceCalls = 0;
    await expectFailure(
      () =>
        applyProvisioning({
          config: cfg,
          state: scenario.state,
          transport: scenario.transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {
            persistenceCalls += 1;
            if (persistenceCalls === 2) {
              throw new Error("fixture custody persistence ambiguity");
            }
          },
        }),
      scenario.code,
    );
    assert.equal(persistenceCalls, 2);
    assert.equal(scenario.state.resource_creation.kind, scenario.kind);
    validateState(scenario.state, cfg);
    const postCount = scenario.transport.calls.filter(
      (call) => call.method === "POST",
    ).length;
    await expectFailure(
      () =>
        applyProvisioning({
          config: cfg,
          state: scenario.state,
          transport: scenario.transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {},
        }),
      scenario.code,
    );
    assert.equal(
      scenario.transport.calls.filter((call) => call.method === "POST").length,
      postCount,
    );
  }
});

test("reviewed exact-ID recovery rotates and adopts both ambiguous resource types", async () => {
  const cfg = validateConfig(config());
  const scenarios = [
    {
      kind: "oidc_app",
      resourceId: APP_ID,
      state: (() => {
        const value = defaultState(cfg);
        value.observer = { id: OBSERVER_ID, slug: cfg.observer.slug };
        return value;
      })(),
      transport: new FixtureTransport({
        appPresent: false,
        accountPresent: false,
        handler(call, fixture) {
          if (
            call.method === "POST" &&
            call.url.endsWith("/api/v1/developer/oauth-clients")
          ) {
            fixture.options.appPresent = true;
            throw new BootstrapFailure(
              "remote_transport_error",
              "fixture lost OIDC create response",
            );
          }
          if (
            call.method === "GET" &&
            call.url.endsWith(`/api/v1/developer/oauth-clients/${APP_ID}`)
          ) {
            return { status: 200, json: app(cfg) };
          }
          if (
            call.method === "POST" &&
            call.url.endsWith(
              `/api/v1/developer/oauth-clients/${APP_ID}/rotate-secret`,
            )
          ) {
            return {
              status: 200,
              json: {
                id: APP_ID,
                client_secret: "fixture-recovered-oidc-secret",
              },
            };
          }
        },
      }),
      expectedCode: "oidc_secret_creation_recovery_required",
    },
    {
      kind: "storage_service_account",
      resourceId: SA_ID,
      state: (() => {
        const value = stateWithResources(cfg);
        value.storage_service_account = null;
        value.buckets.owner_service_account_id = null;
        return value;
      })(),
      transport: new FixtureTransport({
        accountPresent: false,
        handler(call, fixture) {
          if (
            call.method === "POST" &&
            call.url.endsWith("/api/v1/admin/service-accounts")
          ) {
            fixture.options.accountPresent = true;
            throw new BootstrapFailure(
              "remote_transport_error",
              "fixture lost service-account create response",
            );
          }
          if (
            call.method === "GET" &&
            call.url.endsWith(`/api/v1/admin/service-accounts/${SA_ID}`)
          ) {
            return { status: 200, json: serviceAccount(cfg) };
          }
          if (
            call.method === "POST" &&
            call.url.endsWith(
              `/api/v1/admin/service-accounts/${SA_ID}/rotate-secret`,
            )
          ) {
            return {
              status: 200,
              json: {
                client_id: SA_CLIENT,
                client_secret: "fixture-recovered-storage-secret",
                secret_prefix: "fixture-recovered-prefix",
                message:
                  "Secret rotated. All existing tokens have been revoked. Save the new secret now.",
              },
            };
          }
        },
      }),
      expectedCode: "storage_secret_creation_recovery_required",
    },
  ];

  for (const scenario of scenarios) {
    await expectFailure(
      () =>
        applyProvisioning({
          config: cfg,
          state: scenario.state,
          transport: scenario.transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {},
        }),
      scenario.expectedCode,
    );
    const marker = structuredClone(scenario.state.resource_creation);
    const snapshot = await inspectResources({
      config: cfg,
      state: scenario.state,
      transport: scenario.transport,
      adminToken: ADMIN_TOKEN,
    });
    assert.deepEqual(provisioningPlan(snapshot, cfg), [
      `recover_ambiguous_resource_creation:${scenario.kind}:${marker.name}`,
    ]);
    assert.equal(
      planEvidence(snapshot, cfg).resource_creation_incident.attempt_nonce,
      marker.attempt_nonce,
    );
    const callsBeforeWrongTarget = scenario.transport.calls.length;
    await expectFailure(
      () =>
        recoverResourceCreation({
          config: cfg,
          state: scenario.state,
          snapshot,
          transport: scenario.transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {},
          resourceId: OTHER_ID,
          attemptNonce: marker.attempt_nonce,
        }),
      "resource_creation_recovery_target_mismatch",
    );
    assert.equal(scenario.transport.calls.length, callsBeforeWrongTarget);
    await expectFailure(
      () =>
        recoverResourceCreation({
          config: cfg,
          state: scenario.state,
          snapshot,
          transport: scenario.transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {},
          resourceId: scenario.resourceId,
          attemptNonce: "f".repeat(64),
        }),
      "resource_creation_recovery_confirmation_mismatch",
    );
    const persisted = [];
    const result = await recoverResourceCreation({
      config: cfg,
      state: scenario.state,
      snapshot,
      transport: scenario.transport,
      adminToken: ADMIN_TOKEN,
      persistState: async (next) => persisted.push(structuredClone(next)),
      resourceId: scenario.resourceId,
      attemptNonce: marker.attempt_nonce,
    });
    assert.equal(result.kind, scenario.kind);
    assert.equal(result.resource_id, scenario.resourceId);
    assert.notEqual(persisted[0].resource_creation.recovery_rotation, null);
    assert.equal(persisted[1].resource_creation, null);
    assert.equal(scenario.state.resource_creation, null);
    if (scenario.kind === "oidc_app") {
      assert.equal(scenario.state.oidc_app.id, APP_ID);
      assert.equal(
        scenario.state.oidc_app.client_secret,
        "fixture-recovered-oidc-secret",
      );
    } else {
      assert.equal(scenario.state.storage_service_account.id, SA_ID);
      assert.equal(scenario.state.buckets.owner_service_account_id, SA_ID);
    }
  }
});

test("resource recovery rotation and custody ambiguity never resend", async () => {
  const cfg = validateConfig(config());

  async function recoveryFixture({ rotation = "success" } = {}) {
    const state = defaultState(cfg);
    state.observer = { id: OBSERVER_ID, slug: cfg.observer.slug };
    const transport = new FixtureTransport({
      appPresent: false,
      accountPresent: false,
      handler(call, fixture) {
        if (
          call.method === "POST" &&
          call.url.endsWith("/api/v1/developer/oauth-clients")
        ) {
          fixture.options.appPresent = true;
          throw new BootstrapFailure(
            "remote_transport_error",
            "fixture lost OIDC create response",
          );
        }
        if (
          call.method === "GET" &&
          call.url.endsWith(`/api/v1/developer/oauth-clients/${APP_ID}`)
        ) {
          const exact = app(cfg);
          if (fixture.options.postRotationInactive === true) {
            exact.is_active = false;
          }
          return { status: 200, json: exact };
        }
        if (
          call.method === "POST" &&
          call.url.endsWith(
            `/api/v1/developer/oauth-clients/${APP_ID}/rotate-secret`,
          )
        ) {
          if (rotation === "ambiguous") {
            throw new BootstrapFailure(
              "remote_transport_error",
              "fixture lost rotation response",
            );
          }
          if (rotation === "deactivated") {
            fixture.options.postRotationInactive = true;
          }
          return {
            status: 200,
            json: { id: APP_ID, client_secret: "fixture-rotated-secret" },
          };
        }
      },
    });
    await expectFailure(
      () =>
        applyProvisioning({
          config: cfg,
          state,
          transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {},
        }),
      "oidc_secret_creation_recovery_required",
    );
    const snapshot = await inspectResources({
      config: cfg,
      state,
      transport,
      adminToken: ADMIN_TOKEN,
    });
    return { state, transport, snapshot };
  }

  const preFence = await recoveryFixture();
  await expectFailure(
    () =>
      recoverResourceCreation({
        config: cfg,
        ...preFence,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {
          throw new Error("fixture recovery fence persistence failure");
        },
        resourceId: APP_ID,
        attemptNonce: preFence.state.resource_creation.attempt_nonce,
      }),
    "resource_creation_recovery_fence_persistence_failed",
  );
  assert.equal(preFence.state.resource_creation.recovery_rotation, null);
  assert.equal(
    preFence.transport.calls.filter((call) =>
      call.url.endsWith(`/${APP_ID}/rotate-secret`),
    ).length,
    0,
  );

  const ambiguous = await recoveryFixture({ rotation: "ambiguous" });
  const ambiguousNonce = ambiguous.state.resource_creation.attempt_nonce;
  await expectFailure(
    () =>
      recoverResourceCreation({
        config: cfg,
        ...ambiguous,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
        resourceId: APP_ID,
        attemptNonce: ambiguousNonce,
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.notEqual(ambiguous.state.resource_creation.recovery_rotation, null);
  const rotationsAfterAmbiguity = ambiguous.transport.calls.filter((call) =>
    call.url.endsWith(`/${APP_ID}/rotate-secret`),
  ).length;
  await expectFailure(
    () =>
      recoverResourceCreation({
        config: cfg,
        ...ambiguous,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
        resourceId: APP_ID,
        attemptNonce: ambiguousNonce,
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.equal(
    ambiguous.transport.calls.filter((call) =>
      call.url.endsWith(`/${APP_ID}/rotate-secret`),
    ).length,
    rotationsAfterAmbiguity,
  );

  const deactivated = await recoveryFixture({ rotation: "deactivated" });
  const deactivatedNonce = deactivated.state.resource_creation.attempt_nonce;
  await expectFailure(
    () =>
      recoverResourceCreation({
        config: cfg,
        ...deactivated,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
        resourceId: APP_ID,
        attemptNonce: deactivatedNonce,
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.equal(deactivated.state.oidc_app, null);
  assert.notEqual(
    deactivated.state.resource_creation.recovery_rotation,
    null,
  );
  const rotationsAfterDeactivation = deactivated.transport.calls.filter(
    (call) => call.url.endsWith(`/${APP_ID}/rotate-secret`),
  ).length;
  await expectFailure(
    () =>
      recoverResourceCreation({
        config: cfg,
        ...deactivated,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
        resourceId: APP_ID,
        attemptNonce: deactivatedNonce,
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.equal(
    deactivated.transport.calls.filter((call) =>
      call.url.endsWith(`/${APP_ID}/rotate-secret`),
    ).length,
    rotationsAfterDeactivation,
  );

  const custody = await recoveryFixture();
  const custodyNonce = custody.state.resource_creation.attempt_nonce;
  let persistenceCalls = 0;
  await expectFailure(
    () =>
      recoverResourceCreation({
        config: cfg,
        ...custody,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {
          persistenceCalls += 1;
          if (persistenceCalls === 2) {
            throw new Error("fixture recovered custody persistence failure");
          }
        },
        resourceId: APP_ID,
        attemptNonce: custodyNonce,
      }),
    "oidc_secret_creation_recovery_required",
  );
  assert.equal(custody.state.oidc_app, null);
  assert.notEqual(custody.state.resource_creation.recovery_rotation, null);
  assert.equal(
    custody.transport.calls.filter((call) =>
      call.url.endsWith(`/${APP_ID}/rotate-secret`),
    ).length,
    1,
  );
});

test("unproven one-time secret response shapes require explicit recovery", async () => {
  const cfg = validateConfig(config());

  const oidcState = defaultState(cfg);
  const oidcTransport = new FixtureTransport({
    appPresent: false,
    accountPresent: false,
    handler(call) {
      if (
        call.method === "POST" &&
        call.url.endsWith("/api/v1/developer/oauth-clients")
      ) {
        return { status: 200, json: app(cfg) };
      }
    },
  });
  await expectFailure(
    () =>
      applyProvisioning({
        config: cfg,
        state: oidcState,
        transport: oidcTransport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "oidc_secret_creation_recovery_required",
  );

  const accountState = stateWithResources(cfg);
  accountState.storage_service_account = null;
  accountState.buckets.owner_service_account_id = null;
  const accountTransport = new FixtureTransport({
    accountPresent: false,
    handler(call) {
      if (
        call.method === "POST" &&
        call.url.endsWith("/api/v1/admin/service-accounts")
      ) {
        return { status: 200, json: serviceAccount(cfg) };
      }
    },
  });
  await expectFailure(
    () =>
      applyProvisioning({
        config: cfg,
        state: accountState,
        transport: accountTransport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "storage_secret_creation_recovery_required",
  );
});

test("foreign or administrator-forbidden buckets are never created or transferred", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  for (const administratorForbidden of [false, true]) {
    const transport = new FixtureTransport({
      handler(call) {
        if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
          return { status: 200, json: storageToken() };
        }
        if (call.method === "GET" && call.url.includes("/api/buckets/")) {
          const bucket = call.url.split("/").at(-1);
          return administratorForbidden
            ? { status: 403, json: errorEnvelope("PERMISSION_DENIED") }
            : { status: 200, json: bucketDetail(bucket, OTHER_ID) };
        }
      },
    });
    await expectFailure(
      () =>
        applyProvisioning({
          config: cfg,
          state: structuredClone(state),
          transport,
          adminToken: ADMIN_TOKEN,
          persistState: async () => {},
        }),
      "bucket_authority_drift",
    );
    assert.equal(
      transport.calls.filter(
        (call) => call.method === "POST" && call.url.endsWith("/api/buckets"),
      ).length,
      0,
    );
  }
});

test("backing-provider create denial is a typed provisioning prerequisite", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  const transport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        return { status: 200, json: storageToken() };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        return { status: 404, json: errorEnvelope("BUCKET_NOT_FOUND") };
      }
      if (call.method === "POST" && call.url.endsWith("/api/buckets")) {
        return { status: 403, json: errorEnvelope("S3_ACCESS_DENIED") };
      }
    },
  });
  await expectFailure(
    () =>
      applyProvisioning({
        config: cfg,
        state,
        transport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "bucket_provider_provisioning_required",
  );
  assert.equal(
    transport.calls.filter(
      (call) => call.method === "POST" && call.url.endsWith("/api/buckets"),
    ).length,
    1,
  );
});

test("reviewed provider adoption binds only the two exact empty staging buckets", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-adoption-refresh",
    scope: CALLER_SCOPES.join(" "),
    verification_state: "verified",
  };
  state.handoff_binding = { sha256: "d".repeat(64) };
  const adopted = new Set();
  const transport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        return { status: 200, json: storageToken() };
      }
      if (call.method === "PUT" && call.url.endsWith("/ownership")) {
        const bucket = call.url.split("/").at(-2);
        assert.ok(
          new Set([
            cfg.storage.context_bucket,
            cfg.storage.trigger_workspace_bucket,
          ]).has(bucket),
        );
        assert.equal(call.token, ADMIN_TOKEN);
        assert.deepEqual(call.json, { owner: SA_ID });
        adopted.add(bucket);
        return {
          status: 200,
          json: {
            data: {
              name: bucket,
              owner: SA_ID,
              updatedAt: "2026-08-29T00:00:00Z",
            },
            error: null,
          },
        };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        const bucket = call.url.split("/").at(-1);
        return adopted.has(bucket)
          ? { status: 200, json: bucketDetail(bucket) }
          : { status: 404, json: errorEnvelope("BUCKET_NOT_FOUND") };
      }
    },
  });
  const persisted = [];
  const result = await adoptProviderBuckets({
    config: cfg,
    state,
    transport,
    adminToken: ADMIN_TOKEN,
    persistState: async (next) => persisted.push(structuredClone(next)),
  });
  assert.deepEqual(
    result.adopted,
    [cfg.storage.context_bucket, cfg.storage.trigger_workspace_bucket].map(
      (bucket) => ({ bucket, result: "adopted" }),
    ),
  );
  assert.equal(persisted.length, 1);
  assert.equal(persisted[0].handoff_binding, null);
  assert.equal(
    transport.calls.filter((call) => call.method === "PUT").length,
    2,
  );

  const absentTransport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        return { status: 200, json: storageToken() };
      }
      if (call.method === "PUT" && call.url.endsWith("/ownership")) {
        return { status: 404, json: errorEnvelope("BUCKET_NOT_FOUND") };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        return call.token === ADMIN_TOKEN
          ? { status: 404, json: errorEnvelope("BUCKET_NOT_FOUND") }
          : { status: 403, json: errorEnvelope("NOT_BUCKET_OWNER") };
      }
    },
  });
  await expectFailure(
    () =>
      adoptProviderBuckets({
        config: cfg,
        state: stateWithResources(cfg),
        transport: absentTransport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "bucket_provider_provisioning_required",
  );

  const nonemptyBucket = cfg.storage.context_bucket;
  const retryTransport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        return { status: 200, json: storageToken() };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        const bucket = call.url.split("/").at(-1);
        const detail = bucketDetail(bucket);
        if (bucket === nonemptyBucket) {
          detail.data.objects.push({ key: "unexpected-provider-object" });
        }
        return { status: 200, json: detail };
      }
    },
  });
  await expectFailure(
    () =>
      adoptProviderBuckets({
        config: cfg,
        state: stateWithResources(cfg),
        transport: retryTransport,
        adminToken: ADMIN_TOKEN,
        persistState: async () => {},
      }),
    "bucket_adoption_not_empty",
  );
  assert.equal(
    retryTransport.calls.filter((call) => call.method === "PUT").length,
    0,
  );
});

test("ambiguous caller exchange fences every provisioning and mint mutation", async () => {
  const cfg = validateConfig(config());
  const state = stateWithCallerExchange(cfg);
  const snapshot = {
    discovery: discovery(),
    observer: observer(cfg),
    app: app(cfg),
    serviceAccount: serviceAccount(cfg),
    adminSubjectId: SUBJECT_ID,
  };
  const transport = new FixtureTransport();
  let persisted = 0;
  const common = {
    config: cfg,
    state,
    transport,
    adminToken: ADMIN_TOKEN,
    persistState: async () => {
      persisted += 1;
    },
  };
  for (const action of [
    () => applyProvisioning(common),
    () => adoptProviderBuckets(common),
    () =>
      authorizeCallerCustody({
        ...common,
        snapshot,
        serviceId: GITHUB_USER_SERVICE_ID,
      }),
    () =>
      mintCallerToken({
        ...common,
        snapshot,
        outputPath: "/tmp/cma-trigger-unreachable-handoff.json",
        serviceId: GITHUB_USER_SERVICE_ID,
      }),
  ]) {
    await expectFailure(
      action,
      "caller_authorization_exchange_recovery_required",
    );
  }
  assert.equal(transport.calls.length, 0);
  assert.equal(persisted, 0);
});

test("ambiguous caller app retirement is exact, idempotent, and retains the incident marker", async () => {
  const cfg = validateConfig(config());
  for (const ambiguousResponse of [false, true]) {
    const state = stateWithCallerExchange(cfg);
    let active = true;
    const transport = new FixtureTransport({
      handler(call) {
        if (
          call.method === "GET" &&
          call.url.endsWith(`/api/v1/developer/oauth-clients/${APP_ID}`)
        ) {
          const value = app(cfg);
          value.is_active = active;
          return { status: 200, json: value };
        }
        if (
          call.method === "DELETE" &&
          call.url.endsWith(`/api/v1/developer/oauth-clients/${APP_ID}`)
        ) {
          active = false;
          if (ambiguousResponse) {
            throw new BootstrapFailure(
              "remote_transport_error",
              "fixture retirement response loss",
            );
          }
          return {
            status: 200,
            json: { message: "OAuth client deactivated" },
          };
        }
      },
    });
    const result = await retireAmbiguousCallerApp({
      config: cfg,
      state,
      transport,
      adminToken: ADMIN_TOKEN,
    });
    assert.equal(result.status, "caller_app_retired");
    assert.equal(result.app_id, APP_ID);
    assert.equal(result.incident_marker_retained, true);
    assert.equal(result.issuance_fence_proven, false);
    assert.equal(state.caller_exchange.app_id, APP_ID);
    assert.equal(
      transport.calls.filter((call) => call.method === "DELETE").length,
      1,
    );
    const repeated = await retireAmbiguousCallerApp({
      config: cfg,
      state,
      transport,
      adminToken: ADMIN_TOKEN,
    });
    assert.equal(repeated.status, "caller_app_already_retired");
    assert.equal(
      transport.calls.filter((call) => call.method === "DELETE").length,
      1,
    );
  }

  const unresolvedState = stateWithCallerExchange(cfg);
  const unresolved = new FixtureTransport({
    handler(call) {
      if (
        call.method === "GET" &&
        call.url.endsWith(`/api/v1/developer/oauth-clients/${APP_ID}`)
      ) {
        return { status: 200, json: app(cfg) };
      }
      if (call.method === "DELETE") {
        throw new BootstrapFailure(
          "remote_transport_error",
          "fixture pre-write ambiguity",
        );
      }
    },
  });
  await expectFailure(
    () =>
      retireAmbiguousCallerApp({
        config: cfg,
        state: unresolvedState,
        transport: unresolved,
        adminToken: ADMIN_TOKEN,
      }),
    "caller_app_retirement_recovery_required",
  );
  assert.equal(unresolvedState.caller_exchange.app_id, APP_ID);
  assert.equal(
    unresolved.calls.filter((call) => call.method === "DELETE").length,
    1,
  );
});

test("production and open config shapes are rejected before network", async () => {
  const cases = [
    (value) => (value.extra = true),
    (value) => (value.observer.name = "CMA Trigger production observer"),
    (value) =>
      (value.oidc_app.redirect_uris[0] =
        "https://bot.chrono-ai.fun/auth/callback"),
    (value) => (value.storage.context_bucket = "chrono-platform-bot"),
    (value) => (value.nyxid_base_url = "https://nyx-staging.example.test"),
  ];
  for (const mutate of cases) {
    const value = config();
    mutate(value);
    await expectFailure(
      () => validateConfig(value),
      Object.hasOwn(value, "extra")
        ? "invalid_config"
        : "production_config_rejected",
    );
  }
});

test("secret files reject unsafe mode, symlinks, and mismatched authority", async () => {
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-nyxid-test-")),
  );
  await chmod(directory, 0o700);
  const unsafe = join(directory, "unsafe.json");
  await writeFile(unsafe, "{}\n", { mode: 0o644 });
  await expectFailure(() => readSecureJson(unsafe), "unsafe_secret_file");
  const target = join(directory, "target.json");
  await writeFile(target, "{}\n", { mode: 0o600 });
  const link = join(directory, "link.json");
  await symlink(target, link);
  await expectFailure(() => readSecureJson(link), "unsafe_secret_file");

  const writableAncestor = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-writable-ancestor-")),
  );
  const nestedCustody = join(writableAncestor, "custody");
  await mkdir(nestedCustody, { mode: 0o700 });
  const nestedState = join(nestedCustody, "state.json");
  await writeFile(nestedState, "{}\n", { mode: 0o600 });
  await chmod(writableAncestor, 0o777);
  await expectFailure(
    () => readSecureJson(nestedState),
    "unsafe_secret_path",
  );
  await chmod(writableAncestor, 0o700);

  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: OTHER_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-refresh-value",
    scope: CALLER_SCOPES.join(" "),
  };
  await expectFailure(
    () => validateState(state, cfg),
    "state_authority_mismatch",
  );

  const serviceDriftState = stateWithResources(cfg);
  serviceDriftState.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: OTHER_ID,
    refresh_token: "fixture-refresh-value",
    scope: CALLER_SCOPES.join(" "),
  };
  const noNetwork = new FixtureTransport();
  await expectFailure(
    () =>
      mintCallerToken({
        config: cfg,
        state: serviceDriftState,
        snapshot: {
          observer: {},
          app: {},
          serviceAccount: {},
        },
        transport: noNetwork,
        persistState: async () => {},
        outputPath: join(directory, "unused-handoff.json"),
        serviceId: GITHUB_USER_SERVICE_ID,
      }),
    "caller_authority_drift",
  );
  assert.equal(noNetwork.calls.length, 0);
});

test("caller state lock excludes concurrent writers and quarantines a dead owner", async () => {
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-lock-test-")),
  );
  await chmod(directory, 0o700);
  const statePath = join(directory, "state.json");
  const lockPath = `${statePath}.caller-authority.lock`;
  await withCallerStateLock(statePath, async () => {
    await expectFailure(
      () => withCallerStateLock(statePath, async () => {}),
      "caller_state_locked",
    );
    const lock = await readSecureJson(lockPath);
    assert.equal(lock.pid, process.pid);
  });
  await assert.rejects(readFile(lockPath), { code: "ENOENT" });

  await writeFile(
    lockPath,
    `${JSON.stringify({
      schema_version: 1,
      pid: 2_147_483_647,
      hostname: os.hostname(),
      nonce: "a".repeat(64),
    })}\n`,
    { mode: 0o600 },
  );
  await expectFailure(
    () => withCallerStateLock(statePath, async () => {}),
    "caller_state_lock_stale",
  );
  assert.equal((await readSecureJson(lockPath)).nonce, "a".repeat(64));
  await unlink(lockPath);

  await expectFailure(
    () =>
      withCallerStateLock(statePath, async () => {
        await writeFile(
          lockPath,
          `${JSON.stringify({
            schema_version: 1,
            pid: process.pid,
            hostname: os.hostname(),
            nonce: "b".repeat(64),
          })}\n`,
          { mode: 0o600 },
        );
        throw new BootstrapFailure("fixture_primary_failure", "fixture failure");
      }),
    "caller_state_lock_release_failed",
  );
  await unlink(lockPath);
});

test("authority outputs cannot alias caller state or its lock", async () => {
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-path-test-")),
  );
  await chmod(directory, 0o700);
  const statePath = join(directory, "state.json");
  await expectFailure(
    () => requireDistinctAuthorityPaths(statePath, statePath),
    "usage",
  );
  await expectFailure(
    () =>
      requireDistinctAuthorityPaths(
        statePath,
        `${statePath}.caller-authority.lock`,
      ),
    "usage",
  );
  await expectFailure(
    () =>
      requireDistinctAuthorityPaths(
        statePath,
        join(directory, "STATE.JSON.CALLER-AUTHORITY.LOCK"),
      ),
    "usage",
  );

  const aliasRoot = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-path-alias-test-")),
  );
  await chmod(aliasRoot, 0o700);
  const alias = join(aliasRoot, "alias");
  await symlink(directory, alias);
  await expectFailure(
    () => requireDistinctAuthorityPaths(statePath, join(alias, "state.json")),
    "unsafe_secret_path",
  );
  await expectFailure(
    () =>
      requireDistinctAuthorityPaths(
        join(directory, "st\u00e1te.json"),
        join(directory, "sta\u0301te.json"),
      ),
    "usage",
  );
  const inodeStatePath = join(directory, "inode-state.json");
  const inodeAlias = join(directory, "inode-output.json");
  await writeFile(inodeStatePath, "{}\n", { mode: 0o600 });
  await link(inodeStatePath, inodeAlias);
  await expectFailure(
    () => requireDistinctAuthorityPaths(inodeStatePath, inodeAlias),
    "usage",
  );

  const filesystemFoldedOutput = join(
    directory,
    "\u017Ftate.json.caller-authority.lock",
  );
  let filesystemAliasesLock = false;
  await withCallerStateLock(statePath, async () => {
    const lockIdentity = await lstat(
      `${statePath}.caller-authority.lock`,
    );
    let aliasIdentity;
    try {
      aliasIdentity = await lstat(filesystemFoldedOutput);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
    }
    filesystemAliasesLock =
      aliasIdentity !== undefined &&
      aliasIdentity.dev === lockIdentity.dev &&
      aliasIdentity.ino === lockIdentity.ino;
    if (filesystemAliasesLock) {
      await expectFailure(
        () =>
          requireDistinctAuthorityPaths(statePath, filesystemFoldedOutput),
        "usage",
      );
    }
  });
  // Case-insensitive APFS folds long-s to s while JavaScript's lowercase/NFC
  // key does not. Other filesystems legitimately keep the names distinct.
});

test("OAuth callback parser rejects wrong path, state, parameters, and provider errors", async () => {
  const expected = { port: 8765, path: "/callback", state: "expected-state" };
  assert.equal(
    parseCallbackRequest(
      "/callback?code=fixture-code&state=expected-state",
      expected,
    ),
    "fixture-code",
  );
  for (const [url, code] of [
    [
      "/wrong?code=fixture-code&state=expected-state",
      "oauth_callback_rejected",
    ],
    ["/callback?code=fixture-code&state=wrong", "oauth_callback_rejected"],
    [
      "/callback?code=fixture-code&state=expected-state&extra=1",
      "oauth_callback_rejected",
    ],
    [
      "/callback?error=access_denied&state=expected-state",
      "oauth_provider_error",
    ],
  ]) {
    await expectFailure(() => parseCallbackRequest(url, expected), code);
  }
});

test("issuer TTL witness requires fresh exact-subject first-party authority", async () => {
  const token = issuerAccessToken();
  const witness = captureIssuerTtlWitness(token, ADMIN_ID, NOW);
  assert.equal(witness.issuer_token_issued_at, NOW);
  assert.equal(witness.issuer_access_token_ttl_seconds, 8_000);
  assert.equal(witness.issuer_commit, NYXID_SOURCE_REVISION.slice(0, 12));
  assert.equal(witness.issuer_token_sha256.length, 64);

  for (const claims of [
    { sub: OTHER_ID },
    { aud: "https://other.example.test" },
    { client_id: APP_ID },
    { delegated: true },
    { sa: true },
    { relay: true },
    { assistant_forward: true },
    { iat: NOW + 31, exp: NOW + 8_031 },
    { iat: NOW - 901, exp: NOW + 7_099 },
    { exp: NOW + 28_801 },
  ]) {
    await expectFailure(
      () => captureIssuerTtlWitness(issuerAccessToken(claims), ADMIN_ID, NOW),
      "issuer_ttl_witness_invalid",
    );
  }
  await expectFailure(
    () => captureIssuerTtlWitness(token, OTHER_ID, NOW),
    "issuer_ttl_witness_invalid",
  );
});

test("caller authorization CLI accepts headless custody without a handoff output", async () => {
  const configPath = join(os.tmpdir(), "cma-trigger-config.json");
  const statePath = join(os.tmpdir(), "cma-trigger-state.json");
  const handoffPath = join(os.tmpdir(), "cma-trigger-handoff.json");
  const authorized = parseArguments([
    "--authorize-caller",
    configPath,
    "--state",
    statePath,
    "--no-browser",
  ]);
  assert.equal(authorized.mode, "--authorize-caller");
  assert.equal(authorized.statePath, statePath);
  assert.equal(authorized.outputPath, null);
  assert.equal(authorized.noBrowser, true);

  const minted = parseArguments([
    "--mint-caller-token",
    configPath,
    "--state",
    statePath,
    "--output",
    handoffPath,
    "--no-browser",
  ]);
  assert.equal(minted.mode, "--mint-caller-token");
  assert.equal(minted.outputPath, handoffPath);
  assert.equal(minted.noBrowser, true);

  await expectFailure(
    () =>
      parseArguments([
        "--authorize-caller",
        configPath,
        "--state",
        statePath,
        "--output",
        handoffPath,
      ]),
    "usage",
  );
  await expectFailure(
    () =>
      parseArguments([
        "--mint-caller-token",
        configPath,
        "--state",
        statePath,
        "--output",
        `${statePath}.caller-authority.lock`,
      ]),
    "usage",
  );
});

test("caller authorization establishes exact refresh custody without touching storage", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  let authorization;
  const persisted = [];
  const transport = new FixtureTransport({
    handler(call) {
      if (
        call.method === "POST" &&
        call.url.endsWith("/oauth/token") &&
        call.form.grant_type === "authorization_code"
      ) {
        return {
          status: 200,
          json: {
            access_token: "fixture-authorized-caller-access",
            token_type: "Bearer",
            expires_in: 8_000,
            refresh_token: "fixture-authorized-refresh-value",
            id_token: "fixture-authorized-id-token",
            scope: CALLER_SCOPES.join(" "),
          },
        };
      }
    },
  });
  const result = await authorizeCallerCustody({
    config: cfg,
    state,
    snapshot: {
      discovery: discovery(),
      observer: observer(cfg),
      app: app(cfg),
      serviceAccount: serviceAccount(cfg),
      adminSubjectId: SUBJECT_ID,
    },
    transport,
    persistState: async (next) => persisted.push(structuredClone(next)),
    serviceId: GITHUB_USER_SERVICE_ID,
    issuerTtlWitness: issuerTtlWitness(),
    authorize: async (request) => {
      authorization = request;
      return "fixture-authorization-code";
    },
    verifier: async ({ nonce }) => {
      assert.equal(nonce, authorization.nonce);
      return {
        subjectId: SUBJECT_ID,
        expiresAt: NOW + 8_000,
        allowedServiceIds: [GITHUB_USER_SERVICE_ID],
      };
    },
  });
  assert.equal(result.subjectId, SUBJECT_ID);
  assert.equal(result.accessToken, "fixture-authorized-caller-access");
  assert.equal(
    persisted.at(-1).caller.refresh_token,
    "fixture-authorized-refresh-value",
  );
  assert.equal(persisted.at(-1).caller.service_id, GITHUB_USER_SERVICE_ID);
  assert.deepEqual(
    persisted
      .filter((snapshot) => snapshot.caller !== null)
      .map((snapshot) => snapshot.caller.verification_state),
    ["pending", "verified"],
  );
  assert.equal(persisted[0].caller, null);
  assert.equal(persisted[0].caller_exchange.app_id, APP_ID);
  assert.equal(persisted[1].caller_exchange, null);
  assert.equal(
    transport.calls.some(
      (call) =>
        call.form?.grant_type === "client_credentials" ||
        call.url.includes("/api/buckets/"),
    ),
    false,
  );
});

test("authorization-only invalidates every prior final-mint binding", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.handoff_binding = { sha256: "a".repeat(64) };
  const persisted = [];
  const transport = new FixtureTransport({
    handler(call) {
      if (call.form?.grant_type === "authorization_code") {
        return {
          status: 200,
          json: {
            access_token: "fixture-new-access",
            token_type: "Bearer",
            expires_in: 8_000,
            refresh_token: "fixture-new-refresh",
            id_token: "fixture-new-id",
            scope: CALLER_SCOPES.join(" "),
          },
        };
      }
    },
  });
  await authorizeCallerCustody({
    config: cfg,
    state,
    snapshot: {
      discovery: discovery(),
      observer: observer(cfg),
      app: app(cfg),
      serviceAccount: serviceAccount(cfg),
      adminSubjectId: SUBJECT_ID,
    },
    transport,
    persistState: async (next) => persisted.push(structuredClone(next)),
    serviceId: GITHUB_USER_SERVICE_ID,
    issuerTtlWitness: issuerTtlWitness(),
    authorize: async () => "fixture-code",
    verifier: async () => ({
      subjectId: SUBJECT_ID,
      expiresAt: NOW + 8_000,
      allowedServiceIds: [GITHUB_USER_SERVICE_ID],
    }),
  });
  assert.equal(state.handoff_binding, null);
  assert.equal(persisted[0].handoff_binding, null);
  assert.equal(
    persisted.some((entry) => entry.handoff_binding !== null),
    false,
  );
});

test("authorization-code ambiguity requires dedicated-app recovery", async () => {
  const cfg = validateConfig(config());
  const snapshot = {
    discovery: discovery(),
    observer: observer(cfg),
    app: app(cfg),
    serviceAccount: serviceAccount(cfg),
    adminSubjectId: SUBJECT_ID,
  };
  const lostState = stateWithResources(cfg);
  const lostPersisted = [];
  let exchangeCalls = 0;
  await expectFailure(
    () =>
      authorizeCallerCustody({
        config: cfg,
        state: lostState,
        snapshot,
        transport: new FixtureTransport({
          handler(call) {
            if (call.form?.grant_type === "authorization_code") {
              exchangeCalls += 1;
              throw new BootstrapFailure(
                "remote_transport_error",
                "fixture response loss",
              );
            }
          },
        }),
        persistState: async (next) => lostPersisted.push(structuredClone(next)),
        serviceId: GITHUB_USER_SERVICE_ID,
        issuerTtlWitness: issuerTtlWitness(),
        authorize: async () => "fixture-code",
      }),
    "caller_authorization_exchange_recovery_required",
  );
  assert.equal(exchangeCalls, 1);
  assert.equal(lostState.caller, null);
  assert.equal(lostState.caller_exchange.app_id, APP_ID);
  assert.equal(lostPersisted.length, 1);
  assert.equal(lostPersisted[0].caller_exchange.app_id, APP_ID);

  const unpersistedState = stateWithResources(cfg);
  let persistenceCalls = 0;
  await expectFailure(
    () =>
      authorizeCallerCustody({
        config: cfg,
        state: unpersistedState,
        snapshot,
        transport: new FixtureTransport({
          handler(call) {
            if (call.form?.grant_type === "authorization_code") {
              return {
                status: 200,
                json: {
                  access_token: "fixture-unpersisted-access",
                  token_type: "Bearer",
                  expires_in: 8_000,
                  refresh_token: "fixture-unpersisted-refresh",
                  id_token: "fixture-unpersisted-id",
                  scope: CALLER_SCOPES.join(" "),
                },
              };
            }
          },
        }),
        persistState: async () => {
          persistenceCalls += 1;
          if (persistenceCalls > 1) throw new Error("fixture disk fault");
        },
        serviceId: GITHUB_USER_SERVICE_ID,
        issuerTtlWitness: issuerTtlWitness(),
        authorize: async () => "fixture-code",
        verifier: async () => {
          throw new Error("verification must remain unreachable");
        },
      }),
    "caller_authorization_exchange_recovery_required",
  );
});

test("final handoff mint verifies both buckets before rotating caller custody", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-retained-refresh-value",
    scope: CALLER_SCOPES.join(" "),
  };
  const transport = new FixtureTransport({
    handler(call) {
      if (
        call.method === "POST" &&
        call.url.endsWith("/oauth/token") &&
        call.form.grant_type === "client_credentials"
      ) {
        return { status: 200, json: storageToken() };
      }
      if (
        call.method === "GET" &&
        call.url.endsWith(`/api/buckets/${cfg.storage.context_bucket}`)
      ) {
        return {
          status: 200,
          json: bucketDetail(cfg.storage.context_bucket),
        };
      }
      if (
        call.method === "GET" &&
        call.url.endsWith(
          `/api/buckets/${cfg.storage.trigger_workspace_bucket}`,
        )
      ) {
        return { status: 404, json: errorEnvelope("BUCKET_NOT_FOUND") };
      }
    },
  });
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-storage-gate-test-")),
  );
  await chmod(directory, 0o700);
  const outputPath = join(directory, "handoff.json");
  state.handoff_binding = { sha256: "b".repeat(64) };
  await writeFile(outputPath, '{"prior":"handoff"}\n', { mode: 0o600 });
  const persisted = [];
  await expectFailure(
    () =>
      mintCallerToken({
        config: cfg,
        state,
        snapshot: {
          discovery: discovery(),
          observer: observer(cfg),
          app: app(cfg),
          serviceAccount: serviceAccount(cfg),
          adminSubjectId: SUBJECT_ID,
        },
        transport,
        persistState: async (next) => persisted.push(structuredClone(next)),
        outputPath,
        serviceId: GITHUB_USER_SERVICE_ID,
        verifier: async () => {
          throw new Error("caller verification must remain unreachable");
        },
      }),
    "bucket_not_ready",
  );
  assert.equal(persisted.length, 1);
  assert.equal(persisted[0].handoff_binding, null);
  assert.equal(
    transport.calls.some(
      (call) => call.form?.grant_type === "refresh_token",
    ),
    false,
  );
  assert.deepEqual(JSON.parse(await readFile(outputPath, "utf8")), {
    schema_version: 1,
    kind: "cma-trigger-handoff-invalid",
    status: "mint_in_progress",
  });
  assert.equal(state.handoff_binding, null);
});

test("ambiguous refresh retries the predecessor once and then fails typed", async () => {
  const cfg = validateConfig(config());
  const snapshot = {
    discovery: discovery(),
    observer: observer(cfg),
    app: app(cfg),
    serviceAccount: serviceAccount(cfg),
    adminSubjectId: SUBJECT_ID,
  };
  const caller = () => ({
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-predecessor-refresh",
    scope: CALLER_SCOPES.join(" "),
  });

  const recoveredState = stateWithResources(cfg);
  recoveredState.caller = caller();
  const refreshInputs = [];
  const recoveredTransport = new FixtureTransport({
    handler(call) {
      if (call.form?.grant_type !== "refresh_token") return;
      refreshInputs.push(call.form.refresh_token);
      if (refreshInputs.length === 1) {
        throw new BootstrapFailure(
          "remote_transport_error",
          "fixture response loss",
        );
      }
      return {
        status: 200,
        json: {
          access_token: "fixture-retried-access",
          token_type: "Bearer",
          expires_in: 8_000,
          refresh_token: "fixture-retried-refresh",
          id_token: null,
          scope: null,
        },
      };
    },
  });
  await authorizeCallerCustody({
    config: cfg,
    state: recoveredState,
    snapshot,
    transport: recoveredTransport,
    persistState: async () => {},
    serviceId: GITHUB_USER_SERVICE_ID,
    verifier: async () => ({
      subjectId: SUBJECT_ID,
      expiresAt: NOW + 8_000,
      allowedServiceIds: [GITHUB_USER_SERVICE_ID],
    }),
  });
  assert.deepEqual(refreshInputs, [
    "fixture-predecessor-refresh",
    "fixture-predecessor-refresh",
  ]);
  assert.equal(recoveredState.caller.refresh_token, "fixture-retried-refresh");
  assert.equal(recoveredState.caller.verification_state, "verified");

  const failedState = stateWithResources(cfg);
  failedState.caller = caller();
  let failedCalls = 0;
  const failedTransport = new FixtureTransport({
    handler(call) {
      if (call.form?.grant_type !== "refresh_token") return;
      failedCalls += 1;
      throw new BootstrapFailure(
        "remote_transport_error",
        "fixture repeated response loss",
      );
    },
  });
  let persisted = false;
  await expectFailure(
    () =>
      authorizeCallerCustody({
        config: cfg,
        state: failedState,
        snapshot,
        transport: failedTransport,
        persistState: async () => {
          persisted = true;
        },
        serviceId: GITHUB_USER_SERVICE_ID,
      }),
    "caller_refresh_recovery_required",
  );
  assert.equal(failedCalls, 2);
  assert.equal(persisted, false);
  assert.equal(
    failedState.caller.refresh_token,
    "fixture-predecessor-refresh",
  );
});

test("deterministic first-consent authority drift revokes and reauthorizes once", async () => {
  for (const failure of ["service", "subject"]) {
    const cfg = validateConfig(config());
    const state = stateWithResources(cfg);
    let authorizationCount = 0;
    let tokenCount = 0;
    let verificationCount = 0;
    const revoked = [];
    const persisted = [];
    const transport = new FixtureTransport({
      handler(call) {
        if (call.method === "POST" && call.url.endsWith("/oauth/revoke")) {
          revoked.push(call.form.token);
          return { status: 200, json: null };
        }
        if (
          call.method === "POST" &&
          call.url.endsWith("/oauth/token") &&
          call.form.grant_type === "refresh_token"
        ) {
          return {
            status: 401,
            json: {
              error: "invalid_client",
              error_description: "fixture revoked refresh",
            },
          };
        }
        if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
          tokenCount += 1;
          return {
            status: 200,
            json: {
              access_token: `fixture-reauthorized-access-${tokenCount}`,
              token_type: "Bearer",
              expires_in: 8_000,
              refresh_token: `fixture-reauthorized-refresh-${tokenCount}`,
              id_token: `fixture-reauthorized-id-${tokenCount}`,
              scope: CALLER_SCOPES.join(" "),
            },
          };
        }
      },
    });
    const result = await authorizeCallerCustody({
      config: cfg,
      state,
      snapshot: {
        discovery: discovery(),
        observer: observer(cfg),
        app: app(cfg),
        serviceAccount: serviceAccount(cfg),
        adminSubjectId: SUBJECT_ID,
      },
      transport,
      persistState: async (next) => persisted.push(structuredClone(next)),
      serviceId: GITHUB_USER_SERVICE_ID,
      issuerTtlWitness: issuerTtlWitness(),
      authorize: async () => {
        authorizationCount += 1;
        return `fixture-reauthorization-code-${authorizationCount}`;
      },
      verifier: async () => {
        verificationCount += 1;
        if (verificationCount === 1 && failure === "service") {
          throw new BootstrapFailure(
            "caller_service_authority_denied",
            "fixture wrong selected service",
          );
        }
        return {
          subjectId:
            verificationCount === 1 && failure === "subject"
              ? OTHER_ID
              : SUBJECT_ID,
          expiresAt: NOW + 8_000,
          allowedServiceIds: [GITHUB_USER_SERVICE_ID],
        };
      },
    });
    assert.equal(result.subjectId, SUBJECT_ID);
    assert.equal(authorizationCount, 2);
    assert.equal(tokenCount, 2);
    assert.deepEqual(revoked, ["fixture-reauthorized-refresh-1"]);
    assert.equal(state.caller.refresh_token, "fixture-reauthorized-refresh-2");
    assert.equal(state.caller.verification_state, "verified");
    assert.deepEqual(
      persisted.map((entry) => entry.caller?.verification_state ?? null),
      [null, "pending", null, null, "pending", "verified"],
    );
    assert.deepEqual(
      persisted.map((entry) => entry.caller_exchange !== null),
      [true, false, false, true, false, false],
    );
  }
});

test("drifted revocation-probe response retains its recovered replacement as pending", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-original-refresh",
    scope: CALLER_SCOPES.join(" "),
    verification_state: "verified",
  };
  const probeInputs = [];
  let probeAttempts = 0;
  const transport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/revoke")) {
        return { status: 200, json: null };
      }
      if (call.form?.grant_type === "refresh_token") {
        if (call.form.refresh_token === "fixture-original-refresh") {
          return {
            status: 200,
            json: {
              access_token: "fixture-drifted-access",
              token_type: "Bearer",
              expires_in: 8_000,
              refresh_token: "fixture-revocation-predecessor",
              id_token: null,
              scope: null,
            },
          };
        }
        probeInputs.push(call.form.refresh_token);
        probeAttempts += 1;
        if (probeAttempts === 1) {
          throw new BootstrapFailure(
            "remote_transport_error",
            "fixture lost revocation-probe response",
          );
        }
        return {
          status: 200,
          json: {
            access_token: "fixture-recovered-probe-access",
            token_type: "Bearer",
            expires_in: 8_000,
            refresh_token: "fixture-recovered-probe-refresh",
            id_token: null,
            scope: null,
            future: true,
          },
        };
      }
    },
  });
  const persisted = [];
  await expectFailure(
    () =>
      authorizeCallerCustody({
        config: cfg,
        state,
        snapshot: {
          discovery: discovery(),
          observer: observer(cfg),
          app: app(cfg),
          serviceAccount: serviceAccount(cfg),
          adminSubjectId: SUBJECT_ID,
        },
        transport,
        persistState: async (next) => persisted.push(structuredClone(next)),
        serviceId: GITHUB_USER_SERVICE_ID,
        verifier: async () => {
          throw new BootstrapFailure(
            "caller_authority_drift",
            "fixture deterministic drift",
          );
        },
      }),
    "caller_refresh_response_recovery_required",
  );
  assert.deepEqual(probeInputs, [
    "fixture-revocation-predecessor",
    "fixture-revocation-predecessor",
  ]);
  assert.equal(state.caller.refresh_token, "fixture-recovered-probe-refresh");
  assert.equal(state.caller.verification_state, "pending");
  assert.equal(persisted.at(-1).caller.verification_state, "pending");
});

test("failed caller verification retains rotated custody for automatic recovery", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-original-refresh-value",
    scope: CALLER_SCOPES.join(" "),
  };
  const refreshInputs = [];
  let refreshCount = 0;
  const transport = new FixtureTransport({
    handler(call) {
      if (
        call.method === "POST" &&
        call.url.endsWith("/oauth/token") &&
        call.form.grant_type === "refresh_token"
      ) {
        refreshInputs.push(call.form.refresh_token);
        refreshCount += 1;
        return {
          status: 200,
          json: {
            access_token: `fixture-recovery-access-${refreshCount}`,
            token_type: "Bearer",
            expires_in: 8_000,
            refresh_token: `fixture-recovery-refresh-${refreshCount}`,
            id_token: null,
            scope: null,
          },
        };
      }
    },
  });
  const snapshot = {
    discovery: discovery(),
    observer: observer(cfg),
    app: app(cfg),
    serviceAccount: serviceAccount(cfg),
    adminSubjectId: SUBJECT_ID,
  };
  const persisted = [];
  const persistState = async (next) =>
    persisted.push(structuredClone(next));
  await expectFailure(
    () =>
      authorizeCallerCustody({
        config: cfg,
        state,
        snapshot,
        transport,
        persistState,
        serviceId: GITHUB_USER_SERVICE_ID,
        verifier: async () => {
          throw new BootstrapFailure(
            "caller_introspection_failed",
            "fixture transient verification failure",
          );
        },
      }),
    "caller_introspection_failed",
  );
  assert.equal(state.caller.refresh_token, "fixture-recovery-refresh-1");
  assert.equal(state.caller.verification_state, "pending");
  assert.equal(persisted.at(-1).caller.verification_state, "pending");

  await authorizeCallerCustody({
    config: cfg,
    state,
    snapshot,
    transport,
    persistState,
    serviceId: GITHUB_USER_SERVICE_ID,
    verifier: async () => ({
      subjectId: SUBJECT_ID,
      expiresAt: NOW + 8_000,
      allowedServiceIds: [GITHUB_USER_SERVICE_ID],
    }),
  });
  assert.deepEqual(refreshInputs, [
    "fixture-original-refresh-value",
    "fixture-recovery-refresh-1",
  ]);
  assert.equal(state.caller.refresh_token, "fixture-recovery-refresh-2");
  assert.equal(state.caller.verification_state, "verified");
  assert.equal(persisted.at(-1).caller.verification_state, "verified");
});

test("refresh grant rotates custody and atomically writes the closed handoff", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  state.caller = {
    client_id: APP_ID,
    subject_id: SUBJECT_ID,
    service_id: GITHUB_USER_SERVICE_ID,
    refresh_token: "fixture-old-refresh-value",
    scope: CALLER_SCOPES.join(" "),
  };
  const refreshCalls = [];
  const transport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        if (call.form.grant_type === "client_credentials") {
          return { status: 200, json: storageToken() };
        }
        refreshCalls.push(call);
        return {
          status: 200,
          json: {
            access_token: "fixture-fresh-caller-access",
            token_type: "Bearer",
            expires_in: 8_000,
            refresh_token: "fixture-rotated-refresh-value",
            id_token: null,
            scope: null,
          },
        };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        return { status: 200, json: bucketDetail(call.url.split("/").at(-1)) };
      }
    },
  });
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-handoff-test-")),
  );
  await chmod(directory, 0o700);
  const outputPath = join(directory, "handoff.json");
  const persisted = [];
  let issuerWitnessCalls = 0;
  const snapshot = {
    discovery: discovery(),
    observer: observer(cfg),
    app: app(cfg),
    serviceAccount: serviceAccount(cfg),
    adminSubjectId: SUBJECT_ID,
  };
  const result = await mintCallerToken({
    config: cfg,
    state,
    snapshot,
    transport,
    persistState: async (next) => persisted.push(structuredClone(next)),
    outputPath,
    serviceId: GITHUB_USER_SERVICE_ID,
    issuerTtlWitness: () => {
      issuerWitnessCalls += 1;
      throw new Error("existing refresh must not request an issuer witness");
    },
    verifier: async () => ({
      subjectId: SUBJECT_ID,
      expiresAt: NOW + 8_000,
      allowedServiceIds: [GITHUB_USER_SERVICE_ID],
    }),
  });
  assert.equal(result.subjectId, SUBJECT_ID);
  assert.equal(
    persisted.at(-1).caller.refresh_token,
    "fixture-rotated-refresh-value",
  );
  assert.equal(persisted.at(-1).caller.service_id, GITHUB_USER_SERVICE_ID);
  assert.equal(refreshCalls.length, 1);
  assert.equal(issuerWitnessCalls, 0);
  assert.equal(refreshCalls[0].form.client_id, APP_ID);
  assert.equal(
    refreshCalls[0].form.client_secret,
    state.oidc_app.client_secret,
  );
  assert.deepEqual(
    transport.calls
      .filter(
        (call) =>
          call.url.endsWith("/oauth/token") ||
          call.url.includes("/api/buckets/"),
      )
      .map((call) => {
        if (call.form?.grant_type === "client_credentials")
          return "storage_token";
        if (call.form?.grant_type === "refresh_token") return "caller_refresh";
        return `bucket:${call.url.split("/").at(-1)}`;
      }),
    [
      "storage_token",
      `bucket:${cfg.storage.context_bucket}`,
      `bucket:${cfg.storage.trigger_workspace_bucket}`,
      "caller_refresh",
    ],
  );
  const handoff = JSON.parse(await readFile(outputPath, "utf8"));
  validateHandoff(handoff);
  assert.equal(handoff.subject_id, SUBJECT_ID);
  assert.equal(handoff.access_token, "fixture-fresh-caller-access");
  const secure = await readSecureJson(outputPath);
  assert.deepEqual(secure, handoff);
});

test("initial caller mint uses PKCE, state, nonce, and exact confidential app", async () => {
  const cfg = validateConfig(config());
  const state = stateWithResources(cfg);
  let authorization;
  let exchange;
  const transport = new FixtureTransport({
    handler(call) {
      if (call.method === "POST" && call.url.endsWith("/oauth/token")) {
        if (call.form.grant_type === "client_credentials") {
          return { status: 200, json: storageToken() };
        }
        exchange = call;
        return {
          status: 200,
          json: {
            access_token: "fixture-initial-caller-access",
            token_type: "Bearer",
            expires_in: 8_000,
            refresh_token: "fixture-initial-refresh-value",
            id_token: "fixture-initial-id-token",
            scope: CALLER_SCOPES.join(" "),
          },
        };
      }
      if (call.method === "GET" && call.url.includes("/api/buckets/")) {
        return { status: 200, json: bucketDetail(call.url.split("/").at(-1)) };
      }
    },
  });
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-initial-test-")),
  );
  await chmod(directory, 0o700);
  await mintCallerToken({
    config: cfg,
    state,
    snapshot: {
      discovery: discovery(),
      observer: observer(cfg),
      app: app(cfg),
      serviceAccount: serviceAccount(cfg),
      adminSubjectId: SUBJECT_ID,
    },
    transport,
    persistState: async () => {},
    outputPath: join(directory, "handoff.json"),
    serviceId: GITHUB_USER_SERVICE_ID,
    issuerTtlWitness: issuerTtlWitness(),
    authorize: async (request) => {
      authorization = request;
      return "fixture-authorization-code";
    },
    verifier: async ({ nonce }) => {
      assert.equal(nonce, authorization.nonce);
      return {
        subjectId: SUBJECT_ID,
        expiresAt: NOW + 8_000,
        allowedServiceIds: [GITHUB_USER_SERVICE_ID],
      };
    },
  });
  const url = new URL(authorization.authorizationUrl);
  assert.equal(url.searchParams.get("code_challenge_method"), "S256");
  assert.equal(url.searchParams.get("state"), authorization.state);
  assert.equal(url.searchParams.get("nonce"), authorization.nonce);
  assert.equal(url.searchParams.get("client_id"), APP_ID);
  assert.equal(url.searchParams.get("scope"), CALLER_SCOPES.join(" "));
  assert.equal(exchange.form.code, "fixture-authorization-code");
  assert.equal(exchange.form.client_id, APP_ID);
  assert.equal(exchange.form.client_secret, state.oidc_app.client_secret);
  assert.ok(exchange.form.code_verifier.length >= 43);
});

function accessClaims(overrides = {}) {
  return {
    sub: SUBJECT_ID,
    iss: NYXID_BASE_URL,
    aud: NYXID_BASE_URL,
    exp: NOW + 8_000,
    iat: NOW,
    jti: "99999999-9999-4999-8999-999999999999",
    scope: CALLER_SCOPES.join(" "),
    token_type: "access",
    client_id: APP_ID,
    resources: [],
    allowed_service_ids: [GITHUB_USER_SERVICE_ID],
    allow_all_services: false,
    allowed_node_ids: [],
    allow_all_nodes: true,
    ...overrides,
  };
}

function introspection(overrides = {}) {
  const claims = accessClaims();
  return {
    active: true,
    scope: claims.scope,
    token_type: claims.token_type,
    exp: claims.exp,
    iat: claims.iat,
    sub: claims.sub,
    iss: claims.iss,
    jti: claims.jti,
    roles: ["admin"],
    groups: [],
    permissions: ["cma:*"],
    ...overrides,
  };
}

test("caller validation requires active exact-app, exact-service, permissions, scope, and lifetime", async () => {
  const claims = validateAccessClaims(
    accessClaims(),
    APP_ID,
    GITHUB_USER_SERVICE_ID,
    NOW,
  );
  assert.equal(
    validateIntrospection(introspection(), claims, APP_ID, NOW).subjectId,
    SUBJECT_ID,
  );
  await expectFailure(
    () =>
      validateAccessClaims(
        accessClaims({ client_id: OTHER_ID }),
        APP_ID,
        GITHUB_USER_SERVICE_ID,
        NOW,
      ),
    "caller_token_invalid",
  );
  await expectFailure(
    () =>
      validateAccessClaims(
        accessClaims({ exp: NOW + 30 }),
        APP_ID,
        GITHUB_USER_SERVICE_ID,
        NOW,
      ),
    "caller_token_invalid",
  );
  await expectFailure(
    () =>
      validateAccessClaims(
        accessClaims({ allowed_service_ids: [OTHER_ID] }),
        APP_ID,
        GITHUB_USER_SERVICE_ID,
        NOW,
      ),
    "caller_service_authority_denied",
  );
  await expectFailure(
    () =>
      validateAccessClaims(
        accessClaims({ allow_all_services: true }),
        APP_ID,
        GITHUB_USER_SERVICE_ID,
        NOW,
      ),
    "caller_service_authority_denied",
  );
  await expectFailure(
    () =>
      validateIntrospection(
        introspection({ active: false }),
        claims,
        APP_ID,
        NOW,
      ),
    "caller_introspection_failed",
  );
  await expectFailure(
    () =>
      validateIntrospection(
        introspection({ permissions: ["cma:session:create"] }),
        claims,
        APP_ID,
        NOW,
      ),
    "caller_permission_denied",
  );
  await expectFailure(
    () =>
      validateIntrospection(
        introspection({ permissions: ["cma:*"], future: true }),
        claims,
        APP_ID,
        NOW,
      ),
    "contract_drift",
  );
});

test("closed schemas reject mutations across every protected resource shape", async () => {
  const cfg = validateConfig(config());
  const protectedFields = [
    ["identity_include_user_id", true],
    ["identity_include_email", true],
    ["identity_include_name", true],
    ["identity_jwt_audience", "unexpected"],
    ["requires_user_credential", true],
    ["auth_method", "bearer"],
    ["service_category", "connection"],
    ["visibility", "private"],
  ];
  for (const [field, value] of protectedFields) {
    const candidate = observer(cfg);
    candidate[field] = value;
    await expectFailure(
      () => validateObserver(candidate, cfg),
      "observer_contract_drift",
    );
  }
  const openObserver = observer(cfg);
  openObserver.unknown = false;
  await expectFailure(
    () => validateObserver(openObserver, cfg),
    "contract_drift",
  );
  const openHandoff = {
    schema_version: 1,
    nyxid_base_url: NYXID_BASE_URL,
    nyxid_client_id: APP_ID,
    nyxid_client_secret: "fixture-app-value",
    storage_client_id: SA_CLIENT,
    storage_client_secret: "fixture-storage-value",
    storage_owner_service_account_id: SA_ID,
    context_bucket: cfg.storage.context_bucket,
    trigger_workspace_bucket: cfg.storage.trigger_workspace_bucket,
    public_github_service_slug: cfg.observer.slug,
    subject_id: SUBJECT_ID,
    access_token: "fixture-access-value",
    access_token_expires_at: NOW + 8_000,
    future: false,
  };
  await expectFailure(() => validateHandoff(openHandoff), "unsafe_handoff");
  assert.deepEqual(
    [...SA_ITEM_FIELDS].sort(),
    Object.keys(serviceAccount(cfg)).sort(),
  );
});

test("handoff upload binding accepts the exact GitHub limit and rejects one byte over", async () => {
  const exactRawBytes = (MAX_GITHUB_SECRET_PLAINTEXT_BYTES / 4) * 3;
  const exact = Buffer.alloc(exactRawBytes);
  assert.equal(
    Buffer.byteLength(exact.toString("base64"), "utf8"),
    MAX_GITHUB_SECRET_PLAINTEXT_BYTES,
  );
  assert.equal(requireHandoffUploadBound(exact), exact);
  await expectFailure(
    () => requireHandoffUploadBound(Buffer.alloc(exactRawBytes + 1)),
    "handoff_upload_too_large",
  );
});

let failures = 0;
for (const { name, run } of tests) {
  try {
    await run();
    process.stdout.write(`ok - ${name}\n`);
  } catch (error) {
    failures += 1;
    process.stderr.write(`not ok - ${name}\n${error.stack ?? error}\n`);
  }
}
if (failures > 0) {
  process.stderr.write(`${failures} test(s) failed\n`);
  process.exitCode = 1;
} else {
  process.stdout.write(`${tests.length} tests passed\n`);
}
