const assert = require("node:assert/strict");
const { test } = require("node:test");
const { readFileSync } = require("node:fs");
const { runInNewContext } = require("node:vm");
const { setImmediate: tick, setTimeout: delay } = require("node:timers/promises");
const { bridgePath } = require("./persist_conversations.cjs");
const { withoutPromptOutputIdentity } = require("./prompt_output_identity.cjs");
const { sessionOpenOverlap, withoutSessionOpenOverlap } = require("./session_open_overlap.cjs");
const { metadataFixture } = require("./deferred_session_fixture.cjs");

const source = withoutPromptOutputIdentity(readFileSync(process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath, "utf8"));
const base = withoutSessionOpenOverlap(source);
const candidate = sessionOpenOverlap(base);
const start = candidate.indexOf("  async resumeSession(request, onSubscribed) {");
const end = candidate.indexOf("  async closeSession(sessionId) {", start);
assert(start > 0 && end > start);
const Client = runInNewContext("(class {" + candidate.slice(start, end) + "})", {
  readAdditionalDirectories: () => [],
});
const deferred = () => Promise.withResolvers();
const request = { cwd: "/fixture", sessionId: "same-session", mcpServers: [] };
const response = { thread: { id: request.sessionId }, model: "gpt-6", modelProvider: "cma" };

function fixture() {
  const primary = deferred(), models = deferred(), events = [];
  const client = new Client();
  Object.assign(client, {
    refreshSkills: async () => { events.push("skills"); },
    createSessionConfig: async () => ({}),
    getModelProvider: () => "cma", getResumeModelProvider: async () => "cma",
    getCollaborationMode: () => "default", createModelId: () => "gpt-6",
    fetchAvailableModels: () => { events.push("models"); return models.promise; },
    codexClient: {
      threadStart: async () => { events.push("start"); return response; },
      threadResume: async () => { events.push("resume"); return response; },
      sendRequest: async value => {
        assert.equal(value.method, "thread/name/set");
        assert.equal(value.params.threadId, request.sessionId);
        events.push("persist"); return primary.promise;
      },
      threadRead: async value => {
        assert.deepEqual(JSON.parse(JSON.stringify(value)), { threadId: request.sessionId, includeTurns: true });
        events.push("history"); await primary.promise; return { thread: response.thread };
      },
    },
  });
  return { client, primary, models, events };
}

test("session overlap accepts only pinned composition and reverses exactly", () => {
  assert.equal(candidate, source);
  assert.equal(withoutSessionOpenOverlap(candidate), base);
  assert.throws(() => sessionOpenOverlap("unexpected"), /Unexpected ACP source/);
  assert.throws(() => sessionOpenOverlap(candidate), /Unexpected ACP source/);
  assert.throws(() => withoutSessionOpenOverlap(candidate.replace("open(), this.fetchAvailableModels()", "open(), []")));
});

for (const name of ["newSession", "loadSession", "resumeSession"]) {
  test(name + " overlaps models but waits for exact session evidence", async () => {
    const f = fixture(); let settled = false, subscribed = 0;
    const result = f.client[name](request, () => subscribed++).finally(() => { settled = true; });
    await tick();
    assert(f.events.includes("models"));
    assert(f.events.includes(name === "newSession" ? "persist" : "resume"));
    f.models.resolve([{ id: "gpt-6" }]); await tick();
    if (name !== "resumeSession") assert.equal(settled, false);
    f.primary.resolve();
    assert.equal((await result).sessionId, request.sessionId);
    assert.equal(subscribed, name === "newSession" ? 0 : 1);
    assert.equal(f.events.filter(e => e === "models").length, 1);
    assert.equal(f.events[0], "skills");
  });

  test(name + " rejects required skill refresh before discovery or native open", async () => {
    const f = fixture();
    f.client.refreshSkills = async () => { throw new Error("required skills"); };
    await assert.rejects(f.client[name](request), /required skills/);
    assert.deepEqual(f.events, []);
  });
}

for (const name of ["newSession", "loadSession"]) {
  test(name + " model failure waits for native work and preserves native error precedence", async () => {
    const f = fixture(); let settled = false;
    const outcome = f.client[name](request).then(() => "success", error => error.message)
      .finally(() => { settled = true; });
    await tick(); f.models.reject(new Error("discovery failed")); await tick();
    assert.equal(settled, false, "failed discovery cannot abandon required native work");
    f.primary.reject(new Error("native failed"));
    assert.equal(await outcome, "native failed");
  });

  test(name + " successful native work cannot hide discovery failure", async () => {
    const f = fixture();
    const outcome = f.client[name](request);
    f.primary.resolve(); f.models.reject(new Error("discovery failed"));
    await assert.rejects(outcome, /discovery failed/);
  });
}

test("native new and load keep persistence/history and model replies as completion barriers", { timeout: 60000 }, async () => {
  const f = await metadataFixture();
  try {
    await f.bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    f.configure({ persist: "hold", models: "hold", epoch: 1 });
    let complete = false;
    const opening = f.bridge.call("session/new", { cwd: f.directory, mcpServers: [] })
      .then(value => { complete = true; return value; });
    await f.waitFor(() => ["persist", "models"].every(kind => f.trace().some(row => row.event === "held" && row.kind === kind)));
    assert.equal(complete, false);
    f.configure({ persist: "hold" });
    await f.waitFor(() => f.trace().some(row => row.event === "released" && row.kind === "models"));
    await delay(20); assert.equal(complete, false, "required persistence still gates session/new");
    f.configure({}); const { sessionId } = await opening;
    await f.bridge.call("session/close", { sessionId });
    f.configure({ history: "hold", models: "hold", epoch: 2 });
    complete = false;
    const loading = f.bridge.call("session/load", { cwd: f.directory, sessionId, mcpServers: [] })
      .then(value => { complete = true; return value; });
    await f.waitFor(() => f.trace().some(row => row.event === "held" && row.kind === "history"));
    assert.equal(complete, false);
    f.configure({ models: "hold" });
    await f.waitFor(() => f.trace().some(row => row.event === "released" && row.kind === "history"));
    await delay(20); assert.equal(complete, false, "model discovery still gates session/load");
    f.configure({}); await loading;
  } finally { f.configure({}); await f.close(); }
});
