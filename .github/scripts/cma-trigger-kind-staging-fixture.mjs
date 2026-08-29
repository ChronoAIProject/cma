#!/usr/bin/env node

import { chmod, readFile, rename, writeFile } from "node:fs/promises";
import assert from "node:assert/strict";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SDK_ENTRY = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../sdk/typescript/dist/index.js",
);
const SHA1 = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const INPUT_KEY = /^[a-z][a-z0-9_]{0,63}$/;
const REQUIRED_CAPABILITIES = Object.freeze([
  "conversation",
  "durable_conversation",
  "workspace_checkpoint",
  "restricted_service_gateway",
  "credential_refresh",
  "credential_revocation",
]);
const REPOSITORY_OPERATIONS = Object.freeze([
  "repository_read",
  "tree_read",
  "object_create",
  "launch_ref_create",
  "launch_ref_update",
  "status_read",
]);
const ALLOWED_GITHUB_SCOPES = new Set([
  "public_repo",
  "read:user",
  "user:email",
]);

class FixtureFailure extends Error {
  constructor(code) {
    super(code);
    this.name = "FixtureFailure";
    this.code = code;
  }
}

function fail(code) {
  throw new FixtureFailure(code);
}

function environment(name, maximum = 16_384) {
  const value = process.env[name];
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    value.includes("\0")
  ) {
    fail(`invalid_${name.toLowerCase()}`);
  }
  return value;
}

function bounded(value, name, maximum, pattern) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    (pattern !== undefined && !pattern.test(value))
  ) {
    fail(`invalid_${name}`);
  }
  return value;
}

function positiveInteger(raw, name) {
  if (!/^[1-9][0-9]*$/.test(raw)) fail(`invalid_${name}`);
  const parsed = Number(raw);
  if (!Number.isSafeInteger(parsed)) fail(`invalid_${name}`);
  return parsed;
}

function oneMarker(value, name, maximum, allowNewlines = false) {
  bounded(value, name, maximum);
  const forbidden = allowNewlines
    ? /[\u0000-\u0008\u000b\u000c\u000e-\u001f\u007f]/u
    : /[\u0000-\u001f\u007f]/u;
  if (forbidden.test(value) || value.split("{marker}").length !== 2) {
    fail(`invalid_${name}`);
  }
  return value;
}

function githubLogin(value) {
  return bounded(
    value,
    "contributor_login",
    39,
    /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/,
  );
}

function connectionMatches(connection, serviceId, contributorLogin) {
  const scopes = connection?.granted_scopes;
  return (
    connection?.state === "active" &&
    connection.service_id === serviceId &&
    Number.isSafeInteger(connection.account_id) &&
    connection.account_id > 0 &&
    typeof connection.account_login === "string" &&
    connection.account_login.toLowerCase() === contributorLogin.toLowerCase() &&
    Array.isArray(scopes) &&
    new Set(scopes).size === scopes.length &&
    scopes.includes("public_repo") &&
    !scopes.includes("repo") &&
    !scopes.includes("workflow") &&
    scopes.every((scope) => ALLOWED_GITHUB_SCOPES.has(scope))
  );
}

function promptTemplate(pathTemplate, contentTemplate, markerInput) {
  const marker = `{{${markerInput}}}`;
  const path = pathTemplate.replace("{marker}", marker);
  const content = contentTemplate.replace("{marker}", marker);
  return [
    "Work only in the checked-out repository and complete this bounded fixture task.",
    `Create exactly one file at ${JSON.stringify(path)}.`,
    `Its complete UTF-8 content must be exactly:\n${content}`,
    "Do not alter another path. Verify the path and bytes before finishing.",
  ].join("\n\n");
}

async function atomicJson(path, value) {
  const temporary = `${path}.tmp`;
  await writeFile(temporary, `${JSON.stringify(value, null, 2)}\n`, {
    mode: 0o600,
    flag: "wx",
  });
  await rename(temporary, path);
  await chmod(path, 0o600);
}

async function publishFixture(outputPath) {
  const baseUrl = new URL(environment("CMA_TRIGGER_SMOKE_BASE_URL", 512));
  if (
    baseUrl.protocol !== "https:" ||
    baseUrl.username !== "" ||
    baseUrl.password !== "" ||
    baseUrl.pathname !== "/" ||
    baseUrl.search !== "" ||
    baseUrl.hash !== ""
  ) {
    fail("invalid_base_url");
  }
  const tokenPath = environment("CMA_TRIGGER_SMOKE_ACCESS_TOKEN_PATH", 4096);
  const servicePath = environment("CMA_TRIGGER_SMOKE_SERVICE_ID_PATH", 4096);
  if (!tokenPath.startsWith("/") || !servicePath.startsWith("/")) {
    fail("invalid_credential_path");
  }
  const accessToken = (await readFile(tokenPath, "utf8")).trim();
  const serviceId = (await readFile(servicePath, "utf8")).trim();
  bounded(accessToken, "access_token", 16_384);
  bounded(serviceId, "service_id", 255);

  const repositoryId = positiveInteger(
    environment("CMA_TRIGGER_SMOKE_UPSTREAM_REPOSITORY_ID", 20),
    "repository_id",
  );
  const defaultBranch = bounded(
    environment("CMA_TRIGGER_SMOKE_UPSTREAM_DEFAULT_BRANCH", 255),
    "default_branch",
    255,
  );
  const markerInput = bounded(
    environment("CMA_TRIGGER_SMOKE_MARKER_INPUT", 64),
    "marker_input",
    64,
    INPUT_KEY,
  );
  const pathTemplate = oneMarker(
    environment("CMA_TRIGGER_SMOKE_EXPECTED_PATH_TEMPLATE", 512),
    "path_template",
    512,
  );
  if (!pathTemplate.startsWith("cma-trigger-smoke/")) {
    fail("invalid_path_template");
  }
  const contentTemplate = oneMarker(
    environment("CMA_TRIGGER_SMOKE_EXPECTED_CONTENT_TEMPLATE", 4096),
    "content_template",
    4096,
    true,
  );
  const runMarker = bounded(
    environment("CMA_TRIGGER_SMOKE_RUN_MARKER", 63),
    "run_marker",
    63,
    /^[A-Za-z0-9][A-Za-z0-9._-]{0,62}$/,
  );
  const contributorLogin = githubLogin(
    environment("CMA_TRIGGER_SMOKE_CONTRIBUTOR_LOGIN", 39),
  );

  const sdk = await import(SDK_ENTRY);
  let sequence = 0;
  const client = sdk.createClient({
    baseUrl: baseUrl.origin,
    cookies: false,
    credential: () => accessToken,
    requestTimeoutMs: 30_000,
    requestIdProvider: () => `cma-staging-${runMarker}-${++sequence}`,
  });

  const connection = await client.preflightTriggerConnection(serviceId);
  if (!connectionMatches(connection, serviceId, contributorLogin)) {
    fail("github_connection_inactive");
  }

  const content = {
    schema_version: 1,
    publisher_display_identity: "CMA Trigger provider smoke",
    repository: {
      repository_id: repositoryId,
      selected_ref: defaultBranch,
      contribution_base_ref: defaultBranch,
      source_mode: "pinned_commit",
    },
    task: {
      public_summary: "Create one provider-smoke marker file",
      prompt_template: promptTemplate(
        pathTemplate,
        contentTemplate,
        markerInput,
      ),
      renderer_version: 1,
      input_fields: [
        {
          key: markerInput,
          kind: { text: { min_bytes: 1, max_bytes: 63 } },
          required: true,
          default: runMarker,
          editable: true,
        },
      ],
    },
    session: {
      required_capabilities: REQUIRED_CAPABILITIES,
      permission_ceiling: "autonomous",
      sandbox_profile: "cma-trigger-v1",
      resources: {
        default: { cpu_millis: 500, memory_mib: 1024 },
        ceiling: { cpu_millis: 1000, memory_mib: 2048 },
      },
      timeout_default_seconds: 3600,
      timeout_ceiling_seconds: 7200,
    },
    repository_access: {
      credential_route: "contributor_connection",
      required_scope_classes: ["public_repository"],
      allowed_operation_classes: REPOSITORY_OPERATIONS,
      workflow_change_policy: "refuse",
    },
    setup: { executor_policy: "disabled", steps: [] },
    contribution: {
      branch_prefix: "cma/",
      commit_attribution: "contributor_no_reply",
      pull_request: {
        title_template: "CMA Trigger provider smoke",
        body_template: "Automated CMA Trigger provider-smoke contribution.",
      },
    },
    commercial_policy: {
      billing_mode: "zero_cost",
      policy_reference: "public-zero-cost-v1",
    },
    override_policy: {
      editable_fields: ["task_input", "work_label", "resource_size", "timeout"],
      optional_checks: [],
    },
    public_projection: { version: 1 },
  };

  const draft = await client.createTrigger(serviceId, content, {
    idempotencyKey: `cma-staging-${runMarker}-draft`,
  });
  const revision = await client.publishTrigger(
    draft.id,
    draft.version,
    draft.selected_service_id,
    { idempotencyKey: `cma-staging-${runMarker}-publish` },
  );
  const sourceSha = revision.content.upstream.observed_source_sha;
  if (
    !UUID.test(revision.public_id) ||
    !SHA256.test(revision.digest) ||
    typeof sourceSha !== "string" ||
    !SHA1.test(sourceSha) ||
    revision.content.upstream.repository.repository_id !== repositoryId ||
    revision.content.upstream.repository.default_branch !== defaultBranch ||
    revision.availability !== "published"
  ) {
    fail("published_fixture_mismatch");
  }
  const preview = await client.readTriggerPreview(revision.public_id);
  if (
    preview.availability !== "published" ||
    preview.revision_digest !== revision.digest ||
    preview.repository_id !== repositoryId
  ) {
    fail("published_preview_mismatch");
  }

  await atomicJson(outputPath, {
    schema_version: 1,
    public_id: revision.public_id,
    revision_digest: revision.digest,
    source_sha: sourceSha,
  });
}

function selfTest() {
  const serviceId = "9d888cf8-9c0c-4a04-9cac-62bcefe8172f";
  const connection = {
    service_id: serviceId,
    state_version: 3,
    state: "active",
    account_id: 42,
    account_node_id: "MDQ6VXNlcjQy",
    account_login: "chronoai-shining",
    avatar_url: null,
    granted_scopes: ["public_repo", "read:user", "user:email"],
    consent_observed_at: "2026-08-29T00:00:00Z",
  };
  assert.equal(
    connectionMatches(connection, serviceId, "ChronoAI-Shining"),
    true,
  );
  for (const mutation of [
    { state: "inactive" },
    { service_id: "2050f85a-d648-4f2f-9c86-ea9b8f267739" },
    { account_id: null },
    { account_login: "another-user" },
    { granted_scopes: [] },
    { granted_scopes: ["repo"] },
    { granted_scopes: ["public_repo", "workflow"] },
    { granted_scopes: ["public_repo", "admin:org"] },
    { granted_scopes: ["public_repo", "public_repo"] },
  ]) {
    assert.equal(
      connectionMatches(
        { ...connection, ...mutation },
        serviceId,
        "chronoai-shining",
      ),
      false,
    );
  }
  assert.equal(githubLogin("chronoai-shining"), "chronoai-shining");
  assert.throws(() => githubLogin("-invalid"), FixtureFailure);
  assert.throws(
    () => oneMarker("missing-marker", "path_template", 512),
    FixtureFailure,
  );
  assert.match(
    promptTemplate("cma-trigger-smoke/{marker}.txt", "{marker}\n", "marker"),
    /\{\{marker\}\}/,
  );
  console.log("[staging-fixture] self-test passed");
}

async function main() {
  if (process.argv.length === 3 && process.argv[2] === "--self-test") {
    selfTest();
    return;
  }
  if (process.argv.length !== 4 || process.argv[2] !== "--publish") {
    console.error(
      "usage: cma-trigger-kind-staging-fixture.mjs --self-test | --publish OUTPUT",
    );
    process.exitCode = 2;
    return;
  }
  const outputPath = process.argv[3];
  if (!outputPath.startsWith("/")) {
    console.error("[staging-fixture] failed: invalid_output_path");
    process.exitCode = 1;
    return;
  }
  try {
    await publishFixture(outputPath);
    console.log("[staging-fixture] ephemeral Trigger revision published");
  } catch (cause) {
    const code =
      cause instanceof FixtureFailure ? cause.code : "fixture_bootstrap_failed";
    console.error(`[staging-fixture] failed: ${code}`);
    process.exitCode = 1;
  }
}

await main();
