import assert from "node:assert/strict";
import {
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { delimiter, dirname, join } from "node:path";
import { spawnSync } from "node:child_process";
import test from "node:test";
import { jobRunning } from "./ci-web-jobs.mjs";

const workflow = readFileSync(
  new URL("../workflows/ci-web.yml", import.meta.url),
  "utf8",
);
const config = readFileSync(
  new URL("../../web/e2e/channel-setup.config.ts", import.meta.url),
  "utf8",
);
const invocation =
  "node node_modules/playwright/cli.js test --config e2e/channel-setup.config.ts";

function verify(workflow, suite) {
  const job = jobRunning(workflow, "Local channel setup validation");
  const step = job
    .split('name: "Local channel setup validation"')[1]
    ?.split("\n      -")[0];
  assert.ok(
    step?.includes(invocation),
    "Web must execute the channel setup browser contracts",
  );
  assert.doesNotMatch(
    step,
    /--grep|--project|--shard|--retries|--list|--pass-with-no-tests|continue-on-error|\bif:|CMA_E2E_BROWSER_PATH|\|\|/,
  );
  assert.match(step, /timeout-minutes: 5/);
  for (const prerequisite of [
    "Install pinned local browser",
    "Production build",
  ]) {
    const position = job.indexOf(prerequisite);
    assert.ok(position >= 0 && position < job.indexOf(invocation));
  }
  assert.match(
    job,
    /failure\(\) && steps\.channel_setup\.outcome == 'failure'/,
  );
  for (const contract of [
    /testMatch: \['channel-setup\.check\.ts', 'channel-setup-validation\.check\.ts'\]/,
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

test("CI runs the complete channel setup suite with owned preview and failure evidence", () =>
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

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const hmac = read("../../web/e2e/hmac-channel.config.ts");
const authoring = read("../../web/e2e/wh-eg-authoring.config.ts");
const guard = "../.github/scripts/channel-setup-ci.test.mjs";
const commands = ["hmac-channel", "wh-eg-authoring"].map(
  (name) =>
    `node node_modules/playwright/cli.js test --config e2e/${name}.config.ts`,
);
const evidence = "${{ runner.temp }}/cma-channel-authoring";
function step(job, name) {
  const sections = job.split(`name: "${name}"`);
  assert.equal(sections.length, 2, `Require exactly one ${name} step`);
  return sections[1].split("\n      -")[0];
}
function authoringRun(job) {
  const block = step(job, "Local channel authoring contracts");
  return block
    .match(/run: \|\n([\s\S]*)$/)?.[1]
    .trim()
    .split("\n")
    .map((line) => line.trim())
    .join("\n");
}
function verifyAuthoring(workflow, shared, hmacSuite, authoringSuite) {
  verify(workflow, shared);
  const job = jobRunning(workflow, "Local channel authoring contracts");
  const execution = step(job, "Local channel authoring contracts");
  assert.match(execution, /id: channel_authoring\n/);
  assert.match(execution, /shell: bash\n/);
  assert.match(execution, /timeout-minutes: 5\n/);
  assert.ok(
    execution.includes(`CMA_CHANNEL_SETUP_EVIDENCE_DIR: "${evidence}"`),
  );
  assert.doesNotMatch(
    execution,
    /continue-on-error|\bif:|CMA_E2E_BROWSER_PATH/,
  );
  assert.equal(
    authoringRun(job),
    commands.join("\n"),
    "Run both complete suites, fail fast",
  );
  const mechanics = step(job, "Local browser guard mechanics");
  assert.ok(
    mechanics
      .split("\n")
      .some(
        (line) =>
          line.trim().startsWith("node --test ") &&
          line.trim().split(/\s+/).includes(guard),
      ),
  );
  assert.doesNotMatch(
    mechanics,
    /continue-on-error|\bif:|--test-name-pattern|\|\|/,
  );
  for (const name of [
    "Production build",
    "Install pinned local browser",
    "Compiled web build identity",
    "Local browser guard mechanics",
  ]) {
    step(job, name);
    assert.ok(
      job.indexOf(`name: "${name}"`) < job.indexOf(commands[0]),
      `${name} must precede authoring`,
    );
  }
  assert.ok(
    step(job, "Compiled web build identity").includes(
      "npx playwright test --config e2e/build-identity.config.ts",
    ),
  );
  const upload = step(job, "Channel authoring failure evidence");
  assert.match(
    upload,
    /if: "failure\(\) && steps\.channel_authoring\.outcome == 'failure'"/,
  );
  assert.match(upload, /uses: "actions\/upload-artifact@[^"\n]+"/);
  assert.ok(upload.includes(`path: "${evidence}"`));
  assert.match(upload, /name: "channel-authoring-failure"/);
  assert.match(upload, /retention-days: 1/);
  assert.doesNotMatch(upload, /continue-on-error/);
  for (const [suite, file, directory] of [
    [hmacSuite, "hmac-channel", "hmac"],
    [authoringSuite, "wh-eg-authoring", "authoring"],
  ]) {
    assert.match(suite, /import setupConfig from '\.\/channel-setup\.config'/);
    assert.match(suite, /\.\.\.setupConfig,/);
    assert.ok(suite.includes(`testMatch: ['${file}.check.ts']`));
    assert.ok(
      suite.includes(`outputDir: join(setupConfig.outputDir!, '${directory}')`),
    );
    assert.doesNotMatch(
      suite,
      /\b(?:workers|retries|forbidOnly|webServer|projects|use|grep|testIgnore|globalSetup):/,
    );
  }
  assert.doesNotMatch(hmacSuite, /\btimeout:/);
  assert.match(authoringSuite, /timeout: 60_000,/);
  assert.match(shared, /^  timeout: 30_000,$/m);
  for (const contract of [
    "preview.protocol !== 'http:'",
    "!['127.0.0.1', 'localhost'].includes(preview.hostname)",
    "!preview.port",
    "preview.username",
    "preview.password",
    "preview.pathname !== '/'",
    "preview.search",
    "preview.hash",
  ])
    assert.ok(
      shared.includes(contract),
      `Retain shared preview contract: ${contract}`,
    );
}
const originals = [workflow, config, hmac, authoring];
test("CI executes both authoring suites with isolated evidence and owned previews", () =>
  verifyAuthoring(...originals));

const mutations = [
  ...commands.map((command) => [0, command, ""]),
  ...[
    " --grep subset",
    " --project chromium",
    " --shard 1/2",
    " --retries 1",
    " --list",
    " --pass-with-no-tests",
    " || true",
    "; true",
  ].map((suffix) => [0, commands[0], commands[0] + suffix]),
  [0, ` ${guard}`, ""],
  [0, "id: channel_authoring", "id: other"],
  [0, "shell: bash", "shell: sh"],
  [
    0,
    "id: channel_authoring",
    "id: channel_authoring\n        continue-on-error: true",
  ],
  [0, "id: channel_authoring", "id: channel_authoring\n        if: false"],
  [
    0,
    "id: channel_authoring\n        timeout-minutes: 5",
    "id: channel_authoring\n        timeout-minutes: 10",
  ],
  [0, "steps.channel_authoring.outcome", "steps.channel_setup.outcome"],
  [0, `path: "${evidence}"`, 'path: "wrong-output"'],
  [
    0,
    `CMA_CHANNEL_SETUP_EVIDENCE_DIR: "${evidence}"`,
    'CMA_CHANNEL_SETUP_EVIDENCE_DIR: "wrong-output"',
  ],
  [0, 'name: "Channel authoring failure evidence"', 'name: "Missing evidence"'],
  ...[
    "Production build",
    "Install pinned local browser",
    "Compiled web build identity",
  ].map((name) => [0, `name: "${name}"`, 'name: "Missing prerequisite"']),
  ...[
    "workers: 1",
    "retries: 0",
    "forbidOnly: true",
    "reuseExistingServer: false",
    "--strictPort",
    "projects: browserProjects()",
    "trace: 'retain-on-failure'",
    "screenshot: 'only-on-failure'",
    "timeout: 30_000",
    "!preview.port",
    "preview.protocol !== 'http:'",
    "!['127.0.0.1', 'localhost'].includes(preview.hostname)",
  ].map((contract) => [1, contract, "removed"]),
  ...[
    [2, "hmac-channel", "hmac"],
    [3, "wh-eg-authoring", "authoring"],
  ].flatMap(([index, file, directory]) => [
    [index, "...setupConfig,", ""],
    [index, "...setupConfig,", "...setupConfig, retries: 1,"],
    [index, `'${file}.check.ts'`, "'other.check.ts'"],
    [
      index,
      `join(setupConfig.outputDir!, '${directory}')`,
      "setupConfig.outputDir",
    ],
  ]),
  [3, "timeout: 60_000", "timeout: 120_000"],
];
for (const [index, from, to] of mutations) {
  test(`authoring contract refuses input ${index}: ${from} -> ${to}`, () => {
    const inputs = [...originals];
    assert.ok(inputs[index].includes(from), "Mutation must alter a real input");
    inputs[index] = inputs[index].replace(from, to);
    assert.throws(() => verifyAuthoring(...inputs));
  });
}
for (const failure of ["", "hmac-channel", "wh-eg-authoring"]) {
  test(`workflow bash propagates ${failure || "successful execution"} without losing earlier evidence`, () => {
    verifyAuthoring(...originals);
    const root = mkdtempSync(join(tmpdir(), "channel-authoring-ci-"));
    try {
      const cli = join(root, "node_modules/playwright/cli.js");
      mkdirSync(dirname(cli), { recursive: true });
      writeFileSync(
        cli,
        String.raw`const fs = require('node:fs');
const name = process.argv.at(-1).split('/').pop().replace('.config.ts', '');
fs.appendFileSync(process.env.CMA_TEST_LOG, name + '\n');
const output = process.env.CMA_TEST_OUTPUT + '/' + (name === 'hmac-channel' ? 'hmac' : 'authoring');
fs.mkdirSync(output, { recursive: true });
fs.writeFileSync(output + '/receipt', name);
if (name === process.env.CMA_TEST_FAILURE) process.exit(7);
`,
      );
      const log = join(root, "calls");
      const output = join(root, "evidence");
      const result = spawnSync(
        "bash",
        [
          "--noprofile",
          "--norc",
          "-e",
          "-o",
          "pipefail",
          "-c",
          authoringRun(workflow),
        ],
        {
          cwd: root,
          encoding: "utf8",
          env: {
            ...process.env,
            PATH: dirname(process.execPath) + delimiter + process.env.PATH,
            CMA_TEST_LOG: log,
            CMA_TEST_OUTPUT: output,
            CMA_TEST_FAILURE: failure,
          },
        },
      );
      assert.equal(result.status, failure ? 7 : 0, result.stderr);
      const executed =
        failure === "hmac-channel"
          ? ["hmac-channel"]
          : ["hmac-channel", "wh-eg-authoring"];
      assert.deepEqual(readFileSync(log, "utf8").trim().split("\n"), executed);
      assert.equal(
        readFileSync(join(output, "hmac/receipt"), "utf8"),
        "hmac-channel",
      );
      if (executed.length === 2)
        assert.equal(
          readFileSync(join(output, "authoring/receipt"), "utf8"),
          "wh-eg-authoring",
        );
    } finally {
      rmSync(root, { recursive: true, force: true });
    }
  });
}
