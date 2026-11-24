import assert from "node:assert/strict";
import { createHash } from "node:crypto";

export const caseIds = Object.freeze([
  "boundary",
  "shared",
  "load-controls",
  "utc",
]);
export const countKeys = Object.freeze([
  "overquotaAttempts",
  "overquotaRejects",
  "idempotencyChecks",
  "idempotentChargeDuplicates",
  "sideEffectChecks",
  "anonymousSideEffects",
  "collateralAttempts",
  "collateralErrors",
  "offered",
  "admitted",
  "refused",
  "transportErrors",
]);
export const evidenceHash = (bytes) =>
  createHash("sha256").update(bytes).digest("hex");
export function object(value) {
  assert.ok(
    value && typeof value === "object" && !Array.isArray(value),
    "Expected evidence object",
  );
  return value;
}
export function natural(value) {
  assert.ok(
    Number.isSafeInteger(value) && value >= 0,
    "Expected nonnegative evidence count",
  );
  return value;
}
export function identifier(value) {
  assert.ok(
    typeof value === "string" &&
      /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/.test(value),
    "Invalid evidence identity",
  );
  return value;
}
export function instant(value) {
  assert.equal(typeof value, "string");
  const time = Date.parse(value);
  assert.ok(Number.isFinite(time), "Invalid evidence timestamp");
  return time;
}
export function validateScenarioBinding(bytes, expected) {
  const row = object(
    JSON.parse(
      typeof bytes === "string" ? bytes : Buffer.from(bytes).toString("utf8"),
    ),
  );
  assert.equal(row.schema, "cma-tc117-scenario-v1");
  assert.equal(row.campaign, "TC-117");
  assert.equal(row.run, identifier(expected.run));
  assert.ok(caseIds.includes(expected.case));
  assert.equal(row.case, expected.case);
  identifier(row.journalRun);
  assert.equal(row.status, "passed");
  assert.ok(instant(row.startedAt) <= instant(row.endedAt));
  assert.ok(natural(row.N) > 0, "Scenario has no observations");
  assert.ok(Array.isArray(row.requests));
  assert.equal(row.N, row.requests.length);
  const counts = object(row.counts);
  assert.deepEqual(Object.keys(counts).sort(), [...countKeys].sort());
  for (const key of countKeys) natural(counts[key]);
  assert.ok(counts.overquotaRejects <= counts.overquotaAttempts);
  assert.ok(counts.collateralErrors <= counts.collateralAttempts);
  assert.equal(counts.offered, counts.admitted + counts.refused);
  for (const key of [
    "idempotentChargeDuplicates",
    "anonymousSideEffects",
    "transportErrors",
  ])
    assert.equal(counts[key], 0);
  return row;
}
