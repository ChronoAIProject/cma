import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { realpathSync } from "node:fs";
import { mkdir, mkdtemp, readFile, readdir, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { argumentsFor, command, verifyResults } from "./p1-browser-ci.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const readJson = async (path) => JSON.parse(await readFile(path, "utf8"));
const hash = (data) => createHash("sha256").update(data).digest("hex");

export function validateArguments(args, env) {
  assert.equal(args.length, 0, "Default-off qualification accepts no selectors or build overrides");
  argumentsFor(["design"], env);
  assert.equal(env.VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED, undefined, "Default-off requires an unset flag");
}

export async function verifyArtifact(directory, revision) {
  const identity = await readJson(join(directory, "dist/build-info.json"));
  assert.equal(identity.revision, revision, "Default-off build must match actual checkout");
  const files = await readdir(join(directory, "dist"), { recursive: true });
  assert.ok(
    !files.some((file) => /topographic-surface.*\.js$/.test(file)),
    "Enabled renderer found in default-off build",
  );
  return identity;
}

export function configure(original, { web, output, origin }) {
  assert.equal(original.testMatch, "topographic-off.check.ts");
  assert.equal(original.workers, 1);
  assert.equal(original.retries, 0);
  assert.equal(original.forbidOnly, true);
  assert.equal(original.webServer, undefined);
  for (const part of [original, ...(original.projects ?? [])]) {
    for (const key of ["grep", "grepInvert", "shard", "testIgnore", "globalSetup", "globalTeardown"])
      assert.equal(part[key], undefined, `Default-off cannot narrow execution: ${key}`);
    assert.ok(part.retries === undefined || part.retries === 0);
    assert.ok(part.repeatEach === undefined || part.repeatEach === 1);
  }
  for (const project of original.projects ?? []) {
    for (const key of ["testMatch", "testDir", "dependencies"]) assert.equal(project[key], undefined);
    for (const key of ["baseURL", "trace", "screenshot"]) assert.equal(project.use?.[key], undefined);
  }
  return {
    ...original,
    testDir: join(web, "e2e"),
    outputDir: join(output, "results"),
    reporter: [["list"], ["json", { outputFile: process.env.CMA_P7_REPORT }]],
    use: { ...original.use, baseURL: origin, trace: "retain-on-failure", screenshot: "only-on-failure" },
  };
}

function sourceIdentity(repository) {
  const git = (...args) => execFileSync("git", args, { cwd: repository, encoding: "utf8" }).trim();
  assert.equal(git("status", "--porcelain"), "", "Source must be clean");
  return { revision: git("rev-parse", "HEAD"), tree: git("rev-parse", "HEAD^{tree}") };
}

async function freePort() {
  const socket = createServer();
  await new Promise((done, reject) => {
    socket.once("error", reject);
    socket.listen(0, "127.0.0.1", done);
  });
  const port = socket.address().port;
  await new Promise((done) => socket.close(done));
  return port;
}

export async function run({ repository = root, env = process.env, signal } = {}) {
  validateArguments([], env);
  const temporary = env.RUNNER_TEMP ?? (await mkdtemp(join(tmpdir(), "cma-default-off-")));
  const output = resolve(temporary, "cma-p7-default-off");
  assert.ok(!output.startsWith(resolve(repository) + "/"), "Evidence must stay outside the repository");
  await mkdir(dirname(output), { recursive: true });
  await mkdir(output);
  const receipt = { output, flag: "unset", status: "failed", commands: [] };
  let preview;
  try {
    const source = sourceIdentity(repository);
    Object.assign(receipt, source);
    console.log(`default-off: checkout ${source.revision}; tree ${source.tree}`);
    const web = join(repository, "web");
    for (const file of [".env", ".env.local", ".env.production", ".env.production.local"]) {
      const present = await readFile(join(web, file)).then(
        () => true,
        (error) => {
          if (error.code === "ENOENT") return false;
          throw error;
        },
      );
      assert.ok(!present, `Default-off build cannot inherit ${file}`);
    }
    const childEnv = {
      ...env,
      CI: "true",
      CHRONO_BUILD_SHA: source.revision,
      CHRONO_BUILD: `git-${source.revision.slice(0, 12)}`,
      CHRONO_BUILD_ARTIFACT: "local",
      CMA_SOURCE_SHA: source.revision,
    };
    delete childEnv.VITE_HOME_TOPOGRAPHIC_FLOW_ENABLED;
    const directory = join(output, "preview");
    const build = ["run", "build", "--", "--outDir", join(directory, "dist"), "--emptyOutDir"];
    receipt.commands.push({ phase: "build", args: build });
    await command("npm", build, {
      cwd: web,
      env: childEnv,
      signal,
      log: join(output, "build.log"),
      timeout: 180_000,
    });
    receipt.build = await verifyArtifact(directory, source.revision);
    signal?.throwIfAborted();
    const port = await freePort();
    receipt.port = port;
    const { startPreview } = await import(pathToFileURL(join(web, "scripts/preview.mjs")).href);
    preview = await startPreview({ directory, port });
    signal?.throwIfAborted();
    const config = join(output, "config.mts");
    const original = pathToFileURL(join(web, "e2e/topographic-off.config.ts")).href;
    await writeFile(
      config,
      `import original from ${JSON.stringify(original)};\nimport {configure} from ${JSON.stringify(import.meta.url)};\nexport default configure(original,${JSON.stringify({ web, output, origin: preview.origin })});\n`,
    );
    receipt.configSha256 = hash(await readFile(config));
    receipt.runnerSha256 = hash(await readFile(fileURLToPath(import.meta.url)));
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
        env: { ...childEnv, CMA_P7_REPORT: join(output, `${phase}.json`) },
        signal,
        log: join(output, `${phase}.log`),
        timeout: 300_000,
      });
      preview.assertAlive();
    }
    receipt.cases = verifyResults(
      await readJson(join(output, "discovery.json")),
      await readJson(join(output, "execution.json")),
    );
    assert.deepEqual(sourceIdentity(repository), source);
    assert.equal(hash(await readFile(config)), receipt.configSha256);
    assert.deepEqual(await verifyArtifact(directory, source.revision), receipt.build);
    signal?.throwIfAborted();
    receipt.status = "passed";
  } catch (error) {
    receipt.error = error.message;
    throw error;
  } finally {
    await preview?.close();
    receipt.previewClosed = Boolean(preview);
    await writeFile(join(output, "receipt.json"), JSON.stringify(receipt, null, 2) + "\n");
    console.log(`default-off: ${receipt.status}; ${receipt.cases?.length ?? 0} executed; evidence ${output}`);
  }
  return receipt;
}

if (process.argv[1] && import.meta.url === pathToFileURL(realpathSync(process.argv[1])).href) {
  const controller = new AbortController();
  const cancel = (name) => {
    process.exitCode = name === "SIGINT" ? 130 : 143;
    controller.abort(new Error(`Cancelled by ${name}`));
  };
  const interrupt = () => cancel("SIGINT"),
    terminate = () => cancel("SIGTERM");
  process.once("SIGINT", interrupt);
  process.once("SIGTERM", terminate);
  try {
    validateArguments(process.argv.slice(2), process.env);
    await run({ signal: controller.signal });
  } catch (error) {
    console.error(error.message);
    process.exitCode ||= 1;
  } finally {
    process.removeListener("SIGINT", interrupt);
    process.removeListener("SIGTERM", terminate);
  }
}
