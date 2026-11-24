const assert = require("node:assert/strict");
const { execFileSync } = require("node:child_process");
const { createHash } = require("node:crypto");
const { mkdtempSync, mkdirSync, readFileSync, readdirSync, writeFileSync, rmSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { join } = require("node:path");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { processRpc } = require("./native_tools_fixture.cjs");
const { creditFixture } = require("./credit_boundary_fixture.cjs");

function history(root, sessionId) {
  const files = directory => readdirSync(directory, { withFileTypes: true })
    .flatMap(entry => entry.isDirectory() ? files(join(directory, entry.name))
      : entry.name.endsWith(".jsonl") ? [join(directory, entry.name)] : []);
  const matches = files(join(root, "sessions")).map(path => {
    const bytes = readFileSync(path);
    assert.equal(bytes.at(-1), 10, "native history must end at a complete record");
    const records = bytes.toString().trim().split("\n").map(line => JSON.parse(line));
    return { bytes, records };
  }).filter(value => value.records.some(record => record.type === "session_meta"
    && record.payload.id === sessionId));
  assert.equal(matches.length, 1);
  return matches[0];
}

test("credit refusal retains a real effect; reload is passive and a new prompt can repeat it", {
  timeout: 120000,
}, async () => {
  const binary = process.env.CMA_CODEX_TEST_BINARY ?? "/usr/local/bin/codex";
  const program = process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath;
  const version = execFileSync(binary, ["--version"], { encoding: "utf8" }).trim();
  assert.match(version, /^codex-cli 0\.(147|165)\.0$/);
  const evidence = process.env.CMA_CREDIT_EVIDENCE_ROOT;
  const directory = mkdtempSync(join(evidence ?? tmpdir(), "cma-credit-boundary-"));
  const agentHome = join(directory, "agent");
  mkdirSync(agentHome);
  const effectPath = join(directory, "effects.txt");
  writeFileSync(effectPath, "");
  const fixture = await creditFixture(effectPath);
  const frames = [];
  let bridge;
  const connect = () => processRpc(process.execPath, [program], {
    cwd: directory,
    env: { PATH: process.env.PATH, CODEX_HOME: agentHome, CODEX_PATH: binary },
  }, frame => frames.push(frame));
  writeFileSync(join(agentHome, "config.toml"), [
    'model="gpt-6"', 'model_provider="credit_fixture"', 'web_search="disabled"',
    'approval_policy="never"', 'sandbox_mode="danger-full-access"',
    '[model_providers.credit_fixture]', 'name="Offline credit boundary fixture"',
    'base_url="' + fixture.origin + '/v1"', 'wire_api="responses"', 'requires_openai_auth=false',
  ].join("\n"));
  const tools = [{ type: "http", name: "credit_fixture", url: fixture.origin + "/mcp", headers: [] }];
  const initialize = { protocolVersion: 1, clientCapabilities: {
    fs: { readTextFile: false, writeTextFile: false }, elicitation: { url: {} },
    _meta: { jetbrains: { air: { version: 1, capabilities: ["sessionFailure"] } } },
  } };
  let receipt;
  try {
    bridge = connect();
    await bridge.call("initialize", initialize);
    const { sessionId } = await bridge.call("session/new", { cwd: directory, mcpServers: tools });
    await bridge.call("session/set_mode", { sessionId, modeId: "agent-full-access" });
    const original = await bridge.call("session/prompt", { sessionId,
      prompt: [{ type: "text", text: "Perform the original business action, then finish the task." }] });
    if (fixture.state.error) throw fixture.state.error;
    const refusedRequests = fixture.state.requests.length - 1;
    assert(refusedRequests > 0 && refusedRequests <= 6, "native credit retries remain bounded");
    const originalRequests = fixture.state.requests.length;
    const originalTurn = fixture.state.requests[0].client_metadata.turn_id;
    assert.equal(typeof originalTurn, "string");
    assert(originalTurn.length > 0);
    assert(fixture.state.requests.every(request => request.client_metadata.turn_id === originalTurn));
    for (const request of fixture.state.requests.slice(2)) {
      assert.deepEqual(request, fixture.state.requests[1], "credit retries preserve the pending model input");
    }
    assert.deepEqual(fixture.effects(), ["original-business-action"]);
    const failure = original._meta?.jetbrains?.air?.sessionFailure;
    assert.equal(failure?.severity, "error", "negotiated terminal failure must be explicit");
    assert.match(failure.title, /^unexpected status 402 Payment Required:/);
    assert(failure.title.includes('"error":"insufficient_credits"'));
    assert(failure.title.includes('"error_code":11300'));
    assert(JSON.stringify(fixture.state.requests[1].input).includes("Durable effect receipt 1"));
    await bridge.call("session/close", { sessionId });
    await bridge.close();
    const before = history(agentHome, sessionId);
    assert(before.bytes.includes("Durable effect receipt 1"));

    fixture.state.funded = true;
    bridge = connect();
    await bridge.call("initialize", initialize);
    await bridge.call("session/load", { sessionId, cwd: directory, mcpServers: tools });
    await bridge.call("session/set_mode", { sessionId, modeId: "agent-full-access" });
    assert.equal(fixture.state.requests.length, originalRequests, "funding restoration and load do not resume execution");
    assert.deepEqual(fixture.effects(), ["original-business-action"]);
    const resumed = await bridge.call("session/prompt", { sessionId,
      prompt: [{ type: "text", text: "Continue the task using its retained work." }] });
    if (fixture.state.error) throw fixture.state.error;
    assert.equal(resumed.stopReason, "end_turn");
    assert.equal(resumed._meta?.jetbrains?.air?.sessionFailure, undefined,
      "the old credit failure must not contaminate a successful new turn");
    assert.equal(fixture.state.requests.length, originalRequests + 2);
    const newTurn = fixture.state.requests[originalRequests].client_metadata.turn_id;
    assert.equal(typeof newTurn, "string");
    assert(newTurn.length > 0);
    assert.notEqual(newTurn, originalTurn, "explicit continuation starts a new native turn");
    assert(fixture.state.requests.slice(originalRequests).every(request =>
      request.client_metadata.turn_id === newTurn && request.client_metadata.session_id === sessionId));
    assert(JSON.stringify(fixture.state.requests[originalRequests].input).includes("Durable effect receipt 1"),
      "the new prompt sees the completed effect but has no effect-deduplication contract");
    assert.deepEqual(fixture.effects(), ["original-business-action", "original-business-action"],
      "a new model-generated call can repeat an earlier business effect");
    await bridge.call("session/close", { sessionId });
    await bridge.close();
    const after = history(agentHome, sessionId);
    assert(after.bytes.subarray(0, before.bytes.length).equals(before.bytes), "original history stays intact");
    assert.equal(after.records.filter(record => record.type === "event_msg"
      && record.payload.type === "task_started").length, 2, "reload must not create a hidden turn");
    receipt = { version, bridgeSha256: createHash("sha256").update(readFileSync(program)).digest("hex"),
      modelRequests: fixture.state.requests.length, refusedRequests, effects: fixture.effects().length,
      original, resumed, historyPrefixPreserved: true, distinctNativeTurns: true,
      eventTypes: after.records.filter(record => record.type === "event_msg").map(record => record.payload.type) };
    console.log(JSON.stringify({ version, bridgeSha256: receipt.bridgeSha256,
      modelRequests: receipt.modelRequests, refusedRequests, effects: receipt.effects,
      historyPrefixPreserved: true, distinctNativeTurns: true }));
  } finally {
    await bridge?.close();
    await fixture.close();
    if (evidence) {
      writeFileSync(join(directory, "observations.json"), JSON.stringify({ receipt,
        frames, requests: fixture.state.requests, fixtureError: fixture.state.error?.stack }, null, 2));
    } else rmSync(directory, { recursive: true, force: true, maxRetries: 10, retryDelay: 100 });
  }
});
