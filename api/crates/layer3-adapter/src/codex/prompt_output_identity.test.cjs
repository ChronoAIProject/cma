const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { mkdtempSync, mkdirSync, readFileSync, rmSync, writeFileSync } = require("node:fs");
const { createServer } = require("node:http");
const { join } = require("node:path");
const { test } = require("node:test");
const { setTimeout: delay } = require("node:timers/promises");
const { controlledBinary, control } = require("./native_control_fixture.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
const { applyNativeBoundDispatch, withoutNativeBoundDispatch } = require("./native_bound_dispatch.cjs");
const { bridgePath } = require("./persist_conversations.cjs");
const { promptOutputIdentity, withoutPromptOutputIdentity } = require("./prompt_output_identity.cjs");

const oldPrefix = "First answer prefix.";
const oldTail = "Delayed first answer.";
const newPrefix = "Second answer prefix.";
const newTail = "Second answer tail.";
const unboundDelta = "Unqualified model output before binding.";
const boundDelta = "Unqualified model output after binding.";
const title = "Fixture conversation title.";
const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;

function beginAnswer(response, sequence) {
  const item = { id: "message_" + sequence, type: "message", role: "assistant", status: "in_progress", content: [] };
  const send = event => response.write("data: " + JSON.stringify(event) + "\n\n");
  response.writeHead(200, { "content-type": "text/event-stream" });
  send({ type: "response.created", response: { id: "response_" + sequence, status: "in_progress", output: [] } });
  send({ type: "response.output_item.added", output_index: 0, item });
  send({ type: "response.content_part.added", output_index: 0, content_index: 0, item_id: item.id,
    part: { type: "output_text", text: "", annotations: [] } });
  return delta => send({ type: "response.output_text.delta", output_index: 0, content_index: 0,
    item_id: item.id, delta });
}

async function until(read, description) {
  const deadline = Date.now() + 8000;
  do {
    const value = await read();
    if (value) return value;
    await delay(20);
  } while (Date.now() < deadline);
  assert.fail("Timed out waiting for " + description);
}

test("prompt output patch requires the reviewed source and is reversible", () => {
  const installed = withoutNativeBoundDispatch(readFileSync(program, "utf8"));
  const original = withoutPromptOutputIdentity(installed);
  assert.equal(promptOutputIdentity(original), installed);
  assert.throws(() => promptOutputIdentity(installed), /Unexpected ACP source/);
  assert.throws(() => promptOutputIdentity("unreviewed"), /Unexpected ACP source/);
});

test("native dispatch composition preserves the reviewed prompt output source", () => {
  const installed = readFileSync(program, "utf8");
  const identity = withoutNativeBoundDispatch(installed);
  const candidate = applyNativeBoundDispatch(identity);
  assert.equal(withoutNativeBoundDispatch(candidate), identity);
  assert.throws(() => withoutNativeBoundDispatch(candidate + "\n"), /Unexpected pre-binding ACP composition/);
  assert.throws(() => applyNativeBoundDispatch(candidate), /Unexpected ACP composition/);
  if (installed !== identity) assert.equal(candidate, installed);
});

async function checkDelayedOutput(intermediateTurns) {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  assert.match(execFileSync(binary, ["--version"], { encoding: "utf8" }).trim(), /^codex-cli 0\.(147|165)\.0$/);
  // Keep the fixture's Unix socket within the platform path length limit.
  const directory = mkdtempSync("/tmp/cma-prompt-id-");
  const home = join(directory, "home");
  mkdirSync(home);
  let dispatches = 0;
  const finalSequence = intermediateTurns + 2;
  let sendSecond;
  let modelError;
  const server = createServer(async (request, response) => {
    try {
      assert.equal(request.url, "/v1/responses");
      for await (const ignored of request) void ignored;
      const sequence = ++dispatches;
      assert(sequence <= finalSequence);
      const send = beginAnswer(response, sequence);
      if (sequence === 1) {
        send(oldPrefix);
        send(oldTail);
      } else if (sequence === finalSequence) {
        sendSecond = send;
        send(newPrefix);
      } else {
        send("Intermediate output " + sequence);
      }
    } catch (error) {
      modelError = error;
      response.end();
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  writeFileSync(join(home, "config.toml"), [
    'model = "gpt-6"', 'model_provider = "fixture"', 'web_search = "disabled"',
    'approval_policy = "never"', 'sandbox_mode = "danger-full-access"',
    '[model_providers.fixture]', 'name = "Local prompt identity fixture"',
    'base_url = "http://127.0.0.1:' + server.address().port + '/v1"',
    'wire_api = "responses"', 'requires_openai_auth = false',
  ].join("\n"));
  const chunks = [];
  const titles = [];
  const outstanding = [];
  const acp = processRpc(process.execPath, [program], {
    cwd: directory, env: { PATH: process.env.PATH, CODEX_HOME: home,
      CODEX_PATH: controlledBinary(directory, binary, oldTail) },
  }, frame => {
    const update = frame.params?.update;
    if (update?.sessionUpdate === "agent_message_chunk") chunks.push(update.content.text);
    if (update?.sessionUpdate === "session_info_update" && update.title) titles.push(update.title);
  });
  const native = (method, params) => control(directory, method, params);
  try {
    await acp.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const { sessionId: threadId } = await acp.call("session/new", { cwd: directory, mcpServers: [] });
    const start = text => {
      const result = acp.call("session/prompt", { sessionId: threadId, prompt: [{ type: "text", text }] });
      outstanding.push(result);
      void result.catch(() => {});
      return result;
    };
    const first = start("Start the first fixture prompt.");
    await until(() => chunks.includes(oldPrefix), "first model output");
    const held = await until(async () => (await native("fixture/held", {})).source, "first native turn delta");
    const before = await native("thread/read", { threadId, includeTurns: true });
    assert.equal(before.thread.turns.length, 1);
    assert.deepEqual(held, { threadId, turnId: before.thread.turns[0].id });
    await native("turn/interrupt", held);
    assert.equal((await first).stopReason, "cancelled");

    for (let sequence = 2; sequence < finalSequence; sequence++) {
      const intermediate = start("Start an intermediate fixture prompt.");
      await until(() => chunks.includes("Intermediate output " + sequence), "intermediate model output");
      const { thread } = await native("thread/read", { threadId, includeTurns: true });
      assert.equal(thread.turns.length, sequence);
      await native("turn/interrupt", { threadId, turnId: thread.turns.at(-1).id });
      assert.equal((await intermediate).stopReason, "cancelled");
    }
    if (intermediateTurns > 0) await native("fixture/hold-start", { earlyDelta: newPrefix });
    const second = start("Start the second fixture prompt.");
    if (intermediateTurns > 0) {
      await until(async () => {
        const state = await native("fixture/start-state", {});
        return state.responseHeld && state.observedEarlyDelta;
      }, "native output before the start response");
      await native("fixture/emit-without-turn", { delta: unboundDelta });
      await native("thread/name/set", { threadId, name: title });
      await until(() => titles.includes(title), "session title update before the start response");
      assert.equal(chunks.includes(newPrefix), false, "early output must await the native start response");
      assert.deepEqual(await native("fixture/release", {}), { released: true });
      assert.deepEqual(await native("fixture/release-start", {}), { released: true });
    } else {
      await until(() => chunks.includes(newPrefix), "second model output");
      assert.deepEqual(await native("fixture/release", {}), { released: true });
    }
    await until(() => chunks.includes(newPrefix), "second model output after binding");
    if (intermediateTurns > 0) await native("fixture/emit-without-turn", { delta: boundDelta });
    sendSecond(newTail);
    await until(() => chunks.includes(newTail), "second output after the held native frame");
    const after = await native("thread/read", { threadId, includeTurns: true });
    assert.equal(after.thread.turns.length, finalSequence);
    assert.equal(after.thread.turns[0].status, "interrupted");
    const secondTurn = after.thread.turns.at(-1);
    assert.notEqual(secondTurn.id, held.turnId);
    await native("turn/interrupt", { threadId, turnId: secondTurn.id });
    assert.equal((await second).stopReason, "cancelled");
    assert(!chunks.includes(oldTail), "ACP delivered old native output through the replacement prompt");
    assert(!chunks.includes(unboundDelta) && !chunks.includes(boundDelta), "ACP delivered a native delta without a turn identity");
    assert.deepEqual(chunks.filter(chunk => chunk === newPrefix || chunk === newTail), [newPrefix, newTail]);
    assert.equal(dispatches, finalSequence);
    assert.equal(modelError, undefined);
  } finally {
    await acp.close();
    await Promise.allSettled(outstanding);
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
}

test("a delayed output frame is rejected after the next turn binds", { timeout: 45000 },
  () => checkDelayedOutput(0));
test("a delayed output frame is rejected before binding after 16 intervening turns", { timeout: 100000 },
  () => checkDelayedOutput(16));
