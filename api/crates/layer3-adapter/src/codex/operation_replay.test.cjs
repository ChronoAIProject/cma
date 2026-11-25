const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, writeFileSync, readFileSync, existsSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { processRpc } = require("./native_tools_fixture.cjs");

test("four pinned native operations remain four after public replay and browser reload", { timeout: 1800000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY;
  const program = process.env.CMA_ACP_TEST_BRIDGE;
  const capture = process.env.CMA_REPLAY_NATIVE_FILE;
  const release = process.env.CMA_REPLAY_RELEASE_FILE;
  assert(binary && program && capture && release);
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-operation-replay-"));
  const agentHome = join(directory, "agent");
  mkdirSync(agentHome);
  const counter = join(directory, "execution-count");
  const updates = [];
  let calls = 0;
  let failure;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/responses");
      let body = "";
      for await (const chunk of request) body += chunk;
      const value = JSON.parse(body);
      calls += 1;
      assert(calls <= 5, "Replay must not admit another model call");
      let item;
      if (calls <= 4) {
        const declared = value.tools ?? value.input.filter((entry) => entry.type === "additional_tools").flatMap((entry) => entry.tools);
        const command = "printf 'executed\\n' >> " + JSON.stringify(counter) + "; printf 'native-replay-" + calls + "\\n'";
        const exec = declared.find((tool) => tool.name === "functions")?.tools?.find((tool) => tool.name === "exec");
        if (exec) {
          item = { type: "custom_tool_call", name: "exec", namespace: "functions", input: "text(await tools.exec_command(" + JSON.stringify({ cmd: command }) + "));" };
        } else {
          const tool = declared.find((tool) => tool.name === "exec_command");
          assert(tool);
          item = { type: "function_call", name: tool.name, arguments: JSON.stringify({ cmd: command }) };
        }
        item = { ...item, id: "fc_replay_" + calls, call_id: "call_replay_" + calls, status: "completed" };
      } else {
        item = { id: "msg_replay", type: "message", role: "assistant", status: "completed", content: [{ type: "output_text", text: "Four operations completed.", annotations: [] }] };
      }
      response.writeHead(200, { "content-type": "text/event-stream" });
      const send = (event) => response.write("data: " + JSON.stringify(event) + "\n\n");
      const id = "resp_replay_" + calls;
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item], usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (error) {
      failure = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(agentHome, "config.toml"), [
    'model = "gpt-5.6-sol"', 'model_provider = "cma_test"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.cma_test]', 'name = "Isolated operation replay proof"',
    'base_url = "http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api = "responses"', 'requires_openai_auth = false',
  ].join("\n"));
  const bridge = processRpc(process.execPath, [program], {
    cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: agentHome, CODEX_PATH: binary },
  }, (frame) => { if (frame.params?.update) updates.push(frame.params.update); });
  try {
    await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const session = await bridge.call("session/new", { cwd: directory, mcpServers: [] });
    await bridge.call("session/set_mode", { sessionId: session.sessionId, modeId: "agent-full-access" });
    await bridge.call("session/prompt", { sessionId: session.sessionId, prompt: [{ type: "text", text: "Execute the four harmless commands and finish." }] });
    if (failure) throw failure;
    for (let number = 1; number <= 4; number += 1) {
      const completed = updates.find((update) => update.rawOutput?.formatted_output === "native-replay-" + number + "\n");
      assert(completed, "Native stdout must report operation " + number);
      assert.equal(completed.status, "completed");
      assert.equal(completed.rawOutput.exit_code, 0);
    }
    assert.equal(readFileSync(counter, "utf8"), "executed\n".repeat(4));
    writeFileSync(capture, JSON.stringify({ updates, execution_count: 4, model_calls: calls }, null, 2));
    console.log("Four actual executions captured; provider remains alive during public/UI replay.");
    await new Promise((resolve, reject) => {
      const started = Date.now();
      const timer = setInterval(() => {
        if (existsSync(release)) { clearInterval(timer); resolve(); }
        else if (Date.now() - started > 1700000) { clearInterval(timer); reject(new Error("Browser proof did not release native counter check")); }
      }, 250);
    });
    assert.equal(readFileSync(counter, "utf8"), "executed\n".repeat(4));
    assert.equal(calls, 5);
    writeFileSync(capture + ".verified.json", JSON.stringify({ execution_count_before: 4, execution_count_after: 4, model_calls_after: calls }));
  } finally {
    await bridge.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true });
  }
});
