const assert = require("node:assert/strict");
const { spawn, execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { createServer } = require("node:http");
const { mkdirSync, readFileSync, writeFileSync } = require("node:fs");
const { join } = require("node:path");
const { createInterface } = require("node:readline");
const {
  stop,
} = require("../../layer3-adapter/src/codex/native_tools_fixture.cjs");

async function prove() {
  const [origin, directory] = process.argv.slice(2);
  assert.equal(new URL(origin).hostname, "127.0.0.1");
  const binary = process.env.CMA_CODEX_TEST_BINARY;
  const program = process.env.CMA_ACP_TEST_BRIDGE;
  const bridgeHash = createHash("sha256")
    .update(readFileSync(program))
    .digest("hex");
  assert.equal(
    bridgeHash,
    "84f9670793e2848b6c49e0541a325223239eeabd0e406d9a586dbcd2935cb85c",
  );
  const version = execFileSync(binary, ["--version"], {
    encoding: "utf8",
  }).trim();
  assert.equal(version, "codex-cli 0.147.0");
  const markers = new Set();
  let round = 0;
  let failure;
  const model = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/responses");
      assert.equal(request.method, "POST");
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        assert(body.length < 2 * 1024 * 1024);
      }
      const value = JSON.parse(body);
      for (const marker of JSON.stringify(value.input).match(
        /<!--cma-deliverable:v1:[A-Za-z0-9_-]+-->/g,
      ) ?? [])
        markers.add(marker);
      const declared =
        value.tools ??
        value.input
          .filter((item) => item.type === "additional_tools")
          .flatMap((item) => item.tools);
      const execution = declared
        .find((tool) => tool.name === "functions")
        ?.tools?.find((tool) => tool.name === "exec");
      const metadata = JSON.parse(
        value.client_metadata?.["x-codex-turn-metadata"] ?? "{}",
      );
      const names = execution
        ? Object.keys(metadata.code_mode_tool_names ?? {})
        : declared
            .flatMap((tool) => tool.tools ?? [tool])
            .map((tool) => tool.name);
      const call = (suffix, args) => {
        const name = names.find((candidate) => candidate.endsWith(suffix));
        assert(
          name,
          `Missing native tool ${suffix}; inventory=${JSON.stringify(names)}`,
        );
        return {
          id: `fc_${round}`,
          call_id: `call_${round}`,
          status: "completed",
          ...(execution
            ? {
                type: "custom_tool_call",
                name: "exec",
                namespace: "functions",
                input: `text(await tools[${JSON.stringify(name)}](${JSON.stringify(args)}));`,
              }
            : { type: "function_call", name, arguments: JSON.stringify(args) }),
        };
      };
      let item;
      if (round === 0)
        item = call("exec_command", {
          cmd: "printf 'Native answer file\\n' > report.txt",
          workdir: directory,
        });
      else if (round === 1) {
        assert.equal(
          readFileSync(join(directory, "report.txt"), "utf8"),
          "Native answer file\n",
        );
        item = call("__describe_deliverable", {
          kind: "live_file",
          path: "report.txt",
        });
      } else if (round === 2) {
        assert.equal(
          markers.size,
          1,
          "native file tool receipt must supply its verified marker",
        );
        item = call("__publish_document", {
          request_key: "native-delivery-document",
          title: "Native document",
          markdown: "# Native document\n\nKnown persistent content.\n",
        });
      } else {
        assert.equal(round, 3);
        assert.equal(
          markers.size,
          2,
          "native publication receipt must supply the document marker",
        );
        const kinds = [...markers]
          .map(
            (marker) =>
              JSON.parse(
                Buffer.from(
                  /^<!--cma-deliverable:v1:([A-Za-z0-9_-]+)-->$/.exec(
                    marker,
                  )[1],
                  "base64url",
                ).toString(),
              ).resource.kind,
          )
          .sort();
        assert.deepEqual(kinds, ["document", "live_file"]);
        item = {
          id: `msg_${round}`,
          type: "message",
          role: "assistant",
          status: "completed",
          content: [
            {
              type: "output_text",
              text: `Delivered the file and persistent document.\n${[...markers].join("\n")}`,
              annotations: [],
            },
          ],
        };
      }
      round++;
      const id = `resp_native_delivery_${round}`;
      response.writeHead(200, {
        "content-type": "text/event-stream",
        "cache-control": "no-store",
      });
      const send = (event) =>
        response.write(`data: ${JSON.stringify(event)}\n\n`);
      send({
        type: "response.created",
        response: { id, status: "in_progress", output: [] },
      });
      send({
        type: "response.output_item.added",
        output_index: 0,
        item: { ...item, status: "in_progress" },
      });
      if (item.type === "message") {
        send({
          type: "response.content_part.added",
          item_id: item.id,
          output_index: 0,
          content_index: 0,
          part: { type: "output_text", text: "", annotations: [] },
        });
        send({
          type: "response.output_text.delta",
          item_id: item.id,
          output_index: 0,
          content_index: 0,
          delta: item.content[0].text,
        });
        send({
          type: "response.output_text.done",
          item_id: item.id,
          output_index: 0,
          content_index: 0,
          text: item.content[0].text,
        });
        send({
          type: "response.content_part.done",
          item_id: item.id,
          output_index: 0,
          content_index: 0,
          part: item.content[0],
        });
      }
      send({ type: "response.output_item.done", output_index: 0, item });
      send({
        type: "response.completed",
        response: {
          id,
          status: "completed",
          output: [item],
          usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 },
        },
      });
      response.end();
    } catch (error) {
      failure = error;
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => model.listen(0, "127.0.0.1", resolve));
  const home = join(directory, "native-home");
  mkdirSync(home, { mode: 0o700 });
  writeFileSync(
    join(home, "config.toml"),
    [
      'model = "gpt-5.6-sol"',
      'model_provider = "cma_test"',
      'web_search = "disabled"',
      "[model_providers.cma_test]",
      'name = "Isolated delivery proof"',
      `base_url = "http://127.0.0.1:${model.address().port}/v1"`,
      'wire_api = "responses"',
      "requires_openai_auth = false",
    ].join("\n"),
  );
  const child = spawn(process.execPath, [program], {
    cwd: directory,
    detached: true,
    stdio: ["pipe", "pipe", "pipe"],
    env: {
      PATH: process.env.PATH,
      HOME: home,
      TMPDIR: directory,
      CODEX_HOME: home,
      CODEX_PATH: binary,
    },
  });
  let stderr = "";
  child.stderr.on("data", (bytes) => {
    stderr = (stderr + bytes.toString()).slice(-4000);
  });
  const pending = new Map();
  let serial = 0;
  let answer = "";
  const lines = createInterface({ input: child.stdout });
  lines.on("line", (line) => {
    const frame = JSON.parse(line);
    if (frame.method === "session/request_permission") {
      const option = frame.params.options.find(
        (value) => value.kind === "allow_once",
      );
      assert(option);
      child.stdin.write(
        JSON.stringify({
          jsonrpc: "2.0",
          id: frame.id,
          result: {
            outcome: { outcome: "selected", optionId: option.optionId },
          },
        }) + "\n",
      );
    } else if (
      frame.method === "session/update" &&
      frame.params.update.sessionUpdate === "agent_message_chunk"
    ) {
      assert.equal(frame.params.update.content.type, "text");
      answer += frame.params.update.content.text;
    } else if (pending.has(frame.id)) pending.get(frame.id)(frame);
  });
  async function rpc(method, params) {
    const id = ++serial;
    let timer;
    const incoming = new Promise((resolve) => pending.set(id, resolve));
    child.stdin.write(
      JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n",
    );
    try {
      const result = await Promise.race([
        incoming,
        new Promise((_, reject) => {
          timer = setTimeout(
            () =>
              reject(
                failure ?? new Error(`Native deadline ${method}: ${stderr}`),
              ),
            60000,
          );
        }),
      ]);
      assert.equal(result.error, undefined, JSON.stringify(result.error));
      if (failure) throw failure;
      return result.result;
    } finally {
      clearTimeout(timer);
      pending.delete(id);
    }
  }
  try {
    await rpc("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const created = await rpc("session/new", {
      cwd: directory,
      mcpServers: [
        {
          type: "http",
          name: "cma_access",
          url: origin + "/api/v1/agent-tools",
          headers: [
            {
              name: "Authorization",
              value: "Bearer " + process.env.CMA_DELIVERY_CAPABILITY,
            },
          ],
        },
      ],
    });
    const bound = await fetch(origin + "/proof/bind", {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ conversation_id: created.sessionId }),
      signal: AbortSignal.timeout(10000),
    });
    assert.equal(bound.status, 200);
    const result = await rpc("session/prompt", {
      sessionId: created.sessionId,
      prompt: [
        {
          type: "text",
          text: "Create report.txt containing Native answer file and a persistent document. Use the verified tool receipts as standalone references in the final answer.",
        },
      ],
    });
    assert.equal(markers.size, 2);
    assert(
      [...markers].every((marker) => answer.includes(marker)),
      `ACP answer must actually contain both observed receipts: ${JSON.stringify(answer)}`,
    );
    process.stdout.write(
      JSON.stringify({
        version,
        bridge_sha256: bridgeHash,
        sessionId: created.sessionId,
        stopReason: result.stopReason,
        answer,
      }),
    );
  } finally {
    lines.close();
    await stop(child);
    model.closeAllConnections();
    await new Promise((resolve) => model.close(resolve));
  }
}
prove().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
