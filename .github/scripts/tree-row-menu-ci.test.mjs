import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { jobRunning } from "./ci-web-jobs.mjs";

const workflow = readFileSync(
  new URL("../workflows/ci-web.yml", import.meta.url),
  "utf8",
);
const config = readFileSync(
  new URL("../../web/e2e/tree-row-menu.config.ts", import.meta.url),
  "utf8",
);
const invocation =
  "node node_modules/playwright/cli.js test --config e2e/tree-row-menu.config.ts";

function verify(workflow, suite) {
  const job = jobRunning(
    workflow,
    "Local row menu pointer and keyboard contracts",
  );
  const step = job
    .split('name: "Local row menu pointer and keyboard contracts"')[1]
    ?.split("\n      -")[0];
  assert.ok(
    step?.includes(invocation),
    "Web must execute the row menu browser contracts",
  );
  assert.doesNotMatch(step, /--grep|--project|--shard|continue-on-error|\|\|/);
  const browser = job.indexOf("Install pinned local browser");
  assert.ok(browser >= 0 && browser < job.indexOf(invocation));
  assert.match(job, /failure\(\) && steps\.row_menu\.outcome == 'failure'/);
  for (const contract of [
    /testMatch: 'tree-row-menu\.check\.ts'/,
    /projects: browserProjects\(\)/,
    /workers: 1/,
    /retries: 0/,
    /forbidOnly: true/,
    /reuseExistingServer: false/,
    /--strictPort/,
    /trace: 'retain-on-failure'/,
    /screenshot: 'only-on-failure'/,
  ])
    assert.match(suite, contract);
}

test("CI runs the complete row menu suite with owned preview and failure evidence", () =>
  verify(workflow, config));
test("omitted execution, filtered tests, retries and reused previews fail the contract", () => {
  assert.throws(() => verify(workflow.replace(invocation, ""), config));
  assert.throws(() =>
    verify(workflow.replace(invocation, `${invocation} --grep touch`), config),
  );
  for (const [from, to] of [
    ["retries: 0", "retries: 1"],
    ["reuseExistingServer: false", "reuseExistingServer: true"],
  ])
    assert.throws(() => verify(workflow, config.replace(from, to)));
});
