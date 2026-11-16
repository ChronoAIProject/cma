import { test } from "node:test";
import assert from "node:assert/strict";
import { fixture } from "./fixtures.mjs";
import { coverage } from "./coverage.mjs";

test("denominator includes retained home missing from catalog; lists cloud orphans", () => {
  const report = coverage(fixture());
  assert.equal(report.retained_homes, 2);
  assert.equal(report.backed_homes, 1);
  assert.equal(report.coverage, 0.5);
  assert.equal(report.status, "FAIL");
  assert.deepEqual(report.homes[1].reasons, ["catalog_missing"]);
  assert.equal(report.orphan_count, 1);
  assert.equal(report.mode, "fixture");
  assert.equal(JSON.stringify(report).includes("owner-a"), false);
});

test("all retained homes covered passes inventory only", () => {
  const input = fixture();
  input.sandboxes.splice(1, 1);
  const report = coverage(input);
  assert.equal(report.status, "PASS");
  assert.equal(report.restore_status, "NOT_RUN");
  assert.equal(report.qualification_status, "NOT_RUN");
  assert.equal(report.failure_domain_status, "NOT_RUN");
  assert.equal(report.cmek_recovery_status, "NOT_RUN");
  assert.equal(report.homes[0].capture_gap_seconds, 1800);
});

for (const [name, mutate, reason] of [
  ["cloud missing", (x) => x.cloud.snapshots.splice(1, 1), "cloud_missing"],
  ["not ready", (x) => x.cloud.snapshots[1].status = "FAILED", "cloud_not_ready"],
  ["source changed", (x) => x.home_backups[1].source.pvc_uid = "other", "source_changed"],
  ["expired", (x) => x.home_backups[1].retention.expires_at = x.now, "expired"],
  ["key missing", (x) => x.dependencies.available_key_ids.pop(), "key_unavailable"],
  ["PITR stale", (x) => x.dependencies.atlas.latest_at = "2026-11-16T00:54:59Z", "mongo_rpo"],
  ["object unknown", (x) => x.dependencies.objects.status = "unknown", "objects_unavailable"],
  ["capture gap", (x) => x.home_backups[0].cut.requested_at = "2026-11-16T00:09:59Z", "capture_gap"],
  ["completion unknown", (x) => delete x.cloud.snapshots[1].ready_observed_at, "completion_unknown"],
  ["location unknown", (x) => x.cloud.snapshots[1].storage_locations = [], "location_unknown"],
  ["CMEK mismatch", (x) => x.cloud.snapshots[1].cmek_key_version_id = "other", "cmek_mismatch"],
  ["cloud cut mismatch", (x) => x.cloud.snapshots[1].created_at = "2026-11-16T00:41:00Z", "cloud_cut_mismatch"],
  ["object cut stale", (x) => x.dependencies.objects.consistent_at = "2026-11-16T00:39:59Z", "objects_unavailable"],
  ["object behind Mongo", (x) => x.dependencies.objects.consistent_at = "2026-11-16T00:59:59Z", "objects_unavailable"],
  ["Mongo cut unavailable", (x) => x.dependencies.atlas.earliest_at = "2026-11-16T00:41:00Z", "mongo_cut_unavailable"],
  ["catalog completion late", (x) => x.home_backups[1].cut.requested_at = "2026-11-16T00:09:00Z", "catalog_completion_late"],
  ["completion not proved", (x) => { x.cloud.snapshots[1].ready_observed_at = x.now;
    x.home_backups[1].cut.requested_at = "2026-11-16T00:29:59Z"; }, "completion_not_proven_within_30m"],
]) test(name, () => {
  const input = fixture(); mutate(input);
  assert.ok(coverage(input).homes[0].reasons.includes(reason));
});

test("older daily retention does not imply capture cadence failure", () => {
  const input = fixture();
  const old = structuredClone(input.home_backups[0]);
  old._id = "hb-a-0";
  old.cut = { requested_at: "2026-11-15T00:00:00Z", provider_creation_time: "2026-11-15T00:00:00Z", ready_at: "2026-11-15T00:01:00Z" };
  input.home_backups.push(old);
  assert.equal(coverage(input).homes[0].capture_gap_seconds, 1800);
});

test("malformed, duplicate and future observations refuse without raw input in errors", () => {
  for (const mutate of [
    (x) => x.sandboxes.push(x.sandboxes[0]),
    (x) => x.home_backups.push(x.home_backups[0]),
    (x) => x.cloud.snapshots.push(x.cloud.snapshots[0]),
    (x) => delete x.sandboxes[0].record,
    (x) => x.now = "bad",
    (x) => x.cloud.observed_at = "2026-11-17T00:00:00Z",
    (x) => x.mode = "cloud",
  ]) { const input = fixture(); mutate(input); assert.throws(() => coverage(input)); }
});

test("empty inventory never proves 100 percent coverage", () => {
  const input = fixture(); input.sandboxes = [];
  assert.equal(coverage(input).status, "NOT_RUN");
  assert.equal(coverage(input).coverage, null);
});

test("project-wide inventory separates attributed orphans, seed/unrelated and unknown", () => {
  const input = fixture();
  const sample = input.cloud.snapshots.at(-1);
  for (const [name, ownership] of [
    ["seed", { kind: "seed", receipt: "fixture-seed-receipt" }],
    ["unrelated", { kind: "unrelated", receipt: "fixture-other-receipt" }],
    ["unknown", { kind: "unknown" }],
    ["unproved", { kind: "cma-home-backup" }],
    ["unproved-seed", { kind: "seed" }],
  ]) input.cloud.snapshots.push({ ...sample, handle: `projects/fixture/global/snapshots/${name}`, ownership });
  const report = coverage(input);
  assert.equal(report.orphan_count, 1);
  assert.equal(report.excluded_snapshot_count, 2);
  assert.equal(report.ownership_unknown_count, 3);
  // No attribution is inferred from a name, location, key reference or status.
  delete sample.ownership;
  assert.equal(coverage(input).orphan_count, 0);
  assert.equal(coverage(input).ownership_unknown_count, 4);
});
