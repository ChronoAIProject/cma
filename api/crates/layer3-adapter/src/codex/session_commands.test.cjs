const assert = require("node:assert/strict");
const { spawn, execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, writeFileSync, renameSync, rmSync } = require("node:fs");
const { createServer } = require("node:http");
const { createInterface } = require("node:readline");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { sessionCommands } = require("./session_commands.cjs");
const { sessionCommands: candidateSessionCommands } = require("./session_commands_2_12_1.cjs");
const { stop } = require("./native_tools_fixture.cjs");
const { controlledBinary, control } = require("./native_control_fixture.cjs");

test("session catalogue patch refuses unknown provider source", () => {
  assert.throws(() => sessionCommands("unknown"), /Unexpected ACP source/);
  assert.throws(() => candidateSessionCommands("unknown"), /Unexpected codex-acp 2\.12\.1 source/);
});

function connect(program, directory, home, binary) {
  const child = spawn(process.execPath, [program], { cwd: directory,
    env: { PATH: process.env.PATH, CODEX_HOME: home, CODEX_PATH: binary },
    detached: true, stdio: ["pipe", "pipe", "pipe"] });
  child.stderr.resume();
  const frames = [], pending = new Map(), listeners = new Set();
  let serial = 0;
  const lines = createInterface({ input: child.stdout });
  lines.on("line", line => {
    const frame = JSON.parse(line); frames.push(frame);
    pending.get(frame.id)?.(frame);
    for (const listener of listeners) listener(frame);
  });
  return {
    frames,
    async call(method, params) {
      const id = ++serial; let timer;
      const response = new Promise(resolve => pending.set(id, resolve));
      child.stdin.write(JSON.stringify({ jsonrpc: "2.0", id, method, params }) + "\n");
      try {
        const frame = await Promise.race([response, new Promise((_, reject) => {
          timer = setTimeout(() => reject(new Error("Native command deadline: " + method)), 30000);
        })]);
        assert.equal(frame.error, undefined, JSON.stringify(frame.error)); return frame.result;
      } finally { clearTimeout(timer); pending.delete(id); }
    },
    async waitFor(predicate) {
      if (frames.some(predicate)) return;
      await new Promise((resolve, reject) => {
        const receive = frame => { if (predicate(frame)) { clearTimeout(timer); listeners.delete(receive); resolve(); } };
        const timer = setTimeout(() => { listeners.delete(receive); reject(new Error("Native catalogue deadline")); }, 15000);
        listeners.add(receive);
      });
    },
    async close() { lines.close(); await stop(child); },
  };
}

const candidateSource = process.env.CMA_ACP_2_12_1_SOURCE;
test("pinned session advertises real commands, refreshes skills and executes native goal and skill semantics", { timeout: 120000 }, async (t) => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  const binaryVersion = execFileSync(binary, ["--version"], { encoding: "utf8" }).trim();
  if (candidateSource) assert.equal(binaryVersion, "codex-cli 0.172.0");
  else assert.match(binaryVersion, /^codex-cli 0\.(147|165)\.0$/);
  const directory = mkdtempSync(join(tmpdir(), "cma-session-commands-"));
  t.after(() => rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 }));
  let program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  if (candidateSource) {
    const source = readFileSync(candidateSource, "utf8");
    const patched = candidateSessionCommands(source);
    assert.throws(() => candidateSessionCommands(patched), /Unexpected codex-acp 2\.12\.1 source/);
    program = join(directory, "codex-acp-candidate.mjs");
    writeFileSync(program, patched);
  }
  const home = join(directory, "agent-home"); mkdirSync(home); mkdirSync(join(home, "skills"));
  const calls = []; let error, sessionId, pauseGoal = false;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/responses");
      let body = ""; for await (const chunk of request) { body += chunk; assert(body.length < 2 * 1024 * 1024); }
      calls.push(JSON.parse(body));
      // Bound the goal fixture without asking a synthetic model to finish it.
      // Both reads/writes are genuine native requests on this same thread.
      if (pauseGoal) {
        const { goal } = await control(directory, "thread/goal/get", { threadId: sessionId });
        assert.equal(goal.objective, "Verify native goal dispatch");
        await control(directory, "thread/goal/set", { threadId: sessionId, status: "paused" });
      }
      response.writeHead(200, { "content-type": "text/event-stream" });
      const send = event => response.write("data: " + JSON.stringify(event) + "\n\n");
      const id = "resp_commands_" + calls.length;
      const item = { id: "msg_commands_" + calls.length, type: "message", role: "assistant", status: "completed",
        content: [{ type: "output_text", text: "Command fixture completed.", annotations: [] }] };
      send({ type: "response.created", response: { id, status: "in_progress", output: [] } });
      send({ type: "response.output_item.added", output_index: 0, item: { ...item, status: "in_progress" } });
      send({ type: "response.output_item.done", output_index: 0, item });
      send({ type: "response.completed", response: { id, status: "completed", output: [item],
        usage: { input_tokens: 1, output_tokens: 1, total_tokens: 2 } } });
      response.end();
    } catch (failure) { error = failure; if (!response.headersSent) response.writeHead(500); response.end(); }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "config.toml"), [
    'model = "gpt-6"', 'model_provider = "cma_test"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.cma_test]', 'name = "Offline commands fixture"',
    'base_url = "http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api = "responses"', 'requires_openai_auth = false',
  ].join("\n"));
  const bridge = connect(program, directory, home, controlledBinary(directory, binary));
  const catalogue = frame => frame.params?.update?.sessionUpdate === "available_commands_update";
  const names = frame => frame.params.update.availableCommands.map(command => command.name);
  const prompt = text => bridge.call("session/prompt", { sessionId, prompt: [{ type: "text", text }] });
  try {
    const initialization = await bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    assert.equal(initialization.protocolVersion, 1);
    sessionId = (await bridge.call("session/new", { cwd: directory, mcpServers: [] })).sessionId;
    await bridge.waitFor(catalogue);
    const reported = names(bridge.frames.find(catalogue));
    for (const name of ["goal", "plan", "status", "compact", "review", "skills", "mcp"]) assert(reported.includes(name));
    assert(!reported.includes("logout"), "managed API sessions cannot log out of a ChatGPT account");
    await prompt("/status");
    assert.equal(calls.length, 0, "native /status must not become model input");
    assert(bridge.frames.some(frame => frame.params?.update?.content?.text?.includes("**Model:**")));
    await prompt("/plan");
    assert.equal(calls.length, 0, "native /plan must not become model input");
    await prompt("/plan");
    const staging = join(home, "skills", ".stage-command-fixture"); mkdirSync(staging);
    writeFileSync(join(staging, "SKILL.md"), '---\nname: command-proof\ndescription: Native command skill fixture.\n---\nCMA_NATIVE_SKILL_BODY_20261016: Follow the installed skill instructions.\n');
    renameSync(staging, join(home, "skills", "command-proof"));
    await bridge.waitFor(frame => catalogue(frame) && names(frame).includes("$command-proof"));
    assert.equal(calls.length, 0, "skill catalogue refresh must not start a model turn");
    await prompt("$command-proof Verify the installed skill.");
    assert(JSON.stringify(calls.at(-1)).includes("CMA_NATIVE_SKILL_BODY_20261016"),
      "native Codex must inject the chosen skill body, not merely show its name");
    const beforeDisable = bridge.frames.length;
    await control(directory, "skills/config/write", {
      path: join(home, "skills", "command-proof", "SKILL.md"), enabled: false,
    });
    const beforeSkills = bridge.frames.length;
    const beforeSkillsCalls = calls.length;
    await prompt("/skills");
    await bridge.waitFor(frame => bridge.frames.indexOf(frame) >= beforeDisable && catalogue(frame) && !names(frame).includes("$command-proof"));
    const skillReport = bridge.frames.slice(beforeSkills).find(frame =>
      /^(Available skills:|No skills configured\.)/.test(frame.params?.update?.content?.text ?? ""));
    assert(skillReport, "the native /skills command must return the inventory");
    assert(!skillReport.params.update.content.text.includes("command-proof"), "disabled skills stay out of /skills");
    assert.equal(calls.length, beforeSkillsCalls, "/skills must not start a model turn");
    pauseGoal = true;
    await prompt("/goal Verify native goal dispatch");
    if (error) throw error;
    const goal = (await control(directory, "thread/goal/get", { threadId: sessionId })).goal;
    assert.equal(goal.objective, "Verify native goal dispatch");
    assert.equal(goal.status, "paused");
    pauseGoal = false;
    const beforePause = calls.length;
    await prompt("/goal pause");
    assert.equal(calls.length, beforePause, "goal pause is a native state mutation");
    await prompt("/goal clear");
    assert.equal((await control(directory, "thread/goal/get", { threadId: sessionId })).goal, null);
    assert.equal(calls.length, beforePause, "goal clear is a native state mutation");
    if (error) throw error;
  } finally {
    await bridge.close(); server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
  }
});
