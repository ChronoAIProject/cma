const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync, mkdirSync, writeFileSync, appendFileSync, renameSync,
  mkdtempSync, rmSync, existsSync } = require("node:fs");
const { tmpdir } = require("node:os");
const { writeFixtureControl } = require("./fixture_control.cjs");
const { join } = require("node:path");
const { setTimeout: delay } = require("node:timers/promises");
const { bridgePath } = require("./persist_conversations.cjs");
const { deferredSessionMetadata, withoutDeferredSessionMetadata, sourceSha256 } = require("./deferred_session_metadata.cjs");
const { createHash } = require("node:crypto");
const { metadataFixture } = require("./deferred_session_fixture.cjs");

test("deferred metadata patch is exactly pinned and reversible", () => {
  assert.throws(() => deferredSessionMetadata("unknown"), /Unexpected ACP source/);
  const source = readFileSync(process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath, "utf8");
  const original = withoutDeferredSessionMetadata(source);
  assert.equal(createHash("sha256").update(original).digest("hex"), sourceSha256);
  assert.equal(withoutDeferredSessionMetadata(deferredSessionMetadata(original)), original);
});

const commands = frame => frame.params?.update?.sessionUpdate === "available_commands_update";
const history = frame => frame.params?.update?.content?.text?.includes("METADATA_HISTORY_PROOF");
const deadline = async promise => {
  let timer;
  try {
    return await Promise.race([promise, new Promise((_, reject) => {
      timer = setTimeout(() => reject(new Error("Optional metadata blocked load/first turn")), 1500);
    })]);
  } finally { clearTimeout(timer); }
};

test("real pinned load and first skill turn proceed while advisory commands are blocked", { timeout: 60000 }, async () => {
  const f = await metadataFixture();
  try {
    const skill = join(f.home, "skills", "metadata-proof"); mkdirSync(skill, { recursive: true });
    writeFileSync(join(skill, "SKILL.md"), "---\nname: metadata-proof\ndescription: Offline metadata test.\n---\nFIRST_TURN_SKILL_PROOF: Keep this instruction.\n");
    await f.bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const params = { cwd: f.directory, mcpServers: [] };
    const { sessionId } = await f.bridge.call("session/new", params);
    await f.waitFor(() => f.state.frames.some(commands));
    await f.bridge.call("session/prompt", { sessionId, prompt: [{ type: "text", text: "Create a retained history marker." }] });
    await f.bridge.call("session/close", { sessionId });
    f.state.frames.length = 0;
    f.configure({ commands: "hold", epoch: 1, limit: 1 });
    const started = performance.now();
    const loading = f.bridge.call("session/load", { ...params, sessionId });
    await f.waitFor(() => f.trace().some(row => row.event === "held" && row.kind === "commands"));
    await deadline(loading);
    const loadMs = performance.now() - started;
    assert(f.state.frames.some(history), "retained history must precede the load response");
    assert(!f.state.frames.some(commands), "advisory command reply is still held");
    assert(f.trace().some(row => row.method === "skills/list" && row.forced), "required skill refresh remains");
    const promptStarted = performance.now();
    await deadline(f.bridge.call("session/prompt", { sessionId, prompt: [{ type: "text", text: "$metadata-proof Check the installed skill." }] }));
    const promptMs = performance.now() - promptStarted;
    if (f.state.error) throw f.state.error;
    assert(JSON.stringify(f.state.modelCalls.at(-1)).includes("FIRST_TURN_SKILL_PROOF"));
    assert(JSON.stringify(f.state.modelCalls.at(-1)).includes("METADATA_HISTORY_PROOF"));
    assert(!f.state.frames.some(commands));
    f.configure({}); await f.waitFor(() => f.state.frames.some(commands));
    console.log(JSON.stringify({ fixture: "blocked_advisory_commands", load_ms: loadMs, immediate_first_turn_ms: promptMs }));
  } finally { f.configure({}); await f.close(); }
});

test("real pinned load responds before advisory goal and drops metadata after close", { timeout: 60000 }, async () => {
  const f = await metadataFixture();
  try {
    await f.bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const params = { cwd: f.directory, mcpServers: [] };
    const { sessionId } = await f.bridge.call("session/new", params);
    await f.waitFor(() => f.state.frames.some(commands));
    await f.bridge.call("session/close", { sessionId }); f.state.frames.length = 0;
    f.configure({ goal: "hold", epoch: 1, limit: 1 });
    const loading = f.bridge.call("session/load", { ...params, sessionId });
    await f.waitFor(() => f.trace().some(row => row.event === "held" && row.kind === "goal"));
    await deadline(loading);
    await f.bridge.call("session/close", { sessionId }); f.state.frames.length = 0;
    f.configure({}); await f.waitFor(() => f.trace().some(row => row.event === "released" && row.kind === "goal"));
    await delay(30);
    assert(!f.state.frames.some(frame => frame.params?.update?._meta?.goal !== undefined), "closed goal cannot publish");
    f.configure({ commands: "hold", epoch: 2, limit: 1 });
    await deadline(f.bridge.call("session/load", { ...params, sessionId }));
    await f.waitFor(() => f.trace().some(row => row.event === "held" && row.kind === "commands"));
    await f.bridge.call("session/close", { sessionId }); f.state.frames.length = 0;
    f.configure({}); await f.waitFor(() => f.trace().some(row => row.event === "released" && row.kind === "commands"));
    await delay(30);
    assert(!f.state.frames.some(commands), "closed command catalogue cannot publish");
  } finally { f.configure({}); await f.close(); }
});

test("real same-session reload coalesces advisory work and publishes only the replacement", { timeout: 60000 }, async () => {
  const f = await metadataFixture();
  try {
    await f.bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const params = { cwd: f.directory, mcpServers: [] };
    const { sessionId } = await f.bridge.call("session/new", params);
    await f.waitFor(() => f.state.frames.some(commands));
    await f.bridge.call("session/close", { sessionId }); f.state.frames.length = 0;
    f.configure({ commands: "hold", epoch: 1, limit: 1 });
    await deadline(f.bridge.call("session/load", { ...params, sessionId }));
    await f.waitFor(() => f.trace().some(row => row.event === "held" && row.kind === "commands"));
    const optionalBefore = f.trace().filter(row => row.event === "request" && row.optional === "commands").length;
    await f.bridge.call("session/close", { sessionId });
    await deadline(f.bridge.call("session/load", { ...params, sessionId }));
    assert.equal(f.trace().filter(row => row.event === "request" && row.optional === "commands").length, optionalBefore);
    f.state.frames.length = 0; f.configure({});
    await f.waitFor(() => f.state.frames.some(commands)); await delay(30);
    assert.equal(f.state.frames.filter(commands).length, 1);
    assert.equal(f.trace().filter(row => row.event === "request" && row.optional === "commands").length, optionalBefore + 1);
  } finally { f.configure({}); await f.close(); }
});

function controlFixture(t) {
  const directory = mkdtempSync(join(tmpdir(), "cma-fixture-control-"));
  t.after(() => rmSync(directory, { recursive: true, force: true }));
  const path = join(directory, "control.json");
  const previous = { commands: "hold", epoch: 1, limit: 1 };
  writeFixtureControl(path, previous);
  return { path, previous, read: () => JSON.parse(readFileSync(path, "utf8")) };
}

test("control readers see complete snapshots during truncate, partial write and release", t => {
  const f = controlFixture(t);
  const observations = [];
  writeFixtureControl(f.path, {}, {
    writeFileSync(path, value) {
      writeFileSync(path, "");
      observations.push(f.read());
      appendFileSync(path, value.slice(0, 1));
      observations.push(f.read());
      appendFileSync(path, value.slice(1));
      observations.push(f.read());
    },
    renameSync(from, to) {
      renameSync(from, to);
      observations.push(f.read());
    },
  });
  assert.deepEqual(observations, [f.previous, f.previous, f.previous, {}]);
});

for (const operation of ["writeFileSync", "renameSync"]) {
  test("control " + operation + " failure preserves published controls and allows a later retry", t => {
    const f = controlFixture(t);
    const error = Object.assign(new Error("fixture control publication failed"), { code: "EIO" });
    const io = { writeFileSync, renameSync };
    io[operation] = path => {
      if (operation === "writeFileSync") writeFileSync(path, "{");
      throw error;
    };
    assert.throws(() => writeFixtureControl(f.path, {}, io), value => value === error);
    assert.deepEqual(f.read(), f.previous);
    writeFixtureControl(f.path, {});
    assert.deepEqual(f.read(), {});
  });
}

test("unserializable controls fail before changing the published snapshot", t => {
  const f = controlFixture(t);
  const circular = {}; circular.self = circular;
  assert.throws(() => writeFixtureControl(f.path, circular), /circular/i);
  assert.deepEqual(f.read(), f.previous);
  assert.equal(existsSync(f.path + ".next"), false);
});
