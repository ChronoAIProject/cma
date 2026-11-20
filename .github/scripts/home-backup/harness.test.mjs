import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, readFileSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
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
      assert.equal(report.planned_sample_count, 8);
      assert.equal(report.sample_count, ["success", "leak"].includes(scenario) ? 8 : 0);
      assert.equal(calls, 4);
      assert.equal(report.attempts.length, 4);
      assert.equal(report.sentinel_scan.matches, scenario === "leak" ? 4 : 0);
      assert.equal(readFileSync(join(output, "result.json"), "utf8").includes("HOME_BACKUP_EVIDENCE_KEYS"), false);
      await assert.rejects(() => runHarness(output));
    } finally { rmSync(parent, { recursive: true }); }
  });

test("partial auth read failure counts only the two verified sealed records", async () => {
  const parent = mkdtempSync(join(tmpdir(), "backup-harness-partial-"));
  const output = join(parent, "run");
  try {
    const report = await runHarness(output, async (_args, env) => {
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
