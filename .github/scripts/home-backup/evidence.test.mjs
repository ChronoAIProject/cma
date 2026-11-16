import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, writeFileSync, mkdirSync, symlinkSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { scan, validateEvidence } from "./evidence.mjs";
import { inventory } from "./inventory.mjs";

test("scanner includes binary and nested logs; never returns sentinel or path text", () => {
  const root = mkdtempSync(join(tmpdir(), "backup-scan-"));
  const sentinel = "runtime-generated-sensitive-marker";
  try {
    mkdirSync(join(root, "nested"));
    writeFileSync(join(root, "clean.log"), "counts only");
    assert.deepEqual(scan(root, [sentinel]), { files: 1, matches: 0 });
    writeFileSync(join(root, "nested", "raw.bin"), Buffer.from(`\0${sentinel}\0`));
    const result = scan(root, [sentinel]);
    assert.deepEqual(result, { files: 2, matches: 1 });
    assert.equal(JSON.stringify(result).includes(sentinel), false);
    writeFileSync(join(root, "key.bin"), Buffer.from([0, 255, 16, 128]));
    assert.equal(scan(root, [Buffer.from([255, 16, 128])]).matches, 1);
    symlinkSync(join(root, "clean.log"), join(root, "link"));
    assert.throws(() => scan(root, [sentinel]));
    assert.throws(() => scan(root, []));
  } finally { rmSync(root, { recursive: true }); }
});

const evidence = () => ({ source_sha: "a".repeat(40),
  image_digests: { api: `sha256:${"b".repeat(64)}` }, environment: "isolated-local",
  window: { start: "2026-11-16T00:00:00Z", end: "2026-11-16T01:00:00Z" },
  sample_count: 8, configuration: { mode: "synthetic" }, switches: { restore: false },
  raw_results: ["private-local/run-1"], mode: "synthetic", attempt: 1,
});
test("evidence requires complete provenance; no cancelled or retry qualification", () => {
  assert.equal(validateEvidence(evidence()), true);
  for (const field of Object.keys(evidence())) {
    const input = evidence(); delete input[field];
    assert.throws(() => validateEvidence(input));
  }
  for (const patch of [{ sample_count: 0 }, { attempt: 2 }, { cancelled: true },
    { image_digests: { api: "tag" } }, { configuration: {} }, { switches: {} }])
    assert.throws(() => validateEvidence({ ...evidence(), ...patch }));
});

test("cloud adapter executes only read-only project scoped list; refuses malformed results", () => {
  const calls = [];
  const run = (command, args, options) => {
    calls.push({ command, args, options });
    return { status: 0, stdout: JSON.stringify([{ selfLink:
      "https://www.googleapis.com/compute/v1/projects/fixture/global/snapshots/one",
      status: "READY", creationTimestamp: "2026-11-16T00:00:00Z",
      storageLocations: ["fixture-region"] }]) };
  };
  const result = inventory("fixture", run);
  assert.equal(result.snapshots[0].handle, "projects/fixture/global/snapshots/one");
  assert.equal(result.snapshots[0].ready_observed_at, result.observed_at);
  assert.deepEqual(calls[0].args, ["compute", "snapshots", "list", "--project", "fixture", "--format=json", "--quiet"]);
  assert.throws(() => inventory("--bad", run));
  assert.throws(() => inventory("fixture", () => ({ status: 1, stderr: "sensitive" })), /cloud_list_failed/);
  assert.throws(() => inventory("fixture", () => ({ status: 0, stdout: "{}" })));
});
