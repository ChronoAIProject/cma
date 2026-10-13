const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const { createHash, randomBytes } = require("node:crypto");
const { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");
const { test } = require("node:test");
const { bridgePath, persistConversations } = require("./persist_conversations.cjs");
const { subscriptionAuth } = require("./subscription_auth.cjs");
const { managedModelCatalog } = require("./managed_model_catalog.cjs");
const { names, processRpc, stop, fixture } = require("./native_tools_fixture.cjs");

test("pinned Codex keeps exact native tool names through live key rotation and conversation reload", { timeout: 120000 }, async () => {
  const directory = mkdtempSync(join(tmpdir(), "cma-native-tools-"));
  const agentHome = join(directory, "agent");
  const runtimeHome = join(directory, "runtime");
  mkdirSync(agentHome, { mode: 0o700 });
  mkdirSync(runtimeHome, { mode: 0o700 });
  const server = await fixture();
  let runtime;
  let bridge;
  const controlOrigin = "http://127.0.0.1:19090";
  const credential = () => randomBytes(32).toString("hex");
  const control = async (frame) => {
    const response = await fetch(controlOrigin + "/runtime/control", {
      method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(frame),
      signal: AbortSignal.timeout(10000), redirect: "error",
    });
    assert.equal(response.status, 200);
    return response.json();
  };
  let program = bridgePath;
  if (process.env.CMA_ACP_UPSTREAM_SOURCE) {
    program = join(directory, "bridge.mjs");
    writeFileSync(program, managedModelCatalog(subscriptionAuth(persistConversations(readFileSync(process.env.CMA_ACP_UPSTREAM_SOURCE, "utf8")))));
  }
  const connect = async () => {
    bridge = processRpc(process.execPath, [program], {
      cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: agentHome,
        CODEX_PATH: process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex" },
    });
    const hello = await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    assert.equal(hello.agentCapabilities.mcpCapabilities.http, true);
  };
  const tools = [{ type: "http", name: "cmaeg", url: "http://127.0.0.1:9093/", headers: [] }];
  try {
    writeFileSync(join(agentHome, "config.toml"), [
      'model = "gpt-5.6-sol"', 'model_provider = "cma_test"', 'web_search = "disabled"',
      '[model_providers.cma_test]', 'name = "Offline native tools fixture"',
      'base_url = "' + server.origin + '/v1"', 'wire_api = "responses"', 'requires_openai_auth = false',
    ].join("\n"));
    runtime = spawn(process.env.CMA_RUNTIME_TEST_BINARY ?? "/usr/local/bin/layer4-runtime", [], {
      cwd: directory, detached: true, stdio: ["ignore", "ignore", "ignore"],
      env: { PATH: process.env.PATH, TMPDIR: directory, CHRONO_RUNTIME_HOME: runtimeHome,
        CHRONO_RUNTIME_STATE_ROOT: join(directory, "state"), CHRONO_SANDBOX_ID: "native-tools-fixture",
        CHRONO_RUNTIME_GENERATION: "1", CHRONO_RUNTIME_BIND: "127.0.0.1:19090",
        CHRONO_TOOL_PROXY_URL: server.origin + "/mcp" },
    });
    for (let attempt = 0; ; attempt++) {
      try { const response = await fetch(controlOrigin + "/ready", { signal: AbortSignal.timeout(1000) }); assert.equal(response.status, 200); break; }
      catch { assert(attempt < 40, "runtime did not start"); await delay(100); }
    }
    const parts = [Buffer.from("chrono.install.request.v2\0")];
    for (const value of [30, 60]) { const bytes = Buffer.alloc(8); bytes.writeBigUInt64BE(BigInt(value)); parts.push(bytes); }
    for (const value of [4096, 0]) { const bytes = Buffer.alloc(4); bytes.writeUInt32BE(value); parts.push(bytes); }
    const digest = createHash("sha256").update(Buffer.concat(parts)).digest("hex");
    assert.equal((await control({ t: "install", operation_id: "native-tools-install", request_sha256: digest, commands: [], timeout_s: 30, total_timeout_s: 60, output_limit: 4096 })).t, "install");
    // Legacy runtime scratch belongs to this fixture and must leave the image
    // with its cleanup, including the durable installation receipt.
    const installed = JSON.parse(readFileSync(join(directory, "cma-checkpoints/runtime-state/install-v2.json"), "utf8"));
    assert.equal(installed.sandbox_id, "native-tools-fixture");
    assert.equal(installed.state, "completed");
    const deliver = async (generation) => {
      server.state.expectedCredential = credential();
      assert.equal((await control({ t: "credential", operation_id: "native-tools-key-" + generation,
        value: server.state.expectedCredential, custody_generation: generation })).installed, true);
      assert.equal((await control({ t: "tool_proxy_status", sandbox_id: "native-tools-fixture", generation: 1,
        endpoint: server.origin + "/mcp", credential_generation: generation })).t, "tool_proxy_status");
    };
    await deliver(1);
    await connect();
    const created = await bridge.call("session/new", { cwd: directory, mcpServers: tools });
    assert.equal(typeof created.sessionId, "string");
    for (let phase = 0; phase < 3; phase++) {
      if (phase === 1) await deliver(2);
      if (phase === 2) {
        await bridge.close(); await connect();
        await bridge.call("session/load", { sessionId: created.sessionId, cwd: directory, mcpServers: tools });
      }
      server.state.round = 0;
      const before = server.state.calls.length;
      await bridge.call("session/prompt", { sessionId: created.sessionId,
        prompt: [{ type: "text", text: "Execute the offline native tool fixture." }] });
      if (server.state.error) throw server.state.error;
      const calls = server.state.calls.slice(before);
      assert.deepEqual(calls.map((call) => call.name).sort(), names);
      assert(calls.every((call) => call.credential === server.state.expectedCredential));
    }
  } finally {
    await bridge?.close(); await stop(runtime); await server.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
