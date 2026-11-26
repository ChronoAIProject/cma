import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const workflow = read("../workflows/ci-web.yml");
const config = read("../../web/e2e/connection-journey-ui.config.ts");
const startup = read("../../web/e2e/agent-startup.config.ts");
const invocation =
  "node node_modules/playwright/cli.js test --config e2e/connection-journey-ui.config.ts";
const suites = [
  "connection-journey-ui.check.ts",
  "connection-journey-ui-modes.check.ts",
  "connection-popup-blocked.check.ts",
  "connection-cancel-reentry.check.ts",
  "connection-terminal-reentry.check.ts",
  "connection-pending-reentry.check.ts",
];

function verify(job, suite, inherited) {
  const step = job
    .split('name: "Local compiled connection journeys"')[1]
    ?.split("\n      -")[0];
  assert.ok(
    step?.includes(invocation),
    "Web must execute the compiled connection suite",
  );
  assert.match(step, /timeout-minutes: 5/);
  assert.doesNotMatch(
    step,
    /--grep|--project|--shard|--retries|--list|--pass-with-no-tests|continue-on-error|\bif:|CMA_E2E_BROWSER_PATH|\|\|/,
  );
  const guards = job
    .split('name: "Local browser guard mechanics"')[1]
    ?.split("\n      -")[0];
  assert.match(
    guards ?? "",
    /node --test [^\n]*\.\.\/\.github\/scripts\/connection-journey-ui-ci\.test\.mjs/,
  );
  for (const prerequisite of [
    "Install pinned local browser",
    "Production build",
  ]) {
    const position = job.indexOf(prerequisite);
    assert.ok(position >= 0 && position < job.indexOf(invocation));
  }
  const matches = suite.match(/testMatch:\s*\[([^\]]*)\]/)?.[1];
  for (const file of suites)
    assert.ok(
      matches?.includes(`'${file}'`),
      `Compiled selection must retain ${file}`,
    );
  assert.match(suite, /defineConfig\(startup,\s*\{/);
  assert.doesNotMatch(
    suite,
    /^  (?:workers|retries|timeout|grep|testIgnore|projects):/m,
  );
  for (const contract of [
    /reuseExistingServer: false/,
    /--host 127\.0\.0\.1/,
    /--strictPort/,
    /VITE_CONNECTION_INTENT_RECOVERY_ENABLED: 'true'/,
  ])
    assert.match(suite, contract);
  for (const contract of [
    /workers: 1/,
    /retries: 0/,
    /timeout: 30_000/,
    /forbidOnly: true/,
  ])
    assert.match(inherited, contract);
}

test("Web runs all compiled connection cases with an owned local preview", () =>
  verify(workflow, config, startup));

test("omitting or bypassing the compiled connection execution fails the contract", () => {
  assert.throws(() =>
    verify(workflow.replace(invocation, ""), config, startup),
  );
  for (const suffix of [
    " --grep subset",
    " --project chromium",
    " --shard 1/2",
    " --retries 1",
    " --list",
    " --pass-with-no-tests",
    " || true",
  ])
    assert.throws(() =>
      verify(
        workflow.replace(invocation, invocation + suffix),
        config,
        startup,
      ),
    );
  for (const setting of [
    "continue-on-error: true",
    "if: false",
    "timeout-minutes: 10",
  ])
    assert.throws(() =>
      verify(
        workflow.replace(
          'name: "Local compiled connection journeys"\n        timeout-minutes: 5',
          `name: "Local compiled connection journeys"\n        ${setting}`,
        ),
        config,
        startup,
      ),
    );
});

test("omitted guards, cases and inherited execution bounds fail the contract", () => {
  assert.throws(() =>
    verify(
      workflow.replace(
        " ../.github/scripts/connection-journey-ui-ci.test.mjs",
        "",
      ),
      config,
      startup,
    ),
  );
  for (const file of suites)
    assert.throws(() =>
      verify(
        workflow,
        config.replace(`'${file}'`, "'other.check.ts'"),
        startup,
      ),
    );
  for (const [from, to] of [
    ["workers: 1", "workers: 2"],
    ["retries: 0", "retries: 1"],
    ["timeout: 30_000", "timeout: 60_000"],
    ["forbidOnly: true", "forbidOnly: false"],
  ])
    assert.throws(() => verify(workflow, config, startup.replace(from, to)));
  for (const [from, to] of [
    ["reuseExistingServer: false", "reuseExistingServer: true"],
    ["--host 127.0.0.1", "--host 0.0.0.0"],
    ["--strictPort", ""],
    ["defineConfig(startup, {", "defineConfig({"],
    ["testMatch:", "retries: 1, testMatch:"],
  ])
    assert.throws(() => verify(workflow, config.replace(from, to), startup));
});
