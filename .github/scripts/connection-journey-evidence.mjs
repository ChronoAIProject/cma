import assert from "node:assert/strict";
import { createHash } from "node:crypto";

export const connectionCaseTitles = Object.freeze({
  "J4-oauth":
    "J4-oauth G-T2 missing service connection assistant continues via oauth",
  "J4-api_key":
    "J4-api_key G-T2 missing service connection assistant continues via api_key",
  "J4-device_code":
    "J4-device_code G-T2 missing service connection assistant continues via device_code",
  "FAULT-popup-blocked":
    "FAULT-popup-blocked connection falls back to the same tab without duplicate handoff",
  "FAULT-handoff-response":
    "FAULT-handoff-response lost browser response keeps the same unsettled handoff command",
  "FAULT-nyxid":
    "FAULT-nyxid real upstream unavailability preserves handoff command identity",
});

const variants = {
  "FAULT-popup-blocked": "popup_blocked",
  "FAULT-handoff-response": "start_response_loss",
  "FAULT-nyxid": "upstream_outage",
};
const sha = /^[a-f0-9]{64}$/;
const revision = /^[a-f0-9]{40}$/;
const identifier = /^[a-zA-Z0-9][a-zA-Z0-9_.:-]{0,127}$/;

function object(value) {
  assert.ok(
    value !== null && typeof value === "object" && !Array.isArray(value),
    "Expected connection evidence object",
  );
  return value;
}
function exact(value, keys) {
  const row = object(value);
  assert.deepEqual(
    Object.keys(row).sort(),
    [...keys].sort(),
    "Unexpected connection evidence fields",
  );
  return row;
}
function digest(value) {
  assert.ok(
    typeof value === "string" && sha.test(value),
    "Expected connection evidence digest",
  );
}
export function identityDigest(value) {
  return createHash("sha256").update(JSON.stringify(value)).digest("hex");
}

export function assertProviderObservation(value, expected) {
  const row = exact(value, [
    "schema",
    "run",
    "caseId",
    "phase",
    "ownerDigest",
    "targetDigest",
    "serviceDigest",
    "intentDigest",
    "mode",
    "providerRevision",
    "fixtureDigest",
    "observedAt",
    "evidenceDigest",
    "links",
  ]);
  assert.equal(row.schema, "cma-j4-provider-observation-v1");
  for (const field of [
    "run",
    "caseId",
    "phase",
    "ownerDigest",
    "targetDigest",
    "serviceDigest",
    "intentDigest",
    "mode",
    "providerRevision",
    "fixtureDigest",
  ])
    assert.equal(
      row[field],
      expected[field],
      `Connection observation ${field} mismatch`,
    );
  assert.ok(identifier.test(row.run) && identifier.test(row.caseId));
  assert.ok(["before", "after"].includes(row.phase));
  assert.ok(["oauth", "api_key", "device_code"].includes(row.mode));
  assert.ok(revision.test(row.providerRevision));
  for (const field of [
    "ownerDigest",
    "targetDigest",
    "serviceDigest",
    "fixtureDigest",
    "evidenceDigest",
  ])
    digest(row[field]);
  if (row.phase === "before") assert.equal(row.intentDigest, null);
  else digest(row.intentDigest);
  assert.ok(
    typeof row.observedAt === "string" &&
      Number.isFinite(Date.parse(row.observedAt)),
    "Missing observation time",
  );
  assert.ok(
    Array.isArray(row.links) && row.links.length <= 1000,
    "Missing bounded provider link inventory",
  );
  const seen = new Set();
  for (const value of row.links) {
    const link = exact(value, ["linkDigest", "state"]);
    digest(link.linkDigest);
    assert.ok(
      ["pending", "completed", "cancelled", "expired"].includes(link.state),
    );
    assert.ok(!seen.has(link.linkDigest), "Duplicate provider link evidence");
    seen.add(link.linkDigest);
  }
  return row;
}

export function assertConnectionProof(value, expected) {
  const proof = exact(value, [
    "schema",
    "execution",
    "scope",
    "run",
    "caseId",
    "mode",
    "variant",
    "ownerDigest",
    "targetDigest",
    "serviceDigest",
    "intentDigest",
    "providerRevision",
    "fixtureDigest",
    "commands",
    "launchCreates",
    "launchStarts",
    "ownerVerified",
    "sameTab",
    "before",
    "after",
  ]);
  assert.equal(proof.schema, "cma-connection-journey-v1");
  assert.equal(proof.execution, "configured_remote_origin");
  assert.equal(proof.scope, "ordinary_service_connection");
  for (const field of ["run", "caseId", "providerRevision"])
    assert.equal(proof[field], expected[field]);
  assert.ok(
    Object.hasOwn(connectionCaseTitles, proof.caseId),
    "Unregistered connection case",
  );
  const mode = proof.caseId.startsWith("J4-") ? proof.caseId.slice(3) : "oauth";
  assert.equal(proof.mode, mode);
  assert.equal(proof.variant, variants[proof.caseId] ?? "valid");
  assert.equal(proof.sameTab, proof.variant === "popup_blocked");
  assert.equal(proof.ownerVerified, true);
  assert.equal(proof.launchCreates, 1);
  assert.equal(proof.launchStarts, 1);
  digest(proof.intentDigest);
  const before = assertProviderObservation(proof.before, {
    ...proof,
    phase: "before",
    intentDigest: null,
  });
  const after = assertProviderObservation(proof.after, {
    ...proof,
    phase: "after",
  });
  assert.ok(
    Date.parse(after.observedAt) >= Date.parse(before.observedAt),
    "Provider observations are reversed",
  );
  assert.notEqual(
    after.evidenceDigest,
    before.evidenceDigest,
    "Provider observations reused one receipt",
  );
  const original = new Set(before.links.map((row) => row.linkDigest));
  const current = new Set(after.links.map((row) => row.linkDigest));
  assert.ok(
    [...original].every((id) => current.has(id)),
    "Provider inventory lost baseline links",
  );
  const created = after.links.filter((row) => !original.has(row.linkDigest));
  assert.equal(
    created.length,
    1,
    "Exactly one new provider link must be observed",
  );
  assert.equal(
    created[0].state,
    "completed",
    "Readiness alone does not prove provider completion",
  );
  assert.ok(
    Array.isArray(proof.commands) &&
      proof.commands.length > 0 &&
      proof.commands.length <= 10000,
  );
  for (const command of proof.commands) {
    const keys = ["method", "path", "bodyHash"];
    if (Object.hasOwn(command, "key")) keys.push("key");
    exact(command, keys);
    assert.equal(command.method, "POST");
    assert.ok(
      typeof command.path === "string" &&
        /^\/(?:auth\/connections\/(?:start|check|cancel)|api\/v[12]\/triggers\/public\/[a-zA-Z0-9_.-]+\/launches|api\/v1\/launches\/[a-zA-Z0-9_.-]+\/start)$/.test(
          command.path,
        ),
      "Unexpected connection journey command",
    );
    digest(command.bodyHash);
    if (command.key !== undefined)
      assert.ok(
        typeof command.key === "string" && identifier.test(command.key),
      );
  }
  const starts = proof.commands.filter(
    (row) => row.path === "/auth/connections/start",
  );
  const retry = ["start_response_loss", "upstream_outage"].includes(
    proof.variant,
  );
  assert.equal(starts.length, retry ? 2 : 1);
  if (retry)
    assert.deepEqual(
      starts[1],
      starts[0],
      "Connection retry changed the original command",
    );
  const launches = proof.commands.filter((row) =>
    row.path.endsWith("/launches"),
  );
  const work = proof.commands.filter((row) =>
    /^\/api\/v1\/launches\/[^/]+\/start$/.test(row.path),
  );
  assert.equal(launches.length, 1);
  assert.equal(work.length, 1);
  assert.equal(
    identityDigest({ type: "launch", id: work[0].path.split("/")[4] }),
    proof.targetDigest,
  );
  assert.ok(
    proof.commands.some((row) => row.path === "/auth/connections/check"),
    "Missing fresh verification command",
  );
  assert.equal(
    proof.commands.filter((row) => row.path === "/auth/connections/cancel")
      .length,
    0,
  );
  return proof;
}

export function missingConnectionCoverage(proofs) {
  assert.ok(Array.isArray(proofs));
  const supplied = new Set();
  for (const proof of proofs) {
    assertConnectionProof(proof, {
      run: proof?.run,
      caseId: proof?.caseId,
      providerRevision: proof?.providerRevision,
    });
    supplied.add(`${proof.mode}/${proof.variant}`);
  }
  const required = ["oauth", "api_key", "device_code"].flatMap((mode) =>
    [
      "valid",
      "expired",
      "cancelled",
      "callback_loss",
      "callback_replay",
      "receipt_recovery",
      "owner_switch",
    ].map((variant) => `${mode}/${variant}`),
  );
  required.push(
    "oauth/popup_blocked",
    "oauth/start_response_loss",
    "oauth/upstream_outage",
    "service_authorization/explicit_resume",
    "provider_link/cleanup_authority",
  );
  return required.filter((key) => !supplied.has(key));
}
