const assert = require("node:assert/strict");
const { test } = require("node:test");
const { spawn, execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { createServer } = require("node:http");
const { createInterface } = require("node:readline");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { join } = require("node:path");
const { tmpdir } = require("node:os");
const { bridgePath } = require("./persist_conversations.cjs");
const { stop } = require("./native_tools_fixture.cjs");
const { withoutOpenCompletion, sourceSha256 } = require("./open_completion.cjs");

function rpc(program, binary, directory, home, observe = () => {}) {
  const child = spawn(process.execPath, [program], {
    cwd: directory, detached: true, stdio: ["pipe", "pipe", "pipe"],
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary },
  });
  const pending = new Map();
  const lines = createInterface({ input: child.stdout });
  child.stderr.resume();
  lines.on("line", line => {
    const value = JSON.parse(line);
    observe(value);
    pending.get(value.id)?.(value);
  });
  let id = 0;
  return {
    async frame(method, params) {
      const key = ++id;
      let timer;
      try {
        return await new Promise((resolve, reject) => {
          timer = setTimeout(() => reject(new Error("Rejoin probe timed out: " + method)), 10000);
          pending.set(key, resolve);
          child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: key, method, params }) + "\n");
        });
      } finally { clearTimeout(timer); pending.delete(key); }
    },
    async close() { lines.close(); await stop(child); },
  };
}

async function fixture() {
  const state = { gate: null, entered: null, deny: false, modelCalls: 0, error: null, message: "Retained thread ready." };
  const server = createServer(async (request, response) => {
    try {
      if (request.method !== "POST") { response.writeHead(405).end(); return; }
      let body = "";
      for await (const chunk of request) { body += chunk; assert(body.length <= 2 * 1048576); }
      const value = JSON.parse(body);
      if (request.url === "/mcp") {
        if (!Object.hasOwn(value, "id")) { response.writeHead(202).end(); return; }
        if (value.method === "initialize" && state.gate) {
          state.entered();
          await state.gate;
        }
        if (state.deny) { response.writeHead(503).end(); return; }
        const result = value.method === "initialize"
          ? { protocolVersion: value.params.protocolVersion,
              serverInfo: { name: "rejoin-fixture", version: "1" }, capabilities: { tools: {} } }
          : { tools: [] };
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify({ jsonrpc: "2.0", id: value.id, result }));
        return;
      }
      assert.equal(request.url, "/v1/responses");
      const id = "resp_rejoin_" + ++state.modelCalls;
      const item = { id: "msg_rejoin_" + state.modelCalls, type: "message", role: "assistant",
        status: "completed", content: [{ type: "output_text", text: state.message,
          annotations: [] }] };
      response.writeHead(200, { "content-type": "text/event-stream" });
      const send = event => response.write("data: " + JSON.stringify(event) + "\n\n");
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0,
        item: { ...item, status: "in_progress", content: [] } });
      send({ type: "response.content_part.added", output_index: 0, content_index: 0,
        part: { type: "output_text", text: "", annotations: [] } });
      send({ type: "response.output_text.delta", output_index: 0, content_index: 0,
        item_id: item.id, delta: state.message });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (error) {
      state.error = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { state, origin: "http://127.0.0.1:" + server.address().port,
    async close() {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
      if (state.error) throw state.error;
    } };
}

// Run explicitly by node --test in api/crates/layer4-runtime/Dockerfile.
test("retained load permits concurrent initialize and rejects prompts until installed", {
  timeout: 60000,
}, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }), /^codex-cli 0\.(147|165)\.0/);
  const source = withoutOpenCompletion(readFileSync(program, "utf8"));
  assert.equal(createHash("sha256").update(source).digest("hex"), sourceSha256);
  const directory = mkdtempSync(join(tmpdir(), "cma-rejoin-test-"));
  const home = join(directory, "agent-home"); mkdirSync(home);
  const f = await fixture();
  let bridge, release = () => {};
  try {
    writeFileSync(join(home, "config.toml"), [
      'model="gpt-6"', 'model_provider="rejoin"', 'approval_policy="never"',
      'sandbox_mode="danger-full-access"', '[model_providers.rejoin]',
      'name="Offline rejoin fixture"', 'base_url="' + f.origin + '/v1"',
      'wire_api="responses"', 'requires_openai_auth=false',
    ].join("\n"));
    bridge = rpc(program, binary, directory, home);
    const call = async (method, params) => {
      const frame = await bridge.frame(method, params);
      assert.equal(frame.error, undefined, method + " refused: " + JSON.stringify(frame.error));
      return frame.result;
    };
    const initialize = { protocolVersion: 1, clientCapabilities: {} };
    await call("initialize", initialize);
    const params = { cwd: directory, mcpServers: [{ type: "http", name: "cma_access",
      url: f.origin + "/mcp", headers: [] }] };
    const { sessionId } = await call("session/new", params);
    const prompt = { sessionId, prompt: [{ type: "text", text: "Reply briefly without tools." }] };
    await call("session/prompt", prompt);
    await call("session/close", { sessionId });
    f.state.gate = new Promise(resolve => { release = resolve; });
    const entered = new Promise(resolve => { f.state.entered = resolve; });
    let settled = false;
    const loading = bridge.frame("session/load", { ...params, sessionId })
      .then(frame => { settled = true; return frame; });
    await entered;
    const rejoin = await bridge.frame("initialize", initialize);
    assert.deepEqual(rejoin.error, { code: -32603, message: "Internal error",
      data: { details: "Already initialized" } });
    assert.equal(settled, false, "initialize must not be mistaken for load completion");
    const missing = async (method, params) => {
      const frame = await bridge.frame(method, params);
      assert.equal(frame.error?.code, -32603);
      assert.equal(frame.error?.data?.details, "Session " + sessionId + " not found");
      assert.equal(frame.result, undefined);
    };
    await missing("session/set_mode", { sessionId, modeId: "agent-full-access" });
    await missing("session/prompt", prompt);
    assert.equal(f.state.modelCalls, 1, "a missing session must not dispatch the prompt");
    f.state.deny = true; release();
    assert.equal((await loading).error?.code, -32603);
    await missing("session/prompt", prompt);
    assert.equal(f.state.modelCalls, 1);
    f.state.gate = null; f.state.deny = false;
    await call("session/load", { ...params, sessionId });
    assert.equal((await call("session/prompt", prompt)).stopReason, "end_turn");
    assert.equal(f.state.modelCalls, 2, "explicit reload recovers the same retained thread");
  } finally {
    release(); await bridge?.close(); await f.close();
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});

test("the upgraded producer reopens 0.147.0 history and the older producer can reopen it again", {
  timeout: 90000,
  skip: !process.env.CMA_CODEX_TEST_LEGACY_BINARY,
}, async () => {
  const previous = process.env.CMA_CODEX_TEST_LEGACY_BINARY;
  const candidate = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  assert.equal(execFileSync(previous, ["--version"], { encoding: "utf8" }).trim(), "codex-cli 0.147.0");
  assert.equal(execFileSync(candidate, ["--version"], { encoding: "utf8" }).trim(), "codex-cli 0.165.0");
  const directory = mkdtempSync(join(tmpdir(), "cma-provider-history-"));
  const agentHome = join(directory, "agent-home");
  mkdirSync(agentHome);
  const f = await fixture();
  const observed = [];
  let bridge;
  try {
    writeFileSync(join(agentHome, "config.toml"), [
      'model="gpt-6"', 'model_provider="rejoin"', 'approval_policy="never"',
      '[model_providers.rejoin]', 'name="Offline upgrade fixture"',
      'base_url="' + f.origin + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false',
    ].join("\n"));
    const params = { cwd: directory, mcpServers: [] };
    const call = async (method, request) => {
      const frame = await bridge.frame(method, request);
      assert.equal(frame.error, undefined, method + " refused");
      return frame.result;
    };
    const start = async binary => {
      if (bridge) await bridge.close();
      observed.length = 0;
      bridge = rpc(program, binary, directory, agentHome, value => observed.push(value));
      await call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    };
    const hasMessage = text => observed.some(value =>
      value.method === "session/update" && value.params.update?.content?.text === text);
    const prompt = (sessionId, text) => call("session/prompt", {
      sessionId, prompt: [{ type: "text", text }],
    });
    await start(previous);
    const { sessionId } = await call("session/new", params);
    f.state.message = "Legacy response.";
    assert.equal((await prompt(sessionId, "First answer.")).stopReason, "end_turn");
    assert(hasMessage(f.state.message));
    await call("session/close", { sessionId });

    await start(candidate);
    await call("session/load", { ...params, sessionId });
    assert(hasMessage("Legacy response."));
    assert.equal(f.state.modelCalls, 1);
    f.state.message = "Upgraded response.";
    assert.equal((await prompt(sessionId, "Second answer.")).stopReason, "end_turn");
    assert(hasMessage(f.state.message));
    await call("session/close", { sessionId });

    await start(previous);
    await call("session/load", { ...params, sessionId });
    assert(hasMessage("Legacy response."));
    assert(hasMessage("Upgraded response."));
    assert.equal(f.state.modelCalls, 2);
    f.state.message = "Rollback response.";
    assert.equal((await prompt(sessionId, "Third answer.")).stopReason, "end_turn");
    assert(hasMessage(f.state.message));
    assert.equal(f.state.modelCalls, 3);
    await call("session/close", { sessionId });
  } finally {
    if (bridge) await bridge.close();
    await f.close();
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
