import assert from "node:assert/strict";
import test from "node:test";
import { execFileSync, spawn } from "node:child_process";
import { cp, mkdir, mkdtemp, readFile, readdir, rm, symlink, writeFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import { configure, run, validateArguments, verifyArtifact } from "./p7-default-off-ci.mjs";

const root = fileURLToPath(new URL("../..", import.meta.url));
const require = createRequire(new URL("../../web/package.json", import.meta.url));
const workflow = require("js-yaml").load(
  await readFile(new URL("../workflows/ci-web.yml", import.meta.url), "utf8"),
);
const env = Object.fromEntries(
  Object.entries(process.env).filter(
    ([key]) => !/^(CMA_|PW_|PLAYWRIGHT_|NODE_OPTIONS|RUNNER_TEMP|VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED)/.test(key),
  ),
);
for (const key of ["CMA_E2E_BROWSER_PATH", "PLAYWRIGHT_BROWSERS_PATH"])
  if (process.env[key]) env[key] = process.env[key];
const original = () => ({
  workers: 1,
  retries: 0,
  forbidOnly: true,
  testMatch: "topographic-off.check.ts",
  timeout: 30_000,
  projects: [{ name: "chromium", use: { browserName: "chromium" } }],
});
const options = { web: "/synthetic/web", output: "/synthetic/evidence", origin: "http://127.0.0.1:4312" };

test("default-off accepts no filters, injected runner settings or explicit feature flag", () => {
  validateArguments([], {});
  for (const args of [["--list"], ["--grep=landing"], ["--project=chromium"], ["--shard=1/2"], ["--retries=1"]])
    assert.throws(() => validateArguments(args, {}));
  for (const key of [
    "VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED",
    "CMA_SOURCE_SHA",
    "CMA_E2E_BROWSERS",
    "PW_TEST_GREP",
    "NODE_OPTIONS",
  ])
    assert.throws(() => validateArguments([], { [key]: "true" }));
});

test("wrapper retains projects and timeouts and cannot filter or suppress failure evidence", () => {
  const input = original();
  const config = configure(input, options);
  assert.equal(config.timeout, input.timeout);
  assert.deepEqual(config.projects, input.projects);
  assert.equal(config.use.trace, "retain-on-failure");
  assert.equal(config.use.screenshot, "only-on-failure");
  assert.equal(config.testMatch, input.testMatch);
  for (const change of [
    (c) => (c.testMatch = "different.ts"),
    (c) => (c.retries = 1),
    (c) => (c.workers = 2),
    (c) => (c.forbidOnly = false),
    (c) => (c.webServer = { reuseExistingServer: true }),
    ...["grep", "grepInvert", "shard", "testIgnore", "globalSetup", "globalTeardown"].map(
      (key) => (c) => (c[key] = "override"),
    ),
    ...["testMatch", "testDir", "dependencies"].map((key) => (c) => (c.projects[0][key] = "override")),
    ...["baseURL", "trace", "screenshot"].map((key) => (c) => (c.projects[0].use[key] = "override")),
  ]) {
    const copy = original();
    change(copy);
    assert.throws(() => configure(copy, options));
  }
});

function verifyWorkflow(job) {
  assert.equal(job.defaults.run["working-directory"], "web");
  const guard = job.steps.find((s) => s.name === "Default-off production gate mechanics");
  const step = job.steps.find((s) => s.id === "p7_default_off");
  assert.equal(guard.run, "node --test ../.github/scripts/p7-default-off-ci.test.mjs");
  assert.equal(step.run, "node ../.github/scripts/p7-default-off-ci.mjs");
  for (const entry of [guard, step]) {
    for (const key of ["if", "continue-on-error", "env", "working-directory"]) assert.equal(entry[key], undefined);
    assert.ok(entry["timeout-minutes"] > 0 && entry["timeout-minutes"] <= 10);
  }
  assert.ok(job.steps.indexOf(guard) < job.steps.indexOf(step));
  assert.ok(job.steps.findIndex((s) => s.id === "p1_auth") < job.steps.indexOf(guard));
  const upload = job.steps[job.steps.indexOf(step) + 1];
  assert.equal(upload.if, "failure() && steps.p7_default_off.outcome == 'failure'");
  assert.equal(upload.uses, "actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02");
  assert.equal(upload.with.path, "${{ runner.temp }}/cma-p7-default-off");
  assert.equal(upload.with["if-no-files-found"], "error");
  assert.equal(upload.with["retention-days"], 1);
}

test("workflow runs the independent production lane and fails closed", () => {
  verifyWorkflow(workflow.jobs.web);
  for (const change of [
    (j) => (j.steps = j.steps.filter((s) => s.id !== "p7_default_off")),
    (j) => (j.steps.find((s) => s.id === "p7_default_off").run += " --list"),
    (j) => (j.steps.find((s) => s.id === "p7_default_off")["continue-on-error"] = true),
    (j) => (j.steps.find((s) => s.id === "p7_default_off").if = "false"),
    (j) => (j.steps.find((s) => s.name === "Default-off production gate mechanics").run = "true"),
    (j) => (j.steps.find((s) => s.name === "Default-off production failure evidence").with.path = "wrong"),
  ]) {
    const copy = structuredClone(workflow.jobs.web);
    change(copy);
    assert.throws(() => verifyWorkflow(copy));
  }
});

async function temporary(t) {
  const directory = await mkdtemp(join(tmpdir(), "cma-off-control-"));
  t.after(() => rm(directory, { recursive: true, force: true }));
  return directory;
}

test("build identity and renderer absence reject stale and enabled artifacts", async (t) => {
  const directory = await temporary(t);
  await mkdir(join(directory, "dist/assets"), { recursive: true });
  await writeFile(join(directory, "dist/build-info.json"), JSON.stringify({ revision: "current" }));
  await verifyArtifact(directory, "current");
  await assert.rejects(verifyArtifact(directory, "other"), /actual checkout/);
  await writeFile(join(directory, "dist/assets/topographic-surface-synthetic.js"), "export {};");
  await assert.rejects(verifyArtifact(directory, "current"), /Enabled renderer/);
});

async function fixture(t, body, buildMode = "normal") {
  const directory = await temporary(t),
    repository = join(directory, "repository"),
    web = join(repository, "web");
  for (const path of ["web/e2e", "web/scripts", ".github/scripts"])
    await mkdir(join(repository, path), { recursive: true });
  await symlink(join(root, "web/node_modules"), join(web, "node_modules"), "dir");
  await cp(join(root, "web/scripts/preview.mjs"), join(web, "scripts/preview.mjs"));
  for (const name of ["p1-browser-ci.mjs", "p7-default-off-ci.mjs"])
    await cp(join(root, ".github/scripts", name), join(repository, ".github/scripts", name));
  await writeFile(join(repository, ".gitignore"), "web/node_modules/\nweb/started\nweb/.env*\n");
  await writeFile(
    join(web, "package.json"),
    JSON.stringify({ type: "module", scripts: { build: "node build.mjs" } }),
  );
  await writeFile(
    join(web, "build.mjs"),
    `import{mkdir,writeFile}from'node:fs/promises';import{join}from'node:path';
if(process.env.VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED!==undefined)throw new Error('Flag inherited');
if(${JSON.stringify(buildMode)}==='fail')process.exit(3);
if(${JSON.stringify(buildMode)}==='wait'){await writeFile('started','build');setInterval(()=>{},1000);await new Promise(()=>{});}
const dist=process.argv[process.argv.indexOf('--outDir')+1];await mkdir(dist,{recursive:true});
await writeFile(join(dist,'index.html'),'<html><body>Default off</body></html>');
await writeFile(join(dist,'build-info.json'),JSON.stringify({revision:process.env.CHRONO_BUILD_SHA}));`,
  );
  await writeFile(
    join(web, "e2e/topographic-off.config.ts"),
    `export default { workers:1,retries:0,forbidOnly:true,timeout:10000,testMatch:'topographic-off.check.ts',use:{launchOptions:{executablePath:process.env.CMA_E2E_BROWSER_PATH}} };`,
  );
  await writeFile(
    join(web, "e2e/topographic-off.check.ts"),
    `import{test,expect}from'@playwright/test';import{writeFile}from'node:fs/promises';test('default off',async({page})=>{await page.goto('/');${body}});`,
  );
  const git = (...args) =>
    execFileSync("git", args, { cwd: repository, encoding: "utf8", stdio: ["ignore", "pipe", "pipe"] }).trim();
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
    "Synthetic default-off fixture",
  );
  return {
    directory,
    repository,
    web,
    revision: git("rev-parse", "HEAD"),
    env: { ...env, RUNNER_TEMP: join(directory, "evidence") },
  };
}

async function closed(port) {
  await assert.rejects(fetch(`http://127.0.0.1:${port}`, { signal: AbortSignal.timeout(1000) }));
}

test("real production qualification builds, discovers, executes and closes its preview", async (t) => {
  const f = await fixture(t, "await expect(page.locator('body')).toHaveText('Default off');");
  const receipt = await run(f);
  assert.equal(receipt.status, "passed");
  assert.equal(receipt.revision, f.revision);
  assert.equal(receipt.flag, "unset");
  assert.deepEqual(
    receipt.commands.map((c) => c.phase),
    ["build", "discovery", "execution"],
  );
  assert.equal(receipt.cases.length, 1);
  await closed(receipt.port);
  await assert.rejects(run(f), /EEXIST/);
});

test("real browser failure retains a screenshot and trace and closes its preview", async (t) => {
  const f = await fixture(
    t,
    "await expect(page.locator('body')).toHaveText('deliberate mismatch',{timeout:100});",
  );
  await assert.rejects(run(f), /Browser command failed/);
  const output = join(f.env.RUNNER_TEMP, "cma-p7-default-off");
  const files = await readdir(join(output, "results"), { recursive: true });
  assert.ok(files.some((p) => p.endsWith(".png")));
  assert.ok(files.some((p) => p.endsWith("trace.zip")));
  const receipt = JSON.parse(await readFile(join(output, "receipt.json")));
  assert.equal(receipt.status, "failed");
  await closed(receipt.port);
});

test("a failed build and a local environment file stop before browser execution", async (t) => {
  const f = await fixture(t, "", "fail");
  await assert.rejects(run(f), /Browser command failed/);
  const receipt = JSON.parse(await readFile(join(f.env.RUNNER_TEMP, "cma-p7-default-off/receipt.json")));
  assert.equal(receipt.commands.length, 1);
  assert.equal(receipt.port, undefined);
  const other = await fixture(t, "");
  await writeFile(join(other.web, ".env.production.local"), "VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED=true");
  await assert.rejects(run(other), /cannot inherit/);
});

test("uncommitted inputs outside web also invalidate checkout qualification", async (t) => {
  const f = await fixture(t, "");
  await mkdir(join(f.repository, "api/fixtures"), { recursive: true });
  await writeFile(join(f.repository, "api/fixtures/contract.json"), "{}");
  await assert.rejects(run(f), /Source must be clean/);
});

for (const phase of ["build", "browser"])
  for (const signal of ["SIGINT", "SIGTERM"])
    test(`CLI ${signal} during ${phase} stays failed and cleans owned resources`, async (t) => {
      const f = await fixture(
        t,
        "const cdp=await page.context().browser().newBrowserCDPSession();const info=await cdp.send('SystemInfo.getProcessInfo');await writeFile('started',String(info.processInfo.find(p=>p.type==='browser').id));await new Promise(()=>{});",
        phase === "build" ? "wait" : "normal",
      );
      const child = spawn(process.execPath, [join(f.repository, ".github/scripts/p7-default-off-ci.mjs")], {
        cwd: f.repository,
        env: f.env,
        stdio: "ignore",
      });
      t.after(() => {
        if (child.exitCode === null) child.kill("SIGTERM");
      });
      const ended = new Promise((done) => child.once("exit", done));
      let marker;
      for (let i = 0; i < 100; i++) {
        marker = await readFile(join(f.web, "started"), "utf8").catch(() => null);
        if (marker) break;
        assert.equal(child.exitCode, null);
        await delay(100);
      }
      assert.ok(marker);
      child.kill(signal);
      const code = await Promise.race([
        ended,
        delay(10000, undefined, { ref: false }).then(() => {
          throw new Error("Cancellation timed out");
        }),
      ]);
      assert.equal(code, signal === "SIGINT" ? 130 : 143);
      const receipt = JSON.parse(await readFile(join(f.env.RUNNER_TEMP, "cma-p7-default-off/receipt.json")));
      assert.equal(receipt.status, "failed");
      assert.ok(receipt.error.includes(signal));
      if (phase === "browser") {
        await closed(receipt.port);
        for (let i = 0; i < 50; i++) {
          try {
            process.kill(Number(marker), 0);
          } catch (error) {
            if (error.code === "ESRCH") break;
            throw error;
          }
          await delay(100);
        }
        assert.throws(() => process.kill(Number(marker), 0), { code: "ESRCH" });
      } else assert.equal(receipt.port, undefined);
    });
