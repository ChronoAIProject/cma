import assert from "node:assert/strict";
import { spawn, execFileSync } from "node:child_process";
import { createWriteStream, realpathSync } from "node:fs";
import { mkdir, mkdtemp, readFile, writeFile } from "node:fs/promises";
import { createHash } from "node:crypto";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
// Suites that share a config split its entries between them, so each owned
// command keeps a meaningful bound (#3303). Exactly one of them is the
// remainder: it also runs every entry the config registers later that no
// sibling claims, so the suites together always run the whole config.
export const suites = {
  design: {
    config: "design-correction",
    files: ["design-correction", "task-home-states"],
    remainder: true,
  },
  palette: { config: "design-correction", files: ["theme-palette"] },
  shell: { config: "agent-settings", files: ["shell-layout"] },
  auth: { config: "auth-return-target", files: ["auth-return-target"] },
};
const hash = (value) => createHash("sha256").update(value).digest("hex");
const json = (path, value) => writeFile(path, JSON.stringify(value, null, 2) + "\n");
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));

export function argumentsFor(args, env) {
  assert.equal(args.length, 1, `Supply exactly one suite: ${Object.keys(suites).join(", ")}`);
  assert.ok(Object.hasOwn(suites, args[0]), "Unknown browser suite");
  for (const key of Object.keys(env)) {
    if (!env[key]) continue;
    const allowed = ["CMA_E2E_BROWSER_PATH", "PLAYWRIGHT_BROWSERS_PATH"];
    assert.ok(
      !/^(PW_|PLAYWRIGHT_|CMA_)/.test(key) || allowed.includes(key),
      `Unsupported browser environment: ${key}`,
    );
  }
  assert.ok(!env.NODE_OPTIONS, "NODE_OPTIONS cannot alter browser gate execution");
  return args[0];
}

export function configure(original, options) {
  const { suite, web, origin, output, setup, server } = options;
  assert.equal(original.workers, 1, "Browser suites must use one worker");
  assert.equal(original.retries, 0, "Browser suites cannot retry");
  assert.equal(original.forbidOnly, true, "Browser suites must forbid only");
  for (const config of [original, ...(original.projects ?? [])]) {
    for (const key of ["grep", "grepInvert", "shard", "testIgnore", "globalSetup", "globalTeardown"])
      assert.equal(config[key], undefined, `Browser configuration cannot narrow execution: ${key}`);
    assert.ok(config.repeatEach === undefined || config.repeatEach === 1, "Browser suites cannot repeat");
    assert.ok(config.retries === undefined || config.retries === 0, "Project retries are forbidden");
  }
  for (const project of original.projects ?? []) {
    assert.equal(project.testMatch, undefined, "Projects cannot replace the suite selection");
    assert.equal(project.testDir, undefined, "Projects cannot replace the suite directory");
    assert.ok(!project.dependencies?.length, "Unlisted project dependencies are forbidden");
    for (const key of ["baseURL", "trace", "screenshot"])
      assert.equal(project.use?.[key], undefined, `Projects cannot override owned evidence: ${key}`);
  }
  const matches = [original.testMatch].flat();
  const siblings = Object.entries(suites).filter(([, other]) => other.config === suites[suite].config);
  assert.equal(
    siblings.filter(([, other]) => other.remainder).length,
    siblings.length > 1 ? 1 : 0,
    "A shared browser config needs exactly one remainder suite",
  );
  // Every sibling's entries must still exist: removing one from the config
  // would otherwise drop it from every suite without failing.
  for (const [, other] of siblings)
    for (const file of other.files)
      assert.ok(matches.includes(`${file}.check.ts`), `Missing browser entry: ${file}`);
  const claimed = siblings
    .filter(([name]) => name !== suite)
    .flatMap(([, other]) => other.files.map((file) => `${file}.check.ts`));
  const result = {
    ...original,
    testDir: join(web, "e2e"),
    outputDir: join(output, "results"),
    reporter: [["list"], ["json", { outputFile: process.env.CMA_P1_REPORT }]],
    use: {
      ...original.use,
      baseURL: origin,
      trace: "retain-on-failure",
      screenshot: "only-on-failure",
    },
  };
  if (suite === "shell") result.testMatch = "shell-layout.check.ts";
  if (siblings.length > 1)
    result.testMatch = suites[suite].remainder
      ? matches.filter((entry) => !claimed.includes(entry))
      : suites[suite].files.map((file) => `${file}.check.ts`);
  if (suites[suite].config === suites.design.config) {
    assert.equal(original.webServer?.reuseExistingServer, false, "Design must own its server");
    assert.match(original.webServer.command, /^npm run dev -- --host 127\.0\.0\.1 --port \d+ --strictPort$/);
    result.webServer = {
      ...original.webServer,
      command: [process.execPath, server].map((arg) => "'" + arg.replaceAll("'", "'\\''") + "'").join(" "),
      env: {
        ...original.webServer.env,
        CMA_P1_DEV_ARGS: JSON.stringify(
          original.webServer.command
            .replace(/--port \d+/, `--port ${new URL(origin).port}`)
            .split(" ")
            .slice(1),
        ),
      },
      gracefulShutdown: { signal: "SIGTERM", timeout: 4000 },
      cwd: web,
      url: origin,
      reuseExistingServer: false,
    };
    result.globalSetup = setup;
  } else {
    assert.equal(original.webServer, undefined, "Production preview is owned by the gate");
  }
  return result;
}

function cases(report, executed) {
  assert.deepEqual(report.errors, [], "Browser report contains errors");
  assert.equal(report.config.forbidOnly, true);
  assert.equal(report.config.workers, 1);
  assert.equal(report.config.shard, null);
  const entries = [];
  function visit(suite) {
    for (const spec of suite.specs ?? [])
      for (const test of spec.tests) {
        assert.equal(test.expectedStatus, "passed", "Skipped or expected-failure cases cannot qualify");
        assert.ok(!test.annotations.some(({ type }) => ["skip", "fixme", "fail"].includes(type)));
        if (executed) {
          assert.equal(test.status, "expected", "Every discovered case must pass");
          assert.equal(test.results.length, 1, "Every case must execute exactly once");
          assert.equal(test.results[0].status, "passed");
          assert.equal(test.results[0].retry, 0);
        } else assert.deepEqual(test.results, [], "Discovery must not execute cases");
        entries.push(JSON.stringify([spec.id, spec.file, spec.title, test.projectId, test.timeout]));
      }
    for (const child of suite.suites ?? []) visit(child);
  }
  for (const project of report.config.projects) {
    assert.equal(project.retries, 0);
    assert.equal(project.repeatEach, 1);
    assert.deepEqual(project.testIgnore, []);
  }
  report.suites.forEach(visit);
  assert.ok(entries.length, "Browser selection is empty");
  assert.equal(new Set(entries).size, entries.length, "Duplicate browser case identity");
  return entries.sort();
}

export function verifyResults(discovery, execution) {
  const expected = cases(discovery, false);
  assert.deepEqual(cases(execution, true), expected, "Discovered and executed cases differ");
  for (const field of ["configFile", "rootDir", "grep", "grepInvert", "shard"])
    assert.deepEqual(execution.config[field], discovery.config[field], `Browser configuration changed: ${field}`);
  const projects = (report) => report.config.projects.map(({ metadata, ...selection }) => selection);
  assert.deepEqual(projects(execution), projects(discovery), "Project selection changed");
  assert.equal(execution.stats.expected, expected.length);
  for (const field of ["skipped", "unexpected", "flaky"]) assert.equal(execution.stats[field], 0);
  return expected;
}

export async function verifyDevIdentity(origin, revision) {
  const modules = [];
  for (const path of [
    "/src/shell/build.ts",
    "/@vite/env",
  ]) {
    const response = await fetch(origin + path, {
      signal: AbortSignal.timeout(4000),
    });
    const text = await response.text();
    assert.ok(response.ok && !text.trimStart().startsWith("<"), `Invalid development module: ${path}`);
    modules.push({ path, text, sha256: hash(text) });
  }
  assert.match(modules[0].text, /__CHRONO_BUILD_SHA__/, "Build module must consume the checked definition");
  assert.ok(
    modules[1].text.match(/"__CHRONO_BUILD_SHA__"\s*:\s*"([^"]+)"/)?.[1] === revision,
    "Development server revision does not match checkout",
  );
  return { revision, modules };
}

export async function startProductionPreview(web, port, revision) {
  const identity = await readJson(join(web, "dist/build-info.json"));
  assert.equal(identity.revision, revision, "Production artifact is not from the actual checkout");
  const { startPreview } = await import(pathToFileURL(join(web, "scripts/preview.mjs")).href);
  return startPreview({ directory: web, port });
}

async function freePort() {
  const server = createServer();
  await new Promise((done, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", done);
  });
  const port = server.address().port;
  await new Promise((done) => server.close(done));
  return port;
}

export async function command(file, args, { cwd, env, signal, log, timeout = 480_000 }) {
  signal?.throwIfAborted();
  const output = createWriteStream(log);
  const child = spawn(file, args, {
    cwd,
    env,
    detached: true,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let force;
  let timedOut = false;
  function kill(kind) {
    if (!child.pid) return;
    try {
      process.kill(-child.pid, kind);
    } catch (error) {
      if (error.code !== "ESRCH") throw error;
    }
  }
  function stop() {
    kill("SIGTERM");
    force ??= setTimeout(() => kill("SIGKILL"), 2000);
  }
  const timer = setTimeout(() => {
    timedOut = true;
    stop();
  }, timeout);
  signal?.addEventListener("abort", stop, { once: true });
  child.stdout.pipe(output, { end: false });
  child.stderr.pipe(output, { end: false });
  try {
    const result = await new Promise((done, reject) => {
      child.once("error", reject);
      child.once("exit", stop);
      child.once("close", (code, reason) => done({ code, signal: reason }));
    });
    signal?.throwIfAborted();
    assert.ok(!timedOut, "Browser command exceeded its bound");
    assert.equal(result.code, 0, `Browser command failed (${result.signal ?? result.code}); see ${log}`);
  } finally {
    clearTimeout(timer);
    clearTimeout(force);
    signal?.removeEventListener("abort", stop);
    kill("SIGKILL");
    await new Promise((done) => output.end(done));
  }
}

export async function serveDesign() {
  const controller = new AbortController();
  const stop = () => controller.abort(new Error("Owned development server stopped"));
  process.stdin.on("end", stop);
  process.stdin.resume();
  process.once("SIGTERM", stop);
  process.once("SIGINT", stop);
  try {
    await command("npm", JSON.parse(process.env.CMA_P1_DEV_ARGS), {
      cwd: process.cwd(),
      env: process.env,
      signal: controller.signal,
      log: process.argv[2],
      timeout: 540_000,
    });
  } finally {
    process.stdin.destroy();
    process.removeListener("SIGTERM", stop);
    process.removeListener("SIGINT", stop);
  }
}

function sourceIdentity(repository) {
  const git = (...args) => execFileSync("git", args, { cwd: repository, encoding: "utf8" }).trim();
  assert.equal(
    git(
      "status",
      "--porcelain",
      "--untracked-files=normal",
      "--",
      "web",
      ".github/scripts/p1-browser-ci.mjs",
      ".github/workflows/ci-web.yml",
    ),
    "",
    "Browser source and gate must be clean",
  );
  return {
    revision: git("rev-parse", "HEAD"),
    tree: git("rev-parse", "HEAD^{tree}"),
  };
}

export async function runSuite(suite, { repository = root, env = process.env, signal } = {}) {
  argumentsFor([suite], env);
  const temporary = env.RUNNER_TEMP || (await mkdtemp(join(tmpdir(), "cma-browser-gate-")));
  const output = resolve(temporary, "cma-p1-browser", suite);
  assert.ok(!output.startsWith(resolve(repository) + "/"), "Browser evidence must stay outside the source tree");
  await mkdir(dirname(output), { recursive: true });
  await mkdir(output);
  const receipt = { suite, status: "failed", output, commands: [] };
  let preview;
  try {
    const source = sourceIdentity(repository);
    Object.assign(receipt, source);
    receipt.runnerSha256 = hash(await readFile(fileURLToPath(import.meta.url)));
    console.log(`${suite}: checkout ${source.revision}; tree ${source.tree}`);
    const web = join(repository, "web");
    const childEnv = {
      ...env,
      CI: "true",
      CHRONO_BUILD_SHA: source.revision,
      CHRONO_BUILD: `git-${source.revision.slice(0, 12)}`,
      CHRONO_BUILD_ARTIFACT: "local",
      CMA_SOURCE_SHA: source.revision,
    };
    const port = await freePort();
    const origin = `http://127.0.0.1:${port}`;
    receipt.port = port;
    const helper = pathToFileURL(fileURLToPath(import.meta.url)).href;
    const original = pathToFileURL(join(web, "e2e", `${suites[suite].config}.config.ts`)).href;
    const server = join(output, "server.mjs");
    await writeFile(
      server,
      `import {serveDesign} from ${JSON.stringify(helper)};\nprocess.argv[2]=${JSON.stringify(join(output, "server.log"))};\nawait serveDesign();\n`,
    );
    const setup = join(output, "identity.mjs");
    await writeFile(
      setup,
      `import {verifyDevIdentity} from ${JSON.stringify(helper)};\nimport {writeFile} from 'node:fs/promises';\nexport default async()=>writeFile(${JSON.stringify(join(output, "identity.json"))},JSON.stringify(await verifyDevIdentity(${JSON.stringify(origin)},${JSON.stringify(source.revision)}),null,2));\n`,
    );
    const config = join(output, "config.mts");
    await writeFile(
      config,
      `import original from ${JSON.stringify(original)};\nimport {configure} from ${JSON.stringify(helper)};\nexport default configure(original,${JSON.stringify({ suite, web, origin, output, setup, server })});\n`,
    );
    receipt.configSha256 = hash(await readFile(config));
    if (suites[suite].config !== suites.design.config)
      preview = await startProductionPreview(web, port, source.revision);
    signal?.throwIfAborted();
    for (const phase of ["discovery", "execution"]) {
      const args = [
        join(web, "node_modules/playwright/cli.js"),
        "test",
        "--config",
        config,
        ...(phase === "discovery" ? ["--list"] : []),
      ];
      receipt.commands.push({ phase, args });
      await command(process.execPath, args, {
        cwd: web,
        env: { ...childEnv, CMA_P1_REPORT: join(output, `${phase}.json`) },
        signal,
        log: join(output, `${phase}.log`),
      });
      preview?.assertAlive();
      if (phase === "discovery") {
        const discovered = cases(await readJson(join(output, "discovery.json")), false);
        console.log(`${suite}: discovered ${discovered.length} cases\n${discovered.join("\n")}`);
      }
    }
    receipt.cases = verifyResults(
      await readJson(join(output, "discovery.json")),
      await readJson(join(output, "execution.json")),
    );
    assert.deepEqual(sourceIdentity(repository), source, "Source changed during qualification");
    assert.equal(
      hash(await readFile(config)),
      receipt.configSha256,
      "Generated config changed during qualification",
    );
    signal?.throwIfAborted();
    receipt.status = "passed";
  } catch (error) {
    receipt.error = error.message;
    throw error;
  } finally {
    await preview?.close();
    receipt.productionPreviewClosed = Boolean(preview);
    await json(join(output, "receipt.json"), receipt);
    console.log(
      `${suite}: ${receipt.status}${receipt.cases ? ` (${receipt.cases.length} executed)` : ""}; evidence ${output}`,
    );
  }
  return receipt;
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const controller = new AbortController();
  const cancel = (signal) => {
    process.exitCode = signal === "SIGINT" ? 130 : 143;
    controller.abort(new Error(`Cancelled by ${signal}`));
  };
  const interrupt = () => cancel("SIGINT");
  const terminate = () => cancel("SIGTERM");
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  try {
    await runSuite(argumentsFor(process.argv.slice(2), process.env), {
      signal: controller.signal,
    });
  } catch (error) {
    console.error(error.message);
    process.exitCode ||= 1;
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", terminate);
  }
}
