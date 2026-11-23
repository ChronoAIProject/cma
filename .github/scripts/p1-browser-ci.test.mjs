import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync, spawn } from "node:child_process";
import { mkdir, mkdtemp, readFile, writeFile, cp, symlink, rm, readdir } from "node:fs/promises";
import { createServer } from "node:http";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { fileURLToPath } from "node:url";
import { setTimeout as delay } from "node:timers/promises";
import {
  argumentsFor,
  configure,
  verifyResults,
  verifyDevIdentity,
  startProductionPreview,
  command,
  runSuite,
} from "./p1-browser-ci.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const require = createRequire(new URL("../../web/package.json", import.meta.url));
const yaml = require("js-yaml");
const workflow = yaml.load(
  await readFile(new URL("../workflows/ci-web.yml", import.meta.url), "utf8"),
);
const env = Object.fromEntries(
  Object.entries(process.env).filter(
    ([key]) => !/^(CMA_|PW_|PLAYWRIGHT_|NODE_OPTIONS|RUNNER_TEMP)/.test(key),
  ),
);
for (const key of ["CMA_E2E_BROWSER_PATH", "PLAYWRIGHT_BROWSERS_PATH"])
  if (process.env[key]) env[key] = process.env[key];
const options = {
  suite: "design",
  web: "/synthetic/web",
  origin: "http://127.0.0.1:4311",
  output: "/synthetic/output",
  setup: "/synthetic/setup.mjs",
  p7: true,
};
const original = () => ({
  workers: 1,
  retries: 0,
  forbidOnly: true,
  timeout: 45_000,
  testMatch: [
    "design-correction.check.ts",
    "task-home-states.check.ts",
    "theme-palette.check.ts",
    "future.check.ts",
  ],
  webServer: {
    command: "npm run dev -- --host 127.0.0.1 --port 4644 --strictPort",
    reuseExistingServer: false,
    env: { VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED: "true" },
  },
});

test("fixed CLI rejects execution bypasses, including inherited selectors", () => {
  for (const suite of ["design", "shell", "auth"]) assert.equal(argumentsFor([suite], {}), suite);
  for (const args of [
    [],
    ["unknown"],
    ...["--list", "--grep=x", "--project=x", "--shard=1/2", "--retries=1"].map((arg) => [
      "design",
      arg,
    ]),
  ])
    assert.throws(() => argumentsFor(args, {}));
  for (const key of [
    "PW_TEST_GREP",
    "PLAYWRIGHT_JSON_OUTPUT_FILE",
    "CMA_E2E_BROWSERS",
    "CMA_AUTH_PREVIEW_URL",
    "NODE_OPTIONS",
  ])
    assert.throws(() => argumentsFor(["design"], { [key]: "override" }));
});

test("wrapper preserves full design, future registrations, projects, timeouts and enabled flag", () => {
  const input = original();
  input.projects = [{ name: "chromium", use: { browserName: "chromium" } }];
  const config = configure(input, options);
  assert.deepEqual(config.testMatch, input.testMatch);
  assert.deepEqual(config.projects, input.projects);
  assert.equal(config.timeout, input.timeout);
  assert.deepEqual(config.webServer.env, input.webServer.env);
  assert.equal(config.webServer.cwd, options.web);
  assert.match(config.webServer.command, /--port 4311 --strictPort$/);
  assert.equal(config.use.trace, "retain-on-failure");
  assert.equal(config.use.screenshot, "only-on-failure");
  assert.equal(config.outputDir, "/synthetic/output/results");
  const shell = {
    ...input,
    webServer: undefined,
    testMatch: ["agent-settings.check.ts", "shell-layout.check.ts"],
  };
  assert.equal(configure(shell, { ...options, suite: "shell" }).testMatch, "shell-layout.check.ts");
  const auth = { ...shell, testMatch: "auth-return-target.check.ts" };
  assert.deepEqual(configure(auth, { ...options, suite: "auth" }).projects, input.projects);
});

test("configuration mutations cannot remove coverage or reuse a server", () => {
  const mutations = [
    (c) => c.testMatch.pop() && c.testMatch.pop(),
    (c) => (c.workers = 2),
    (c) => (c.retries = 1),
    (c) => (c.forbidOnly = false),
    (c) => (c.webServer.reuseExistingServer = true),
    (c) => (c.webServer.command = c.webServer.command.replace("--strictPort", "")),
    (c) => (c.webServer.env = {}),
    ...["grep", "grepInvert", "shard", "testIgnore", "globalSetup", "globalTeardown"].map(
      (key) => (c) => (c[key] = "override"),
    ),
    ...["testMatch", "testDir", "retries", "dependencies"].map(
      (key) => (c) => (c.projects = [{ [key]: ["override"] }]),
    ),
    ...["trace", "screenshot", "baseURL"].map(
      (key) => (c) => (c.projects = [{ use: { [key]: "off" } }]),
    ),
  ];
  for (const mutate of mutations) {
    const input = original();
    mutate(input);
    assert.throws(() => configure(input, options));
  }
});

function report(executed) {
  return {
    config: {
      forbidOnly: true,
      workers: 1,
      shard: null,
      projects: [{ retries: 0, repeatEach: 1, testIgnore: [] }],
    },
    errors: [],
    stats: {
      expected: executed ? 1 : 0,
      skipped: executed ? 0 : 1,
      unexpected: 0,
      flaky: 0,
    },
    suites: [
      {
        specs: [
          {
            id: "one",
            file: "one.check.ts",
            title: "one",
            tests: [
              {
                projectId: "chromium",
                timeout: 30_000,
                expectedStatus: "passed",
                annotations: [],
                status: executed ? "expected" : "skipped",
                results: executed ? [{ status: "passed", retry: 0 }] : [],
              },
            ],
          },
        ],
      },
    ],
  };
}
test("only exactly discovered and successfully executed cases qualify", () => {
  assert.equal(verifyResults(report(false), report(true)).length, 1);
  assert.throws(() => verifyResults(report(false), report(false)));
  const mutations = [
    (r) => (r.suites = []),
    (r) => r.errors.push({ message: "error" }),
    (r) => (r.suites[0].specs[0].id = "different"),
    (r) => (r.stats.flaky = 1),
    (r) => (r.config.projects[0].retries = 1),
    (r) => (r.config.shard = { current: 1, total: 2 }),
    ...[
      (t) => (t.results = []),
      (t) => t.results.push(t.results[0]),
      (t) => (t.results[0].retry = 1),
      (t) => (t.results[0].status = "failed"),
      (t) => (t.expectedStatus = "skipped"),
      (t) => t.annotations.push({ type: "fail" }),
      (t) => (t.status = "flaky"),
      (t) => t.timeout++,
    ].map((mutate) => (r) => mutate(r.suites[0].specs[0].tests[0])),
  ];
  for (const mutate of mutations) {
    const result = report(true);
    mutate(result);
    assert.throws(() => verifyResults(report(false), result));
  }
});

function verifyWorkflow(job) {
  assert.equal(job.defaults.run["working-directory"], "web");
  assert.equal(job["continue-on-error"], undefined);
  const steps = job.steps;
  const mechanics = steps.filter((step) => step.name === "Design, shell and auth gate mechanics");
  assert.equal(mechanics.length, 1);
  assert.equal(mechanics[0].run, "node --test ../.github/scripts/p1-browser-ci.test.mjs");
  for (const suite of ["design", "shell", "auth"]) {
    const selected = steps.filter((step) => step.id === `p1_${suite}`);
    assert.equal(selected.length, 1);
    const step = selected[0];
    assert.equal(step.run, `node ../.github/scripts/p1-browser-ci.mjs ${suite}`);
    assert.ok(steps.indexOf(mechanics[0]) < steps.indexOf(step));
    assert.ok(steps.indexOf(step) < steps.findIndex((s) => s.id === "agent_v2"));
    assert.ok(
      steps.indexOf(step) > steps.findIndex((s) => s.name === "New agent request failure evidence"),
    );
    for (const checked of [step, mechanics[0]]) {
      for (const key of ["if", "continue-on-error", "env", "working-directory"])
        assert.equal(checked[key], undefined);
      assert.ok(checked["timeout-minutes"] > 0 && checked["timeout-minutes"] <= 10);
    }
    const artifact = steps[steps.indexOf(step) + 1];
    assert.equal(artifact.if, `failure() && steps.p1_${suite}.outcome == 'failure'`);
    assert.equal(artifact.uses, "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02");
    assert.equal(artifact.with.path, "${{ runner.temp }}/cma-p1-browser/" + suite);
    assert.equal(artifact.with["if-no-files-found"], "error");
    assert.equal(artifact.with["retention-days"], 1);
  }
}
test("workflow executes the guard and all suites, fails closed and uploads matching evidence", () => {
  verifyWorkflow(workflow.jobs.web);
  for (const mutate of [
    (j) => (j.steps = j.steps.filter((s) => s.id !== "p1_shell")),
    (j) => (j.steps.find((s) => s.name === "Design, shell and auth gate mechanics").run = "true"),
    (j) => (j.steps.find((s) => s.id === "p1_design").run += " --list"),
    (j) => (j.steps.find((s) => s.id === "p1_design")["continue-on-error"] = true),
    (j) => (j.steps.find((s) => s.id === "p1_design").if = "false"),
    (j) => (j.steps.find((s) => s.name === "Design browser failure evidence").with.path = "wrong"),
    (j) =>
      delete j.steps.find((s) => s.name === "Design browser failure evidence").with[
        "if-no-files-found"
      ],
  ]) {
    const job = structuredClone(workflow.jobs.web);
    mutate(job);
    assert.throws(() => verifyWorkflow(job));
  }
});

async function temporary(t) {
  const directory = await mkdtemp(join(tmpdir(), "cma-p1-control-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}
async function listen(server) {
  await new Promise((done) => server.listen(0, "127.0.0.1", done));
  return server.address().port;
}
async function eventually(check) {
  for (let i = 0; i < 100; i++) {
    if (await check()) return;
    await delay(100);
  }
  assert.fail("Timed out waiting for owned process state");
}
const exists = (path) =>
  readFile(path).then(
    () => true,
    () => false,
  );
async function closed(port) {
  await assert.rejects(fetch(`http://127.0.0.1:${port}`, { signal: AbortSignal.timeout(1000) }));
}
test("dev identity rejects a wrong checkout, HTML fallback and missing enabled flag", async (t) => {
  let mode = "valid";
  const server = createServer((req, res) => {
    res.end(
      mode === "html"
        ? "<html>fallback</html>"
        : req.url === "/src/shell/build.ts"
          ? "__CHRONO_BUILD_SHA__"
          : req.url === "/@vite/env"
            ? JSON.stringify(
                { __CHRONO_BUILD_SHA__: mode === "stale" ? "old" : "checkout" },
                null,
                2,
              )
            : JSON.stringify({
                VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED: mode === "off" ? "false" : "true",
              }),
    );
  });
  const port = await listen(server);
  t.after(() => new Promise((done) => server.close(done)));
  await verifyDevIdentity(`http://127.0.0.1:${port}`, "checkout", true);
  for (mode of ["stale", "html", "off"])
    await assert.rejects(verifyDevIdentity(`http://127.0.0.1:${port}`, "checkout", true));
});

async function fixture(t, body) {
  const directory = await temporary(t);
  const repository = join(directory, "repository");
  const web = join(repository, "web");
  for (const path of ["web/e2e", "web/scripts", "web/dist", ".github/scripts"])
    await mkdir(join(repository, path), { recursive: true });
  await symlink(join(root, "web/node_modules"), join(web, "node_modules"), "dir");
  await cp(join(root, "web/scripts/preview.mjs"), join(web, "scripts/preview.mjs"));
  await cp(
    new URL("./p1-browser-ci.mjs", import.meta.url),
    join(repository, ".github/scripts/p1-browser-ci.mjs"),
  );
  await writeFile(join(repository, ".gitignore"), "web/dist/\nweb/node_modules/\n");
  await writeFile(join(web, "package.json"), '{"type":"module"}');
  await writeFile(join(web, "dist/index.html"), "<html><body>synthetic preview</body></html>");
  await writeFile(
    join(web, "e2e/agent-settings.config.ts"),
    `export default { workers:1, retries:0, forbidOnly:true, timeout:10000, testMatch:['shell-layout.check.ts'], use:{launchOptions:{executablePath:process.env.CMA_E2E_BROWSER_PATH}} };`,
  );
  await writeFile(
    join(web, "e2e/shell-layout.check.ts"),
    `import {test,expect} from '@playwright/test';\nimport {writeFile} from 'node:fs/promises';\ntest('synthetic contract',async({page})=>{await page.goto('/');${body}});`,
  );
  const git = (...args) =>
    execFileSync("git", args, {
      cwd: repository,
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
    }).trim();
  git("init", "-q");
  git("add", ".");
  git(
    "-c",
    "user.name=Browser Contract",
    "-c",
    "user.email=browser@example.test",
    "-c",
    "commit.gpgsign=false",
    "commit",
    "-qm",
    "Synthetic browser fixture",
  );
  const revision = git("rev-parse", "HEAD");
  await writeFile(join(web, "dist/build-info.json"), JSON.stringify({ revision }));
  return {
    directory,
    repository,
    web,
    revision,
    env: { ...env, RUNNER_TEMP: join(directory, "evidence") },
  };
}

test("production preview rejects a stale build and occupied port without killing the occupant", async (t) => {
  const f = await fixture(t, "");
  await assert.rejects(startProductionPreview(f.web, 4311, "different"), /actual checkout/);
  const server = createServer((req, res) => res.end("unrelated"));
  const port = await listen(server);
  t.after(() => new Promise((done) => server.close(done)));
  await assert.rejects(startProductionPreview(f.web, port, f.revision), /Owned preview/);
  assert.equal(await (await fetch(`http://127.0.0.1:${port}`)).text(), "unrelated");
});

test("command cleans descendant listeners on success, failure, timeout and cancellation", async (t) => {
  const directory = await temporary(t);
  for (const mode of ["success", "failure", "timeout", "cancel"]) {
    const marker = join(directory, mode + ".json");
    const child = `const s=require('node:http').createServer();s.listen(0,'127.0.0.1',()=>{require('node:fs').writeFileSync(${JSON.stringify(marker)},JSON.stringify({port:s.address().port}));});process.on('SIGTERM',()=>{});`;
    const parent = `const {spawn}=require('node:child_process');spawn(process.execPath,['-e',${JSON.stringify(child)}],{stdio:'inherit'});setInterval(()=>{if(require('node:fs').existsSync(${JSON.stringify(marker)}) && ${JSON.stringify(mode)}==='success')process.exit(0);if(require('node:fs').existsSync(${JSON.stringify(marker)}) && ${JSON.stringify(mode)}==='failure')process.exit(3);},20);`;
    const controller = new AbortController();
    const pending = command(process.execPath, ["-e", parent], {
      cwd: directory,
      env,
      log: join(directory, mode + ".log"),
      timeout: mode === "timeout" ? 1000 : 15_000,
      signal: controller.signal,
    });
    const result = pending.then(
      () => null,
      (error) => error,
    );
    await eventually(() => exists(marker));
    if (mode === "cancel") controller.abort(new Error("controlled cancellation"));
    const error = await result;
    if (mode === "success") assert.equal(error, null);
    else assert.ok(error);
    await closed(JSON.parse(await readFile(marker)).port);
  }
  await assert.rejects(
    command("/no-such-browser-command", [], {
      cwd: directory,
      env,
      log: join(directory, "spawn.log"),
    }),
    /ENOENT/,
  );
});

test("real failed browser case returns failure with trace and screenshot in the upload directory", async (t) => {
  const f = await fixture(
    t,
    "await expect(page.locator('body')).toHaveText('deliberate mismatch',{timeout:100});",
  );
  await assert.rejects(runSuite("shell", f), /Browser command failed/);
  const output = join(f.env.RUNNER_TEMP, "cma-p1-browser/shell");
  const files = await readdir(join(output, "results"), { recursive: true });
  assert.ok(files.some((file) => file.endsWith("trace.zip")));
  assert.ok(files.some((file) => file.endsWith(".png")));
  const receipt = JSON.parse(await readFile(join(output, "receipt.json")));
  assert.equal(receipt.status, "failed");
  assert.equal(receipt.productionPreviewClosed, true);
  await closed(receipt.port);
});

test("real successful browser case binds discovery, execution, checkout and preview cleanup", async (t) => {
  const f = await fixture(t, "await expect(page.locator('body')).toHaveText('synthetic preview');");
  const receipt = await runSuite("shell", f);
  assert.equal(receipt.status, "passed");
  assert.equal(receipt.revision, f.revision);
  assert.equal(receipt.cases.length, 1);
  await closed(receipt.port);
  await assert.rejects(runSuite("shell", f), /EEXIST/);
});

for (const signal of ["SIGINT", "SIGTERM"])
  test(`real CLI ${signal} closes its preview and browser and remains nonzero`, async (t) => {
    const f = await fixture(
      t,
      "const cdp=await page.context().browser().newBrowserCDPSession();const info=await cdp.send('SystemInfo.getProcessInfo');await writeFile('dist/started',String(info.processInfo.find(p=>p.type==='browser').id));await new Promise(()=>{});",
    );
    const child = spawn(
      process.execPath,
      [join(f.repository, ".github/scripts/p1-browser-ci.mjs"), "shell"],
      { cwd: f.repository, env: f.env, stdio: ["ignore", "pipe", "pipe"] },
    );
    t.after(() => {
      if (child.exitCode === null) child.kill("SIGKILL");
    });
    let diagnostics = "";
    child.stdout.on("data", (chunk) => {
      diagnostics += chunk;
    });
    child.stderr.on("data", (chunk) => {
      diagnostics += chunk;
    });
    const ended = new Promise((done) =>
      child.once("exit", (code, signal) => done({ code, signal })),
    );
    await eventually(async () => {
      assert.equal(child.exitCode, null, diagnostics);
      return exists(join(f.web, "dist/started"));
    });
    const browserPid = Number(await readFile(join(f.web, "dist/started"), "utf8"));
    child.kill(signal);
    const outcome = await Promise.race([
      ended,
      delay(10_000, undefined, { ref: false }).then(() => {
        throw new Error("Cancellation did not finish");
      }),
    ]);
    assert.equal(outcome.code, signal === "SIGINT" ? 130 : 143);
    const receipt = JSON.parse(
      await readFile(join(f.env.RUNNER_TEMP, "cma-p1-browser/shell/receipt.json")),
    );
    assert.ok(receipt.error.includes(`Cancelled by ${signal}`));
    await eventually(() => {
      try {
        process.kill(browserPid, 0);
        return false;
      } catch (error) {
        return error.code === "ESRCH";
      }
    });
    assert.equal(receipt.productionPreviewClosed, true);
    await closed(receipt.port);
  });
