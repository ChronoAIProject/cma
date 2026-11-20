import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const workflow = readFileSync(
  new URL("../workflows/ci-web.yml", import.meta.url),
  "utf8",
);
const suites = [
  {
    name: "Local resource lifecycle contracts",
    id: "resource_lifecycle",
    config: "resource-lifecycle",
    evidence: "CMA_LIFECYCLE_EVIDENCE_DIR",
    directory: "cma-resource-lifecycle",
    files: [
      "resource-lifecycle-creation",
      "resource-lifecycle-progress",
      "resource-lifecycle-shell",
      "resource-lifecycle-overview",
      "resource-lifecycle-history",
      "resource-lifecycle-deletion",
      "agent-stop-deletion",
    ],
  },
  {
    name: "Local Sandbox status filter contracts",
    id: "sandbox_status_filter",
    config: "sandbox-status-filter",
    evidence: "CMA_SANDBOX_FILTER_EVIDENCE_DIR",
    directory: "cma-sandbox-filter",
    files: ["sandbox-status-filter"],
  },
];

function invocation(suite) {
  return `node node_modules/playwright/cli.js test --config e2e/${suite.config}.config.ts`;
}

function verify(job, config, suite) {
  const step = job.split(`name: "${suite.name}"`)[1]?.split("\n      -")[0];
  assert.ok(step, `Web must execute ${suite.config}`);
  assert.match(step, /timeout-minutes: 5/);
  assert.ok(step.includes(`id: ${suite.id}\n`));
  assert.ok(
    step.includes(
      `${suite.evidence}: "\${{ runner.temp }}/${suite.directory}"`,
    ),
  );
  const command = invocation(suite);
  const run = step
    .split("run: |\n")[1]
    ?.trim()
    .split("\n")
    .map((line) => line.trim());
  assert.deepEqual(run, [`${command} --list`, command]);
  assert.doesNotMatch(step, /continue-on-error|\bif:|CMA_E2E_BROWSER_PATH/);
  for (const prerequisite of [
    "Production build",
    "Install pinned local browser",
  ]) {
    const position = job.indexOf(`name: "${prerequisite}"`);
    assert.ok(position >= 0 && position < job.indexOf(`name: "${suite.name}"`));
  }
  assert.equal(job.match(/name: "Production build"/g)?.length, 1);
  assert.ok(job.includes("../.github/scripts/lifecycle-browser-ci.test.mjs"));
  const upload = job
    .split(`if: "failure() && steps.${suite.id}.outcome == 'failure'"`)[1]
    ?.split("\n      -")[0];
  assert.ok(upload?.includes('uses: "actions/upload-artifact@'));
  assert.ok(upload?.includes(`path: "\${{ runner.temp }}/${suite.directory}"`));
  assert.match(upload, /retention-days: 1/);
  const selection = config.match(/testMatch:\s*(\[[\s\S]*?\]|'[^']*')/)?.[1];
  assert.deepEqual(
    [...selection.matchAll(/'([^']+)'/g)].map((match) => match[1]),
    suite.files.map((file) => `${file}.check.ts`),
  );
  for (const contract of [
    /projects: browserProjects\(\)/,
    /workers: 1/,
    /retries: 0/,
    /forbidOnly: true/,
    /reporter: 'list'/,
    /webServer: \{/,
    /reuseExistingServer: false/,
    /--strictPort/,
    /serviceWorkers: 'block'/,
    /trace: 'retain-on-failure'/,
    /screenshot: 'only-on-failure'/,
    /preview.protocol !== 'http:'/,
    /\['127\.0\.0\.1', 'localhost'\]\.includes\(preview.hostname\)/,
    /!preview.port/,
    /preview.username/,
    /preview.password/,
    /preview.pathname !== '\/'/,
    /preview.search/,
    /preview.hash/,
  ])
    assert.match(config, contract);
  assert.ok(config.includes(`process.env.${suite.evidence}`));
}

for (const suite of suites) {
  const config = readFileSync(
    new URL(`../../web/e2e/${suite.config}.config.ts`, import.meta.url),
    "utf8",
  );
  test(`Web runs every ${suite.config} case using the existing build and an owned preview`, () =>
    verify(workflow, config, suite));
  test(`${suite.config} cannot be omitted, filtered, masked or retried`, () => {
    const command = invocation(suite);
    for (const replacement of [
      "",
      `${command} --grep subset`,
      `${command} || true`,
      `${command} --retries=1`,
      `${command} --pass-with-no-tests`,
      `${command} --list`,
    ])
      assert.throws(() =>
        verify(workflow.replaceAll(command, replacement), config, suite),
      );
    assert.throws(() =>
      verify(
        workflow.replace(
          `id: ${suite.id}`,
          `continue-on-error: true\n        id: ${suite.id}`,
        ),
        config,
        suite,
      ),
    );
    for (const [from, to] of [
      [`'${suite.files[0]}.check.ts'`, "'omitted.check.ts'"],
      ["workers: 1", "workers: 4"],
      ["retries: 0", "retries: 1"],
      ["reuseExistingServer: false", "reuseExistingServer: true"],
      ["trace: 'retain-on-failure'", "trace: 'off'"],
    ])
      assert.throws(() => verify(workflow, config.replace(from, to), suite));
  });
}
