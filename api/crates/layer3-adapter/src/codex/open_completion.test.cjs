const assert = require("node:assert/strict");
const { createHash } = require("node:crypto");
const { mkdtempSync, readFileSync, writeFileSync, rmSync } = require("node:fs");
const { test } = require("node:test");
const { bridgePath } = require("./persist_conversations.cjs");
const { openCompletion, withoutOpenCompletion, sourceSha256 } = require("./open_completion.cjs");
const { createOpenCompletionRuntime } = require("./open_completion_runtime.cjs");
const canonical = value => value === null || typeof value !== "object" ? JSON.stringify(value)
  : Array.isArray(value) ? "[" + value.map(canonical).join(",") + "]"
  : "{" + Object.keys(value).sort().map(key => JSON.stringify(key) + ":" + canonical(value[key])).join(",") + "}";
const digest = params => "v1:" + createHash("sha256").update(canonical({
  cwd: params.cwd, mcpServers: params.mcpServers ?? [],
})).digest("hex");
const request = () => ({ cwd: "/fixture", mcpServers: [{ name: "cma_access", type: "http",
  url: "http://127.0.0.1:3210/mcp", headers: [{ name: "Authorization", value: "Bearer open-fixture" }] }] });
const deferred = () => { let resolve; const promise = new Promise(done => { resolve = done; }); return { promise, resolve }; };

function fixture() {
  const native = new Map(), generations = new Map(), closing = new Set();
  let serial = 0;
  const state = { alive: true, configNames: new Set(), inventory: null, nativeCalls: 0, reads: 0 };
  const agent = { sessions: new Map(), getSessionGeneration: id => generations.get(id) ?? 0,
    sessionIsClosing: id => closing.has(id), codexAcpClient: {
      getConfigMcpServerNames: async () => state.configNames,
      createMcpSeverConfig: server => ({ url: server.url, required: server.name === "cma_access",
        http_headers: Object.fromEntries(server.headers.map(header => [header.name, header.value])) }),
      codexClient: { sendRequest: async frame => {
        assert.equal(frame.method, "thread/loaded/list");
        assert.deepEqual(frame.params, { limit: 128 });
        state.reads += 1;
        return state.inventory ?? { data: [...native.keys()], nextCursor: null };
      } },
    } };
  const proof = createOpenCompletionRuntime(require, agent, () => state.alive);
  const read = (id, params, challenge = "fixture_challenge_1234") => proof.read({
    sessionId: id, bindingDigest: digest(params), challenge,
  });
  async function open(kind, params, options = {}) {
    return proof.open(kind, params, async copied => {
      if (options.beforeNative) await options.beforeNative(copied);
      if (options.failBefore) throw new Error("setup failed");
      const config = { mcp_servers: Object.fromEntries(copied.mcpServers.map(server =>
        [server.name, agent.codexAcpClient.createMcpSeverConfig(server)])) };
      if (options.badConfig) config.mcp_servers = {};
      const result = await proof.nativeOpen(kind === "new" ? "new" : "resume", {
        threadId: copied.sessionId, cwd: copied.cwd, config,
      }, async () => {
        state.nativeCalls += 1;
        const id = kind === "new" ? "session-" + ++serial : copied.sessionId;
        if (!native.has(id)) native.set(id, { cwd: copied.cwd, config });
        return { thread: { id }, cwd: native.get(id).cwd };
      });
      const id = result.thread.id;
      const session = { cwd: result.cwd, sessionId: id };
      proof.installing(id, session);
      agent.sessions.set(id, session);
      if (options.afterInstall) await options.afterInstall(id, session);
      if (options.failHistory) throw new Error("history stream failed");
      return { sessionId: id };
    });
  }
  return { state, agent, proof, read, open, native, generations, closing };
}

test("open completion patch is pinned, reversible, and wired after full outer setup", () => {
  const installed = require("./deferred_session_metadata.cjs").withoutDeferredSessionMetadata(
    readFileSync(process.env.CMA_ACP_TEST_BRIDGE ?? bridgePath, "utf8"));
  const base = withoutOpenCompletion(installed);
  assert.equal(createHash("sha256").update(base).digest("hex"), sourceSha256);
  assert.equal(openCompletion(base), installed);
  assert.throws(() => openCompletion("unreviewed"), /Unexpected ACP source/);
  assert.throws(() => openCompletion(installed), /Unexpected ACP source/);
  assert(installed.includes('codexConnection.process.signalCode === null'));
  assert(installed.includes('codexConnection.process.once("exit", () => proof.retireAll())'));
});

test("proof binds immutable original descriptors and local reads have no native effects", async () => {
  const f = fixture(), params = request(), copied = structuredClone(params);
  const release = deferred(), entered = deferred();
  const work = f.open("new", params, { beforeNative: async () => { entered.resolve(); await release.promise; } });
  await entered.promise;
  params.mcpServers[0].headers[0].value = "Bearer changed-fixture";
  release.resolve();
  const { sessionId } = await work;
  const before = { native: f.state.nativeCalls, reads: f.state.reads };
  const receipt = f.read(sessionId, copied);
  assert.equal(receipt.complete, true);
  assert.equal(f.read(sessionId, params).complete, false);
  for (let index = 0; index < 100; index++) assert.deepEqual(f.read(sessionId, copied), receipt);
  assert.deepEqual({ native: f.state.nativeCalls, reads: f.state.reads }, before);
  assert(!JSON.stringify(receipt).includes("Bearer"));
  assert(!JSON.stringify(receipt).includes("127.0.0.1"));
});

test("exact cached setup carries prior proof and mismatched reuse cannot relabel it", async () => {
  const f = fixture(), params = request();
  const { sessionId } = await f.open("new", params);
  const original = f.read(sessionId, params);
  for (const kind of ["load", "resume"]) {
    await f.open(kind, { ...params, sessionId });
    const receipt = f.read(sessionId, params);
    assert.equal(receipt.complete, true);
    assert.equal(receipt.processNonce, original.processNonce);
    assert.notEqual(receipt.openGeneration, original.openGeneration);
  }
  const changed = request(); changed.mcpServers[0].headers[0].value = "Bearer newly-supplied";
  await f.open("load", { ...changed, sessionId });
  assert.equal(f.read(sessionId, changed).complete, false);
  assert.equal(f.read(sessionId, params).complete, false);
  await f.open("load", { ...params, sessionId });
  assert.equal(f.read(sessionId, params).complete, false, "retired proof cannot be recreated from cached state");
});

test("cold load and resume require complete empty native inventory", async () => {
  for (const kind of ["load", "resume"]) {
    const f = fixture(), params = { ...request(), sessionId: "persisted" };
    await f.open(kind, params);
    assert.equal(f.read(params.sessionId, params).complete, true);
    assert.equal(f.state.reads, 1);
  }
  for (const inventory of [{}, { data: [] , nextCursor: "more" }, { data: ["persisted"] },
    { data: ["unobserved-subagent"] }, { data: Array(129).fill("other") }]) {
    const f = fixture(), params = { ...request(), sessionId: "persisted" };
    f.state.inventory = inventory;
    await f.open("load", params);
    assert.equal(f.read(params.sessionId, params).complete, false);
  }
});

test("failure after native setup and history installation leaves no receipt", async () => {
  for (const options of [{ failBefore: true }, { failHistory: true }]) {
    const f = fixture(), params = { ...request(), sessionId: "persisted" };
    await assert.rejects(f.open("load", params, options), /failed/);
    assert.equal(f.read(params.sessionId, params).complete, false);
    if (options.failHistory) {
      await f.open("load", params);
      assert.equal(f.read(params.sessionId, params).complete, false);
    }
  }
});

test("close fences, native closure, process death, and state or generation replacement retire proof", async () => {
  for (const invalidate of [
    (f, id) => f.proof.retire(id),
    (f, id) => f.proof.observe({ eventType: "notification", method: "thread/closed", params: { threadId: id } }),
    f => { f.state.alive = false; },
    f => f.proof.retireAll(),
    (f, id) => f.agent.sessions.set(id, { ...f.agent.sessions.get(id) }),
    (f, id) => f.generations.set(id, 1),
    (f, id) => f.closing.add(id),
    (f, id) => { f.agent.sessions.get(id).cwd = "/different"; },
  ]) {
    const f = fixture(), params = request();
    const { sessionId } = await f.open("new", params);
    assert.equal(f.read(sessionId, params).complete, true);
    invalidate(f, sessionId);
    assert.equal(f.read(sessionId, params).complete, false);
  }
});

test("close or prompt racing completion cannot mint proof; opening retires prior proof immediately", async () => {
  for (const interrupt of [(f, id) => f.proof.retire(id), f => f.proof.promptStarting()]) {
    const f = fixture(), params = request();
    const { sessionId } = await f.open("new", params);
    const release = deferred(), entered = deferred();
    const loading = f.open("load", { ...params, sessionId }, { afterInstall: async () => {
      entered.resolve(); await release.promise;
    } });
    await entered.promise;
    assert.equal(f.read(sessionId, params).complete, false);
    interrupt(f, sessionId); release.resolve(); await loading;
    assert.equal(f.read(sessionId, params).complete, false);
  }
});

test("queued load/resume cannot reopen a target closed before dispatch or change files", async () => {
  const directory = mkdtempSync(require("node:path").join(require("node:os").tmpdir(), "cma-queued-close-"));
  const file = require("node:path").join(directory, "preserved-work.txt");
  try {
    for (const kind of ["load", "resume"]) {
      const f = fixture(), params = request();
      const target = await f.open("new", params);
      const release = deferred(), entered = deferred();
      const blocker = f.open("new", params, { afterInstall: async () => { entered.resolve(); await release.promise; } });
      await entered.promise;
      writeFileSync(file, "preserved");
      let dispatched = false;
      const queued = assert.rejects(f.open(kind, { ...params, sessionId: target.sessionId }, {
        beforeNative: () => { dispatched = true; writeFileSync(file, "unexpected setup"); },
      }), /Session changed while operation was queued/);
      f.generations.set(target.sessionId, 1); f.proof.retire(target.sessionId);
      f.agent.sessions.delete(target.sessionId); f.native.delete(target.sessionId);
      release.resolve(); await blocker; await queued;
      assert.equal(dispatched, false);
      assert.equal(f.state.nativeCalls, 2, "no native resume follows the completed close");
      assert.equal(f.agent.sessions.has(target.sessionId), false);
      assert.equal(f.native.has(target.sessionId), false);
      assert.equal(readFileSync(file, "utf8"), "preserved");
      assert.equal(f.read(target.sessionId, params).complete, false);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test("queued target work rejects changed state and closed fences even with unchanged generation", async () => {
  for (const replacement of [false, true]) {
    const f = fixture(), params = request();
    const { sessionId } = await f.open("new", params);
    const release = deferred();
    const blocker = f.proof.exclusive(() => release.promise);
    let applied = false;
    const queued = assert.rejects(f.proof.exclusive(() => { applied = true; }, sessionId),
      /Session changed while operation was queued/);
    if (replacement) f.agent.sessions.set(sessionId, { ...f.agent.sessions.get(sessionId) });
    else f.proof.retire(sessionId); // Instructions can close/reopen using the same state and ACP generation.
    release.resolve(); await blocker; await queued;
    assert.equal(applied, false);
  }
});

test("ambiguous descriptor names, inherited conflicts, and dropped native configuration refuse proof", async () => {
  const cases = [
    { configure: f => f.state.configNames.add("cma_access") },
    { options: { badConfig: true } },
    { mutate: params => { params.mcpServers.push(structuredClone(params.mcpServers[0])); } },
    { mutate: params => { params.mcpServers[0].name = "cma access"; } },
    { mutate: params => { params.mcpServers[0].name = "owner_tool"; } },
    { mutate: params => { params.mcpServers[0].type = "stdio"; } },
    { mutate: params => { params.mcpServers = []; } },
    { mutate: params => { params.mcpServers[0].headers[0].value = "x".repeat(65536); } },
  ];
  for (const value of cases) {
    const f = fixture(), params = request(); value.configure?.(f); value.mutate?.(params);
    const { sessionId } = await f.open("new", params, value.options);
    assert.equal(f.read(sessionId, params).complete, false);
  }
});

test("a native fork, review, or turn already in flight prevents cold absence proof", async () => {
  const f = fixture(), params = { ...request(), sessionId: "persisted" };
  const release = deferred();
  const nativeWork = f.proof.nativeWork(() => release.promise);
  await f.open("load", params);
  assert.equal(f.read(params.sessionId, params).complete, false);
  release.resolve(); await nativeWork;
});

test("each normal CMA tool family qualifies independently without requiring access", async () => {
  for (const name of ["cma_access", "cmaeg", "cma_channel"]) {
    const f = fixture(), params = request();
    params.mcpServers[0].name = name;
    if (name === "cmaeg") params.mcpServers[0].headers = [];
    const { sessionId } = await f.open("new", params);
    assert.equal(f.read(sessionId, params).complete, true);
    f.state.configNames.add(name);
    await f.open("load", { ...params, sessionId });
    assert.equal(f.read(sessionId, params).complete, false, "each family rejects inherited name conflicts");
  }
});

test("canonical binding digest agrees with fixed cross-language JSON vectors", async () => {
  const vectors = JSON.parse(readFileSync(require("node:path").join(__dirname, "open_completion_vectors.json"), "utf8"));
  for (const vector of vectors) {
    const f = fixture();
    assert.equal(canonical({ cwd: vector.input.cwd, mcpServers: vector.input.mcpServers ?? [] }), vector.canonical, vector.name);
    assert.equal(digest(vector.input), vector.bindingDigest, vector.name);
    if (vector.exerciseReceipt === false) continue;
    const { sessionId } = await f.open("new", vector.input);
    const receipt = f.proof.read({ sessionId, bindingDigest: vector.bindingDigest, challenge: "vector_challenge_1234" });
    assert.equal(receipt.complete, true, vector.name);
    assert.equal(receipt.bindingDigest, vector.bindingDigest, vector.name);
  }
});

test("current lists only this process's completed live opens for an interrupted first open", async () => {
  const f = fixture(), params = request(), challenge = "fixture_current_1234";
  assert.deepEqual(f.proof.current({ challenge }).sessions, []);
  const release = deferred(), entered = deferred();
  const work = f.open("new", params, { beforeNative: async () => { entered.resolve(); await release.promise; } });
  await entered.promise;
  // An open still running is reported so a caller waits instead of duplicating it.
  const inFlight = f.proof.current({ challenge });
  assert.equal(inFlight.opening, true);
  assert.deepEqual(inFlight.sessions, []);
  release.resolve();
  const { sessionId } = await work;
  const listed = f.proof.current({ challenge });
  assert.deepEqual({ ...listed, processNonce: undefined }, { version: 1, challenge, processNonce: undefined,
    opening: false, sessions: [{ sessionId, bindingDigest: digest(params) }] });
  assert.equal(listed.processNonce, f.read(sessionId, params).processNonce);
  assert(!JSON.stringify(listed).includes("Bearer"));
  // A closed session and a failed open never become adoptable.
  f.proof.retire(sessionId);
  await assert.rejects(f.open("new", params, { failHistory: true }));
  assert.deepEqual(f.proof.current({ challenge }).sessions, []);
  f.proof.retireAll();
  assert.deepEqual(f.proof.current({ challenge }).sessions, []);
});
