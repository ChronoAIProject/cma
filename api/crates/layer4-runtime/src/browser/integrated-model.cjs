const assert = require("node:assert/strict");
const { createServer } = require("node:http");
const { randomUUID } = require("node:crypto");
const { readFile, writeFile, rename, unlink } = require("node:fs/promises");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");

exports.start = async function start(directory, atomic) {
  const repository = await require("./integrated-repository.cjs").prepare(
    atomic,
  );
  const progress = {
    modelRequests: 0,
    completedTurns: 0,
    pages: 0,
    suggestions: 0,
  };
  let failure;
  const values = [];
  let stage = 0;
  const extended = process.env.CMA_INTEGRATED_EXTENDED === "1";
  const markers = new Set();
  const publicRequest = async (body) => {
    const name = "public-" + randomUUID();
    await atomic(name + ".request.json", body);
    for (let attempt = 0; attempt < 900; attempt += 1) {
      const bytes = await readFile(
        join(directory, name + ".response.json"),
      ).catch(() => undefined);
      if (bytes) {
        await unlink(join(directory, name + ".response.json"));
        const result = JSON.parse(bytes);
        assert.equal(result.status, 200, JSON.stringify(result));
        return result.body;
      }
      await delay(50);
    }
    throw new Error("Authorized public delivery did not return");
  };
  const server = createServer(async (request, response) => {
    try {
      if (request.method === "GET") {
        if (repository?.handle(request, response)) return;
        if (request.url === "/reconnect-second") {
          await atomic("second-running.json", { started: Date.now() });
          for (let attempt = 0; attempt < 600; attempt += 1) {
            if (
              await readFile(join(directory, "release-second.json")).catch(
                () => undefined,
              )
            ) {
              response.end("Second turn continued after browser reconnect");
              return;
            }
            await delay(50);
          }
          throw new Error(
            "The actual running turn was not released after reconnect",
          );
        }
        if (request.url === "/page") {
          progress.pages += 1;
          response
            .writeHead(200, { "content-type": "text/html" })
            .end(
              '<title>Task comparison</title><body style="margin:20px"><h1>COMPARISON TASK</h1><input aria-label="Notes" style="position:absolute;left:20px;top:70px;width:500px;height:40px" oninput="fetch(\'/value\',{method:\'POST\',body:this.value})"><p style="margin-top:80px">Tool A: simple. Tool B: flexible. Tool C: portable.</p></body>',
            );
        } else
          response.writeHead(200, { "content-type": "application/json" }).end(
            JSON.stringify({
              data: [{ id: "gpt-5.6-sol", object: "model" }],
            }),
          );
        return;
      }
      let body = "";
      for await (const chunk of request) {
        body += chunk;
        assert(body.length < 2 * 1024 * 1024);
      }
      if (request.url === "/value") {
        values.push(body);
        await atomic("native-form.json", { values });
        response.end("ok");
        return;
      }
      const value = JSON.parse(body);
      if (request.url === "/delivery") {
        const agent = JSON.parse(
          await readFile(join(directory, "agent.json"), "utf8"),
        );
        const result = await publicRequest({
          method: "POST",
          path: "/api/v1/agents/" + agent.id + "/deliverable-references",
          body: { kind: "live_file", path: value.path },
        });
        assert(result.marker.startsWith("<!--cma-deliverable:v1:"));
        response
          .writeHead(200, { "content-type": "application/json" })
          .end(JSON.stringify(result));
        return;
      }
      if (value.stream === false) {
        progress.suggestions += 1;
        const context = JSON.parse(value.input[0].content);
        assert(
          JSON.stringify(context).includes(
            progress.completedTurns > 2 ? "maintenance effort" : "comparison",
          ),
        );
        response.writeHead(200, { "content-type": "application/json" }).end(
          JSON.stringify({
            id: "suggestion-" + progress.suggestions,
            status: "completed",
            model: "synthetic-model-boundary",
            output: [
              {
                type: "message",
                role: "assistant",
                content: [
                  {
                    type: "output_text",
                    text: JSON.stringify({
                      suggestions:
                        progress.completedTurns > 2
                          ? []
                          : [
                              {
                                text:
                                  extended && progress.completedTurns === 2
                                    ? "Compare maintenance effort for the three tools."
                                    : "Explain the flexibility trade-off in the comparison.",
                                evidence:
                                  progress.completedTurns === 2
                                    ? "flexibility trade-off"
                                    : "one remaining trade-off",
                              },
                            ],
                    }),
                  },
                ],
              },
            ],
            usage: {
              input_tokens: 30,
              output_tokens: 15,
              input_tokens_details: { cached_tokens: 0 },
            },
          }),
        );
        await atomic("native-progress.json", progress);
        return;
      }
      progress.modelRequests += 1;
      await atomic("model-request-" + progress.modelRequests + ".json", {
        input: value.input,
      });
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
        assert(name, "Native tool unavailable: " + suffix);
        return {
          id: "fc_integrated_" + stage,
          call_id: "call_integrated_" + stage,
          status: "completed",
          ...(execution
            ? {
                type: "custom_tool_call",
                name: "exec",
                namespace: "functions",
                input:
                  "text(await tools[" +
                  JSON.stringify(name) +
                  "](" +
                  JSON.stringify(args) +
                  "));",
              }
            : { type: "function_call", name, arguments: JSON.stringify(args) }),
        };
      };
      const quote = (text) => "'" + text.replaceAll("'", "'\\''") + "'";
      let item;
      if (stage === 0) {
        assert(JSON.stringify(value.input).includes("Compare three tools"));
        item = call("update_plan", {
          plan: [
            { step: "Create the comparison report", status: "in_progress" },
            { step: "Explain the remaining trade-off", status: "pending" },
          ],
        });
      } else if (stage === 1) {
        const script =
          (repository
            ? "require('node:assert/strict').equal(require('node:fs').readFileSync('README.md','utf8'),'Integrated comparison source\\nTool A: simple\\nTool B: flexible\\nTool C: portable\\n');"
            : "") +
          "const fs=require('node:fs');fs.appendFileSync('/home/cma/physical-dispatches.txt','first\\n');fs.writeFileSync('/home/cma/report.txt','Tool A: simple\\nTool B: flexible\\nTool C: portable\\n');fetch('http://127.0.0.1:16416/delivery',{method:'POST',headers:{'content-type':'application/json'},body:JSON.stringify({path:'cma/report.txt'})}).then(async response=>{if(!response.ok)throw new Error(await response.text());console.log(await response.text())}).catch(error=>{console.error(error);process.exitCode=1})";
        item = call("exec_command", {
          cmd:
            "node -e " +
            quote(script) +
            " && /usr/local/bin/cma-browser navigate http://127.0.0.1:16416/page",
          yield_time_ms: 10000,
          max_output_tokens: 2000,
        });
      } else if (stage === 2) {
        assert.equal(
          markers.size,
          1,
          "Actual authorized file description must precede the answer",
        );
        item = call("update_plan", {
          plan: [
            { step: "Create the comparison report", status: "completed" },
            { step: "Explain the remaining trade-off", status: "completed" },
          ],
        });
      } else if (stage === 3) {
        item = {
          type: "message",
          id: "msg_integrated_first",
          role: "assistant",
          status: "completed",
          content: [
            {
              type: "output_text",
              text:
                "The comparison is complete. Tool A is simple, Tool B is flexible, and Tool C is portable. One remaining trade-off is flexibility versus setup effort.\n" +
                [...markers].join("\n"),
              annotations: [],
            },
          ],
        };
        progress.completedTurns += 1;
      } else if (stage === 4) {
        assert(
          JSON.stringify(value.input).includes(
            "Explain the flexibility trade-off in the comparison in two sentences.",
          ),
        );
        item = call("exec_command", {
          cmd:
            "printf 'second\\n' >> /home/cma/physical-dispatches.txt" +
            (extended
              ? " && node -e " +
                quote(
                  "fetch('http://127.0.0.1:16416/reconnect-second').then(async response=>{if(!response.ok)throw new Error(await response.text());console.log(await response.text())}).catch(error=>{console.error(error);process.exitCode=1})",
                )
              : ""),
          yield_time_ms: 30000,
          max_output_tokens: 1000,
        });
      } else if (stage === 5) {
        item = {
          type: "message",
          id: "msg_integrated_second",
          role: "assistant",
          status: "completed",
          content: [
            {
              type: "output_text",
              text: "The flexibility trade-off is explained.",
              annotations: [],
            },
          ],
        };
        progress.completedTurns += 1;
      } else if (stage === 6 && extended) {
        assert(
          JSON.stringify(value.input).includes(
            "Compare maintenance effort for the three tools.",
          ),
        );
        item = call("exec_command", {
          cmd: "printf 'third\\n' >> /home/cma/physical-dispatches.txt",
          yield_time_ms: 10000,
          max_output_tokens: 1000,
        });
      } else {
        assert(extended);
        assert.equal(stage, 7);
        item = {
          type: "message",
          id: "msg_integrated_third",
          role: "assistant",
          status: "completed",
          content: [
            {
              type: "output_text",
              text: "Maintenance effort compared for all three tools.",
              annotations: [],
            },
          ],
        };
        progress.completedTurns += 1;
      }
      stage += 1;
      const identifier = "resp_integrated_" + progress.modelRequests;
      response.writeHead(200, { "content-type": "text/event-stream" });
      const send = (event) =>
        response.write("data: " + JSON.stringify(event) + "\n\n");
      send({
        type: "response.created",
        response: { id: identifier, status: "in_progress", output: [] },
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
          id: identifier,
          status: "completed",
          output: [item],
          usage: { input_tokens: 30, output_tokens: 15, total_tokens: 45 },
        },
      });
      response.end();
      await atomic("native-progress.json", progress);
    } catch (error) {
      failure = error;
      console.error(error);
      if (!response.headersSent) response.writeHead(500);
      response.end();
    }
  });
  await new Promise((resolve) => server.listen(16416, "127.0.0.1", resolve));
  return {
    server,
    progress,
    async verify() {
      assert.equal(failure, undefined);
      assert.equal(progress.completedTurns, extended ? 3 : 2);
      assert.equal(progress.pages, 1);
      if (process.env.CMA_INTEGRATED_CONTROL === "1")
        assert.deepEqual(values, ["TICKET19-ONCE"]);
      await repository?.verify();
    },
  };
};
