const assert = require("node:assert/strict");
const { test } = require("node:test");
const { createPromptOutputBuffer } = require("./prompt_output_buffer.cjs");

const currentTurn = { threadId: "thread", turnId: "current" };
const oldTurn = { threadId: "thread", turnId: "retired" };
const frame = (identity, text, method = "item/agentMessage/delta") => ({ identity, text, method });

function fixture() {
  const jobs = [];
  const seen = [];
  let active = true;
  let overflows = 0;
  const output = createPromptOutputBuffer(job => jobs.push(job), event => event.identity,
    () => active, () => overflows++);
  const route = output.route(async event => { seen.push(event.text); });
  return { output, route, seen, run: async () => {
    for (const job of jobs.splice(0)) await job();
  }, replace: () => { active = false; }, overflows: () => overflows };
}

test("known turn identity releases matching early frames once in order", async () => {
  const f = fixture();
  await f.route(frame(oldTurn, "stale"));
  await f.route(frame(currentTurn, "first"));
  await f.route(frame({ threadId: "other-thread", turnId: "current" }, "wrong thread"));
  await f.route(frame({ threadId: "thread", turnId: null }, "session metadata", "thread/name/updated"));
  await f.route(frame(currentTurn, "second"));
  assert.deepEqual(f.seen, ["session metadata"]);
  f.output.bind(currentTurn);
  await f.route(frame(currentTurn, "third"));
  await f.run();
  assert.deepEqual(f.seen, ["session metadata", "first", "second", "third"]);
});

test("only known session methods can publish without a turn identity", async () => {
  const f = fixture();
  const noTurn = { threadId: "thread", turnId: null };
  await f.route(frame(noTurn, "unbound model output"));
  await f.route(frame(noTurn, "unknown method", "unreviewed/notification"));
  await f.route(frame(noTurn, "title", "thread/name/updated"));
  await f.route(frame(currentTurn, "matching text"));
  f.output.bind(currentTurn);
  await f.route(frame(noTurn, "unqualified tool output", "item/commandExecution/outputDelta"));
  await f.route(frame(noTurn, "unqualified terminal", "turn/completed"));
  await f.route(frame(noTurn, "late unqualified model output"));
  await f.route(frame(currentTurn, "matching terminal", "turn/completed"));
  await f.run();
  assert.deepEqual(f.seen, ["title", "matching text", "matching terminal"]);
});

test("start confirmation drains early output with no subsequent frame", async () => {
  const f = fixture();
  await f.route(frame(currentTurn, "early"));
  f.output.bind(currentTurn);
  await f.run();
  assert.deepEqual(f.seen, ["early"]);
});

test("cancellation, start failure, and prompt replacement discard unbound output", async () => {
  for (const stop of [f => f.output.cancelPending(), f => f.output.close(), f => f.replace()]) {
    const f = fixture();
    await f.route(frame(currentTurn, "must be retired"));
    stop(f);
    f.output.bind(currentTurn);
    await f.run();
    assert.deepEqual(f.seen, []);
  }
});

test("a follow-up native turn within one prompt binds its own output", async () => {
  const f = fixture();
  f.output.bind(oldTurn);
  await f.route(frame(oldTurn, "first turn"));
  f.output.nextTurn();
  await f.route(frame(oldTurn, "late"));
  await f.route(frame(currentTurn, "second turn"));
  f.output.bind(currentTurn);
  await f.run();
  assert.deepEqual(f.seen, ["first turn", "second turn"]);
});

test("either bound reports one overflow and discards unqualified frames", async () => {
  for (const texts of [Array(129).fill("a frame"), ["x".repeat(1024 * 1024)]]) {
    const f = fixture();
    for (const text of texts) await f.route(frame(currentTurn, text));
    await f.route(frame(currentTurn, "another overflow"));
    f.output.bind(currentTurn);
    await f.run();
    assert.deepEqual(f.seen, []);
    assert.equal(f.overflows(), 1);
    assert.equal(f.output.hasLostOutput(), true);
  }
});
