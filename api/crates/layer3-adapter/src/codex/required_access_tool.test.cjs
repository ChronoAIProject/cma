const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { createHash, randomBytes } = require("node:crypto");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");
const { test } = require("node:test");
const { runInNewContext } = require("node:vm");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
const { requiredAccessTool, sourceSha256, before, after } = require("./required_access_tool.cjs");

function sources() {
  const installed = readFileSync(process.env.CMA_ACP_ACCESS_BASE_SOURCE ?? bridgePath, "utf8");
  const base = installed.includes(after) ? installed.replace(after, before) : installed;
  const digest = (source) => createHash("sha256").update(source).digest("hex");
  const compositions = {
    "11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4": "27d888d0fc2c2a7ae6e2b5fc0c6695a3b765528373abf247fba322b39701cae7",
    "ddb625f66a5a14d94df3512a525c4a674a359e564602c09085769c8b61e2bca1": "87b4f06de7c6e4da1eb818b591697acab63c0722735c36994bdb7fe1c9247273",
    e26bce27dbdcf4d665e081b4a81762542795e2af6453da2a2f050fab5110f484: "9fbf59d5c94c2c6d47912e6f9ab024b92b538e78a35c16985e2ad66e0cc8fdae",
    "8f0c023269496f9ad21aa2958491772fefe65011b9aebaf674c67d2aaa9fbcf4": "51781c3913a3679f8333525bbff8723c86949a9a01dcfa35cd6daa6110d413aa",
    "fc9f74358c6ce5aeda4ac164c1daf83c513fb0f18df4b75f3d20bd0816fc6fa7": "8c09bddfed33f2cb07766bf438f8be83cc4023ef8e2804660b7a03c9bff284e3",
    "ac50122023f18cec80c4ef543dfbcffe8029de4206cb2692c2f5df51341c2c66": "92c1131392cf39f1cd9bd0af9404f32ba416db509d27eb53b5b6190f504e6f85",
    "186537078bb1a13292c927a7da1290f345fd76c7918025fb603a5c1eb89c8203": "09f179f1e1681b87df06fbf73a66922f62e3c8f56ef79524f4a1298063e09034",
    "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c": "b0d4c534287a41824d94b87ea0f3dcf629f9052c03705238624dd48d80d88301",
    "4e3fbc3fb4850f57019f4ba955193363c25c1c044445f165e96d7a65dcf64888": "6aabe4f5724f0183015b2cd7bfdc6dbbca1cd3d604ba759c30476e512316876d",
    b2676ef9a4a57a5bedf15c0b576b96f2c67c45e53880f08b4b8ba80f2f8ce8d9: "87d08828c723db16ada3e7cd56a075373d7d01837af389bd5906c1ff7c3187c8",
    "113762e08aaa46cbea8ec3710e334024d13f1cc026a1c264130d5e642e0748c4": "f2dc53b3c73f3844cb596ac5e7ee42e4057f49b06c82449ef43f3713a9b57bb5",
  };
  if (Object.hasOwn(compositions, digest(installed))) {
    // Exercise the final image composition while removing only this patch for
    // the native before/after comparison. The session command patch is retained.
    assert.equal(installed.split(after).length, 2);
    assert.equal(digest(base), compositions[digest(installed)]);
    assert.equal(base.replace(before, after), installed);
    return { base, patched: installed };
  }
  assert.equal(digest(base), sourceSha256);
  return { base, patched: requiredAccessTool(base) };
}

test("only the injected CMA access server is required, with pinned source validation", () => {
  const { patched } = sources();
  assert.throws(() => requiredAccessTool("unreviewed"), /Unexpected ACP source/);
  assert.throws(() => requiredAccessTool(patched), /Unexpected ACP source/);
  const start = patched.indexOf("  createMcpSeverConfig(mcpServer) {");
  const end = patched.indexOf("  /**", start);
  const Client = runInNewContext("(class {" + patched.slice(start, end) + "})");
  const client = new Client();
  for (const name of ["cma_access", "cmaeg", "owner_server"]) {
    const config = client.createMcpSeverConfig({ name, type: "http", url: "https://example.invalid/mcp", headers: [] });
    assert.equal(config.required, name === "cma_access" ? true : undefined);
    assert.equal(config.url, "https://example.invalid/mcp");
  }
});

async function fixture() {
  const capability = randomBytes(32).toString("hex");
  const state = { requests: [], calls: [], modelCalls: 0, failure: null, deny: false };
  let release, initialized;
  const reset = () => {
    state.ready = false;
    state.started = new Promise((resolve) => { initialized = resolve; });
    state.gate = new Promise((resolve) => { release = resolve; });
  };
  reset();
  const tool = "mcp__cma_access__agent_key_scope_revision";
  const proposal = { request_key: "missing-github-once", services: ["github"], reason: "Read the owner-selected repository" };
  const server = createServer(async (request, response) => {
    try {
      if (request.method !== "POST") { response.writeHead(405).end(); return; }
      let raw = "";
      for await (const chunk of request) { raw += chunk; assert(raw.length <= 2 * 1024 * 1024); }
      const value = JSON.parse(raw);
      if (request.url === "/api/v1/agent-tools") {
        assert.equal(request.headers.authorization, "Bearer " + capability);
        state.requests.push(value.method);
        response.setHeader("content-type", "application/json");
        if (value.method === "initialize") { initialized(); await state.gate; }
        if (state.deny) { response.writeHead(401).end(JSON.stringify({ error: "Capability refused" })); return; }
        assert([undefined, "2025-06-18"].includes(request.headers["mcp-protocol-version"]));
        if (value.id === undefined) { response.writeHead(202).end(); return; }
        let result;
        switch (value.method) {
          case "initialize": result = { protocolVersion: "2025-06-18", serverInfo: { name: "cma-agent-access", version: "1" }, capabilities: { tools: { listChanged: false } } }; break;
          case "tools/list":
            state.ready = true;
            result = { tools: [{ name: "agent_key_scope_revision", description: "Request owner consent for missing service access",
              inputSchema: { type: "object", properties: { request_key: { type: "string" }, services: { type: "array", items: { type: "string" } }, reason: { type: "string" } }, required: ["request_key", "services", "reason"] },
              annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: true, openWorldHint: false } }] }; break;
          case "tools/call":
            assert.equal(value.params.name, "agent_key_scope_revision");
            assert.deepEqual(value.params.arguments, proposal);
            state.calls.push(value.params.arguments);
            result = { content: [{ type: "text", text: "Owner permission request recorded; wait for CMA confirmation." }] }; break;
          default: assert.fail("Unexpected MCP request: " + value.method);
        }
        response.end(JSON.stringify({ jsonrpc: "2.0", id: value.id, result }));
        return;
      }
      assert.equal(request.url, "/v1/responses");
      assert(state.ready, "The model must not run before the required access tool is ready");
      state.modelCalls += 1;
      const declared = value.tools ?? value.input.filter((item) => item.type === "additional_tools").flatMap((item) => item.tools);
      const exec = declared.find((item) => item.name === "functions")?.tools?.find((item) => item.name === "exec");
      const invoke = state.calls.length === 0;
      if (invoke) {
        if (exec) {
          const metadata = JSON.parse(value.client_metadata["x-codex-turn-metadata"]);
          // Some CLI versions omit this diagnostic metadata. The fixture still
          // looks up and executes the exact tool through ALL_TOOLS below.
          if (metadata.code_mode_tool_names) assert(Object.hasOwn(metadata.code_mode_tool_names, tool), "Access tool missing from the first model request");
        } else assert(declared.some((item) => item.name === tool), "Access tool missing from the first model request");
      }
      const id = "resp_access_" + state.modelCalls;
      const item = invoke ? { id: "fc_access", call_id: "call_access", status: "completed",
        ...(exec ? { type: "custom_tool_call", name: "exec", namespace: "functions",
          input: "const entry = ALL_TOOLS.find(t => t.name === " + JSON.stringify(tool) + "); text(await tools[entry.name](" + JSON.stringify(proposal) + "));" }
          : { type: "function_call", name: tool, arguments: JSON.stringify(proposal) }) }
        : { id: "msg_access", type: "message", role: "assistant", status: "completed", content: [{ type: "output_text", text: "Waiting for owner approval.", annotations: [] }] };
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
      const send = (event) => response.write("data: " + JSON.stringify(event) + "\n\n");
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item], usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (error) {
      state.failure = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const origin = "http://127.0.0.1:" + server.address().port;
  return { state, reset, release: () => release(), origin,
    tools: [{ type: "http", name: "cma_access", url: origin + "/api/v1/agent-tools", headers: [{ name: "Authorization", value: "Bearer " + capability }] }],
    async close() { release(); server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); } };
}

test("pinned Codex waits for access discovery on new and prepared conversations, and refuses failed setup", { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-required-access-"));
  const { base, patched } = sources();
  let bridge;
  const server = await fixture();
  let serial = 0;
  async function connect(source) {
    const home = join(directory, "agent-" + serial++);
    mkdirSync(home);
    writeFileSync(join(home, "config.toml"), [
      'model = "gpt-5.6-sol"', 'model_provider = "fixture"', 'web_search = "disabled"',
      'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
      '[model_providers.fixture]', 'name = "Offline access fixture"',
      'base_url = "' + server.origin + '/v1"', 'wire_api = "responses"', 'requires_openai_auth = false',
    ].join("\n"));
    const program = join(home, "bridge.mjs"); writeFileSync(program, source);
    bridge = processRpc(process.execPath, [program], { cwd: directory,
      env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary } });
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
  }
  try {
    // Demonstrate the defect against the unmodified pinned bridge: it declares
    // the conversation open while the access capability is still unavailable.
    await connect(base);
    await bridge.call("session/new", { cwd: directory, mcpServers: server.tools });
    assert.equal(server.state.ready, false);
    server.release(); await bridge.close();
    server.reset(); await connect(patched);
    let opened = false;
    const opening = bridge.call("session/new", { cwd: directory, mcpServers: server.tools }).then((result) => { opened = true; return result; });
    await server.state.started; await delay(50);
    assert.equal(opened, false, "Required discovery must hold new conversation readiness");
    server.release(); const created = await opening;
    assert.equal(server.state.ready, true);
    await bridge.call("session/close", { sessionId: created.sessionId });
    server.reset(); opened = false;
    const loading = bridge.call("session/load", { sessionId: created.sessionId, cwd: directory, mcpServers: server.tools }).then((result) => { opened = true; return result; });
    await server.state.started; await delay(50);
    assert.equal(opened, false, "Prepared conversation rebind must wait for discovery");
    server.release(); await loading;
    await bridge.call("session/prompt", { sessionId: created.sessionId, prompt: [{ type: "text", text: "Request GitHub access using the owner consent tool." }] })
      .catch(error => { throw server.state.failure ?? error; });
    if (server.state.failure) throw server.state.failure;
    assert.equal(server.state.calls.length, 1);
    await bridge.close();
    server.reset(); server.state.deny = true; server.release();
    await connect(patched);
    await assert.rejects(bridge.call("session/new", { cwd: directory, mcpServers: server.tools }), /Native tools operation refused: session\/new/);
  } finally {
    server.release(); await bridge?.close(); await server.close();
    rmSync(directory, { recursive: true, force: true });
  }
});
