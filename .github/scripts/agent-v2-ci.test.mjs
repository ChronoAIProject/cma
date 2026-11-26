import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(
  new URL("../workflows/ci-web.yml", import.meta.url),
  "utf8",
);
const config = readFileSync(
  new URL("../../web/e2e/agent-v2.config.ts", import.meta.url),
  "utf8",
);
const invocation =
  "node node_modules/playwright/cli.js test --config e2e/agent-v2.config.ts";
function verify(job, suite) {
  const step = job
    .split('name: "Local Agent v2 contracts"')[1]
    ?.split("\n      -")[0];
  assert.ok(
    step?.includes(invocation),
    "Web must execute the LOCAL Agent v2 config",
  );
  assert.doesNotMatch(step, /--grep|--project|--shard|continue-on-error|\|\|/);
  assert.match(
    job,
    /node --test \.\.\/\.github\/scripts\/agent-v2-ci\.test\.mjs/,
  );
  const matches = suite.match(/testMatch:\s*\[([^\]]*)\]/)?.[1];
  for (const file of [
    "agent-v2.check.ts",
    "agent-draft-identity-refresh.check.ts",
    "agent-v2-queued.check.ts",
    "agent-v2-queued-edit.check.ts",
    "agent-v2-composer.check.ts",
    "agent-terminal-recovery.check.ts",
  ])
    assert.ok(
      matches?.includes(`'${file}'`),
      `LOCAL selection must retain ${file}`,
    );
  for (const contract of [
    /workers: 1/,
    /retries: 0/,
    /reuseExistingServer: false/,
    /--host 127\.0\.0\.1/,
    /--strictPort/,
    /trace: 'retain-on-failure'/,
    /screenshot: 'only-on-failure'/,
  ])
    assert.match(suite, contract);
}
test("Web runs the complete LOCAL Agent v2 suite with an owned preview and failure evidence", () =>
  verify(workflow, config));
test("omitting or filtering the browser execution fails the contract", () => {
  assert.throws(() => verify(workflow.replace(invocation, ""), config));
  assert.throws(() =>
    verify(workflow.replace(invocation, `${invocation} --grep subset`), config),
  );
});
test("omitting a test family, retrying or reusing another preview fails the contract", () => {
  for (const [from, to] of [
    ["'agent-v2-queued.check.ts'", "'other.check.ts'"],
    ["retries: 0", "retries: 1"],
    ["reuseExistingServer: false", "reuseExistingServer: true"],
  ])
    assert.throws(() => verify(workflow, config.replace(from, to)));
});
