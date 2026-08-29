#!/usr/bin/env node

import assert from "node:assert/strict";
import { chmod, mkdtemp, readFile, symlink, writeFile } from "node:fs/promises";
import os from "node:os";
import { join } from "node:path";

import {
  BootstrapFailure,
  CALLER_SCOPES,
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
  defaultState,
  inspectResources,
  mintCallerToken,
  parseCallbackRequest,
  planEvidence,
  provisioningPlan,
  readSecureJson,
  validateAccessClaims,
  validateConfig,
  validateHandoff,
  validateIntrospection,
  validateObserver,
  validateState,
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
  assert.equal(persisted.length, 0);
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

test("ambiguous one-time secret creates are never retried", async () => {
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
  assert.equal(
    transport.calls.filter((call) =>
      call.url.endsWith("/api/v1/developer/oauth-clients"),
    ).length,
    2,
    "one GET plus exactly one POST is expected",
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
  const directory = await mkdtemp(join(os.tmpdir(), "cma-trigger-nyxid-test-"));
  await chmod(directory, 0o700);
  const unsafe = join(directory, "unsafe.json");
  await writeFile(unsafe, "{}\n", { mode: 0o644 });
  await expectFailure(() => readSecureJson(unsafe), "unsafe_secret_file");
  const target = join(directory, "target.json");
  await writeFile(target, "{}\n", { mode: 0o600 });
  const link = join(directory, "link.json");
  await symlink(target, link);
  await expectFailure(() => readSecureJson(link), "unsafe_secret_file");

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
  const directory = await mkdtemp(
    join(os.tmpdir(), "cma-trigger-handoff-test-"),
  );
  await chmod(directory, 0o700);
  const outputPath = join(directory, "handoff.json");
  const persisted = [];
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
  assert.equal(refreshCalls[0].form.client_id, APP_ID);
  assert.equal(
    refreshCalls[0].form.client_secret,
    state.oidc_app.client_secret,
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
  const directory = await mkdtemp(
    join(os.tmpdir(), "cma-trigger-initial-test-"),
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
