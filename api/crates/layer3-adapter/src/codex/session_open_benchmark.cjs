// Run with the same pinned native binary and each before/after ACP composition.
// This measures protocol session RPCs, not lifecycle admission or browser Ready.
const { metadataFixture } = require("./deferred_session_fixture.cjs");

(async () => {
  const f = await metadataFixture(), samples = [];
  const count = Number(process.env.CMA_SESSION_SAMPLES ?? 12);
  const modelDelay = Number(process.env.CMA_SESSION_MODEL_DELAY_MS ?? 0);
  try {
    await f.bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    for (let i = -2; i < count; i++) {
      f.configure({ models: modelDelay, epoch: i });
      const params = { cwd: f.directory, mcpServers: [] };
      let start = performance.now();
      const { sessionId } = await f.bridge.call("session/new", params);
      const fresh = performance.now() - start;
      await f.bridge.call("session/close", { sessionId });
      start = performance.now();
      await f.bridge.call("session/load", { ...params, sessionId });
      const load = performance.now() - start;
      await f.bridge.call("session/close", { sessionId });
      if (i >= 0) samples.push({ new_ms: fresh, load_ms: load });
    }
    const summary = key => {
      const sorted = samples.map(row => row[key]).sort((a, b) => a - b);
      return { n: sorted.length, p50: sorted[Math.ceil(sorted.length * 0.5) - 1], p95: sorted[Math.ceil(sorted.length * 0.95) - 1] };
    };
    console.log(JSON.stringify({ model_reply_delay_ms: modelDelay, new: summary("new_ms"), load: summary("load_ms"), samples }, null, 2));
  } finally { f.configure({}); await f.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
