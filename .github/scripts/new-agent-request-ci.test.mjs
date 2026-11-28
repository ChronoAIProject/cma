import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { jobRunning } from "./ci-web-jobs.mjs";

const workflow = readFileSync(
  new URL("../workflows/ci-web.yml", import.meta.url),
  "utf8",
);
const config = readFileSync(
  new URL("../../web/e2e/new-agent-request.config.ts", import.meta.url),
  "utf8",
);
const invocation =
  "node node_modules/playwright/cli.js test --config e2e/new-agent-request.config.ts";

function verify(workflow, suite) {
  const job = jobRunning(workflow, "Local New agent request isolation");
  const step = job
    .split('name: "Local New agent request isolation"')[1]
    ?.split("\n      -")[0];
  assert.ok(
    step?.includes(invocation),
    "Web must execute the New agent request browser contracts",
  );
  assert.doesNotMatch(step, /--grep|--project|--shard|continue-on-error|\|\|/);
  const browser = job.indexOf("Install pinned local browser");
  assert.ok(browser >= 0 && browser < job.indexOf(invocation));
  assert.match(
    job,
    /failure\(\) && steps\.new_agent_request\.outcome == 'failure'/,
  );
  for (const contract of [
    /testMatch:\s*\[\s*'new-agent-request\.check\.ts',\s*'task-draft-ownership\.check\.ts',\s*'task-templates\.check\.ts',\s*'public-journeys\.check\.ts',\s*'home-simplification\.check\.ts',\s*'preparation-simplification\.check\.ts',?\s*\]/,
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

test("CI runs the complete New agent request suite with owned preview and failure evidence", () =>
  verify(workflow, config));
test("omitted execution, filtered tests, retries and reused previews fail the contract", () => {
  assert.throws(() => verify(workflow.replace(invocation, ""), config));
  assert.throws(() =>
    verify(workflow.replace(invocation, `${invocation} --grep active`), config),
  );
  for (const [from, to] of [
    ["'task-templates.check.ts'", "'other.check.ts'"],
    ["'public-journeys.check.ts'", "'other.check.ts'"],
    ["'home-simplification.check.ts'", "'other.check.ts'"],
    ["'preparation-simplification.check.ts'", "'other.check.ts'"],
    ["retries: 0", "retries: 1"],
    ["reuseExistingServer: false", "reuseExistingServer: true"],
  ])
    assert.throws(() => verify(workflow, config.replace(from, to)));
});

for (const file of [
  "new-agent-request.check.ts",
  "task-draft-ownership.check.ts",
  "task-templates.check.ts",
  "public-journeys.check.ts",
  "home-simplification.check.ts",
  "preparation-simplification.check.ts",
]) {
  test(`omitting or replacing ${file} fails the complete-suite contract`, () => {
    assert.throws(() => verify(workflow, config.replace(`'${file}',`, "")));
    assert.throws(() =>
      verify(workflow, config.replace(`'${file}'`, "'other.check.ts'")),
    );
  });
}
