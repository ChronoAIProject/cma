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
            scenario === "leak" ? env.HOME_BACKUP_EVIDENCE_SENTINEL : "test result: ok. 1 passed;",
          stderr: "bounded synthetic diagnostics" };
      });
      assert.equal(report.key_tests_status, scenario === "success" ? "PASS" : "FAIL");
      assert.equal(report.qualification_status, "NOT_RUN");
      assert.equal(report.home_restore_status, "NOT_RUN");
      assert.equal(calls, 4);
      assert.equal(report.attempts.length, 4);
      assert.equal(report.sentinel_scan.matches, scenario === "leak" ? 4 : 0);
      assert.equal(readFileSync(join(output, "result.json"), "utf8").includes("HOME_BACKUP_EVIDENCE_KEYS"), false);
      await assert.rejects(() => runHarness(output));
    } finally { rmSync(parent, { recursive: true }); }
  });
