const assert = require("node:assert/strict");
const { test } = require("node:test");
const { createNativePromptStarter, createNativeStopCoordinator } = require("./native_bound_dispatch.cjs");

const key = `cma-prompt-v1-${"a".repeat(64)}`;
const turn = { threadId: "fixture-thread", input: [{ type: "text", text: "fixture" }] };

function fixture(discover, bound = () => ({ turn: { id: "bound-turn" } })) {
  const requests = [];
  let nativeWork = false;
  const client = {
    async turnStart(params) {
      requests.push({ method: "turn/start", params });
      return { turn: { id: "ordinary-turn" } };
    },
    async sendRequest(request) {
      requests.push(request);
      if (request.method === "turn/stopCapability") {
        assert.equal(nativeWork, false);
        return await discover();
      }
      assert.equal(request.method, "turn/startBound");
      assert.equal(nativeWork, true);
      return await bound();
    },
    cmaOpenCompletion: {
      async nativeWork(operation) {
        nativeWork = true;
        try {
          return await operation();
        } finally {
          nativeWork = false;
        }
      },
    },
  };
  return { client, requests };
}

test("disabled and unrecognized prompts keep the ordinary start request", async () => {
  const { client, requests } = fixture(() => assert.fail("unexpected discovery"));
  assert.deepEqual(await createNativePromptStarter(false)(client, turn, key), {
    turn: { id: "ordinary-turn" },
  });
  assert.deepEqual(await createNativePromptStarter(true)(client, turn, "fixture-id"), {
    turn: { id: "ordinary-turn" },
  });
  assert.deepEqual(requests, [
    { method: "turn/start", params: turn },
    { method: "turn/start", params: turn },
  ]);
});

test("discovery failure or an unsupported binding version preserves ordinary start", async () => {
  for (const discover of [() => { throw new Error("Unknown method"); }, () => ({ bindingVersion: 2 })]) {
    const { client, requests } = fixture(discover);
    assert.deepEqual(await createNativePromptStarter(true)(client, turn, key), {
      turn: { id: "ordinary-turn" },
    });
    assert.deepEqual(requests.map(({ method }) => method), ["turn/stopCapability", "turn/start"]);
  }
});

test("cancellation during discovery prevents either start", async () => {
  let release;
  const capability = new Promise((resolve) => { release = resolve; });
  const { client, requests } = fixture(() => capability);
  let cancelled = false;
  const pending = createNativePromptStarter(true)(client, turn, key, () => cancelled);
  cancelled = true;
  release({ bindingVersion: 1, listenerIncarnation: "fixture-listener" });
  assert.equal(await pending, null);
  assert.deepEqual(requests, [{ method: "turn/stopCapability", params: { threadId: turn.threadId } }]);
});

test("the exact key and listener are sent once and a lost reply never triggers ordinary start", async () => {
  let listenerIncarnation = "first-listener";
  const lost = new Error("Bound reply lost");
  const { client, requests } = fixture(
    () => ({ bindingVersion: 1, listenerIncarnation }),
    () => { throw lost; },
  );
  const start = createNativePromptStarter(true);
  await assert.rejects(start(client, turn, key), (error) => error === lost);
  listenerIncarnation = "second-listener";
  const secondKey = `cma-prompt-v1-${"b".repeat(64)}`;
  await assert.rejects(start(client, turn, secondKey), (error) => error === lost);
  const discovery = { method: "turn/stopCapability", params: { threadId: turn.threadId } };
  assert.deepEqual(requests, [
    discovery,
    { method: "turn/startBound", params: { turn, listenerIncarnation: "first-listener",
      dispatchSequence: 1, dispatchKey: key } },
    discovery,
    { method: "turn/startBound", params: { turn, listenerIncarnation: "second-listener",
      dispatchSequence: 2, dispatchKey: secondKey } },
  ]);
});

test("only an ordered fence for the requested turn retires its binding", async () => {
  const requests = [];
  const client = { async sendRequest(request) { requests.push(request); throw new Error("reply lost"); } };
  const target = { threadId: turn.threadId, turnId: "native-turn", dispatchKey: key,
    listenerIncarnation: "current-listener", dispatchSequence: 3 };
  const fence = { ...target, status: "stoppedWithoutCanonical" };
  const receipt = "e".repeat(32);
  const stops = createNativeStopCoordinator(true);
  stops.bind(client, target);
  assert.equal(stops.acceptFence(client, fence), null);
  assert.deepEqual(await stops.stop(client, turn.threadId, target.turnId, "wrong-key", receipt), { status: "hold" });
  assert.deepEqual(await stops.stop(client, turn.threadId, target.turnId, key, "invalid"), { status: "hold" });
  assert.deepEqual(await stops.stop(client, turn.threadId, target.turnId, key, receipt), { status: "hold" });
  assert.equal(stops.acceptFence(client, { ...fence, status: "canonical" }), null);
  assert.equal(stops.acceptFence(client, { ...fence, listenerIncarnation: "lost-listener" }), null);
  assert.equal(stops.acceptFence(client, { ...fence, dispatchSequence: 2 }), null);
  assert.equal(stops.acceptFence({}, fence), null);
  assert.deepEqual(stops.acceptFence(client, fence), { ...target, receipt });
  assert.equal(stops.acceptFence(client, fence), null);
  stops.release(client, target.threadId, target.turnId);
  assert.deepEqual(await stops.stop(client, turn.threadId, target.turnId, key, receipt), { status: "hold" });
  assert.deepEqual(requests, [{ method: "turn/stopAndSeal", params: target }]);
});
