const assert = require("node:assert/strict");
const { spawn } = require("node:child_process");
const { createInterface } = require("node:readline");
const { createServer } = require("node:http");

const names = ["readEventContext", "replyToEvent", "sendTyping", "updateMessage"];

function processRpc(program, args, options, observe = () => {}, timeoutMs = 30000) {
  const child = spawn(program, args, { ...options, detached: true, stdio: ["pipe", "pipe", "pipe"] });
  child.stderr.resume();
  const pending = new Map();
  const lines = createInterface({ input: child.stdout });
  let serial = 0;
  lines.on("line", (line) => {
    const frame = JSON.parse(line);
    observe(frame);
    if (frame.method === "session/request_permission") {
      const option = frame.params.options.find((value) => value.kind === "allow_once");
      assert(option, "fixture permission request must offer allow_once");
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: frame.id,
        result: { outcome: { outcome: "selected", optionId: option.optionId } } }) + "\n");
    } else if (pending.has(frame.id)) pending.get(frame.id)(frame);
  });
  return {
    pid: child.pid,
    async call(method, params) {
      const id = ++serial;
      let timer;
      const response = new Promise((resolve) => pending.set(id, resolve));
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
      try {
        const frame = await Promise.race([response, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("Native tools deadline: " + method)), timeoutMs);
        })]);
        assert.equal(frame.error, undefined, "Native tools operation refused: " + method);
        return frame.result;
      } finally { clearTimeout(timer); pending.delete(id); }
    },
    notify(method, params) {
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n");
    },
    close() { lines.close(); return stop(child); },
  };
}

async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  const exited = new Promise((resolve) => child.once("exit", resolve));
  try { process.kill(-child.pid, "SIGTERM"); } catch {}
  const force = setTimeout(() => { try { process.kill(-child.pid, "SIGKILL"); } catch {} }, 1500);
  try { await exited; } finally { clearTimeout(force); }
}

async function fixture() {
  const state = { expectedCredential: null, calls: [], modelCalls: 0, round: 0, error: null };
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.method, "POST");
      let body = "";
      for await (const chunk of request) { body += chunk; assert(body.length <= 1048576); }
      const value = JSON.parse(body);
      if (request.url === "/mcp") {
        assert.equal(request.headers.authorization, "Bearer " + state.expectedCredential);
        response.setHeader("content-type", "application/json");
        if (!Object.hasOwn(value, "id")) { response.writeHead(202).end(); return; }
        let result;
        switch (value.method) {
          case "initialize": result = { protocolVersion: value.params.protocolVersion,
            serverInfo: { name: "cmaeg-fixture", version: "1" }, capabilities: { tools: {} } }; break;
          case "tools/list": result = { tools: names.map((name) => ({ name,
            description: "Offline fixture: " + name,
            inputSchema: { type: "object", properties: {}, additionalProperties: false },
            annotations: { readOnlyHint: true, destructiveHint: false, openWorldHint: false },
          })) }; break;
          case "tools/call":
            assert(names.includes(value.params.name));
            state.calls.push({ name: value.params.name, credential: state.expectedCredential });
            result = { content: [{ type: "text", text: JSON.stringify({ fixture: value.params.name }) }] }; break;
          default: assert.fail("unexpected MCP method: " + value.method);
        }
        response.end(JSON.stringify({ jsonrpc: "2.0", id: value.id, result }));
        return;
      }
      assert.equal(request.url, "/v1/responses");
      const declared = value.tools ?? value.input.filter((i) => i.type === "additional_tools").flatMap((i) => i.tools);
      state.modelCalls += 1;
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-store" });
      const send = (event) => response.write("data: " + JSON.stringify(event) + "\n\n");
      const id = "resp_fixture_" + state.modelCalls;
      const toolRound = state.round++ === 0;
      const output = toolRound ? names.map((name, i) => {
        const expected = "mcp__cmaeg__" + name;
        const exec = declared.find((t) => t.name === "functions")?.tools?.find((t) => t.name === "exec");
        if (exec) {
          const metadata = JSON.parse(value.client_metadata["x-codex-turn-metadata"]);
          // Actual ALL_TOOLS lookup and execution below remain mandatory when
          // newer CLI versions omit the optional diagnostic metadata.
          if (metadata.code_mode_tool_names) assert(Object.hasOwn(metadata.code_mode_tool_names, expected), "exact nested tool name missing: " + expected);
        } else {
          assert(declared.some((tool) => tool.name === expected), "exact tool name missing: " + expected);
        }
        return { id: "fc_" + state.modelCalls + "_" + i,
          call_id: "call_" + state.modelCalls + "_" + i, status: "completed",
          ...(exec ? { type: "custom_tool_call", name: "exec", namespace: "functions",
            input: "const entry = ALL_TOOLS.find(t => t.name === " + JSON.stringify(expected) + "); if (!entry) throw new Error('Missing native tool inventory'); text(await tools[entry.name]({}));" }
            : { type: "function_call", name: expected, arguments: "{}" }) };
      }) : [{ id: "msg_fixture_" + state.modelCalls, type: "message", role: "assistant", status: "completed",
        content: [{ type: "output_text", text: "Offline tools verified.", annotations: [] }] }];
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      output.forEach((item, output_index) => {
        send({ type: "response.output_item.added", output_index, item: { ...item, status: "in_progress" } });
        send({ type: "response.output_item.done", output_index, item });
      });
      send({ type: "response.completed", response: { id, status: "completed", output,
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (error) {
      state.error = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return { state, origin: "http://127.0.0.1:" + server.address().port,
    async close() { server.closeAllConnections(); await new Promise((resolve) => server.close(resolve)); } };
}

module.exports = { names, processRpc, stop, fixture };
