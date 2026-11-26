const assert = require("node:assert/strict");
const { openSync, writeSync, fsyncSync, closeSync, readFileSync } = require("node:fs");
const { createServer } = require("node:http");

function completed(response, number, output) {
  response.writeHead(200, { "content-type": "text/event-stream" });
  const id = "resp_credit_" + number;
  const send = event => response.write("data: " + JSON.stringify(event) + "\n\n");
  send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
  output.forEach((item, output_index) => {
    send({ type: "response.output_item.added", output_index,
      item: { ...item, status: "in_progress" } });
    send({ type: "response.output_item.done", output_index, item });
  });
  send({ type: "response.completed", response: { id, status: "completed", output,
    usage: { input_tokens: 2, output_tokens: 1, total_tokens: 3 } } });
  response.end();
}

function effectCall(request, number) {
  const declared = request.tools ?? request.input
    .filter(item => item.type === "additional_tools").flatMap(item => item.tools);
  const exact = "mcp__credit_fixture__record_effect";
  const host = declared.find(tool => tool.name === "functions")?.tools
    ?.find(tool => tool.name === "exec");
  const direct = declared.some(tool => tool.name === exact);
  assert(host || direct || declared.find(tool => tool.name === "mcp__credit_fixture")?.tools
    ?.some(tool => tool.name === "record_effect"), "durable effect tool must be available");
  const args = { action: "original-business-action" };
  return { id: "fc_effect_" + number, call_id: "call_effect_" + number, status: "completed",
    ...(host ? { type: "custom_tool_call", name: "exec", namespace: "functions",
      input: "const entry = ALL_TOOLS.find(t => t.name === " + JSON.stringify(exact) +
        "); if (!entry) throw new Error('Missing effect tool'); text(await tools[entry.name](" +
        JSON.stringify(args) + "));" }
      : { type: "function_call", name: direct ? exact : "record_effect",
        ...(direct ? {} : { namespace: "mcp__credit_fixture" }), arguments: JSON.stringify(args) }),
  };
}

async function creditFixture(effectPath) {
  const state = { funded: false, fundedRequests: 0, requests: [], error: null };
  const effects = () => readFileSync(effectPath, "utf8").trim().split("\n").filter(Boolean);
  const server = createServer(async (request, response) => {
    try {
      if (request.method === "GET") { response.writeHead(405).end(); return; }
      assert.equal(request.method, "POST");
      let body = "";
      for await (const part of request) {
        body += part;
        assert(body.length < 2 * 1024 * 1024, "bounded fixture body");
      }
      const value = JSON.parse(body);
      if (request.url === "/mcp") {
        if (!Object.hasOwn(value, "id")) { response.writeHead(202).end(); return; }
        let result;
        switch (value.method) {
          case "initialize":
            result = { protocolVersion: value.params.protocolVersion,
              serverInfo: { name: "durable-credit-fixture", version: "1" },
              capabilities: { tools: {} } };
            break;
          case "tools/list":
            result = { tools: [{ name: "record_effect", description: "Record a synthetic effect",
              inputSchema: { type: "object", properties: { action: { type: "string" } },
                required: ["action"], additionalProperties: false },
              annotations: { readOnlyHint: false, destructiveHint: false, idempotentHint: false },
            }] };
            break;
          case "tools/call": {
            assert.equal(value.params.name, "record_effect");
            assert.equal(value.params.arguments.action, "original-business-action");
            const file = openSync(effectPath, "a");
            try {
              writeSync(file, value.params.arguments.action + "\n");
              fsyncSync(file);
            } finally { closeSync(file); }
            result = { content: [{ type: "text", text: "Durable effect receipt " + effects().length }] };
            break;
          }
          default: assert.fail("unexpected MCP method: " + value.method);
        }
        response.writeHead(200, { "content-type": "application/json" });
        response.end(JSON.stringify({ jsonrpc: "2.0", id: value.id, result }));
        return;
      }
      assert.equal(request.url, "/v1/responses");
      state.requests.push(value);
      const number = state.requests.length;
      if (number === 1 || (state.funded && state.fundedRequests++ === 0)) {
        completed(response, number, [effectCall(value, number)]);
      } else if (!state.funded) {
        response.writeHead(402, { "content-type": "application/json" });
        response.end(JSON.stringify({ error: "insufficient_credits", error_code: 11300,
          message: "Synthetic model funding is exhausted." }));
      } else {
        completed(response, number, [{ id: "msg_credit_" + number, type: "message",
          role: "assistant", status: "completed", content: [{ type: "output_text",
            text: "The explicit new turn completed.", annotations: [] }] }]);
      }
    } catch (error) {
      state.error = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { state, effects, origin: "http://127.0.0.1:" + server.address().port,
    async close() {
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    } };
}

module.exports = { creditFixture };
