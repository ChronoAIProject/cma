const assert = require("node:assert/strict");
const { spawn, execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { createInterface } = require("node:readline");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { stop } = require("./native_tools_fixture.cjs");

function connect(program, directory, home, binary, frames) {
  const child = spawn(process.execPath, [program], {
    cwd: directory,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary },
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
  });
  child.stderr.resume();
  const pending = new Map();
  let serial = 0;
  const lines = createInterface({ input: child.stdout });
  lines.on("line", line => {
    const frame = JSON.parse(line);
    frames.push(frame);
    pending.get(frame.id)?.(frame);
  });
  return {
    async call(method, params) {
      const id = ++serial;
      let timer;
      const response = new Promise(resolve => pending.set(id, resolve));
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
      try {
        const frame = await Promise.race([response, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("Plan proof deadline: " + method)), 30000);
        })]);
        assert.equal(frame.error, undefined, JSON.stringify(frame.error));
        return frame.result;
      } finally { clearTimeout(timer); pending.delete(id); }
    },
    async close() { lines.close(); await stop(child); },
  };
}

test("pinned provider reports a real plan change and resumes without inventing completed steps", { timeout: 120000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  const provider = execFileSync(binary, ["--version"], { encoding: "utf8" }).trim();
  assert.match(provider, /^codex-cli 0\.(147|165)\.0$/);
  const bridgeSha256 = createHash("sha256").update(readFileSync(program)).digest("hex");
  assert(['84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c', '11c84d6c37f3b2e15020405a2301df3ed8c16ed2d9b05c3a3a43620bba78a7d4'].includes(bridgeSha256));
  const directory = mkdtempSync(join(tmpdir(), "cma-plan-proof-"));
  const home = join(directory, "agent-home");
  mkdirSync(home);
  const frames = [];
  let calls = 0;
  let modelError;
  const first = [
    { step: "Read the instructions", status: "in_progress" },
    { step: "Implement the change", status: "pending" },
    { step: "Verify the result", status: "pending" },
  ];
  const changed = [
    { step: "Read the instructions", status: "completed" },
    { step: "Implement the change", status: "in_progress" },
    { step: "Verify the result", status: "pending" },
  ];
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/responses");
      let body = "";
      for await (const chunk of request) body += chunk;
      const input = JSON.parse(body);
      calls++;
      const plan = calls === 1 ? first : calls === 2 ? changed : undefined;
      const declared = input.tools ?? input.input.filter(item => item.type === "additional_tools").flatMap(item => item.tools);
      const execution = declared.find(tool => tool.name === "functions")?.tools?.find(tool => tool.name === "exec");
      if (plan && execution) {
        const metadata = JSON.parse(input.client_metadata["x-codex-turn-metadata"]);
        if (metadata.code_mode_tool_names) assert(Object.hasOwn(metadata.code_mode_tool_names, "update_plan"), "native plan inventory: " + JSON.stringify(Object.keys(metadata.code_mode_tool_names)));
      } else if (plan) assert(declared.some(tool => tool.name === "update_plan"));
      const item = plan
        ? { type: "function_call", id: "fc_plan_" + calls, call_id: "plan_" + calls,
            ...(execution ? { type: "custom_tool_call", namespace: "functions", name: "exec",
              input: "const entry = ALL_TOOLS.find(t => t.name === 'update_plan'); if (!entry) throw new Error('Missing native plan tool'); text(await tools[entry.name](" + JSON.stringify({ plan }) + "));" }
              : { name: "update_plan", arguments: JSON.stringify({ plan }) }) }
        : { id: "msg_plan_" + calls, type: "message", role: "assistant", status: "completed",
            content: [{ type: "output_text", text: "The response ended with verification still pending.", annotations: [] }] };
      response.writeHead(200, { "content-type": "text/event-stream" });
      const send = event => response.write("data: " + JSON.stringify(event) + "\n\n");
      const id = "resp_plan_" + calls;
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 10, output_tokens: 3, total_tokens: 13 } } });
      response.end();
    } catch (error) {
      modelError = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "config.toml"), [
    'model = "gpt-5.6-sol"', 'model_provider = "cma_test"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.cma_test]', 'name = "Offline plan proof model boundary"',
    'base_url = "http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api = "responses"', 'requires_openai_auth = false',
    '[tools.update_plan]', 'enabled = true',
  ].join("\n"));
  let bridge;
  const open = async () => {
    bridge = connect(program, directory, home, binary, frames);
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
  };
  const plans = () => frames.filter(frame => frame.params?.update?.sessionUpdate === "plan");
  try {
    await open();
    const session = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    const result = await bridge.call("session/prompt", { sessionId: session.sessionId,
      prompt: [{ type: "text", text: "Report a three-step plan and update its progress." }] });
    if (modelError) throw modelError;
    assert.equal(result.stopReason, "end_turn");
    assert.equal(calls, 3);
    const reported = plans();
    const entries = plan => plan.map(step => ({ content: step.step, status: step.status, priority: "medium" }));
    assert.deepEqual(reported.map(frame => frame.params.update.entries), [entries(first), entries(changed)]);
    await bridge.close();
    await open();
    const reloadStart = frames.length;
    await bridge.call("session/load", { sessionId: session.sessionId, cwd: directory, mcpServers: [] });
    assert(frames.slice(reloadStart).some(frame => frame.params?.update?.sessionUpdate === "agent_message_chunk"));
    const beforeNextTurn = plans().length;
    await bridge.call("session/prompt", { sessionId: session.sessionId,
      prompt: [{ type: "text", text: "Reply without a plan." }] });
    if (modelError) throw modelError;
    assert.equal(plans().length, beforeNextTurn);
    if (process.env.CMA_PLAN_PROOF_OUTPUT) writeFileSync(process.env.CMA_PLAN_PROOF_OUTPUT,
      JSON.stringify({ provider, bridgeSha256, plans: reported.map(frame => frame.params),
        reloadPlanCount: beforeNextTurn - reported.length, noPlanTurn: true }, null, 2) + "\n");
  } finally {
    await bridge?.close();
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    rmSync(directory, { recursive: true, force: true });
  }
});
