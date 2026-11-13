// Local native/provider comparison; optional reply delays are deliberate fixtures.
const assert = require("node:assert/strict");
const { metadataFixture } = require("./deferred_session_fixture.cjs");

async function measure(samples = 20) {
  const f = await metadataFixture(), rows = [];
  const catalogue = frame => frame.params?.update?.sessionUpdate === "available_commands_update";
  const goal = frame => Object.hasOwn(frame.params?.update?._meta ?? {}, "goal");
  try {
    await f.bridge.call("initialize", { protocolVersion: 1, clientCapabilities: {} });
    const params = { cwd: f.directory, mcpServers: [] };
    const { sessionId } = await f.bridge.call("session/new", params);
    await f.waitFor(() => f.state.frames.some(catalogue));
    for (let sample = 0; sample < samples; sample++) {
      await f.bridge.call("session/close", { sessionId }); f.state.frames.length = 0;
      f.configure({ commands: 220, goal: 180, epoch: sample, limit: 1 });
      const start = performance.now();
      await f.bridge.call("session/load", { ...params, sessionId });
      const load = performance.now();
      await f.bridge.call("session/prompt", { sessionId, prompt: [{ type: "text", text: "Complete the offline first-turn fixture." }] });
      const prompt = performance.now();
      if (f.state.error) throw f.state.error;
      assert.equal(f.state.modelCalls.at(-1).client_metadata.session_id, sessionId);
      await f.waitFor(() => f.state.frames.some(catalogue) && f.state.frames.some(goal));
      rows.push({ sample, load_ms: load - start, immediate_first_turn_ms: prompt - load,
        load_through_first_turn_ms: prompt - start });
    }
    const summary = {};
    for (const key of ["load_ms", "immediate_first_turn_ms", "load_through_first_turn_ms"]) {
      const values = rows.map(row => row[key]).sort((a, b) => a - b);
      summary[key] = { n: samples, p50: values[Math.ceil(samples * .5) - 1],
        p95: values[Math.ceil(samples * .95) - 1], max: values.at(-1) };
    }
    return { samples, native_version: "0.147.0 or 0.165.0 verified by fixture", injected_reply_ms: { commands: 220, goal: 180 },
      bridge: process.env.CMA_ACP_TEST_BRIDGE, summary, rows, synthetic_provider: true,
      production_or_end_to_end_acceptance: false };
  } finally { f.configure({}); await f.close(); }
}

module.exports = { measure };
if (require.main === module) measure().then(result => console.log(JSON.stringify(result, null, 2))).catch(error => {
  console.error(error); process.exitCode = 1;
});
