const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, renameSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { agentInstructions, method, recording } = require("./agent_instructions.cjs");
const { method: steeringMethod, route: steeringRoute } = require("./turn_steering.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
async function modelFixture() {
  const calls = [];
  let mcpInitializations = 0;
  let error, hold, release;
  let arrived;
  const waiting = new Promise(resolve => { arrived = resolve; });
  const server = createServer(async (request, response) => {
    try {
      if (request.method === "GET" && request.url === "/mcp") {
        response.writeHead(405).end(); return;
      }
      assert.equal(request.method, "POST");
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        assert(body.length < 2 * 1024 * 1024);
      }
      const value = JSON.parse(body);
      if (request.url === "/mcp") {
        assert.equal(request.headers.authorization, "Bearer instructions-fixture");
        if (!Object.hasOwn(value, "id")) { response.writeHead(202).end(); return; }
        let result;
        if (value.method === "initialize") {
          mcpInitializations += 1;
          result = { protocolVersion: value.params.protocolVersion,
            serverInfo: { name: "instructions-fixture", version: "1" }, capabilities: { tools: {} } };
        } else if (value.method === "tools/list") {
          result = { tools: [{ name: "instructions_probe", description: "Verify retained instruction tools",
            inputSchema: { type: "object", properties: {}, additionalProperties: false } }] };
        } else assert.fail("Unexpected MCP request: " + value.method);
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify({ jsonrpc: "2.0", id: value.id, result })); return;
      }
      assert.equal(request.url, "/v1/responses");
      calls.push(value);
      if (hold) { arrived(); await new Promise(resolve => { release = resolve; }); }
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
      const id = "resp_settings_" + calls.length;
      const item = { id: "msg_settings_" + calls.length, type: "message", role: "assistant",
        status: "completed", content: [{ type: "output_text", text: "Settings fixture.", annotations: [] }] };
      const send = (event) => response.write("data: " + JSON.stringify(event) + "\n\n");
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (failure) {
      error = failure;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { calls, mcpInitializations: () => mcpInitializations,
    hold() { hold = true; return waiting; }, release() { hold = false; release(); }, origin: "http://127.0.0.1:" + server.address().port,
    check() { if (error) throw error; },
    async close() { server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); } };
}

function config(directory, provider, origin) {
  mkdirSync(directory);
  writeFileSync(join(directory, "config.toml"), [
    'model = "gpt-6"', 'model_reasoning_effort = "high"',
    'model_provider = "' + provider + '"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.' + provider + ']', 'name = "Offline settings fixture"',
    'base_url = "' + origin + '/v1"', 'wire_api = "responses"', 'requires_openai_auth = false',
  ].join("\n"));
}


function legacyInstructions(installed) {
    const oldMethod = method.replace(`    const mcpServers = session.cmaInstructionMcpServers;
    if (!Array.isArray(mcpServers))
      throw RequestError.invalidRequest("Reload the Agent session before updating instructions");
`, "").replace("additionalDirectories: session.additionalDirectories, mcpServers\n",
      "additionalDirectories: session.additionalDirectories, mcpServers: session.sessionMcpServers\n");
    const oldRecording = recording.replace(`    // ACP's sessionMcpServers contains names for display, not resume definitions.
    sessionState.cmaInstructionMcpServers = structuredClone(requestedMcpServers);
`, "");
    // Reconstruct the exact deployed legacy bytes, including removal of later
    // steering support. Retain the original hash as the recovery authority.
    const source = installed.replace(steeringMethod, "").replace(steeringRoute, "")
      .replace(method, oldMethod).replaceAll(recording, oldRecording);
    assert.equal(createHash("sha256").update(source).digest("hex"),
      "113762e08aaa46cbea8ec3710e334024d13f1cc026a1c264130d5e642e0748c4",
      "Recovery regression must use the exact previously deployed bridge bytes");
    return source;
}

test("instruction patch refuses unreviewed source", () => {
  assert.throws(() => agentInstructions("unknown"), /Unexpected ACP source/);
  assert.throws(() => legacyInstructions("unknown"), /exact previously deployed bridge bytes/);
});

for (const legacy of [false, true]) test(
  legacy ? "legacy instruction failure recovers through exact session load without close or new conversation"
    : "pinned native instructions change at idle without replacing the thread, process, files or history",
  { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.equal(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), "codex-cli 0.147.0");
  let program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  // Validate before opening the HTTP fixture so a source-pin assertion cannot
  // leave a listening server that holds the whole image build open.
  const source = legacy ? legacyInstructions(readFileSync(program, "utf8")) : null;
  const directory = mkdtempSync(join(tmpdir(), "cma-agent-instructions-test-"));
  const home = join(directory, "agent-home");
  const fixture = await modelFixture();
  config(home, "cma_test", fixture.origin);
  writeFileSync(join(directory, "dirty-work.txt"), "preserved user work");
  writeFileSync(join(home,"AGENTS.md"), "OLD_OWNER_INSTRUCTION_MARKER: be precise.");
  if (legacy) {
    program = join(directory, "legacy-bridge.mjs");
    writeFileSync(program, source);
  }
  const bridge = processRpc(process.execPath, [program], {
    cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary },
  });
  const mcpServers = [{ type: "http", name: "cma_access", url: fixture.origin + "/mcp",
    headers: [{ name: "Authorization", value: "Bearer instructions-fixture" }] }];
  try {
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call("session/new", { cwd: directory, mcpServers });
    const prompt = async () => {
      await bridge.call("session/prompt", { sessionId: session.sessionId,
        prompt: [{ type: "text", text: "Reply briefly without using tools." }] });
      fixture.check();
      return JSON.stringify(fixture.calls.at(-1));
    };
    const before = await prompt();
    assert(before.includes("OLD_OWNER_INSTRUCTION_MARKER"));
    assert(before.includes("instructions_probe"), "initial MCP definition is usable");
    assert.deepEqual(await bridge.call("cma/agent-instructions",{action:"support"}), {supported:true,version:1});
    const apply = async systemPrompt => {
      const request = { action:"apply", sessionId:session.sessionId, systemPrompt };
      try { return await bridge.call("cma/agent-instructions", request); }
      catch (error) {
        if (!legacy || error.actual?.code !== -32603 ||
            error.actual?.data?.details !== "Cannot read properties of undefined (reading 'replace')") throw error;
        assert.equal(readFileSync(join(home,"AGENTS.md"), "utf8"), systemPrompt);
        // Existing pending operations reconnect to an already-failed bridge.
        // A repeated original apply must still reach the same narrow recovery.
        await assert.rejects(bridge.call("cma/agent-instructions", request), retry =>
          retry.actual?.code === -32603 &&
          retry.actual?.data?.details === "Cannot read properties of undefined (reading 'replace')");
        const inventory = await bridge.call("session/list", {});
        const native = inventory.sessions.find(value => value.sessionId === session.sessionId);
        assert(native);
        await bridge.call("session/load", {sessionId:session.sessionId,cwd:native.cwd,
          ...(native.additionalDirectories ? {additionalDirectories:native.additionalDirectories} : {}), mcpServers});
        return await bridge.call("cma/agent-instructions", request);
      }
    };
    const updated = "NEW_OWNER_INSTRUCTION_MARKER: be concise.";
    assert.equal((await apply(updated)).sessionId, session.sessionId);
    const after = await prompt();
    assert(after.includes("NEW_OWNER_INSTRUCTION_MARKER"));
    assert(after.includes("OLD_OWNER_INSTRUCTION_MARKER"), "original conversation remains in history");
    assert(after.includes("instructions_probe"), "MCP definitions and headers survive instruction refresh");
    assert(fixture.mcpInitializations() >= 2, "instruction refresh reopens the authenticated MCP connection");
    assert.equal(readFileSync(join(directory, "dirty-work.txt"), "utf8"), "preserved user work");
    assert.equal(fixture.calls.length, 2);
    const arrived = fixture.hold();
    const active = prompt();
    await arrived;
    assert.equal((await apply(updated)).applied, true, "identical durable replay never reopens an active thread");
    await assert.rejects(apply("changed during a turn"), /operation refused/);
    assert.equal(readFileSync(join(home, "AGENTS.md"), "utf8"), updated);
    fixture.release(); await active;
    await assert.rejects(apply("x".repeat(32769)), /operation refused/);
    await assert.rejects(apply("invalid\0prompt"), /operation refused/);
    assert.equal(readFileSync(join(home, "AGENTS.md"), "utf8"), updated);
    await bridge.call("session/close", {sessionId:session.sessionId});
    await bridge.call("session/load", {sessionId:session.sessionId,cwd:directory,mcpServers});
    assert.equal((await apply(updated)).applied, true);
    assert((await prompt()).includes("NEW_OWNER_INSTRUCTION_MARKER"));
    assert.equal((await apply("")).applied, true);
    assert.equal(readFileSync(join(home, "AGENTS.md"), "utf8"), "");
  } finally {
    await bridge.close(); await fixture.close(); rmSync(directory, { recursive: true, force: true });
  }
});
