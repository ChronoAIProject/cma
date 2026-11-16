import test from "node:test";
import assert from "node:assert/strict";
import { once } from "node:events";
import http from "node:http";
import { capacityProbe, holdBody, assertSameServing } from "./capacity.mjs";
import { SOURCE } from "./matrix.mjs";
import { API } from "../cma/core.mjs";

const image = `fixture/am@sha256:${"a".repeat(64)}`;
// Transport regression only: this server is not an AM and creates no product
// compatibility PASS. It catches body completion, socket leakage and bad receipts.
async function fixture({ code = "file_capacity", requestId = true } = {}) {
  const sockets = new Set(),
    held = new Set(),
    completedBodies = [];
  let sequence = 0;
  const server = http.createServer((request, response) => {
    if (request.method === "POST" || request.method === "PUT") {
      held.add(request);
      request.on("close", () => held.delete(request));
      let body = "";
      request.on("data", (chunk) => {
        body += chunk;
      });
      request.on("end", () => {
        completedBodies.push(body);
        response.writeHead(200).end("{}");
      });
      return;
    }
    response
      .writeHead(held.size === 2 ? 429 : 200, {
        "content-type": "application/problem+json",
        ...(requestId ? { "x-request-id": `request-${++sequence}` } : {}),
      })
      .end(JSON.stringify({ code: held.size === 2 ? code : "ok" }));
  });
  server.on("connection", (socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const origin = `http://127.0.0.1:${server.address().port}`;
  const api = new API({
    ams: { a: origin, b: origin },
    bearer: "disposable-fixture-only",
    host: "native-am.local.test",
    public_origin: "http://native-am.local.test",
  });
  const sampleServing = async () => ({
    am: "a",
    origin,
    source: SOURCE,
    image,
    ready: true,
    draining: false,
    pod_uid: "a-pod",
    instance_id: "a-instance",
    container_id: "a-container",
    observed_at: new Date().toISOString(),
  });
  const records = [];
  const options = {
    api,
    am: "a",
    agentId: "fixture-agent",
    runtimeIncarnation: "runtime",
    image,
    sampleServing,
    record: async (value) => {
      records.push(value);
    },
    whileFull: async () => {
      assert.equal(held.size, 2);
      return { fixture_controls_called: true };
    },
  };
  return {
    options,
    records,
    completedBodies,
    held,
    close: async () => {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

test("real HTTP holders saturate both routes without completing a body and always join cleanup", async () => {
  const f = await fixture();
  try {
    const result = await capacityProbe(f.options);
    assert.equal(result.refusals.length, 2);
    assert.ok(result.holders_closed);
    assert.deepEqual(f.completedBodies, []);
    assert.equal(f.records.at(-1).joined, true);
    // A completed GET after holders drain must work; no application mutation.
    await new Promise((resolve) => setTimeout(resolve, 20));
    assert.equal(
      (
        await f.options.api.rawRequest(
          "a",
          "GET",
          "/api/v1/agents/fixture-agent/files/tree",
        )
      ).status,
      200,
    );
  } finally {
    await f.close();
  }
});

for (const [name, setup, message] of [
  ["generic 429", { code: "busy" }, "typed_file_capacity_required"],
  ["missing request identity", { requestId: false }, "request_id_missing"],
  ["failed control effect", {}, "actual_control_failed"],
]) {
  test(`refuses ${name} and closes both incomplete requests`, async () => {
    const f = await fixture(setup);
    try {
      if (name === "failed control effect")
        f.options.whileFull = async () => {
          throw new Error("actual_control_failed");
        };
      await assert.rejects(capacityProbe(f.options), new RegExp(message));
      assert.deepEqual(f.completedBodies, []);
      assert.equal(f.records.at(-1).joined, true);
      assert.ok(f.records.at(-1).holders.every((holder) => holder.closed));
    } finally {
      await f.close();
    }
  });
}

test("refuses a nonlocal target before opening a socket", async () => {
  await assert.rejects(
    holdBody({
      url: "https://example.com/api/v1/agents/a/files",
      headers: {},
      method: "POST",
      prefix: '{"path":',
    }),
    /loopback_http_required/,
  );
});

test("full JSON body cannot be passed to the incomplete-body holder", async () => {
  await assert.rejects(
    holdBody({
      url: "http://127.0.0.1:1/api/v1/agents/a/files",
      headers: {},
      method: "POST",
      prefix: '{"path":"a"}',
    }),
    /incomplete_json_prefix_required/,
  );
});

test("rejects a replaced process behind an unchanged ingress label", async () => {
  const before = {
    am: "a",
    origin: "http://127.0.0.1:1",
    source: SOURCE,
    image,
    ready: true,
    draining: false,
    pod_uid: "pod",
    instance_id: "instance",
    container_id: "container",
    observed_at: new Date().toISOString(),
  };
  assert.throws(
    () =>
      assertSameServing(
        before,
        { ...before, container_id: "replacement" },
        { am: "a", origin: before.origin, image },
      ),
    /serving_process_changed/,
  );
});
