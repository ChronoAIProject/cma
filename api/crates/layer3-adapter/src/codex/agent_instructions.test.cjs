const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, renameSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { agentInstructions } = require("./agent_instructions.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
async function modelFixture() {
  const calls = [];
  let error, hold, release;
  let arrived;
  const waiting = new Promise(resolve => { arrived = resolve; });
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.method, "POST");
      assert.equal(request.url, "/v1/responses");
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        assert(body.length < 2 * 1024 * 1024);
      }
      calls.push(JSON.parse(body));
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
  return { calls, hold() { hold = true; return waiting; }, release() { hold = false; release(); }, origin: "http://127.0.0.1:" + server.address().port,
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


test("instruction patch refuses unreviewed source", () => { assert.throws(() => agentInstructions("unknown"), /Unexpected ACP source/); });

test("pinned native instructions change at idle without replacing the thread, process, files or history", { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.equal(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), "codex-cli 0.147.0");
  const directory = mkdtempSync(join(tmpdir(), "cma-agent-instructions-test-"));
  const home = join(directory, "agent-home");
  const fixture = await modelFixture();
  config(home, "cma_test", fixture.origin);
  writeFileSync(join(directory, "dirty-work.txt"), "preserved user work");
  writeFileSync(join(home,"AGENTS.md"), "OLD_OWNER_INSTRUCTION_MARKER: be precise.");
  const bridge = processRpc(process.execPath, [process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath], {
    cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary },
  });
  try {
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    const prompt = async () => {
      await bridge.call("session/prompt", { sessionId: session.sessionId,
        prompt: [{ type: "text", text: "Reply briefly without using tools." }] });
      fixture.check();
      return JSON.stringify(fixture.calls.at(-1));
    };
    const before = await prompt();
    assert(before.includes("OLD_OWNER_INSTRUCTION_MARKER"));
    assert.deepEqual(await bridge.call("cma/agent-instructions",{action:"support"}), {supported:true,version:1});
    const apply = systemPrompt => bridge.call("cma/agent-instructions", { action:"apply", sessionId:session.sessionId, systemPrompt });
    const updated = "NEW_OWNER_INSTRUCTION_MARKER: be concise.";
    assert.equal((await apply(updated)).sessionId, session.sessionId);
    const after = await prompt();
    assert(after.includes("NEW_OWNER_INSTRUCTION_MARKER"));
    assert(after.includes("OLD_OWNER_INSTRUCTION_MARKER"), "original conversation remains in history");
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
    await bridge.call("session/load", {sessionId:session.sessionId,cwd:directory,mcpServers:[]});
    assert.equal((await apply(updated)).applied, true);
    assert((await prompt()).includes("NEW_OWNER_INSTRUCTION_MARKER"));
    assert.equal((await apply("")).applied, true);
    assert.equal(readFileSync(join(home, "AGENTS.md"), "utf8"), "");
  } finally {
    await bridge.close(); await fixture.close(); rmSync(directory, { recursive: true, force: true });
  }
});
