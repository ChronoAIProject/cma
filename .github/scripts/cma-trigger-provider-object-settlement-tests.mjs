#!/usr/bin/env node

import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { mkdtemp, readFile, rm, stat, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  DEFAULT_LIMITS,
  SettlementFailure,
  main,
  settleProviderObjects,
  validateBucketInventory,
  validateDeleteAcknowledgement,
  validateHandoff,
  validateReceipt,
} from "./cma-trigger-provider-object-settlement.mjs";

const NYXID_BASE_URL = "https://nyx-api.chrono-ai.fun";
const OWNER_ID = "00000000-0000-4000-8000-000000000011";
const OTHER_OWNER_ID = "00000000-0000-4000-8000-000000000012";
const SUBJECT_ID = "00000000-0000-4000-8000-000000000013";
const NYXID_CLIENT_ID = "00000000-0000-4000-8000-000000000014";
const LAUNCH_ID = "00000000-0000-4000-8000-000000000015";
const OTHER_LAUNCH_ID = "00000000-0000-4000-8000-000000000016";
const SESSION_ID = "sesn_provider_smoke";
const OTHER_SESSION_ID = "sesn_prior_smoke";
const CONTEXT_BUCKET = "cma-trigger-context-smoke";
const WORKSPACE_BUCKET = "cma-trigger-workspace-smoke";
const SUBJECT_HASH = sha256(SUBJECT_ID);
const WORKFLOW_SHA = "b".repeat(40);
const STORAGE_SOURCE_SHA = "4087662e2a65e3747b3203ba619eca7342089272";
const STORAGE_SCOPES = ["proxy", "object:read", "object:delete"];
const SETTLED_AT = "2026-08-29T02:06:00Z";

function sha256(value) {
  return createHash("sha256").update(value, "utf8").digest("hex");
}

function handoffFixture() {
  return {
    schema_version: 1,
    nyxid_base_url: NYXID_BASE_URL,
    nyxid_client_id: NYXID_CLIENT_ID,
    nyxid_client_secret: "fixture-nyxid-client-secret",
    storage_client_id: "sa_0123456789abcdef01234567",
    storage_client_secret: "fixture-storage-client-secret",
    storage_owner_service_account_id: OWNER_ID,
    context_bucket: CONTEXT_BUCKET,
    trigger_workspace_bucket: WORKSPACE_BUCKET,
    public_github_service_slug: "cma-trigger-smoke-observer",
    subject_id: SUBJECT_ID,
    access_token: "fixture-caller-access-token",
    access_token_expires_at: 2_000_000_000,
  };
}

function evidenceFixture() {
  return {
    schema_version: 6,
    evidence_id: "p-01-gha-42-3",
    ledger_id: "P-01",
    result: "pass",
    expected_result:
      "one_completed_launch_to_one_reviewed_contribution_and_open_pull_request_with_provider_settlement",
    actual_result: "matched_expected",
    started_at: "2026-08-29T02:00:00Z",
    finished_at: "2026-08-29T02:04:00Z",
    provenance: {
      workflow: {
        repository: "ChronoAIProject/cma",
        repository_id: 1_330_597_394,
        workflow_commit: WORKFLOW_SHA,
        run_id: 42,
        run_attempt: 3,
        run_url: "https://github.com/ChronoAIProject/cma/actions/runs/42",
        artifact_name: "cma-trigger-provider-smoke-42-3",
        artifact_url:
          "https://github.com/ChronoAIProject/cma/actions/runs/42#artifacts",
      },
      deployment: {},
    },
    actors: {},
    fixture: {},
    action: {},
    external_inventory: {},
    sandbox_inventory: {},
    counts: {},
    resources: {
      launch_id: LAUNCH_ID,
      fork_repository_id: 401,
      fork_owner_account_id: 302,
      branch_name: `cma/${LAUNCH_ID}`,
      workspace_manifest_digest: "c".repeat(64),
      workspace_epoch: 1,
      session_id: SESSION_ID,
      runtime_generation: 1,
      delivery_origin_key: "trigger:launch:delivery:1",
      diff_artifact_id: "diff_provider_smoke",
      contribution_id: "contribution_provider_smoke",
      commit_id: "d".repeat(40),
      pull_request_provider_id: 501,
      pull_request_number: 7,
      pull_request_url:
        "https://github.com/ChronoAIProject/cma-trigger-fixture/pull/7",
    },
    correlations: {},
    stages: [],
    runtime_attestation: {},
    provider_settlement: {
      launch_state: "completed",
      reach_state: "revoked",
      session_credential_retirement: {
        session_id: SESSION_ID,
        settled_at: "2026-08-29T02:02:00Z",
      },
      verified_at: "2026-08-29T02:03:00Z",
    },
    cleanup: {
      github: "preserved_by_policy",
      cma_launch: "retained_until_terminal_ephemeral_teardown",
      session: "idle_awaiting_terminal_ephemeral_teardown",
      provider_object_settlement: null,
    },
    redaction: {
      status: "passed",
      audited_at: "2026-08-29T02:05:00Z",
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

function contextKey(sessionId, leaf) {
  return `sessions/${sessionId}/${leaf}`;
}

function workspaceKey(
  launchId,
  suffix = "baseline-v1.pack",
  subjectHash = SUBJECT_HASH,
) {
  return `trigger-workspaces/subjects/${subjectHash}/launches/${launchId}/${suffix}`;
}

function legacy(key, size = 1) {
  return { key, size, generation: 0 };
}

function legacyWithoutSize(key) {
  return { key, generation: 0 };
}

function fenced(key, generation = 1, size = 1, digest = "e".repeat(64)) {
  return { key, size, generation, digest, fenceVersion: 1 };
}

function sortObjects(objects) {
  return [...objects].sort((left, right) =>
    left.key < right.key ? -1 : left.key > right.key ? 1 : 0,
  );
}

function jsonResponse(value, status = 200) {
  return new Response(JSON.stringify(value), {
    status,
    headers: { "content-type": "application/json" },
  });
}

function bucketEnvelope(bucket, owner, objects) {
  return {
    data: {
      name: bucket,
      owner,
      createdAt: "2026-08-29T01:00:00Z",
      metadata: {},
      objects: sortObjects(objects),
    },
    error: null,
  };
}

function tokenEnvelope() {
  return {
    access_token: "fixture-storage-access-token",
    token_type: "Bearer",
    expires_in: 900,
    refresh_token: null,
    id_token: null,
    scope: STORAGE_SCOPES.join(" "),
  };
}

function defaultObjects() {
  return {
    [CONTEXT_BUCKET]: [
      fenced(contextKey(SESSION_ID, "context.jsonl"), 2, 120),
      fenced(contextKey(SESSION_ID, "handover.snapshot"), 4, 80),
      fenced(contextKey(SESSION_ID, "skill-packs.v1"), 1, 40),
    ],
    [WORKSPACE_BUCKET]: [
      fenced(workspaceKey(LAUNCH_ID), 3, 1_000),
      fenced(
        workspaceKey(
          LAUNCH_ID,
          `diffs/${"f".repeat(64)}-${"1".repeat(64)}.json`,
        ),
        7,
        300,
      ),
    ],
  };
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function createMockHttp({
  objects = defaultObjects(),
  owners = {},
  token = tokenEnvelope(),
  mutateBucketResponse,
  acknowledgement,
  throwDeleteKey,
  removeBeforeThrow = false,
  residualKeys = new Set(),
  unreadableDeleteKey,
} = {}) {
  const state = new Map(
    Object.entries(objects).map(([bucket, values]) => [bucket, clone(values)]),
  );
  const calls = [];
  const readCounts = new Map();

  const fetch = async (input, init = {}) => {
    const url = new URL(typeof input === "string" ? input : input.url);
    const method = init.method ?? "GET";
    calls.push({
      method,
      url: url.href,
      headers: { ...(init.headers ?? {}) },
      body: init.body,
    });
    if (method === "POST" && url.pathname === "/oauth/token") {
      return jsonResponse(clone(token));
    }
    const marker = `/api/v1/proxy/s/chrono-storage-service/api/buckets/`;
    const markerIndex = url.pathname.indexOf(marker);
    assert.notEqual(markerIndex, -1, `unexpected mocked path: ${url.pathname}`);
    const tail = url.pathname.slice(markerIndex + marker.length);
    const segments = tail.split("/");
    const bucket = decodeURIComponent(segments[0]);
    assert.ok(state.has(bucket), `unexpected mocked bucket: ${bucket}`);

    if (method === "GET" && segments.length === 1) {
      const read = (readCounts.get(bucket) ?? 0) + 1;
      readCounts.set(bucket, read);
      let body = bucketEnvelope(
        bucket,
        owners[bucket] ?? OWNER_ID,
        state.get(bucket),
      );
      if (mutateBucketResponse !== undefined) {
        body =
          mutateBucketResponse({ bucket, read, body: clone(body), state }) ??
          body;
      }
      return jsonResponse(body);
    }

    if (method === "DELETE" && segments[1] === "objects") {
      const key = url.searchParams.get("key");
      const values = state.get(bucket);
      const index = values.findIndex((object) => object.key === key);
      assert.notEqual(index, -1, `delete targeted an absent key: ${key}`);
      const object = clone(values[index]);
      const remove = () => {
        if (!residualKeys.has(key)) values.splice(index, 1);
      };
      if (key === throwDeleteKey) {
        if (removeBeforeThrow) remove();
        throw new Error("connection ended without a response");
      }
      remove();
      if (key === unreadableDeleteKey) {
        return new Response("not-json", {
          status: 200,
          headers: { "content-type": "text/plain" },
        });
      }
      const defaultBody =
        object.generation === 0
          ? { data: { deleted: true }, error: null }
          : {
              data: {
                deleted: true,
                generation: object.generation + 1,
                fenceVersion: 1,
              },
              error: null,
            };
      const body =
        acknowledgement?.({ bucket, key, object, body: clone(defaultBody) }) ??
        defaultBody;
      return jsonResponse(body);
    }
    assert.fail(`unexpected mocked request: ${method} ${url.href}`);
  };
  return { fetch, calls, state, readCounts };
}

async function settle(mock, options = {}) {
  return settleProviderObjects({
    handoff: options.handoff ?? handoffFixture(),
    evidence: options.evidence ?? evidenceFixture(),
    fetchImpl: mock.fetch,
    clock: options.clock ?? (() => SETTLED_AT),
    limits: options.limits ?? {},
  });
}

async function expectFailure(promise, code) {
  await assert.rejects(promise, (cause) => {
    assert.ok(cause instanceof SettlementFailure);
    assert.equal(cause.code, code);
    return true;
  });
}

function deleteCalls(mock) {
  return mock.calls.filter((call) => call.method === "DELETE");
}

test("successful settlement returns an exact redacted, run-bound receipt", async () => {
  const mock = createMockHttp();
  const receipt = await settle(mock);

  validateReceipt(receipt);
  assert.equal(receipt.schema_version, 1);
  assert.equal(receipt.evidence_id, "p-01-gha-42-3");
  assert.deepEqual(receipt.workflow, {
    repository_id: 1_330_597_394,
    run_id: 42,
    run_attempt: 3,
    source_sha: WORKFLOW_SHA,
  });
  assert.deepEqual(receipt.storage_contract, {
    repository: "ChronoAIProject/chrono-bucket",
    source_sha: STORAGE_SOURCE_SHA,
  });
  assert.equal(receipt.owner_service_account_id_sha256, sha256(OWNER_ID));
  assert.equal(receipt.launch_id_sha256, sha256(LAUNCH_ID));
  assert.equal(receipt.session_id_sha256, sha256(SESSION_ID));
  assert.equal(receipt.buckets.context.selected_objects, 3);
  assert.equal(receipt.buckets.context.selected_bytes_observed, 240);
  assert.equal(receipt.buckets.context.fenced_deletions_acknowledged, 3);
  assert.equal(receipt.buckets.trigger_workspace.selected_objects, 2);
  assert.equal(
    receipt.buckets.trigger_workspace.selected_bytes_observed,
    1_300,
  );
  assert.equal(deleteCalls(mock).length, 5);
  assert.equal(mock.readCounts.get(CONTEXT_BUCKET), 2);
  assert.equal(mock.readCounts.get(WORKSPACE_BUCKET), 2);

  const serialized = JSON.stringify(receipt);
  for (const protectedValue of [
    CONTEXT_BUCKET,
    WORKSPACE_BUCKET,
    OWNER_ID,
    LAUNCH_ID,
    SESSION_ID,
    "fixture-storage-client-secret",
    "fixture-storage-access-token",
  ]) {
    assert.equal(serialized.includes(protectedValue), false);
  }
});

test("mixed legacy and fenced objects use the exact deletion transports", async () => {
  const objects = {
    [CONTEXT_BUCKET]: [
      legacyWithoutSize(contextKey(SESSION_ID, "context.jsonl")),
      fenced(contextKey(SESSION_ID, "handover.snapshot"), 9, 12),
    ],
    [WORKSPACE_BUCKET]: [legacy(workspaceKey(LAUNCH_ID), 33)],
  };
  const mock = createMockHttp({ objects });
  const receipt = await settle(mock);
  const calls = deleteCalls(mock).map((call) => new URL(call.url));

  assert.equal(receipt.buckets.context.selected_objects_without_size, 1);
  assert.equal(receipt.buckets.context.selected_bytes_observed, 12);
  assert.equal(receipt.buckets.context.legacy_deletions_acknowledged, 1);
  assert.equal(receipt.buckets.context.fenced_deletions_acknowledged, 1);
  assert.equal(
    receipt.buckets.trigger_workspace.legacy_deletions_acknowledged,
    1,
  );
  const legacyCall = calls.find(
    (url) =>
      url.searchParams.get("key") === contextKey(SESSION_ID, "context.jsonl"),
  );
  const fencedCall = calls.find(
    (url) =>
      url.searchParams.get("key") ===
      contextKey(SESSION_ID, "handover.snapshot"),
  );
  assert.equal(legacyCall.searchParams.has("mutationGeneration"), false);
  assert.equal(fencedCall.searchParams.get("mutationGeneration"), "10");
  assert.equal(
    [...fencedCall.searchParams.keys()].sort().join(","),
    "key,mutationGeneration",
  );
});

test("unrelated live objects are preserved exactly and reported honestly", async () => {
  const contextPrior = fenced(
    contextKey(OTHER_SESSION_ID, "context.jsonl"),
    2,
    17,
  );
  const contextArbitrary = legacy("archive/retained.bin", 4);
  const workspacePrior = fenced(workspaceKey(OTHER_LAUNCH_ID), 5, 91);
  const objects = defaultObjects();
  objects[CONTEXT_BUCKET].push(contextPrior, contextArbitrary);
  objects[WORKSPACE_BUCKET].push(workspacePrior);
  const mock = createMockHttp({ objects });

  const receipt = await settle(mock);

  assert.equal(receipt.buckets.context.unrelated_live_objects_observed, 2);
  assert.equal(
    receipt.buckets.trigger_workspace.unrelated_live_objects_observed,
    1,
  );
  assert.deepEqual(
    sortObjects(mock.state.get(CONTEXT_BUCKET)),
    sortObjects([contextPrior, contextArbitrary]),
  );
  assert.deepEqual(mock.state.get(WORKSPACE_BUCKET), [workspacePrior]);
  const deletedKeys = deleteCalls(mock).map((call) =>
    new URL(call.url).searchParams.get("key"),
  );
  assert.equal(deletedKeys.includes(contextPrior.key), false);
  assert.equal(deletedKeys.includes(contextArbitrary.key), false);
  assert.equal(deletedKeys.includes(workspacePrior.key), false);
});

test("storage token mint uses only the accepted client-credentials form", async () => {
  const mock = createMockHttp();
  await settle(mock);
  const tokenCall = mock.calls[0];
  const form = new URLSearchParams(tokenCall.body);

  assert.equal(tokenCall.method, "POST");
  assert.equal(tokenCall.url, `${NYXID_BASE_URL}/oauth/token`);
  assert.deepEqual([...form.keys()].sort(), [
    "client_id",
    "client_secret",
    "grant_type",
    "scope",
  ]);
  assert.equal(form.get("grant_type"), "client_credentials");
  assert.equal(form.get("client_id"), handoffFixture().storage_client_id);
  assert.equal(
    form.get("client_secret"),
    handoffFixture().storage_client_secret,
  );
  assert.equal(form.get("scope"), STORAGE_SCOPES.join(" "));
});

test("bucket owner drift aborts before the first delete", async () => {
  const mock = createMockHttp({
    owners: { [WORKSPACE_BUCKET]: OTHER_OWNER_ID },
  });
  await expectFailure(settle(mock), "bucket_authority_drift");
  assert.equal(deleteCalls(mock).length, 0);
});

test("closed remote schemas reject extra, missing, and contradictory fields", async (context) => {
  await context.test("bucket response extra field", async () => {
    const mock = createMockHttp({
      mutateBucketResponse: ({ bucket, read, body }) => {
        if (bucket === CONTEXT_BUCKET && read === 1)
          body.data.unreviewed = true;
        return body;
      },
    });
    await expectFailure(settle(mock), "remote_contract_drift");
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("object response missing generation", async () => {
    const objects = defaultObjects();
    delete objects[CONTEXT_BUCKET][0].generation;
    const mock = createMockHttp({ objects });
    await expectFailure(settle(mock), "inventory_malformed");
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("legacy object carrying fenced metadata", async () => {
    const objects = defaultObjects();
    objects[CONTEXT_BUCKET][0] = {
      ...objects[CONTEXT_BUCKET][0],
      generation: 0,
    };
    const mock = createMockHttp({ objects });
    await expectFailure(settle(mock), "inventory_malformed");
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("token response extra field", async () => {
    const mock = createMockHttp({
      token: { ...tokenEnvelope(), unsafe: true },
    });
    await expectFailure(settle(mock), "storage_token_drift");
    assert.equal(mock.calls.length, 1);
  });
});

test("duplicate logical objects are rejected before deletion", async () => {
  const objects = defaultObjects();
  objects[CONTEXT_BUCKET].push(clone(objects[CONTEXT_BUCKET][0]));
  const mock = createMockHttp({ objects });
  await expectFailure(settle(mock), "duplicate_inventory_object");
  assert.equal(deleteCalls(mock).length, 0);
});

test("wrong dedicated-bucket namespaces fail closed", async (context) => {
  await context.test("workspace namespace in context bucket", async () => {
    const objects = defaultObjects();
    objects[CONTEXT_BUCKET].push(legacy(workspaceKey(OTHER_LAUNCH_ID), 1));
    const mock = createMockHttp({ objects });
    await expectFailure(settle(mock), "inventory_namespace_invalid");
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("context namespace in workspace bucket", async () => {
    const objects = defaultObjects();
    objects[WORKSPACE_BUCKET].push(
      legacy(contextKey(OTHER_SESSION_ID, "context.jsonl"), 1),
    );
    const mock = createMockHttp({ objects });
    await expectFailure(settle(mock), "inventory_namespace_invalid");
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("malformed workspace subject hash", async () => {
    const objects = defaultObjects();
    objects[WORKSPACE_BUCKET].push(
      legacy(
        workspaceKey(OTHER_LAUNCH_ID, "baseline-v1.pack", "A".repeat(64)),
        1,
      ),
    );
    const mock = createMockHttp({ objects });
    await expectFailure(settle(mock), "inventory_namespace_invalid");
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test(
    "same Launch under another valid subject hash",
    async () => {
      const objects = defaultObjects();
      objects[WORKSPACE_BUCKET].push(
        legacy(
          workspaceKey(LAUNCH_ID, "checkpoints/foreign.pack", "9".repeat(64)),
          1,
        ),
      );
      const mock = createMockHttp({ objects });
      await expectFailure(settle(mock), "workspace_subject_mismatch");
      assert.equal(deleteCalls(mock).length, 0);
    },
  );
});

test("inventory count and byte bounds reject overflow before deletion", async (context) => {
  await context.test("object count", async () => {
    const objects = {
      [CONTEXT_BUCKET]: [legacy("archive/a", 1), legacy("archive/b", 1)],
      [WORKSPACE_BUCKET]: [],
    };
    const mock = createMockHttp({ objects });
    await expectFailure(
      settle(mock, {
        limits: { ...DEFAULT_LIMITS, maxObjectsPerBucket: 1 },
      }),
      "inventory_count_overflow",
    );
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("per-bucket observed bytes", async () => {
    const objects = {
      [CONTEXT_BUCKET]: [legacy("archive/a", 11)],
      [WORKSPACE_BUCKET]: [],
    };
    const mock = createMockHttp({ objects });
    await expectFailure(
      settle(mock, {
        limits: { ...DEFAULT_LIMITS, maxInventoryBytesPerBucket: 10 },
      }),
      "inventory_byte_overflow",
    );
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("total selected bytes", async () => {
    const objects = {
      [CONTEXT_BUCKET]: [legacy(contextKey(SESSION_ID, "context.jsonl"), 6)],
      [WORKSPACE_BUCKET]: [legacy(workspaceKey(LAUNCH_ID), 6)],
    };
    const mock = createMockHttp({ objects });
    await expectFailure(
      settle(mock, {
        limits: { ...DEFAULT_LIMITS, maxSelectedBytes: 10 },
      }),
      "selected_byte_overflow",
    );
    assert.equal(deleteCalls(mock).length, 0);
  });

  await context.test("total selected deletion count", async () => {
    const objects = {
      [CONTEXT_BUCKET]: [legacy(contextKey(SESSION_ID, "context.jsonl"), 1)],
      [WORKSPACE_BUCKET]: [legacy(workspaceKey(LAUNCH_ID), 1)],
    };
    const mock = createMockHttp({ objects });
    await expectFailure(
      settle(mock, {
        limits: { ...DEFAULT_LIMITS, maxDeletes: 1 },
      }),
      "selected_object_count_overflow",
    );
    assert.equal(deleteCalls(mock).length, 0);
  });
});

test("acknowledgement drift is rejected for fenced and legacy deletes", async (context) => {
  await context.test("fenced generation drift", async () => {
    const mock = createMockHttp({
      acknowledgement: ({ object, body }) => {
        if (object.generation > 0) body.data.generation += 1;
        return body;
      },
    });
    await expectFailure(settle(mock), "acknowledgement_drift");
  });

  await context.test("legacy response gains fenced fields", async () => {
    const objects = {
      [CONTEXT_BUCKET]: [legacy(contextKey(SESSION_ID, "context.jsonl"), 1)],
      [WORKSPACE_BUCKET]: [],
    };
    const mock = createMockHttp({
      objects,
      acknowledgement: ({ body }) => {
        body.data.generation = 1;
        return body;
      },
    });
    await expectFailure(settle(mock), "acknowledgement_drift");
  });
});

test("an ambiguous delete transport failure never produces a receipt", async () => {
  const key = contextKey(SESSION_ID, "context.jsonl");
  const mock = createMockHttp({
    throwDeleteKey: key,
    removeBeforeThrow: true,
  });
  await expectFailure(settle(mock), "delete_outcome_ambiguous");
  assert.equal(
    mock.state.get(CONTEXT_BUCKET).some((object) => object.key === key),
    false,
  );
});

test("an unreadable post-delete response is also an ambiguous outcome", async () => {
  const key = contextKey(SESSION_ID, "context.jsonl");
  const mock = createMockHttp({ unreadableDeleteKey: key });
  await expectFailure(settle(mock), "delete_outcome_ambiguous");
  assert.equal(
    mock.state.get(CONTEXT_BUCKET).some((object) => object.key === key),
    false,
  );
});

test("a residual current-run live object after reread fails settlement", async () => {
  const key = contextKey(SESSION_ID, "context.jsonl");
  const mock = createMockHttp({ residualKeys: new Set([key]) });
  await expectFailure(settle(mock), "residual_current_run_object");
  assert.equal(mock.readCounts.get(CONTEXT_BUCKET), 2);
  assert.equal(mock.readCounts.get(WORKSPACE_BUCKET), 2);
});

test("unrelated inventory drift during settlement is detected", async () => {
  const objects = defaultObjects();
  objects[CONTEXT_BUCKET].push(legacy("archive/retained.bin", 5));
  const mock = createMockHttp({
    objects,
    mutateBucketResponse: ({ bucket, read, body }) => {
      if (bucket === CONTEXT_BUCKET && read === 2) {
        const retained = body.data.objects.find(
          (object) => object.key === "archive/retained.bin",
        );
        retained.size = 6;
      }
      return body;
    },
  });
  await expectFailure(settle(mock), "unrelated_object_drift");
});

test("generation overflow is refused before a deletion request", async () => {
  const objects = {
    [CONTEXT_BUCKET]: [
      fenced(
        contextKey(SESSION_ID, "context.jsonl"),
        Number.MAX_SAFE_INTEGER,
        1,
      ),
    ],
    [WORKSPACE_BUCKET]: [],
  };
  const mock = createMockHttp({ objects });
  await expectFailure(settle(mock), "inventory_malformed");
  assert.equal(deleteCalls(mock).length, 0);
});

test("default and invalid invocations make no network call", async (context) => {
  await context.test("no arguments", async () => {
    let calls = 0;
    await expectFailure(
      main([], {
        fetchImpl: async () => {
          calls += 1;
          throw new Error("must not run");
        },
      }),
      "invalid_arguments",
    );
    assert.equal(calls, 0);
  });

  await context.test("invalid evidence", async () => {
    let calls = 0;
    const evidence = evidenceFixture();
    evidence.schema_version = 5;
    await expectFailure(
      settleProviderObjects({
        handoff: handoffFixture(),
        evidence,
        fetchImpl: async () => {
          calls += 1;
          throw new Error("must not run");
        },
      }),
      "unsafe_evidence",
    );
    assert.equal(calls, 0);
  });

  await context.test(
    "handoff missing the owner service-account UUID",
    async () => {
      let calls = 0;
      const handoff = handoffFixture();
      delete handoff.storage_owner_service_account_id;
      await expectFailure(
        settleProviderObjects({
          handoff,
          evidence: evidenceFixture(),
          fetchImpl: async () => {
            calls += 1;
            throw new Error("must not run");
          },
        }),
        "unsafe_handoff",
      );
      assert.equal(calls, 0);
    },
  );

  await context.test("handoff owner is not an exact UUID", async () => {
    let calls = 0;
    const handoff = handoffFixture();
    handoff.storage_owner_service_account_id = "service-account-owner";
    await expectFailure(
      settleProviderObjects({
        handoff,
        evidence: evidenceFixture(),
        fetchImpl: async () => {
          calls += 1;
          throw new Error("must not run");
        },
      }),
      "unsafe_handoff",
    );
    assert.equal(calls, 0);
  });

  await context.test(
    "already-finalized evidence cannot drive another settlement",
    async () => {
      let calls = 0;
      const evidence = evidenceFixture();
      evidence.cleanup.provider_object_settlement = {};
      await expectFailure(
        settleProviderObjects({
          handoff: handoffFixture(),
          evidence,
          fetchImpl: async () => {
            calls += 1;
            throw new Error("must not run");
          },
        }),
        "unsafe_evidence",
      );
      assert.equal(calls, 0);
    },
  );

  await context.test("unaudited evidence cannot drive settlement", async () => {
    let calls = 0;
    const evidence = evidenceFixture();
    evidence.redaction.status = "pending";
    evidence.redaction.audited_at = null;
    await expectFailure(
      settleProviderObjects({
        handoff: handoffFixture(),
        evidence,
        fetchImpl: async () => {
          calls += 1;
          throw new Error("must not run");
        },
      }),
      "unsafe_evidence",
    );
    assert.equal(calls, 0);
  });
});

test("live CLI mode validates explicit files and reserves output before HTTP", async () => {
  const directory = await mkdtemp(
    join(tmpdir(), "cma-provider-settlement-test-"),
  );
  const handoffPath = join(directory, "handoff.json");
  const evidencePath = join(directory, "evidence.json");
  const outputPath = join(directory, "receipt.json");
  try {
    await Promise.all([
      writeFile(handoffPath, `${JSON.stringify(handoffFixture())}\n`, {
        mode: 0o600,
      }),
      writeFile(evidencePath, `${JSON.stringify(evidenceFixture())}\n`, {
        mode: 0o600,
      }),
    ]);
    const mock = createMockHttp();
    const receipt = await main(
      [
        "settle",
        "--handoff",
        handoffPath,
        "--evidence",
        evidencePath,
        "--output",
        outputPath,
      ],
      { fetchImpl: mock.fetch, clock: () => SETTLED_AT },
    );
    assert.deepEqual(JSON.parse(await readFile(outputPath, "utf8")), receipt);
    assert.equal((await stat(outputPath)).mode & 0o777, 0o600);

    let secondNetworkCalls = 0;
    await expectFailure(
      main(
        [
          "settle",
          "--handoff",
          handoffPath,
          "--evidence",
          evidencePath,
          "--output",
          outputPath,
        ],
        {
          fetchImpl: async () => {
            secondNetworkCalls += 1;
            throw new Error("must not run");
          },
        },
      ),
      "invalid_output",
    );
    assert.equal(secondNetworkCalls, 0);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
});

test("exported pure validators enforce strict acknowledgements and ownership", () => {
  assert.equal(
    validateHandoff(handoffFixture()).storage_owner_service_account_id,
    OWNER_ID,
  );
  const legacyObject = legacy("archive/object", 1);
  assert.deepEqual(
    validateDeleteAcknowledgement(
      { data: { deleted: true }, error: null },
      legacyObject,
    ),
    { kind: "legacy" },
  );
  assert.throws(
    () =>
      validateBucketInventory(
        bucketEnvelope(CONTEXT_BUCKET, OTHER_OWNER_ID, []),
        {
          bucket: CONTEXT_BUCKET,
          ownerServiceAccountId: OWNER_ID,
          limits: DEFAULT_LIMITS,
        },
      ),
    (cause) =>
      cause instanceof SettlementFailure &&
      cause.code === "bucket_authority_drift",
  );
});
