const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, writeFileSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { processRpc } = require("./native_tools_fixture.cjs");

test("pinned native command reports actual output through ACP", { timeout: 60000 }, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY;
  const program = process.env.CMA_ACP_TEST_BRIDGE;
  assert(binary && program, "Pinned native Codex and ACP bridge paths are required");
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-computer-output-"));
  const agentHome = join(directory, "agent");
  mkdirSync(agentHome);
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
      assert(calls <= 3, "The controlled model must complete the command turn");
      const declared = value.tools ?? value.input.filter((item) => item.type === "additional_tools").flatMap((item) => item.tools);
      const command = "printf 'native-computer-proof\\n'";
      let item;
      if (calls === 1) {
        const exec = declared.find((tool) => tool.name === "functions")?.tools?.find((tool) => tool.name === "exec");
        if (exec) {
          const metadata = JSON.parse(value.client_metadata["x-codex-turn-metadata"]);
          if (metadata.code_mode_tool_names) assert(Object.hasOwn(metadata.code_mode_tool_names, "exec_command"), "Native exec_command must be declared");
          item = { type: "custom_tool_call", name: "exec", namespace: "functions",
            input: "const entry = ALL_TOOLS.find(t => t.name === 'exec_command'); if (!entry) throw new Error('Missing native command tool'); text(await tools[entry.name](" + JSON.stringify({ cmd: command }) + "));" };
        } else {
          const tool = declared.find((tool) => tool.name === "exec_command");
          assert(tool, "Native command tool must be declared");
          item = { type: "function_call", name: tool.name, arguments: JSON.stringify({ cmd: command }) };
        }
        item = { ...item, id: "fc_computer", call_id: "call_computer", status: "completed" };
      } else {
        item = { id: "msg_computer", type: "message", role: "assistant", status: "completed",
          content: [{ type: "output_text", text: "Command finished.", annotations: [] }] };
      }
      response.writeHead(200, { "content-type": "text/event-stream" });
      const send = (event) => response.write("data: " + JSON.stringify(event) + "\n\n");
      const id = "resp_computer_" + calls;
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
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
    '[model_providers.cma_test]', 'name = "Isolated command proof"',
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
    await bridge.call("session/prompt", { sessionId: session.sessionId,
      prompt: [{ type: "text", text: "Run the harmless proof command and report its output." }] });
    if (failure) throw failure;
    const completed = updates.find((update) => update.rawOutput?.formatted_output === "native-computer-proof\n");
    assert(completed, "ACP must report actual native stdout: " + JSON.stringify(updates));
    assert.equal(completed.status, "completed");
    assert.equal(completed.rawOutput.exit_code, 0);
    assert(updates.some((update) => update.sessionUpdate === "tool_call" && update.kind === "execute" && update.toolCallId === completed.toolCallId));
    if (process.env.CMA_OUTPUT_PROOF_FILE) writeFileSync(process.env.CMA_OUTPUT_PROOF_FILE, JSON.stringify(updates, null, 2));
  } finally {
    await bridge.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
