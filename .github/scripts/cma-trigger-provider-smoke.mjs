#!/usr/bin/env node

import { createHash } from "node:crypto";
import { access, readFile, rename, writeFile } from "node:fs/promises";
import { constants as fsConstants } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const TRIGGER_CONFORMANCE = "https://spec.chrono-ai.fun/cma/1.0/conf/trigger";
const REQUIRED_OPENAPI_PATHS = [
  "/api/v1/triggers/public/{public_id}",
  "/api/v1/triggers/public/{public_id}/launches",
  "/api/v1/launches/{launch_id}",
  "/api/v1/launches/{launch_id}/start",
  "/api/v1/launches/{launch_id}/contributions/review",
  "/api/v1/launches/{launch_id}/contributions/publish",
  "/api/v1/launches/{launch_id}/pull-request/review",
  "/api/v1/launches/{launch_id}/pull-request/open",
  "/api/v1/launches/{launch_id}/events",
];
const FAILURE_STATES = new Set([
  "failed_retryable",
  "failed_terminal",
  "cancelled",
]);
const SETTLED_PRIOR_LAUNCH_STATES = new Set([
  "pr_open",
  "completed",
  "failed_terminal",
  "cancelled",
]);
const MAX_PUBLIC_JSON_BYTES = 8 * 1024 * 1024;
const SDK_ENTRY = resolve(
  dirname(fileURLToPath(import.meta.url)),
  "../../sdk/typescript/dist/index.js",
);
const UUID =
  /^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
const SHA1 = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const BUILD_SHA = /^[0-9a-f]{7,64}$/;
const LOGIN = /^[A-Za-z0-9](?:[A-Za-z0-9-]{0,37}[A-Za-z0-9])?$/;
const REPOSITORY = /^[A-Za-z0-9_.-]{1,100}$/;
const INPUT_KEY = /^[a-z][a-z0-9_]{0,63}$/;
const RUN_MARKER = /^[a-z0-9][a-z0-9-]{0,62}$/;

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

function object(value, path) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    fail("invalid_config", `${path} must be an object`);
  }
  return value;
}

function exactKeys(value, expected, path) {
  const actual = Object.keys(object(value, path)).sort();
  const wanted = [...expected].sort();
  if (JSON.stringify(actual) !== JSON.stringify(wanted)) {
    fail("invalid_config", `${path} must contain exactly ${wanted.join(", ")}`);
  }
}

function text(value, path, maximum = 1024) {
  if (
    typeof value !== "string" ||
    value.length === 0 ||
    value.length > maximum ||
    /[\u0000-\u001f\u007f]/u.test(value)
  ) {
    fail("invalid_config", `${path} must be bounded non-control text`);
  }
  return value;
}

function integer(value, path, minimum, maximum) {
  if (!Number.isSafeInteger(value) || value < minimum || value > maximum) {
    fail(
      "invalid_config",
      `${path} must be an integer from ${minimum} through ${maximum}`,
    );
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

function validateConfig(raw) {
  exactKeys(
    raw,
    [
      "schema_version",
      "base_url",
      "expected_build_sha",
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
  if (raw.schema_version !== 1)
    fail("invalid_config", "config.schema_version must be 1");

  const baseUrl = new URL(text(raw.base_url, "config.base_url", 512));
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
  if (!BUILD_SHA.test(raw.expected_build_sha)) {
    fail(
      "invalid_config",
      "config.expected_build_sha must pin one hexadecimal build",
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
      "session_cleanup",
    ],
    "config.fixture_policy",
  );
  for (const [field, expected] of Object.entries({
    repository_owner: "ChronoAIProject",
    lifecycle: "persistent_public_fixture",
    secrets: "forbidden",
    github_cleanup: "preserve",
    session_cleanup: "stop_runtime_preserve_session",
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
  return raw;
}

async function loadConfig(path) {
  let parsed;
  try {
    parsed = JSON.parse(await readFile(path, "utf8"));
  } catch {
    fail("invalid_config", "configuration is not readable JSON");
  }
  return validateConfig(parsed);
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

class Evidence {
  constructor(path, config, marker) {
    this.path = path;
    this.document = {
      schema_version: 1,
      status: "running",
      started_at: now(),
      finished_at: null,
      run: {
        marker,
        repository: process.env.GITHUB_REPOSITORY ?? "local",
        revision: process.env.GITHUB_SHA ?? "local",
        workflow_run_id: process.env.GITHUB_RUN_ID ?? "local",
        workflow_run_attempt: process.env.GITHUB_RUN_ATTEMPT ?? "local",
      },
      fixture: {
        upstream_repository_id: config.upstream.repository_id,
        upstream_owner: config.upstream.owner,
        upstream_repository: config.upstream.repository,
        source_sha: config.source_sha,
        public_trigger_id: config.public_id,
        trigger_revision_digest: config.revision_digest,
        contributor_login: config.contributor.login,
        expected_fork_mode: config.contributor.expected_fork_mode,
        launch_creation_mode: null,
        policy: config.fixture_policy,
      },
      deployment: null,
      stages: [],
      counts: {
        launch: 0,
        fork: 0,
        fork_created_delta: 0,
        branch: 0,
        session: 0,
        initial_owner_message: 0,
        contribution_commit: 0,
        pull_request: 0,
      },
      resources: {
        launch_id: null,
        fork_repository_id: null,
        branch_name: null,
        session_id: null,
        delivery_origin_key: null,
        diff_artifact_id: null,
        contribution_id: null,
        commit_id: null,
        pull_request_number: null,
        pull_request_url: null,
      },
      correlations: [],
      cleanup: {
        github: "preserved_by_policy",
        cma_launch: "preserved_by_policy",
        session: "not_created",
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
  if (response.status === 404) return { status: 404, body: null };
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
  return { status: response.status, body };
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
  return SETTLED_PRIOR_LAUNCH_STATES.has(launch.state);
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
  assert(
    settledPriorLaunch({ state: "pr_open" }),
    "self_test_failed",
    "preserved open PR was not treated as settled",
  );
  assert(
    !settledPriorLaunch({ state: "ready" }) &&
      !settledPriorLaunch({ state: "contributing" }) &&
      !settledPriorLaunch({ state: "failed_retryable" }),
    "self_test_failed",
    "an unsettled Launch was allowed to overlap the smoke",
  );
  console.log("[smoke] launch selection self-test passed");
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
  const config = await loadConfig(configPath);
  const marker = requiredEnvironment("CMA_TRIGGER_SMOKE_RUN_MARKER", 63);
  if (!RUN_MARKER.test(marker))
    fail("invalid_run_marker", "run marker is invalid");
  const evidence = new Evidence(evidencePath, config, marker);
  await evidence.write();
  let currentStage = "environment";
  let client;
  let sessionId;

  try {
    const accessToken = requiredEnvironment("CMA_TRIGGER_SMOKE_ACCESS_TOKEN");
    const serviceId = requiredEnvironment("CMA_TRIGGER_SMOKE_SERVICE_ID", 255);
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
      evidence.document.deployment = {
        version: conformance.version,
        build_sha: conformance.buildSha,
        trigger_conformance: true,
      };
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
      evidence.document.fixture.launch_creation_mode = selected.creationMode;

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
      sessionId = launch.session.session_id;
      evidence.document.counts.fork = 1;
      evidence.document.counts.branch = 1;
      evidence.document.counts.session = 1;
      evidence.document.resources.fork_repository_id =
        launch.fork.repository.repository_id;
      evidence.document.resources.branch_name = launch.branch.name;
      evidence.document.resources.session_id = sessionId;
      evidence.document.resources.delivery_origin_key =
        launch.delivery.origin_key;
      evidence.document.cleanup.session = "preserved_for_investigation";
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
      const provider = (
        await fetchJson(
          githubUrl(
            config,
            config.upstream.owner,
            config.upstream.repository,
            `/pulls/${String(pullRequest.number)}`,
          ),
          [200],
        )
      ).body;
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

      const query = new URLSearchParams({
        state: "all",
        head: `${config.contributor.login}:${launch.branch.name}`,
        base: config.upstream.default_branch,
        per_page: "100",
      });
      const matches = (
        await fetchJson(
          githubUrl(
            config,
            config.upstream.owner,
            config.upstream.repository,
            `/pulls?${query.toString()}`,
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
      evidence.document.correlations = correlations;
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

    currentStage = "session_cleanup";
    await evidence.stage(currentStage, async () => {
      await client.stopSession(sessionId);
      const stopped = await client.readSession(sessionId);
      assertEqual(
        stopped.lifecycle,
        "idle",
        "session_cleanup_failed",
        "smoke session was not stopped",
      );
      evidence.document.cleanup.session = "idle_and_preserved";
    });

    evidence.document.status = "passed";
    evidence.document.finished_at = now();
    await evidence.write();
  } catch (cause) {
    evidence.document.status = "failed";
    evidence.document.finished_at = now();
    evidence.document.failure = safeFailure(cause, currentStage);
    if (sessionId !== undefined)
      evidence.document.cleanup.session = "preserved_for_investigation";
    await evidence.write();
    console.error(
      `[smoke] failed at ${currentStage}: ${evidence.document.failure.code}`,
    );
    process.exitCode = 1;
  }
}

async function auditEvidence(path) {
  let serialized;
  try {
    serialized = await readFile(path, "utf8");
    JSON.parse(serialized);
  } catch {
    fail("evidence_missing", "evidence file is missing or not JSON");
  }
  for (const name of [
    "CMA_TRIGGER_SMOKE_ACCESS_TOKEN",
    "CMA_TRIGGER_SMOKE_SERVICE_ID",
  ]) {
    const secret = process.env[name];
    if (
      typeof secret === "string" &&
      secret.length > 0 &&
      serialized.includes(secret)
    ) {
      fail("evidence_secret_detected", "evidence contains a protected value");
    }
  }
  const forbidden = [
    /\bBearer\s+[^\s"]+/iu,
    /"(?:access_token|refresh_token|authorization|cookie|set-cookie|provider_body|rendered_prompt)"\s*:/iu,
    /\b(?:github_pat_|gh[opusr]_|sk-[A-Za-z0-9])/u,
  ];
  if (forbidden.some((pattern) => pattern.test(serialized))) {
    fail(
      "evidence_secret_detected",
      "evidence contains a forbidden credential or response field",
    );
  }
  console.log("[smoke] evidence redaction audit passed");
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
    return;
  }
  if (rest.length > 0 || first === undefined) {
    fail(
      "invalid_arguments",
      "usage: --self-test | --validate-config FILE | --run CONFIG EVIDENCE | --audit-evidence FILE",
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
  fail(
    "invalid_arguments",
    "usage: --self-test | --validate-config FILE | --run CONFIG EVIDENCE | --audit-evidence FILE",
  );
}

await main().catch((cause) => {
  const code = cause instanceof SmokeFailure ? cause.code : "harness_failed";
  console.error(`[smoke] ${code}`);
  process.exitCode = 1;
});
