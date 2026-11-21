const assert = require("node:assert/strict");
const { spawn, execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const {
  mkdir,
  readFile,
  readdir,
  rename,
  unlink,
  writeFile,
} = require("node:fs/promises");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");
const {
  stop,
} = require("../../../layer3-adapter/src/codex/native_tools_fixture.cjs");

async function main() {
  const directory = process.env.CMA_DESKTOP_RELAY_DIRECTORY;
  const binary = process.env.CMA_RUNTIME_TEST_BINARY;
  assert(directory && binary);
  assert.equal(
    execFileSync(process.env.CMA_CODEX_TEST_BINARY, ["--version"], {
      encoding: "utf8",
    }).trim(),
    "codex-cli 0.147.0",
  );
  const digest = async (file) =>
    createHash("sha256")
      .update(await readFile(file))
      .digest("hex");
  assert.equal(
    await digest(binary),
    "6283b369b5bedcd8c8d742786f061cce45fbcbb3ead18a2241db473286f624e3",
  );
  assert.equal(
    await digest(process.env.CMA_ACP_TEST_BRIDGE),
    "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c",
  );
  const atomic = async (name, value) => {
    await writeFile(join(directory, name + ".tmp"), JSON.stringify(value));
    await rename(join(directory, name + ".tmp"), join(directory, name));
  };
  const model = await require("./integrated-model.cjs").start(
    directory,
    atomic,
  );
  const origin = "http://127.0.0.1:19419";
  const sockets = new Map();
  let runtime;
  let provision;
  let runtimeRequests = 0;
  const runtimeTrace = [];
  let diagnostic = "";
  const handle = async (request) => {
    if (request.kind === "provision") {
      if (provision) {
        assert.deepEqual(request.body, provision);
        return { ready: true };
      }
      provision = request.body;
      assert.equal(provision.image.uri, process.env.CMA_DESKTOP_RUNTIME_IMAGE);
      await mkdir(provision.env.CHRONO_RUNTIME_HOME, { recursive: true });
      runtime = spawn(binary, [], {
        detached: true,
        stdio: ["ignore", "pipe", "pipe"],
        env: {
          ...provision.env,
          PATH: process.env.PATH,
          TMPDIR: "/tmp",
          CHRONO_RUNTIME_BIND: "127.0.0.1:19419",
          CHRONO_RUNTIME_STATE_ROOT: "/tmp/integrated-runtime-state",
        },
      });
      const log = (bytes) => {
        diagnostic = (diagnostic + bytes).slice(-24000);
      };
      runtime.stdout.on("data", log);
      runtime.stderr.on("data", log);
      for (let attempt = 0; ; attempt += 1) {
        try {
          assert((await fetch(origin + "/ready")).ok);
          break;
        } catch {
          assert(attempt < 100, diagnostic);
          await delay(100);
        }
      }
      await atomic("provisioned.json", {
        sandbox_id: provision.env.CHRONO_SANDBOX_ID,
        image: provision.image.uri,
        runtime_generation: provision.env.CHRONO_RUNTIME_GENERATION,
        resource_limits: provision.resourceLimits,
        runtime_sha256: await digest(binary),
      });
      return { ready: true };
    }
    if (request.kind === "http") {
      const started = Date.now();
      const modelRequest = request.path.startsWith("/proof-model/");
      assert(
        runtime || modelRequest,
        "Formal provisioning must precede runtime access",
      );
      const url = modelRequest
        ? "http://127.0.0.1:16416" + request.path.slice("/proof-model".length)
        : origin + request.path;
      const response = await fetch(url, {
        method: request.method,
        headers: request.headers,
        body:
          request.body === undefined
            ? undefined
            : Buffer.from(request.body, "base64"),
        signal: AbortSignal.timeout(60000),
        redirect: "error",
      });
      const body = Buffer.from(await response.arrayBuffer());
      if (!modelRequest) runtimeRequests += 1;
      runtimeTrace.push({
        path: request.path,
        method: request.method,
        status: response.status,
        started,
        elapsedMs: Date.now() - started,
      });
      await atomic("runtime-http-trace.json", runtimeTrace);
      await atomic("runtime-diagnostic.json", { diagnostic });
      return {
        status: response.status,
        headers: Object.fromEntries(response.headers),
        body: body.toString("base64"),
      };
    }
    if (request.kind === "socket_open") {
      const socket = new WebSocket(
        origin.replace("http:", "ws:") + request.path,
      );
      socket.binaryType = "arraybuffer";
      let sequence = 0;
      let writes = Promise.resolve();
      socket.addEventListener("message", (event) => {
        const packet = {
          binary: typeof event.data !== "string",
          data: Buffer.from(event.data).toString("base64"),
        };
        const name =
          request.id +
          "." +
          String(++sequence).padStart(8, "0") +
          ".packet.json";
        writes = writes.then(() => atomic(name, packet));
      });
      await new Promise((resolve, reject) => {
        socket.addEventListener("open", resolve, { once: true });
        socket.addEventListener("error", reject, { once: true });
      });
      sockets.set(request.id, socket);
      return { ready: true };
    }
    if (request.kind === "socket_send") {
      const data = Buffer.from(request.data, "base64");
      sockets.get(request.id).send(request.binary ? data : data.toString());
      return {};
    }
    if (request.kind === "socket_close") {
      sockets.get(request.id)?.close();
      return {};
    }
    throw new Error("Unknown exact private transport request");
  };
  try {
    await atomic("native-ready.json", {
      provider: "0.147.0",
      node: process.version,
      runtime_sha256: await digest(binary),
    });
    const deadline = Date.now() + 480000;
    while (!(await readdir(directory)).includes("stop.json")) {
      assert(Date.now() < deadline, "Integrated proof deadline: " + diagnostic);
      for (const file of (await readdir(directory))
        .filter((name) => /^[a-f0-9-]+\.request\.json$/.test(name))
        .sort()) {
        const request = JSON.parse(
          await readFile(join(directory, file), "utf8"),
        );
        await unlink(join(directory, file));
        let result;
        try {
          result = await handle(request);
        } catch (error) {
          console.error(error, diagnostic);
          result = { error: String(error) };
        }
        await atomic(file.replace(".request.json", ".response.json"), result);
      }
      await delay(15);
    }
    await model.verify();
    const dispatches = (
      await readFile("/home/cma/physical-dispatches.txt", "utf8")
    )
      .trim()
      .split("\n");
    assert.deepEqual(
      dispatches,
      process.env.CMA_INTEGRATED_EXTENDED === "1"
        ? ["first", "second", "third"]
        : ["first", "second"],
    );
    await atomic("native-result.json", {
      passed: true,
      ...model.progress,
      runtimeRequests,
      dispatches,
      sandbox_id: provision.env.CHRONO_SANDBOX_ID,
      runtime_sha256: await digest(binary),
    });
  } finally {
    await atomic("runtime-diagnostic.json", { diagnostic });
    for (const socket of sockets.values()) socket.close();
    if (runtime) await stop(runtime);
    model.server.closeAllConnections();
    await new Promise((resolve) => model.server.close(resolve));
  }
}
main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
