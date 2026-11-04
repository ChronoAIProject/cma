const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { setTimeout: delay } = require("node:timers/promises");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
const { controlledBinary, control } = require("./native_control_fixture.cjs");
const { turnSteering } = require("./turn_steering.cjs");

test("steering patch rejects unreviewed provider bytes", () => {
  assert.throws(() => turnSteering("unknown"), /Unexpected ACP source/);
});

test("native long-running turn remains cancellable after repeated steering without spawning a new turn", { timeout: 45000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-steering-interrupt-"));
  const home = join(directory, "home"); mkdirSync(home);
  let requests = 0;
  // Never complete model output: cancellation must work while real native work
  // is blocked, independent of elapsed turn time or a fabricated tool result.
  const server = createServer(async (request, response) => {
    for await (const ignored of request) { void ignored; }
    requests++;
    response.writeHead(200, { "content-type": "text/event-stream" });
    response.write("data: " + JSON.stringify({ type: "response.created", response: { id: "resp_wait_" + requests, status: "in_progress", output: [] } }) + "\n\n");
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "config.toml"), ['model="gpt-6"','model_provider="cma"','web_search="disabled"','approval_policy="never"','sandbox_mode="danger-full-access"','[model_providers.cma]','name="Offline cancellation fixture"','base_url="http://127.0.0.1:' + server.address().port + '/v1"','wire_api="responses"','requires_openai_auth=false'].join("\n"));
  const bridge = processRpc(process.execPath, [process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath], { cwd: directory,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: controlledBinary(directory, binary) } });
  let prompt;
  try {
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId } = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    const support = await bridge.call("cma/steer", { action: "support" });
    assert.deepEqual(support, { supported: true, version: 1 });
    let ended = false;
    prompt = bridge.call("session/prompt", { sessionId, prompt: [{ type: "text", text: "Keep the offline fixture active." }] });
    void prompt.then(() => { ended = true; }, () => { ended = true; });
    for (let n = 0; requests === 0; n++) { assert(n < 100, "native model request did not start"); await delay(50); }
    for (let n = 0; n < 2; n++) {
      assert.deepEqual(await bridge.call("cma/steer", { sessionId, prompt: [{ type: "text", text: "Steering instruction " + n }] }), { injected: true, sessionId });
      assert.equal(ended, false, "steering must not complete the prompt");
    }
    const before = await control(directory, "thread/read", { threadId: sessionId, includeTurns: true });
    assert.equal(before.thread.turns.length, 1, "steering must keep exactly one native turn");
    const started = Date.now();
    bridge.notify("session/cancel", { sessionId });
    assert.equal((await prompt).stopReason, "cancelled");
    assert(Date.now() - started < 5000, "cancellation must not wait for model output");
    const after = await control(directory, "thread/read", { threadId: sessionId, includeTurns: true });
    assert.equal(after.thread.turns.length, 1);
    assert.equal(after.thread.turns[0].id, before.thread.turns[0].id);
    assert.equal(after.thread.turns[0].status, "interrupted");
    const count = requests;
    await assert.rejects(bridge.call("cma/steer", { sessionId, prompt: [{ type: "text", text: "Must not start another turn." }] }), /operation refused/);
    const idle = await control(directory, "thread/read", { threadId: sessionId, includeTurns: true });
    assert.equal(idle.thread.turns.length, 1, "late steering must not start a replacement turn");
    assert.equal(requests, count);
    assert.equal(readFileSync(join(home, "config.toml"), "utf8").includes("Offline cancellation fixture"), true);
  } finally {
    await bridge.close(); await prompt?.catch(() => {});
    server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
