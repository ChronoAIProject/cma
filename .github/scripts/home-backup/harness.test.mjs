import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { runHarness } from "./harness.mjs";

for (const scenario of ["success", "failed", "zero-tests", "leak", "cancelled"])
  test(`harness ${scenario} preserves raw attempts and fails closed`, async () => {
    const parent = mkdtempSync(join(tmpdir(), "backup-harness-"));
    const output = join(parent, "run");
    let calls = 0;
    try {
      const report = await runHarness(output, async (_args, env) => {
        calls++;
        return { code: scenario === "failed" ? 1 : 0,
          signal: scenario === "cancelled" ? "SIGTERM" : null,
          stdout: scenario === "zero-tests" ? "test result: ok. 0 passed;" :
            scenario === "leak" ? `test result: ok. 1 passed;\n${env.HOME_BACKUP_EVIDENCE_SENTINEL}` : "test result: ok. 1 passed;",
          stderr: "bounded synthetic diagnostics" };
      });
      assert.equal(report.key_tests_status, scenario === "success" ? "PASS" : "FAIL");
      assert.equal(report.qualification_status, "NOT_RUN");
      assert.equal(report.home_restore_status, "NOT_RUN");
      assert.equal(report.full_dr_status, "NOT_RUN");
      assert.match(report.source_sha, /^[a-f0-9]{40}$/);
      assert.equal(typeof report.dirty_tree, "boolean");
      assert.equal(report.image_digests, null);
      assert.equal(report.switches.cloud_inventory, false);
      assert.equal(statSync(output).mode & 0o777, 0o700);
      assert.equal(report.planned_sample_count, 8);
      assert.equal(report.sample_count, ["success", "leak"].includes(scenario) ? 8 : 0);
      assert.equal(calls, 4);
      assert.equal(report.attempts.length, 4);
      assert.equal(report.sentinel_scan.matches, scenario === "leak" ? 4 : 0);
      assert.equal(readFileSync(join(output, "result.json"), "utf8").includes("HOME_BACKUP_EVIDENCE_KEYS"), false);
      const retained = readFileSync(join(output, "result.json"));
      await assert.rejects(() => runHarness(output));
      assert.deepEqual(readFileSync(join(output, "result.json")), retained);
    } finally { rmSync(parent, { recursive: true }); }
  });

test("partial auth read failure counts only the two verified sealed records", async () => {
  const parent = mkdtempSync(join(tmpdir(), "backup-harness-partial-"));
  const output = join(parent, "run");
  try {
    const report = await runHarness(output, async (_args, env) => {
      assert.equal(env.CARGO_INCREMENTAL, "0");
      const failed = env.HOME_BACKUP_EVIDENCE_PHASE === "read" &&
        _args.includes("cma");
      return { code: failed ? 101 : 0, signal: null,
        stdout: failed ? "test result: FAILED. 0 passed; 1 failed;" : "test result: ok. 1 passed;",
        stderr: failed ? "synthetic auth artifact decode failure" : "" };
    });
    assert.equal(report.key_tests_status, "FAIL");
    assert.equal(report.sample_count, 2);
    assert.equal(report.planned_sample_count, 8);
    assert.deepEqual(report.attempts.map((attempt) => attempt.status), ["PASS", "PASS", "FAIL", "PASS"]);
    assert.equal(report.attempts[2].exit_code, 101);
    assert.equal(report.attempts[2].raw_result, "read-auth.log");
    assert.equal(report.sentinel_scan.matches, 0);
    const persisted = JSON.parse(readFileSync(join(output, "result.json"), "utf8"));
    assert.equal(persisted.sample_count, 2);
    assert.equal(persisted.key_tests_status, "FAIL");
  } finally { rmSync(parent, { recursive: true }); }
});

test("imports and missing CLI arguments never start cloud inventory or Cargo", () => {
  const parent = mkdtempSync(join(tmpdir(), "backup-harness-opt-in-"));
  const harness = fileURLToPath(new URL("harness.mjs", import.meta.url));
  const inventory = fileURLToPath(new URL("inventory.mjs", import.meta.url));
  const env = { ...process.env, PATH: parent };
  try {
    const imported = spawnSync(process.execPath, ["--input-type=module", "-e",
      `await import(${JSON.stringify(new URL("harness.mjs", import.meta.url).href)}); await import(${JSON.stringify(new URL("inventory.mjs", import.meta.url).href)});`],
      { cwd: parent, env, encoding: "utf8" });
    assert.equal(imported.status, 0);
    assert.equal(imported.stdout, "");
    assert.equal(imported.stderr, "");
    for (const entry of [harness, inventory]) {
      const result = spawnSync(process.execPath, [entry], { cwd: parent, env, encoding: "utf8" });
      assert.equal(result.status, 1);
      assert.equal(result.stdout, "");
      assert.match(result.stderr, /^(harness|inventory)_failed\n$/);
    }
  } finally { rmSync(parent, { recursive: true }); }
});

test("missing source provenance cannot produce a successful key report", () => {
  const parent = mkdtempSync(join(tmpdir(), "backup-harness-provenance-"));
  const output = join(parent, "run");
  try {
    const result = spawnSync(process.execPath,
      [fileURLToPath(new URL("harness.mjs", import.meta.url)), "--keys", output],
      { env: { ...process.env, PATH: parent }, encoding: "utf8" });
    assert.equal(result.status, 1);
    assert.equal(result.stdout, "");
    assert.equal(result.stderr, "harness_failed\n");
    assert.equal(existsSync(join(output, "result.json")), false);
    assert.equal(existsSync(join(output, "write-auth.log")), false);
  } finally { rmSync(parent, { recursive: true }); }
});

test("interrupted attempts retain provenance and never overwrite earlier evidence", async () => {
  const parent = mkdtempSync(join(tmpdir(), "backup-harness-interrupted-"));
  const output = join(parent, "run");
  let calls = 0;
  try {
    await assert.rejects(() => runHarness(output, async () => {
      if (++calls === 2) throw new Error("synthetic interruption");
      return { code: 0, signal: null, stdout: "test result: ok. 1 passed;", stderr: "" };
    }), /synthetic interruption/);
    const start = readFileSync(join(output, "started.json"));
    const log = readFileSync(join(output, "write-auth.log"));
    assert.match(JSON.parse(start).source_sha, /^[a-f0-9]{40}$/);
    assert.equal(JSON.parse(start).qualification_status, "NOT_RUN");
    assert.equal(existsSync(join(output, "result.json")), false);
    await assert.rejects(() => runHarness(output));
    assert.deepEqual(readFileSync(join(output, "started.json")), start);
    assert.deepEqual(readFileSync(join(output, "write-auth.log")), log);
  } finally { rmSync(parent, { recursive: true }); }
});
