#!/usr/bin/env node

import assert from "node:assert/strict";
import { generateKeyPairSync, sign as signData } from "node:crypto";
import {
  chmod,
  link,
  lstat,
  mkdir,
  mkdtemp,
  readFile,
  realpath,
  symlink,
  writeFile,
} from "node:fs/promises";
import os from "node:os";
import { join } from "node:path";
import test from "node:test";

import { withCallerStateLock } from "../../scripts/cma-trigger-nyxid-smoke-bootstrap.mjs";

import {
  CLEANUP_CONFIRMATION,
  EPHEMERAL_HANDOFF_SECRET_PREFIX,
  GITHUB_CATALOG_SERVICE_ID,
  KIND_CA_MATERIAL_NAME,
  KUBECONFIG_SECRET,
  NYXID_SOURCE_REVISION,
  OperatorFailure,
  RUNTIME_SERVICE_SECRET,
  RUN_CONFIRMATION,
  SETUP_CONFIRMATION,
  TARGET,
  Transport,
  cleanupEnvironment,
  canonicalJson,
  deleteEphemeralHandoff,
  dispatchWorkflow,
  ephemeralHandoffSecretName,
  generateAuthority,
  identifyDispatchedRun,
  loadHandoff,
  loadState,
  main as operatorMain,
  planEnvironment,
  putEnvironmentSecret,
  readProvisioningToken,
  requireCurrentApprovedPlan,
  requireDistinctAuthorityPaths,
  setupEnvironment,
  sha256,
  stageRun,
  upsertVariable,
  validateBootstrapState,
  validateConfig,
  validateHandoff,
  validatePlan,
  verifyAuthorityDirectory,
} from "./operator.mjs";

const PROVISIONING_SERVICE_ID = "ac1adb9a-9056-480d-8a7a-411007164a86";
const RUNTIME_SERVICE_ID = "1187564d-3b66-47c7-9397-60eb9e3ce6b6";
const SUBJECT_ID = "12a5a3be-8357-4190-bc86-17f81390a765";
const OIDC_APP_ID = "08b92325-c4e7-4b38-9e6e-47efc3ca2624";
const STORAGE_ID = "22b92325-c4e7-4b38-9e6e-47efc3ca2624";
const STORAGE_CLIENT = "sa_0123456789abcdef01234567";
const BROAD_KEY_ID = "32b92325-c4e7-4b38-9e6e-47efc3ca2624";
const RUNTIME_KEY_ID = "42b92325-c4e7-4b38-9e6e-47efc3ca2624";
const NOW = Math.floor(Date.now() / 1000);

function config() {
  return validateConfig({
    schema_version: 1,
    environment: "cma-trigger-smoke",
    nyxid_base_url: "https://nyx-api.chrono-ai.fun",
    provisioning_user_service_id: PROVISIONING_SERVICE_ID,
    dispatch: {
      ref: "develop",
      expected_fork_mode: "reuse",
      preservation_confirmation: "PRESERVE",
      poll_seconds: 2,
      watch_timeout_seconds: 900,
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
  });
}

function state({ caller = true, currentHandoff = undefined } = {}) {
  const boundHandoff =
    currentHandoff === undefined && caller
      ? handoff("header.payload.signature")
      : currentHandoff;
  return validateBootstrapState({
    schema_version: 1,
    environment: "cma-trigger-smoke",
    nyxid_base_url: "https://nyx-api.chrono-ai.fun",
    source_revision: NYXID_SOURCE_REVISION,
    api_version: "0.10.0",
    observer: {
      id: "52b92325-c4e7-4b38-9e6e-47efc3ca2624",
      slug: "cma-trigger-github-observer-staging",
    },
    oidc_app: {
      id: OIDC_APP_ID,
      client_secret: "fixture-oidc-secret",
    },
    storage_service_account: {
      id: STORAGE_ID,
      client_id: STORAGE_CLIENT,
      client_secret: "fixture-storage-secret",
    },
    buckets: {
      context_bucket: "cma-trigger-context-test",
      trigger_workspace_bucket: "cma-trigger-workspace-test",
      owner_service_account_id: STORAGE_ID,
    },
    caller: caller
      ? {
          client_id: OIDC_APP_ID,
          subject_id: SUBJECT_ID,
          service_id: RUNTIME_SERVICE_ID,
          refresh_token: "fixture-refresh-token",
          scope: "openid profile email roles proxy offline_access",
          verification_state: "verified",
        }
      : null,
    handoff_binding:
      boundHandoff === null
        ? null
        : { sha256: sha256(serializeHandoff(boundHandoff)) },
  });
}

function stateWithCallerExchange() {
  const value = state({ caller: false, currentHandoff: null });
  value.caller_exchange = {
    app_id: OIDC_APP_ID,
    exchange_started_at: NOW,
    issuer_token_issued_at: NOW,
    issuer_access_token_ttl_seconds: 8_000,
    issuer_token_sha256: "d".repeat(64),
    issuer_commit: NYXID_SOURCE_REVISION.slice(0, 12),
  };
  return validateBootstrapState(value);
}

function stateWithResourceCreation(kind, { recoveryRotation = true } = {}) {
  const value = state({ caller: false, currentHandoff: null });
  const oidc = kind === "oidc_app";
  if (oidc) {
    value.oidc_app = null;
  } else {
    value.storage_service_account = null;
    value.buckets.owner_service_account_id = null;
  }
  value.resource_creation = {
    kind,
    name: oidc
      ? "CMA Trigger staging app"
      : "CMA Trigger staging storage account",
    administrator_subject_id: SUBJECT_ID,
    attempt_started_at: NOW,
    attempt_nonce: "a".repeat(64),
    request_sha256: "b".repeat(64),
    recovery_rotation: recoveryRotation
      ? {
          resource_id: oidc ? OIDC_APP_ID : STORAGE_ID,
          rotation_started_at: NOW,
          rotation_nonce: "c".repeat(64),
        }
      : null,
  };
  return validateBootstrapState(value);
}

function serializeHandoff(value) {
  return Buffer.from(`${JSON.stringify(value, null, 2)}\n`, "utf8");
}

function base64url(value) {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}

function callerToken(privateKey, kid, expiresAt = NOW + 8_000) {
  const header = base64url({ alg: "RS256", kid, typ: "JWT" });
  const claims = base64url({
    sub: SUBJECT_ID,
    iss: "https://nyx-api.chrono-ai.fun",
    aud: "https://nyx-api.chrono-ai.fun",
    exp: expiresAt,
    iat: NOW,
    jti: "52b92325-c4e7-4b38-9e6e-47efc3ca2624",
    scope: "openid profile email roles proxy offline_access",
    token_type: "access",
    client_id: OIDC_APP_ID,
    resources: [],
    allowed_service_ids: [RUNTIME_SERVICE_ID],
    allow_all_services: false,
    allowed_node_ids: [],
    allow_all_nodes: true,
    delegated: false,
    sa: false,
    relay: false,
  });
  const signed = `${header}.${claims}`;
  const signature = signData("RSA-SHA256", Buffer.from(signed), privateKey);
  return `${signed}.${signature.toString("base64url")}`;
}

function handoff(token, expiresAt = NOW + 8_000) {
  return {
    schema_version: 1,
    nyxid_base_url: "https://nyx-api.chrono-ai.fun",
    nyxid_client_id: OIDC_APP_ID,
    nyxid_client_secret: "fixture-oidc-secret",
    storage_client_id: STORAGE_CLIENT,
    storage_client_secret: "fixture-storage-secret",
    storage_owner_service_account_id: STORAGE_ID,
    context_bucket: "cma-trigger-context-test",
    trigger_workspace_bucket: "cma-trigger-workspace-test",
    public_github_service_slug: "cma-trigger-github-observer-staging",
    subject_id: SUBJECT_ID,
    access_token: token,
    access_token_expires_at: expiresAt,
  };
}

function userService(id, keyId, scopes) {
  return {
    id,
    slug:
      id === PROVISIONING_SERVICE_ID ? "api-github-broad" : "api-github-public",
    label: "GitHub",
    catalog_service_name: "GitHub",
    resource_uri:
      `https://nyx-api.chrono-ai.fun/api/v1/proxy/s/` +
      (id === PROVISIONING_SERVICE_ID
        ? "api-github-broad"
        : "api-github-public"),
    endpoint_id:
      id === PROVISIONING_SERVICE_ID
        ? "62b92325-c4e7-4b38-9e6e-47efc3ca2624"
        : "72b92325-c4e7-4b38-9e6e-47efc3ca2624",
    api_key_id: keyId,
    auth_method: "bearer",
    auth_key_name: "Authorization",
    catalog_service_id: GITHUB_CATALOG_SERVICE_ID,
    node_priority: 0,
    ssh_auth_mode: "proxy_only",
    ssh_node_keys_stale: false,
    admin_only: false,
    is_active: true,
    identity_propagation_mode: "none",
    identity_include_user_id: false,
    identity_include_email: false,
    identity_include_name: false,
    forward_access_token: false,
    inject_delegation_token: false,
    delegation_token_scope: "llm:proxy",
    ws_frame_injections: [],
    created_at: "2026-08-29T00:00:00Z",
    updated_at: "2026-08-29T00:00:00Z",
    state_version: 3,
    credential_source: { type: "personal" },
    fixture_scopes: scopes,
  };
}

function environmentFixture() {
  return {
    id: TARGET.environment.id,
    node_id: TARGET.environment.node_id,
    name: "cma-trigger-smoke",
    can_admins_bypass: false,
    protection_rules: [
      {
        id: 1,
        type: "required_reviewers",
        prevent_self_review: true,
        reviewers: [
          {
            type: "Team",
            reviewer: {
              id: TARGET.environment.reviewer.id,
              node_id: TARGET.environment.reviewer.node_id,
              slug: TARGET.environment.reviewer.slug,
            },
          },
        ],
      },
      { id: 2, type: "branch_policy" },
    ],
    deployment_branch_policy: {
      protected_branches: true,
      custom_branch_policies: false,
    },
  };
}

function repositoryFixture() {
  return {
    id: TARGET.repository.id,
    node_id: TARGET.repository.node_id,
    name: TARGET.repository.name,
    full_name: "ChronoAIProject/cma",
    owner: { login: "ChronoAIProject" },
    default_branch: "develop",
    archived: false,
    disabled: false,
  };
}

function branchFixture() {
  return {
    required_status_checks: {
      strict: true,
      checks: [
        { context: "Backend" },
        { context: "Frontend" },
        { context: "Images (api)" },
        { context: "Images (runtime)" },
        { context: "Images (web)" },
        { context: "Manifests" },
        { context: "Repository structure" },
        { context: "Rust SDK" },
        { context: "Secret scanning" },
        { context: "Trigger provider smoke contract" },
        { context: "Trigger release contract" },
        { context: "TypeScript SDK" },
      ],
    },
    enforce_admins: { enabled: true },
    required_conversation_resolution: { enabled: true },
    allow_force_pushes: { enabled: false },
    allow_deletions: { enabled: false },
    lock_branch: { enabled: false },
  };
}

class FixtureTransport {
  constructor({ jwk = null } = {}) {
    this.token = "fixture-provisioning-token";
    this.provisioningUserServiceId = PROVISIONING_SERVICE_ID;
    this.variables = {};
    this.secrets = {};
    this.calls = [];
    this.clock = 0;
    this.jwk = jwk;
    this.workflowRuns = [];
    this.dispatches = 0;
    this.variableAmbiguity = null;
    this.secretAmbiguity = null;
    this.deleteAmbiguity = null;
    this.dispatchAmbiguity = false;
    this.developSha = "a".repeat(40);
    this.moveHeadAfterSecret = null;
  }

  timestamp() {
    this.clock += 1;
    return new Date(Date.UTC(2026, 7, 29, 0, 0, this.clock)).toISOString();
  }

  async nyxid(method, path, { token } = {}) {
    this.calls.push({ plane: "nyxid", method, path, token });
    if (path === "/.well-known/openid-configuration") {
      return {
        status: 200,
        json: {
          issuer: "https://nyx-api.chrono-ai.fun",
          jwks_uri: "https://nyx-api.chrono-ai.fun/.well-known/jwks.json",
        },
      };
    }
    if (path === "/.well-known/jwks.json") {
      return { status: 200, json: { keys: [this.jwk] } };
    }
    if (path === "/api/v1/users/me") {
      return { status: 200, json: { id: SUBJECT_ID, is_active: true } };
    }
    if (path === "/api/v1/user-services") {
      return {
        status: 200,
        json: {
          services: [
            userService(PROVISIONING_SERVICE_ID, BROAD_KEY_ID, [
              "repo",
              "read:user",
              "user:email",
            ]),
            userService(RUNTIME_SERVICE_ID, RUNTIME_KEY_ID, [
              "public_repo",
              "read:user",
              "user:email",
            ]),
          ],
        },
      };
    }
    const authorization = path.match(
      /^\/api\/v1\/keys\/([^/]+)\/authorization$/u,
    );
    if (authorization) {
      const id = authorization[1];
      const runtime = id === RUNTIME_SERVICE_ID;
      return {
        status: 200,
        json: {
          id,
          api_key_id: runtime ? RUNTIME_KEY_ID : BROAD_KEY_ID,
          is_active: true,
          status: "active",
          connection_status: "active",
          granted_scopes: runtime
            ? ["public_repo", "read:user", "user:email"]
            : ["repo", "read:user", "user:email"],
          last_authorized_at: "2026-08-29T00:00:00Z",
          state_version: 3,
        },
      };
    }
    throw new Error(`unhandled NyxID fixture ${method} ${path}`);
  }

  async github(method, path, options = {}) {
    this.calls.push({ plane: "github", method, path, options });
    if (path === "/user") {
      return {
        status: 200,
        json: {
          login: "chronoai-shining",
          id: TARGET.account.id,
          node_id: TARGET.account.node_id,
          type: "User",
          site_admin: false,
        },
      };
    }
    if (path === "/repos/ChronoAIProject/NyxID/git/ref/heads/main") {
      return {
        status: 200,
        json: {
          ref: "refs/heads/main",
          object: { type: "commit", sha: NYXID_SOURCE_REVISION },
        },
      };
    }
    if (path === `/repositories/${TARGET.repository.id}`) {
      return { status: 200, json: repositoryFixture() };
    }
    if (path.endsWith("/environments/cma-trigger-smoke")) {
      return { status: 200, json: environmentFixture() };
    }
    if (path.endsWith("/branches/develop/protection")) {
      return { status: 200, json: branchFixture() };
    }
    if (path.endsWith("/git/ref/heads/develop")) {
      return {
        status: 200,
        json: {
          ref: "refs/heads/develop",
          object: { type: "commit", sha: this.developSha },
        },
      };
    }
    if (path.endsWith(`/actions/workflows/${WORKFLOW_FILE}`)) {
      return {
        status: 200,
        json: { ...TARGET.workflow, state: "active" },
      };
    }
    if (path.endsWith("/environments/cma-trigger-smoke/secrets/public-key")) {
      return {
        status: 200,
        json: {
          key_id: "fixture-key-id",
          key: Buffer.alloc(32, 7).toString("base64"),
        },
      };
    }
    if (path.endsWith("/environments/cma-trigger-smoke/variables")) {
      if (method === "GET") return this.variableList();
      if (method === "POST") {
        if (this.variableAmbiguity === "before") {
          this.variableAmbiguity = null;
          throw new OperatorFailure("remote_transport_ambiguous", "fixture");
        }
        this.variables[options.json.name] = options.json.value;
        if (this.variableAmbiguity === "after") {
          this.variableAmbiguity = null;
          throw new OperatorFailure("remote_transport_ambiguous", "fixture");
        }
        return { status: 201, json: null };
      }
    }
    const variable = path.match(
      /\/environments\/cma-trigger-smoke\/variables\/([^/]+)$/u,
    );
    if (variable && method === "PATCH") {
      const name = decodeURIComponent(variable[1]);
      if (this.variableAmbiguity === "before") {
        this.variableAmbiguity = null;
        throw new OperatorFailure("remote_transport_ambiguous", "fixture");
      }
      this.variables[name] = options.json.value;
      if (this.variableAmbiguity === "after") {
        this.variableAmbiguity = null;
        throw new OperatorFailure("remote_transport_ambiguous", "fixture");
      }
      return { status: 204, json: null };
    }
    if (path.endsWith("/environments/cma-trigger-smoke/secrets")) {
      return this.secretList();
    }
    const secret = path.match(
      /\/environments\/cma-trigger-smoke\/secrets\/([^/]+)$/u,
    );
    if (secret) {
      const name = decodeURIComponent(secret[1]);
      if (method === "PUT") {
        if (this.secretAmbiguity === "before") {
          this.secretAmbiguity = null;
          throw new OperatorFailure("remote_transport_ambiguous", "fixture");
        }
        const created = this.secrets[name]?.created_at ?? this.timestamp();
        this.secrets[name] = {
          created_at: created,
          updated_at: this.timestamp(),
        };
        if (
          this.moveHeadAfterSecret === name ||
          (this.moveHeadAfterSecret === "ephemeral" &&
            name.startsWith(EPHEMERAL_HANDOFF_SECRET_PREFIX))
        ) {
          this.developSha = "b".repeat(40);
          this.moveHeadAfterSecret = null;
        }
        if (this.secretAmbiguity === "after") {
          this.secretAmbiguity = null;
          throw new OperatorFailure("remote_transport_ambiguous", "fixture");
        }
        return { status: 201, json: null };
      }
      if (method === "DELETE") {
        if (this.deleteAmbiguity === "before") {
          this.deleteAmbiguity = null;
          throw new OperatorFailure("remote_transport_ambiguous", "fixture");
        }
        delete this.secrets[name];
        if (this.deleteAmbiguity === "after") {
          this.deleteAmbiguity = null;
          throw new OperatorFailure("remote_transport_ambiguous", "fixture");
        }
        return { status: 204, json: null };
      }
    }
    if (path.endsWith("/actions/runs")) {
      return { status: 200, json: { total_count: 0, workflow_runs: [] } };
    }
    if (path.endsWith(`/actions/workflows/${TARGET.workflow.id}/runs`)) {
      return {
        status: 200,
        json: {
          total_count: this.workflowRuns.length,
          workflow_runs: this.workflowRuns,
        },
      };
    }
    if (path.endsWith(`/actions/workflows/${TARGET.workflow.id}/dispatches`)) {
      assert.equal(options.json.ref, "develop");
      assert.match(options.json.inputs.dispatch_nonce, /^[0-9a-f]{32}$/u);
      assert.deepEqual(options.json.inputs, {
        dispatch_nonce: options.json.inputs.dispatch_nonce,
        expected_head_sha: this.developSha,
        expected_fork_mode: "reuse",
        handoff_secret_name: ephemeralHandoffSecretName(
          options.json.inputs.dispatch_nonce,
        ),
        preservation_confirmation: "PRESERVE",
      });
      this.dispatches += 1;
      const run = {
        id: 9000 + this.dispatches,
        workflow_id: TARGET.workflow.id,
        path: WORKFLOW_PATH,
        event: "workflow_dispatch",
        head_branch: "develop",
        head_sha: this.developSha,
        display_title: `CMA Trigger provider smoke ${options.json.inputs.dispatch_nonce}`,
        actor: { login: "chronoai-shining" },
        triggering_actor: { login: "chronoai-shining" },
        created_at: new Date().toISOString(),
        status: "queued",
      };
      this.workflowRuns.unshift(run);
      if (this.dispatchAmbiguity) {
        this.dispatchAmbiguity = false;
        throw new OperatorFailure("remote_transport_ambiguous", "fixture");
      }
      return { status: 204, json: null };
    }
    throw new Error(`unhandled GitHub fixture ${method} ${path}`);
  }

  variableList() {
    const variables = Object.entries(this.variables).map(([name, value]) => ({
      name,
      value,
      created_at: "2026-08-29T00:00:00Z",
      updated_at: "2026-08-29T00:00:00Z",
    }));
    return { status: 200, json: { total_count: variables.length, variables } };
  }

  secretList() {
    const secrets = Object.entries(this.secrets).map(([name, value]) => ({
      name,
      ...value,
    }));
    return { status: 200, json: { total_count: secrets.length, secrets } };
  }
}

const WORKFLOW_FILE = "cma-trigger-provider-smoke.yml";
const WORKFLOW_PATH = ".github/workflows/cma-trigger-provider-smoke.yml";

async function expectFailure(action, code) {
  await assert.rejects(Promise.resolve().then(action), (error) => {
    assert.equal(error.code, code);
    return true;
  });
}

async function privateDirectory(prefix) {
  const directory = await mkdtemp(join(os.tmpdir(), prefix));
  await chmod(directory, 0o700);
  return realpath(directory);
}

function ephemeralSecretNames(transport) {
  return Object.keys(transport.secrets).filter((name) =>
    name.startsWith(EPHEMERAL_HANDOFF_SECRET_PREFIX),
  );
}

function refingerprintPlan(plan) {
  plan.approval_fingerprint = sha256(
    canonicalJson({
      generated_at: plan.generated_at,
      expires_at: plan.expires_at,
      observed: plan.observed,
    }),
  );
  return plan;
}

function fixtureSeal(_binary, _publicKey, plaintext) {
  return Promise.resolve(
    Buffer.alloc(Buffer.byteLength(plaintext) + 48, 9).toString("base64"),
  );
}

test("config, state, handoff, and approved-plan schemas fail closed", async () => {
  const cfg = config();
  for (const mutate of [
    (value) => (value.extra = true),
    (value) => (value.environment = "production"),
    (value) => (value.dispatch.ref = "main"),
    (value) =>
      (value.static_variables.CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN = "other"),
  ]) {
    const value = structuredClone(cfg);
    mutate(value);
    await expectFailure(() => validateConfig(value), "invalid_config");
  }
  const currentState = state();
  const validHandoff = handoff("header.payload.signature");
  validateHandoff(validHandoff, currentState, PROVISIONING_SERVICE_ID, NOW);
  const exactRawBytes = (48 * 1024 * 3) / 4;
  const serialized = serializeHandoff(validHandoff);
  const exactArtifact = Buffer.concat([
    serialized,
    Buffer.alloc(exactRawBytes - serialized.length, 0x20),
  ]);
  assert.equal(
    Buffer.byteLength(exactArtifact.toString("base64"), "utf8"),
    48 * 1024,
  );
  const exactState = structuredClone(currentState);
  exactState.handoff_binding = { sha256: sha256(exactArtifact) };
  validateHandoff(
    validHandoff,
    exactState,
    PROVISIONING_SERVICE_ID,
    NOW,
    exactArtifact,
  );
  const oversizedArtifact = Buffer.concat([exactArtifact, Buffer.from(" ")]);
  const oversizedState = structuredClone(currentState);
  oversizedState.handoff_binding = { sha256: sha256(oversizedArtifact) };
  await expectFailure(
    () =>
      validateHandoff(
        validHandoff,
        oversizedState,
        PROVISIONING_SERVICE_ID,
        NOW,
        oversizedArtifact,
      ),
    "handoff_upload_too_large",
  );
  const legacyState = structuredClone(currentState);
  delete legacyState.caller.verification_state;
  assert.equal(
    validateBootstrapState(legacyState).caller.verification_state,
    "verified",
  );
  const legacyUnboundState = structuredClone(currentState);
  delete legacyUnboundState.handoff_binding;
  delete legacyUnboundState.caller_exchange;
  validateBootstrapState(legacyUnboundState);
  assert.equal(legacyUnboundState.caller_exchange, null);
  await expectFailure(
    () =>
      validateHandoff(
        validHandoff,
        legacyUnboundState,
        PROVISIONING_SERVICE_ID,
        NOW,
      ),
    "current_handoff_missing",
  );
  const pendingState = structuredClone(currentState);
  pendingState.caller.verification_state = "pending";
  pendingState.handoff_binding = null;
  validateBootstrapState(pendingState);
  await expectFailure(
    () =>
      validateHandoff(
        validHandoff,
        pendingState,
        PROVISIONING_SERVICE_ID,
        NOW,
      ),
    "caller_custody_pending",
  );
  const invalidState = structuredClone(currentState);
  invalidState.caller.verification_state = "future";
  await expectFailure(
    () => validateBootstrapState(invalidState),
    "unsafe_state",
  );
  const resourceCreationState = stateWithResourceCreation("oidc_app", {
    recoveryRotation: false,
  });
  delete resourceCreationState.resource_creation.recovery_rotation;
  validateBootstrapState(resourceCreationState);
  assert.equal(resourceCreationState.resource_creation.recovery_rotation, null);
  const exchangeState = stateWithCallerExchange();
  const exchangeTransport = new FixtureTransport();
  const { plan: exchangePlan } = await planEnvironment({
    config: cfg,
    state: exchangeState,
    transport: exchangeTransport,
  });
  validatePlan(exchangePlan);
  assert.ok(exchangeTransport.calls.length > 0);
  assert.equal(
    exchangeTransport.calls.filter((call) => call.method !== "GET").length,
    0,
  );
  const callsAfterPlan = exchangeTransport.calls.length;
  await expectFailure(
    () =>
      setupEnvironment({
        config: cfg,
        state: exchangeState,
        transport: exchangeTransport,
        custodyDirectory: "/unreachable",
        approvedPlan: exchangePlan,
        environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: SETUP_CONFIRMATION },
      }),
    "caller_authorization_exchange_recovery_required",
  );
  await expectFailure(
    () =>
      stageRun({
        config: cfg,
        state: exchangeState,
        handoff: validHandoff,
        handoffBytes: serializeHandoff(validHandoff),
        transport: exchangeTransport,
        custodyDirectory: "/unreachable",
        approvedPlan: exchangePlan,
        environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: RUN_CONFIRMATION },
      }),
    "caller_authorization_exchange_recovery_required",
  );
  assert.equal(exchangeTransport.calls.length, callsAfterPlan);
  const mismatchedHandoff = structuredClone(validHandoff);
  mismatchedHandoff.access_token = "other.payload.signature";
  await expectFailure(
    () =>
      validateHandoff(
        mismatchedHandoff,
        currentState,
        PROVISIONING_SERVICE_ID,
        NOW,
      ),
    "handoff_binding_mismatch",
  );
  for (const mutate of [
    (value) => (value.extra = true),
    (value) => (value.subject_id = "82b92325-c4e7-4b38-9e6e-47efc3ca2624"),
    (value) => (value.nyxid_client_secret = "different"),
    (value) => (value.access_token_expires_at = NOW + 60),
  ]) {
    const value = structuredClone(validHandoff);
    mutate(value);
    await assert.rejects(
      Promise.resolve().then(() =>
        validateHandoff(value, currentState, PROVISIONING_SERVICE_ID, NOW),
      ),
    );
  }
  const transport = new FixtureTransport();
  const { plan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  validatePlan(plan);
  const changed = structuredClone(plan);
  changed.observed.branch.head.sha = "b".repeat(40);
  await expectFailure(() => validatePlan(changed), "invalid_plan");
  const expired = structuredClone(plan);
  const generated = new Date(Date.now() - 20 * 60 * 1000);
  expired.generated_at = generated.toISOString();
  expired.expires_at = new Date(
    generated.getTime() + 10 * 60 * 1000,
  ).toISOString();
  refingerprintPlan(expired);
  await expectFailure(
    () => requireCurrentApprovedPlan(expired, expired),
    "plan_expired",
  );
  const future = structuredClone(plan);
  const futureGenerated = new Date(Date.now() + 60 * 60 * 1000);
  future.generated_at = futureGenerated.toISOString();
  future.expires_at = new Date(
    futureGenerated.getTime() + 10 * 60 * 1000,
  ).toISOString();
  refingerprintPlan(future);
  await expectFailure(
    () => requireCurrentApprovedPlan(future, future),
    "plan_not_yet_valid",
  );
});

test("operator authority work fails while bootstrap owns the caller state lock", async () => {
  const directory = await realpath(
    await mkdtemp(join(os.tmpdir(), "cma-trigger-operator-lock-")),
  );
  await chmod(directory, 0o700);
  const configPath = join(directory, "config.json");
  const statePath = join(directory, "state.json");
  const outputPath = join(directory, "plan.json");
  await writeFile(configPath, `${JSON.stringify(config())}\n`, { mode: 0o600 });
  await writeFile(statePath, `${JSON.stringify(state())}\n`, { mode: 0o600 });

  await withCallerStateLock(statePath, async () => {
    await expectFailure(
      () =>
        operatorMain([
          "--plan",
          configPath,
          "--state",
          statePath,
          "--output",
          outputPath,
        ]),
      "caller_state_locked",
    );
  });
});

test("operator CLI reports bootstrap incidents before reading credentials or artifacts", async () => {
  const directory = await privateDirectory("cma-trigger-operator-fence-order-");
  const configPath = join(directory, "config.json");
  const statePath = join(directory, "state.json");
  const missingPlan = join(directory, "missing-plan.json");
  const missingHandoff = join(directory, "missing-handoff.json");
  const missingCustody = join(directory, "missing-custody");
  await writeFile(configPath, `${JSON.stringify(config())}\n`, { mode: 0o600 });
  for (const [incidentState, expectedCode] of [
    [
      stateWithCallerExchange(),
      "caller_authorization_exchange_recovery_required",
    ],
    [
      stateWithResourceCreation("oidc_app"),
      "oidc_secret_creation_recovery_required",
    ],
    [
      stateWithResourceCreation("storage_service_account"),
      "storage_secret_creation_recovery_required",
    ],
  ]) {
    await writeFile(statePath, `${JSON.stringify(incidentState)}\n`, {
      mode: 0o600,
    });
    for (const invocation of [
      [
        "--setup",
        configPath,
        "--state",
        statePath,
        "--approved-plan",
        missingPlan,
        "--custody",
        missingCustody,
      ],
      [
        "--stage-run",
        configPath,
        "--state",
        statePath,
        "--approved-plan",
        missingPlan,
        "--custody",
        missingCustody,
        "--handoff",
        missingHandoff,
      ],
    ]) {
      await expectFailure(() => operatorMain(invocation), expectedCode);
    }
  }
});

test("resource creation incidents allow only planning and exact cleanup", async () => {
  const cfg = config();
  for (const [kind, expectedCode] of [
    ["oidc_app", "oidc_secret_creation_recovery_required"],
    [
      "storage_service_account",
      "storage_secret_creation_recovery_required",
    ],
  ]) {
    const incidentState = stateWithResourceCreation(kind);
    const transport = new FixtureTransport();
    const timestamp = transport.timestamp();
    const secretName = ephemeralHandoffSecretName(
      kind === "oidc_app" ? "3".repeat(32) : "4".repeat(32),
    );
    for (const name of [
      secretName,
      RUNTIME_SERVICE_SECRET,
      KIND_CA_MATERIAL_NAME,
      KUBECONFIG_SECRET,
    ]) {
      transport.secrets[name] = {
        created_at: timestamp,
        updated_at: timestamp,
      };
    }

    const { plan } = await planEnvironment({
      config: cfg,
      state: incidentState,
      transport,
    });
    validatePlan(plan);
    assert.equal(
      transport.calls.filter((call) => call.method !== "GET").length,
      0,
    );
    const callsAfterPlan = transport.calls.length;
    await expectFailure(
      () =>
        setupEnvironment({
          config: cfg,
          state: incidentState,
          transport,
          custodyDirectory: "/unreachable",
          approvedPlan: plan,
          environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: SETUP_CONFIRMATION },
        }),
      expectedCode,
    );
    await expectFailure(
      () =>
        stageRun({
          config: cfg,
          state: incidentState,
          handoff: handoff("header.payload.signature"),
          handoffBytes: serializeHandoff(
            handoff("header.payload.signature"),
          ),
          transport,
          custodyDirectory: "/unreachable",
          approvedPlan: plan,
          environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: RUN_CONFIRMATION },
        }),
      expectedCode,
    );
    assert.equal(transport.calls.length, callsAfterPlan);

    const result = await cleanupEnvironment({
      config: cfg,
      state: incidentState,
      transport,
      approvedPlan: plan,
      environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: CLEANUP_CONFIRMATION },
    });
    assert.equal(result.status, "ephemeral_cleanup_complete");
    assert.ok(!Object.hasOwn(transport.secrets, secretName));
    for (const stable of [
      RUNTIME_SERVICE_SECRET,
      KIND_CA_MATERIAL_NAME,
      KUBECONFIG_SECRET,
    ]) {
      assert.ok(Object.hasOwn(transport.secrets, stable));
    }
    const deletes = transport.calls.filter(
      (call) => call.method === "DELETE",
    );
    assert.equal(deletes.length, 1);
    assert.ok(deletes[0].path.endsWith(`/${secretName}`));
  }
});

test("plan output cannot overwrite or alias caller authority files", async () => {
  const directory = await privateDirectory("cma-trigger-plan-output-test-");
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

  const aliasRoot = await privateDirectory("cma-trigger-plan-alias-test-");
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
  const inodeAlias = join(directory, "inode-plan.json");
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

  if (filesystemAliasesLock) {
    const configPath = join(directory, "config.json");
    await writeFile(configPath, `${JSON.stringify(config())}\n`, {
      mode: 0o600,
    });
    await writeFile(statePath, `${JSON.stringify(state())}\n`, {
      mode: 0o600,
    });
    const directToken = process.env.CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN;
    const tokenFile =
      process.env.CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE;
    delete process.env.CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN;
    delete process.env.CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE;
    try {
      await expectFailure(
        () =>
          operatorMain([
            "--plan",
            configPath,
            "--state",
            statePath,
            "--output",
            filesystemFoldedOutput,
          ]),
        "usage",
      );
    } finally {
      if (directToken !== undefined) {
        process.env.CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN = directToken;
      }
      if (tokenFile !== undefined) {
        process.env.CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE = tokenFile;
      }
    }
    await assert.rejects(
      lstat(`${statePath}.caller-authority.lock`),
      { code: "ENOENT" },
    );
  }
});

test("GitHub REST always uses the catalog-ID route and explicit UserService selector", async () => {
  const requests = [];
  const transport = new Transport({
    token: "fixture-token",
    provisioningUserServiceId: PROVISIONING_SERVICE_ID,
    fetchImpl: async (url, options) => {
      requests.push({ url, options });
      return new Response(JSON.stringify({ ok: true }), {
        status: 200,
        headers: { "content-type": "application/json" },
      });
    },
  });
  await transport.github("GET", "/user", { query: { per_page: 1 } });
  assert.equal(requests.length, 1);
  const url = new URL(requests[0].url);
  assert.equal(url.pathname, `/api/v1/proxy/${GITHUB_CATALOG_SERVICE_ID}/user`);
  assert.equal(url.searchParams.get("_nyxid_via"), PROVISIONING_SERVICE_ID);
  assert.equal(url.searchParams.get("per_page"), "1");
  assert.equal(
    requests[0].options.headers.authorization,
    "Bearer fixture-token",
  );
  assert.ok(!requests[0].url.includes("fixture-token"));

  const oversized = new Transport({
    token: "fixture-token",
    provisioningUserServiceId: PROVISIONING_SERVICE_ID,
    fetchImpl: async () =>
      new Response("{}", {
        status: 200,
        headers: { "content-length": String(5 * 1024 * 1024) },
      }),
  });
  await expectFailure(
    () => oversized.github("GET", "/user"),
    "remote_response_oversized",
  );
});

test("variable and secret ambiguous writes reconcile without unsafe automatic secret replay", async () => {
  const transport = new FixtureTransport();
  transport.variableAmbiguity = "after";
  const reconciled = await upsertVariable(
    transport,
    "CMA_TRIGGER_SMOKE_MARKER_INPUT",
    "smoke_marker",
    false,
  );
  assert.equal(reconciled.status, "reconciled_after_ambiguous_write");

  transport.variableAmbiguity = "before";
  await expectFailure(
    () =>
      upsertVariable(
        transport,
        "CMA_TRIGGER_SMOKE_FORK_REPOSITORY",
        "cma-trigger-fixture",
        false,
      ),
    "variable_write_recovery_required",
  );

  transport.secretAmbiguity = "after";
  await expectFailure(
    () =>
      putEnvironmentSecret(transport, {
        name: KUBECONFIG_SECRET,
        plaintext: "fixture-secret",
        publicKey: {
          key_id: "fixture",
          key: Buffer.alloc(32).toString("base64"),
        },
        sealerBinary: "fixture",
        seal: fixtureSeal,
      }),
    "secret_write_recovery_required",
  );
  const secretPuts = transport.calls.filter(
    (call) =>
      call.method === "PUT" && call.path?.endsWith(`/${KUBECONFIG_SECRET}`),
  );
  assert.equal(secretPuts.length, 1);

  await expectFailure(
    () =>
      putEnvironmentSecret(transport, {
        name: KUBECONFIG_SECRET,
        plaintext: Buffer.alloc(48 * 1024 + 1),
        publicKey: {
          key_id: "fixture",
          key: Buffer.alloc(32).toString("base64"),
        },
        sealerBinary: "fixture",
        seal: fixtureSeal,
      }),
    "secret_input_invalid",
  );

  const rejectedTransport = {
    github: async () => {
      throw new OperatorFailure("remote_status_error", "fixture rejection", {
        status: 422,
      });
    },
  };
  await expectFailure(
    () =>
      putEnvironmentSecret(rejectedTransport, {
        name: KUBECONFIG_SECRET,
        plaintext: "fixture-secret",
        publicKey: {
          key_id: "fixture",
          key: Buffer.alloc(32).toString("base64"),
        },
        sealerBinary: "fixture",
        seal: fixtureSeal,
      }),
    "secret_write_rejected",
  );
});

test("ephemeral cleanup reconciles ambiguous deletion and never targets stable resources", async () => {
  const transport = new FixtureTransport();
  const timestamp = transport.timestamp();
  const secretName = ephemeralHandoffSecretName("1".repeat(32));
  transport.secrets[secretName] = {
    created_at: timestamp,
    updated_at: timestamp,
  };
  transport.secrets[KUBECONFIG_SECRET] = {
    created_at: timestamp,
    updated_at: timestamp,
  };
  transport.deleteAmbiguity = "after";
  const result = await deleteEphemeralHandoff(transport);
  assert.match(result.status, /absence/u);
  assert.ok(!Object.hasOwn(transport.secrets, secretName));
  assert.ok(Object.hasOwn(transport.secrets, KUBECONFIG_SECRET));
  const deletes = transport.calls.filter((call) => call.method === "DELETE");
  assert.ok(deletes.every((call) => call.path.endsWith(`/${secretName}`)));

  const unresolved = new FixtureTransport();
  unresolved.secrets[secretName] = {
    created_at: timestamp,
    updated_at: timestamp,
  };
  unresolved.deleteAmbiguity = "before";
  await expectFailure(
    () => deleteEphemeralHandoff(unresolved, secretName),
    "ephemeral_cleanup_recovery_required",
  );
  assert.ok(Object.hasOwn(unresolved.secrets, secretName));
  assert.equal(
    unresolved.calls.filter((call) => call.method === "DELETE").length,
    1,
  );
});

test("Kind authority is preprovisioned, exact, private, and reusable", async () => {
  const custody = await privateDirectory("cma-trigger-authority-test-");
  const first = await generateAuthority(custody);
  const second = await generateAuthority(custody);
  assert.equal(first.path, second.path);
  assert.equal(
    first.manifest.digests.kubeconfig,
    second.manifest.digests.kubeconfig,
  );
  assert.equal(
    (await readFile(first.files.kubeconfig, "utf8")).includes("token:"),
    false,
  );
  await verifyAuthorityDirectory(first.path);
});

test("unsafe state and handoff custody modes and symlinks are rejected", async () => {
  const directory = await privateDirectory("cma-trigger-custody-test-");
  const statePath = join(directory, "state.json");
  await writeFile(statePath, `${JSON.stringify(state())}\n`, { mode: 0o644 });
  await chmod(statePath, 0o644);
  await expectFailure(() => loadState(statePath), "unsafe_secret_file");
  await chmod(statePath, 0o600);
  assert.equal((await loadState(statePath)).environment, "cma-trigger-smoke");
  const link = join(directory, "state-link.json");
  await symlink(statePath, link);
  await expectFailure(() => loadState(link), "unsafe_secret_path");

  const linkedTarget = await privateDirectory("cma-trigger-linked-target-");
  const linkedInner = join(linkedTarget, "inner");
  await mkdir(linkedInner, { mode: 0o700 });
  const linkedState = join(linkedInner, "state.json");
  await writeFile(linkedState, `${JSON.stringify(state())}\n`, { mode: 0o600 });
  const linkedRoot = join(directory, "linked-root");
  await symlink(linkedTarget, linkedRoot);
  await expectFailure(
    () => loadState(join(linkedRoot, "inner", "state.json")),
    "unsafe_secret_path",
  );

  const writableAncestor = await privateDirectory(
    "cma-trigger-operator-writable-ancestor-",
  );
  const nestedCustody = join(writableAncestor, "custody");
  await mkdir(nestedCustody, { mode: 0o700 });
  const nestedState = join(nestedCustody, "state.json");
  await writeFile(nestedState, `${JSON.stringify(state())}\n`, { mode: 0o600 });
  await chmod(writableAncestor, 0o777);
  await expectFailure(() => loadState(nestedState), "unsafe_secret_path");
  await chmod(writableAncestor, 0o700);

  const handoffPath = join(directory, "handoff.json");
  await writeFile(
    handoffPath,
    serializeHandoff(handoff("header.payload.signature")),
    {
      mode: 0o600,
    },
  );
  const loaded = await loadHandoff(
    handoffPath,
    state(),
    PROVISIONING_SERVICE_ID,
  );
  assert.equal(loaded.handoff.subject_id, SUBJECT_ID);

  const exactBytes = serializeHandoff(handoff("header.payload.signature"));
  await writeFile(
    handoffPath,
    Buffer.concat([exactBytes.subarray(0, exactBytes.length - 1), Buffer.from(" \n")]),
    { mode: 0o600 },
  );
  await expectFailure(
    () => loadHandoff(handoffPath, state(), PROVISIONING_SERVICE_ID),
    "handoff_binding_mismatch",
  );

  const serialized = exactBytes.toString("utf8");
  const subjectLine = `  "subject_id": "${SUBJECT_ID}",`;
  assert.equal(serialized.split(subjectLine).length, 2);
  await writeFile(
    handoffPath,
    serialized.replace(subjectLine, `${subjectLine}\n${subjectLine}`),
    { mode: 0o600 },
  );
  await expectFailure(
    () => loadHandoff(handoffPath, state(), PROVISIONING_SERVICE_ID),
    "handoff_binding_mismatch",
  );
});

test("provisioning bearer accepts one environment or secure-file input only", async () => {
  assert.equal(
    await readProvisioningToken({
      CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN: "fixture-bearer-token",
    }),
    "fixture-bearer-token",
  );
  await expectFailure(
    () =>
      readProvisioningToken({
        CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN: "fixture",
        CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE: "/tmp/other",
      }),
    "credential_input_invalid",
  );
  const directory = await privateDirectory("cma-trigger-token-file-test-");
  const path = join(directory, "token");
  await writeFile(path, "fixture-file-bearer\n", { mode: 0o600 });
  assert.equal(
    await readProvisioningToken({
      CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE: path,
    }),
    "fixture-file-bearer",
  );
  await chmod(path, 0o644);
  await expectFailure(
    () =>
      readProvisioningToken({
        CMA_TRIGGER_SMOKE_PROVISIONING_NYXID_TOKEN_FILE: path,
      }),
    "unsafe_secret_file",
  );
});

test("setup is reviewed, idempotent for variables, and records stable-secret custody", async () => {
  const custody = await privateDirectory("cma-trigger-setup-test-");
  const cfg = config();
  const currentState = state();
  const transport = new FixtureTransport();
  const authority = await generateAuthority(custody);
  const { plan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  const first = await setupEnvironment({
    config: cfg,
    state: currentState,
    transport,
    custodyDirectory: custody,
    approvedPlan: plan,
    environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: SETUP_CONFIRMATION },
    generate: async () => authority,
    build: async () => "fixture-sealer",
    seal: fixtureSeal,
  });
  assert.equal(first.status, "staging_environment_ready");
  assert.ok(Object.hasOwn(transport.secrets, KUBECONFIG_SECRET));
  assert.ok(Object.hasOwn(transport.secrets, KIND_CA_MATERIAL_NAME));
  assert.equal(ephemeralSecretNames(transport).length, 0);
  const variableWrites = transport.calls.filter(
    (call) => call.method === "POST" || call.method === "PATCH",
  ).length;
  const { plan: secondPlan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  await setupEnvironment({
    config: cfg,
    state: currentState,
    transport,
    custodyDirectory: custody,
    approvedPlan: secondPlan,
    environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: SETUP_CONFIRMATION },
    generate: async () => authority,
    build: async () => "fixture-sealer",
    seal: fixtureSeal,
  });
  const variableWritesAfter = transport.calls.filter(
    (call) => call.method === "POST" || call.method === "PATCH",
  ).length;
  assert.equal(variableWritesAfter, variableWrites);
});

test("stage-run binds state, JWT, runtime UserService, exact run, and EXIT cleanup", async () => {
  const { privateKey, publicKey } = generateKeyPairSync("rsa", {
    modulusLength: 2048,
  });
  const kid = "fixture-signing-key";
  const jwk = publicKey.export({ format: "jwk" });
  jwk.kid = kid;
  jwk.alg = "RS256";
  jwk.use = "sig";
  const token = callerToken(privateKey, kid);
  const currentHandoff = handoff(token);
  const custody = await privateDirectory("cma-trigger-stage-run-test-");
  const cfg = config();
  const currentState = state({ currentHandoff });
  const transport = new FixtureTransport({ jwk });
  const authority = await generateAuthority(custody);
  const { plan: setupPlan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  await setupEnvironment({
    config: cfg,
    state: currentState,
    transport,
    custodyDirectory: custody,
    approvedPlan: setupPlan,
    environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: SETUP_CONFIRMATION },
    generate: async () => authority,
    build: async () => "fixture-sealer",
    seal: fixtureSeal,
  });
  const { plan: runPlan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  const result = await stageRun({
    config: cfg,
    state: currentState,
    handoff: currentHandoff,
    handoffBytes: serializeHandoff(currentHandoff),
    transport,
    custodyDirectory: custody,
    approvedPlan: runPlan,
    environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: RUN_CONFIRMATION },
    build: async () => "fixture-sealer",
    seal: fixtureSeal,
    watch: async (_transport, run) => ({
      id: run.id,
      status: "completed",
      conclusion: "success",
      head_sha: run.head_sha,
      html_url: "https://github.example.test/run",
    }),
  });
  assert.equal(result.run.conclusion, "success");
  assert.equal(result.runtime_authority.service_id, RUNTIME_SERVICE_ID);
  assert.equal(transport.dispatches, 1);
  assert.equal(ephemeralSecretNames(transport).length, 0);
  assert.ok(Object.hasOwn(transport.secrets, RUNTIME_SERVICE_SECRET));
  assert.ok(Object.hasOwn(transport.secrets, KUBECONFIG_SECRET));

  const { plan: failedRunPlan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  await expectFailure(
    () =>
      stageRun({
        config: cfg,
        state: currentState,
        handoff: currentHandoff,
        handoffBytes: serializeHandoff(currentHandoff),
        transport,
        custodyDirectory: custody,
        approvedPlan: failedRunPlan,
        environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: RUN_CONFIRMATION },
        build: async () => "fixture-sealer",
        seal: fixtureSeal,
        watch: async (_transport, run) => ({
          id: run.id,
          status: "completed",
          conclusion: "failure",
          head_sha: run.head_sha,
          html_url: "https://github.example.test/failed-run",
        }),
      }),
    "provider_smoke_failed",
  );
  assert.equal(transport.dispatches, 2);
  assert.equal(ephemeralSecretNames(transport).length, 0);

  const { plan: movedHeadPlan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  transport.moveHeadAfterSecret = "ephemeral";
  await expectFailure(
    () =>
      stageRun({
        config: cfg,
        state: currentState,
        handoff: currentHandoff,
        handoffBytes: serializeHandoff(currentHandoff),
        transport,
        custodyDirectory: custody,
        approvedPlan: movedHeadPlan,
        environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: RUN_CONFIRMATION },
        build: async () => "fixture-sealer",
        seal: fixtureSeal,
      }),
    "approved_head_changed",
  );
  assert.equal(transport.dispatches, 2);
  assert.equal(ephemeralSecretNames(transport).length, 0);
});

test("ambiguous dispatch adopts one exact run and never resends", async () => {
  const transport = new FixtureTransport();
  transport.dispatchAmbiguity = true;
  const result = await dispatchWorkflow(transport, {
    config: config(),
    headSha: "a".repeat(40),
    interrupted: { signal: null },
    baselineRuns: [],
  });
  assert.equal(result.dispatch_acknowledged, false);
  assert.equal(result.run.id, 9001);
  assert.equal(transport.dispatches, 1);

  await expectFailure(
    () =>
      identifyDispatchedRun(transport, {
        baselineIds: new Set(transport.workflowRuns.map((run) => run.id)),
        headSha: "a".repeat(40),
        startedAt: Date.now(),
        dispatchNonce: result.dispatch_nonce,
        interrupted: { signal: null },
        timeoutMs: 1,
      }),
    "dispatch_recovery_required",
  );

  const interrupted = new FixtureTransport();
  await expectFailure(
    () =>
      dispatchWorkflow(interrupted, {
        config: config(),
        headSha: "a".repeat(40),
        dispatchNonce: "3".repeat(32),
        interrupted: { signal: "SIGTERM" },
        baselineRuns: [],
      }),
    "operator_interrupted",
  );
  assert.equal(interrupted.dispatches, 0);

  const duplicate = new FixtureTransport();
  const duplicateNonce = "4".repeat(32);
  const title = `CMA Trigger provider smoke ${duplicateNonce}`;
  duplicate.workflowRuns = [
    {
      id: 9101,
      workflow_id: TARGET.workflow.id,
      path: WORKFLOW_PATH,
      event: "workflow_dispatch",
      head_branch: "develop",
      head_sha: "a".repeat(40),
      display_title: title,
      actor: { login: "chronoai-shining" },
      triggering_actor: { login: "chronoai-shining" },
      created_at: new Date().toISOString(),
      status: "queued",
    },
    {
      id: 9102,
      workflow_id: TARGET.workflow.id,
      path: WORKFLOW_PATH,
      event: "workflow_dispatch",
      head_branch: "develop",
      head_sha: "a".repeat(40),
      display_title: title,
      actor: { login: "chronoai-shining" },
      triggering_actor: { login: "chronoai-shining" },
      created_at: new Date().toISOString(),
      status: "queued",
    },
  ];
  await expectFailure(
    () =>
      identifyDispatchedRun(duplicate, {
        baselineIds: new Set(),
        headSha: "a".repeat(40),
        startedAt: Date.now(),
        dispatchNonce: duplicateNonce,
        interrupted: { signal: null },
        timeoutMs: 10,
      }),
    "dispatch_recovery_required",
  );
});

test("separate cleanup removes only the ephemeral handoff under reviewed preflight", async () => {
  const cfg = config();
  const currentState = stateWithCallerExchange();
  const transport = new FixtureTransport();
  const timestamp = transport.timestamp();
  const secretName = ephemeralHandoffSecretName("2".repeat(32));
  for (const name of [
    secretName,
    RUNTIME_SERVICE_SECRET,
    KIND_CA_MATERIAL_NAME,
    KUBECONFIG_SECRET,
  ]) {
    transport.secrets[name] = { created_at: timestamp, updated_at: timestamp };
  }
  const { plan } = await planEnvironment({
    config: cfg,
    state: currentState,
    transport,
  });
  const result = await cleanupEnvironment({
    config: cfg,
    state: currentState,
    transport,
    approvedPlan: plan,
    environment: { CMA_TRIGGER_SMOKE_ENV_CONFIRM: CLEANUP_CONFIRMATION },
  });
  assert.equal(result.status, "ephemeral_cleanup_complete");
  assert.ok(!Object.hasOwn(transport.secrets, secretName));
  for (const stable of [
    RUNTIME_SERVICE_SECRET,
    KIND_CA_MATERIAL_NAME,
    KUBECONFIG_SECRET,
  ]) {
    assert.ok(Object.hasOwn(transport.secrets, stable));
  }
  const deletes = transport.calls.filter((call) => call.method === "DELETE");
  assert.equal(deletes.length, 1);
  assert.ok(deletes[0].path.endsWith(`/${secretName}`));
});

test("runtime acceptance rejects the broad staging-administration UserService", async () => {
  const currentState = structuredClone(state());
  currentState.caller.service_id = PROVISIONING_SERVICE_ID;
  const value = handoff("header.payload.signature");
  await expectFailure(
    () => validateHandoff(value, currentState, PROVISIONING_SERVICE_ID, NOW),
    "runtime_authority_broadened",
  );
});
