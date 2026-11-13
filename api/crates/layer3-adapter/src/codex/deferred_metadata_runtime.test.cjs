const assert = require("node:assert/strict");
const { test } = require("node:test");
const { setImmediate: tick } = require("node:timers/promises");
const { createDeferredMetadata } = require("./deferred_metadata_runtime.cjs");
const deferred = () => {
  let resolve, reject;
  const promise = new Promise((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

test("metadata does not start synchronously and stays ordered within a load", async () => {
  const events = [], gate = deferred(), state = { sessionId: "one" };
  const schedule = createDeferredMetadata({ current: () => true,
    commands: async () => { events.push("commands"); await gate.promise; },
    goal: async () => { events.push("goal"); }, report: outcome => assert.fail(outcome) });
  schedule(state, 1); assert.deepEqual(events, []);
  await tick(); assert.deepEqual(events, ["commands"]);
  gate.resolve(); await tick(); assert.deepEqual(events, ["commands", "goal"]);
});

test("replacement coalesces to the latest exact state and drops stale results", async () => {
  const first = { sessionId: "same" }, second = { sessionId: "same" }, last = { sessionId: "same" };
  let active = first;
  const gate = deferred(), reads = [], publications = [], goals = [];
  const schedule = createDeferredMetadata({ current: state => state === active,
    commands: async (state, current) => {
      reads.push(state); if (state === first) await gate.promise;
      if (current()) publications.push(state);
    }, goal: async state => goals.push(state), report: outcome => assert.fail(outcome) });
  schedule(first, 0); await tick();
  active = second; schedule(second, 1); schedule(second, 1);
  active = last; schedule(last, 2); schedule(last, 2);
  assert.deepEqual(reads, [first]);
  gate.resolve(); await tick();
  assert.deepEqual(reads, [first, last]); assert.deepEqual(publications, [last]);
  assert.deepEqual(goals, [last]);
});

test("close before dispatch or during read suppresses metadata and goal", async () => {
  for (const during of [false, true]) {
    let live = true, reads = 0, published = 0, goals = 0;
    const gate = deferred();
    const schedule = createDeferredMetadata({ current: () => live,
      commands: async (_, current) => { reads++; await gate.promise; if (current()) published++; },
      goal: async () => { goals++; }, report: outcome => assert.fail(outcome) });
    schedule({ sessionId: "closed" }, 0);
    if (during) await tick();
    live = false; gate.resolve(); await tick();
    assert.equal(reads, Number(during)); assert.equal(published, 0); assert.equal(goals, 0);
  }
});

test("blocked work is bounded, duplicates coalesce, failures release capacity", async () => {
  const gate = deferred(), state = { sessionId: "first" }, reports = [];
  let reads = 0;
  const schedule = createDeferredMetadata({ current: () => true,
    commands: async () => { reads++; await gate.promise; }, goal: async () => {},
    report: outcome => reports.push(outcome) }, 1);
  schedule(state, 0); await tick();
  for (let i = 0; i < 100; i++) schedule(state, 0);
  schedule({ sessionId: "overflow" }, 0);
  assert.equal(reads, 1); assert.deepEqual(reports, ["capacity"]);
  gate.reject(new Error("fixture failure")); await tick();
  assert.deepEqual(reports, ["capacity", "failed"]);
  schedule({ sessionId: "after" }, 0); await tick();
  assert.equal(reads, 2); assert.deepEqual(reports, ["capacity", "failed", "failed"]);
});
