import test from "node:test";
import assert from "node:assert/strict";
import { CLIENTS, SOURCE, matrix, missingBindings, plan } from "./matrix.mjs";
import { validateLifetime } from "./observe-lifetime.mjs";

const sha = `sha256:${"1".repeat(64)}`;
function binding() {
  const image = `fixture/am@${sha}`;
  const runtime = {
    image: `fixture/runtime@${sha}`,
    source: "2".repeat(40),
    boot_id: "boot",
    inventory_sha256: sha,
    ownership_floor_receipt_sha256: sha,
  };
  return {
    api: { source: SOURCE, image },
    environment: { kind: "local-kind", sole_parent: "local-verifier-02" },
    ams: Object.fromEntries(
      ["a", "b"].map((am) => [
        am,
        {
          source: SOURCE,
          image,
          ready: true,
          draining: false,
          pod_uid: `${am}-pod`,
          instance_id: `${am}-instance`,
        },
      ]),
    ),
    runtimes: {
      native: { previous: { ...runtime }, current: { ...runtime } },
      hosted: { previous: { ...runtime }, current: { ...runtime } },
    },
    clients: Object.fromEntries(
      Object.entries(CLIENTS).map(([name, source]) => [
        name,
        {
          source,
          kind: "compiled-web-with-novnc",
          artifact_sha256: sha,
          build_receipt_sha256: sha,
        },
      ]),
    ),
    parent_contract_sha256: sha,
    legacy_transition_contract_sha256: sha,
  };
}

test("matrix covers every family/release/client cell without promoting bindings to results", () => {
  const rows = matrix();
  assert.equal(rows.length, 48);
  assert.equal(new Set(rows.map((row) => row.id)).size, 48);
  assert.equal(plan().counts.blocked, 48);
  const ready = plan(binding());
  assert.equal(ready.counts.blocked, 0);
  assert.equal(ready.counts.passed, 0);
  assert.ok(ready.rows.every((row) => row.status === "NOT_RUN"));
});

for (const [name, change, expected] of [
  [
    "wrong source image",
    (b) => {
      b.api.source = "3".repeat(40);
    },
    "exact_e4047c926_image",
  ],
  [
    "old preparation module only",
    (b) => {
      b.clients.previous.kind = "typescript-preparation-module";
    },
    "full_previous_desktop_executable",
  ],
  [
    "unqualified receiver",
    (b) => {
      delete b.runtimes.native.previous.ownership_floor_receipt_sha256;
    },
    "receiver_ownership_floor_receipt",
  ],
  [
    "two labels on one process",
    (b) => {
      b.ams.b.pod_uid = b.ams.a.pod_uid;
    },
    "distinct_AM_processes",
  ],
  [
    "a draining AM",
    (b) => {
      b.ams.a.draining = true;
    },
    "ready_exact_AM_a",
  ],
  [
    "runtime tag only",
    (b) => {
      b.runtimes.native.previous.image = "fixture/runtime:previous";
    },
    "retained_native_previous_executable_inventory",
  ],
  [
    "production fixture",
    (b) => {
      b.environment.kind = "production";
    },
    "sole_parent_local_kind_binding",
  ],
]) {
  test(`blocks ${name}`, () => {
    const b = binding();
    change(b);
    assert.ok(missingBindings(b, matrix()[0]).includes(expected));
  });
}

test("replica-ready binding cannot silently substitute for the legacy ninety-second transition", () => {
  const b = binding();
  delete b.legacy_transition_contract_sha256;
  const result = plan(b);
  assert.equal(result.counts.blocked, 8);
  assert.ok(
    result.rows
      .filter((row) => row.status === "BLOCKED")
      .every((row) => row.scenario === "legacy-90s-transition"),
  );
});

const lifetime = () => ({
  elapsed_ms: 1800010,
  close_initiator: "server",
  socket_ids: ["original"],
  samples: 361,
  max_gap_ms: 5100,
});
test("complete thirty-minute receipt is structurally valid, without implying a real execution", () => {
  assert.doesNotThrow(() => validateLifetime(lifetime()));
});
for (const [name, patch, reason] of [
  ["shortened clock", { elapsed_ms: 90000 }, "early_socket_close"],
  ["no upper bound", { elapsed_ms: 1900000 }, "socket_lifetime_overrun"],
  [
    "client-initiated close",
    { close_initiator: "client" },
    "server_close_not_observed",
  ],
  [
    "automatic reconnect",
    { socket_ids: ["original", "replacement"] },
    "original_socket_not_preserved",
  ],
  ["final snapshot only", { samples: 1 }, "lifetime_observation_gap"],
  [
    "missing observation interval",
    { max_gap_ms: 18000 },
    "lifetime_observation_gap",
  ],
]) {
  test(`rejects lifetime evidence with ${name}`, () => {
    assert.throws(
      () => validateLifetime({ ...lifetime(), ...patch }),
      new RegExp(reason),
    );
  });
}
