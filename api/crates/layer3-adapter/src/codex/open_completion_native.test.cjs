const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, readdirSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
const { withoutOpenCompletion, sourceSha256 } = require("./open_completion.cjs");
const canonical = value => value === null || typeof value !== "object" ? JSON.stringify(value)
  : Array.isArray(value) ? "[" + value.map(canonical).join(",") + "]"
  : "{" + Object.keys(value).sort().map(key => JSON.stringify(key) + ":" + canonical(value[key])).join(",") + "}";
const digest = params => "v1:" + createHash("sha256").update(canonical({
  cwd: params.cwd, mcpServers: params.mcpServers ?? [],
})).digest("hex");

async function localFixture() {
  const state = { modelCalls: [], mcpInitializations: [], requests: 0, error: null };
  const server = createServer(async (request, response) => {
    state.requests += 1;
    try {
      if (request.method === "GET") { response.writeHead(405).end(); return; }
      assert.equal(request.method, "POST");
      let body = "";
      for await (const chunk of request) { body += chunk; assert(body.length <= 2 * 1048576); }
      const value = JSON.parse(body);
      if (request.url.startsWith("/mcp")) {
        if (request.url === "/mcp-unavailable") { response.writeHead(503).end(); return; }
        const changed = request.url === "/mcp-changed";
        assert.equal(request.headers.authorization, request.url === "/mcp-provider" ? undefined
          : "Bearer " + (changed ? "changed-open-fixture" : "original-open-fixture"));
        if (!Object.hasOwn(value, "id")) { response.writeHead(202).end(); return; }
        let result;
        if (value.method === "initialize") {
          state.mcpInitializations.push(request.url);
          result = { protocolVersion: value.params.protocolVersion,
            serverInfo: { name: "open-completion-fixture", version: "1" }, capabilities: { tools: {} } };
        } else if (value.method === "tools/list") {
          result = { tools: [{ name: changed ? "changed_open_probe" : "original_open_probe",
            description: "Offline open completion probe", inputSchema: { type: "object", properties: {} } }] };
        } else assert.fail("Unexpected MCP method");
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify({ jsonrpc: "2.0", id: value.id, result })); return;
      }
      assert.equal(request.url, "/v1/responses");
      state.modelCalls.push(value);
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
      const id = "resp_open_" + state.modelCalls.length;
      const item = { id: "msg_open_" + state.modelCalls.length, type: "message", role: "assistant",
        status: "completed", content: [{ type: "output_text", text: "Open completion fixture response.", annotations: [] }] };
      const send = event => response.write("data: " + JSON.stringify(event) + "\n\n");
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
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
    async close() { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); } };
}

async function fixture(family = "cma_access") {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  assert.equal(createHash("sha256").update(withoutOpenCompletion(readFileSync(program, "utf8"))).digest("hex"), sourceSha256);
  const directory = mkdtempSync(join(tmpdir(), "cma-open-completion-test-"));
  const home = join(directory, "agent-home"); mkdirSync(home);
  const server = await localFixture();
  writeFileSync(join(home, "config.toml"), [
    'model = "gpt-6"', 'model_reasoning_effort = "high"', 'model_provider = "cma_test"',
    'web_search = "disabled"', 'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.cma_test]', 'name = "Offline open completion fixture"',
    'base_url = "' + server.origin + '/v1"', 'wire_api = "responses"', 'requires_openai_auth = false',
  ].join("\n"));
  writeFileSync(join(directory, "dirty-work.txt"), "preserved user work");
  const params = { cwd: directory, mcpServers: [{ type: "http", name: family,
    url: server.origin + (family === "cmaeg" ? "/mcp-provider" : "/mcp"),
    headers: family === "cmaeg" ? [] : [{ name: "Authorization", value: "Bearer original-open-fixture" }] }] };
  const bridges = [];
  async function connect(nativePath = binary) {
    const bridge = processRpc(process.execPath, [program], {
      cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: nativePath },
    });
    bridges.push(bridge);
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    return bridge;
  }
  const read = (bridge, sessionId, binding = params, challenge = "native_fixture_challenge_1234") =>
    bridge.call("cma/open-completion", { action: "read", sessionId, bindingDigest: digest(binding), challenge });
  const prompt = (bridge, sessionId) => bridge.call("session/prompt", { sessionId,
    prompt: [{ type: "text", text: "Remember OPEN_COMPLETION_HISTORY_MARKER. Reply briefly without using tools." }] });
  // Native plugin downloads create/remove private .tmp directories concurrently.
  // Conversation persistence assertions read only the provider's history tree.
  const history = () => Object.fromEntries(readdirSync(join(home, "sessions"), { recursive: true, withFileTypes: true })
    .filter(entry => entry.isFile() && entry.name.endsWith(".jsonl"))
    .map(entry => { const path = join(entry.parentPath, entry.name); return [path, readFileSync(path, "utf8")]; }));
  return { binary, program, directory, home, params, server, connect, read, prompt, history,
    async close() {
      await Promise.all(bridges.map(bridge => bridge.close()));
      await server.close(); rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
      if (server.state.error) throw server.state.error;
    } };
}

for (const family of ["cma_access", "cmaeg", "cma_channel"]) test(
  "pinned native " + family + " alone proves new, cached load/resume, and cold setup", { timeout: 120000 }, async () => {
  const f = await fixture(family);
  try {
    const bridge = await f.connect();
    assert.deepEqual(await bridge.call("cma/open-completion", { action: "support" }), { supported: true, version: 1, current: true });
    assert.equal((await f.read(bridge, "unobserved-thread")).complete, false);
    await assert.rejects(bridge.call("cma/open-completion", { action: "read", sessionId: "x", challenge: "short" }), /operation refused/);
    const { sessionId } = await bridge.call("session/new", f.params);
    const initial = await f.read(bridge, sessionId);
    assert.equal(initial.complete, true);
    await f.prompt(bridge, sessionId);
    for (const method of ["session/load", "session/resume"]) {
      await bridge.call(method, { ...f.params, sessionId });
      const receipt = await f.read(bridge, sessionId);
      assert.equal(receipt.complete, true);
      assert.equal(receipt.processNonce, initial.processNonce);
      assert.notEqual(receipt.openGeneration, initial.openGeneration);
    }
    const historyBefore = f.history();
    assert(Object.values(historyBefore).some(value => value.includes("OPEN_COMPLETION_HISTORY_MARKER")));
    const requestsBefore = f.server.state.requests;
    const readOne = await f.read(bridge, sessionId);
    for (let index = 0; index < 25; index++) assert.deepEqual(await f.read(bridge, sessionId), readOne);
    assert.equal(f.server.state.requests, requestsBefore, "receipt reads contact neither MCP nor model");
    assert.deepEqual(f.history(), historyBefore, "receipt reads do not rewrite history");
    assert.equal(f.server.state.modelCalls.length, 1);
    assert.equal(readFileSync(join(f.directory, "dirty-work.txt"), "utf8"), "preserved user work");
    assert(!JSON.stringify(initial).includes(f.server.origin));
    assert(!JSON.stringify(initial).includes("Bearer"));
    await bridge.call("session/close", { sessionId });
    assert.equal((await f.read(bridge, sessionId)).complete, false);
    await bridge.close();
    for (const method of ["session/load", "session/resume"]) {
      const restarted = await f.connect();
      await restarted.call(method, { ...f.params, sessionId });
      const receipt = await f.read(restarted, sessionId);
      assert.equal(receipt.complete, true, method + " observes empty native inventory before cold setup");
      assert.notEqual(receipt.processNonce, initial.processNonce);
      assert.equal(f.server.state.modelCalls.length, 1, "cold setup never starts a turn");
      assert.equal(readFileSync(join(f.directory, "dirty-work.txt"), "utf8"), "preserved user work");
      assert(Object.values(f.history()).some(value => value.includes("OPEN_COMPLETION_HISTORY_MARKER")));
      await restarted.close();
    }
  } finally { await f.close(); }
});

for (const family of ["cma_access", "cmaeg"]) test(
  "pinned native cached " + family + " resume ignores changed descriptors without matching proof", { timeout: 60000 }, async () => {
  const f = await fixture(family);
  try {
    const bridge = await f.connect();
    const { sessionId } = await bridge.call("session/new", f.params);
    assert.equal((await f.read(bridge, sessionId)).complete, true);
    await f.prompt(bridge, sessionId);
    const changed = structuredClone(f.params);
    changed.mcpServers[0].url = f.server.origin + "/mcp-changed";
    changed.mcpServers[0].headers = [{ name: "Authorization", value: "Bearer changed-open-fixture" }];
    await bridge.call("session/load", { ...changed, sessionId });
    assert.equal((await f.read(bridge, sessionId, changed)).complete, false);
    assert.equal((await f.read(bridge, sessionId)).complete, false);
    await f.prompt(bridge, sessionId);
    const nativeRequest = JSON.stringify(f.server.state.modelCalls.at(-1));
    assert(nativeRequest.includes("original_open_probe"), "native retains its previous configured server");
    assert(!nativeRequest.includes("changed_open_probe"));
    assert(!f.server.state.mcpInitializations.includes("/mcp-changed"));
    await bridge.call("session/resume", { ...f.params, sessionId });
    assert.equal((await f.read(bridge, sessionId)).complete, false, "lost prior proof is not reconstructed from supplied descriptors");
  } finally { await f.close(); }
});

test("pinned native unknown loaded state stays unprovable and optional MCP failure is not catalog proof", { timeout: 60000 }, async () => {
  const f = await fixture();
  try {
    const bridge = await f.connect();
    const unknown = await bridge.call("session/new", { ...f.params, mcpServers: [] });
    await bridge.call("session/load", { ...f.params, sessionId: unknown.sessionId });
    assert.equal((await f.read(bridge, unknown.sessionId)).complete, false);
    const optional = structuredClone(f.params);
    optional.mcpServers.push({ type: "http", name: "cmaeg", url: f.server.origin + "/mcp-unavailable", headers: [] });
    const created = await bridge.call("session/new", optional);
    assert.equal((await f.read(bridge, created.sessionId, optional)).complete, true,
      "successful ACP setup does not assert optional cmaeg tool availability");
    const providerOnly = { ...f.params, mcpServers: [optional.mcpServers[1]] };
    const provider = await bridge.call("session/new", providerOnly);
    assert.equal((await f.read(bridge, provider.sessionId, providerOnly)).complete, true,
      "provider-only setup remains optional and does not require an access binding");
  } finally { await f.close(); }
});

test("pinned native signal exit retires the local receipt without discovery or recovery", { timeout: 60000 }, async () => {
  const f = await fixture();
  try {
    const quote = value => "'" + value.replaceAll("'", "'\\''") + "'";
    const pidFile = join(f.directory, "native.pid");
    const launcher = join(f.directory, "native-launcher");
    writeFileSync(launcher, '#!/bin/sh\nprintf "%s" "$$" > ' + quote(pidFile) + '\nexec ' + quote(f.binary) + ' "$@"\n', { mode: 0o700 });
    const bridge = await f.connect(launcher);
    const { sessionId } = await bridge.call("session/new", f.params);
    assert.equal((await f.read(bridge, sessionId)).complete, true);
    const pid = Number(readFileSync(pidFile, "utf8")); assert(Number.isSafeInteger(pid) && pid > 1);
    process.kill(pid, "SIGKILL");
    let receipt;
    for (let index = 0; index < 20; index++) {
      receipt = await f.read(bridge, sessionId);
      if (!receipt.complete) break;
      await delay(25);
    }
    assert.equal(receipt.complete, false);
    assert.equal(f.server.state.modelCalls.length, 0);
  } finally { await f.close(); }
});
