import assert from "node:assert/strict";
import test from "node:test";
import {
  assertConnectionProof,
  connectionCaseTitles,
  missingConnectionCoverage,
} from "./connection-journey-evidence.mjs";
import { connectionProof } from "./connection-journey-evidence.fixture.mjs";

const validate = (proof) =>
  assertConnectionProof(proof, {
    run: "local-run",
    caseId: proof.caseId,
    providerRevision: "a".repeat(40),
  });

for (const version of ['v1', 'v2']) test(`accepts ${version} Launch admission without changing connection proof requirements`, () => {
  for (const caseId of Object.keys(connectionCaseTitles)) {
    const proof = connectionProof(caseId);
    proof.commands[0].path = `/api/${version}/triggers/public/trg_local/launches`;
    assert.equal(validate(proof).caseId, caseId);
  }
});

for (const path of [
  '/api/v3/triggers/public/trg_local/launches',
  '/api/v2/triggers/public/trg_local/launches/extra',
  '/api/v2/triggers/public/trg_local/launches?extra=true',
  '/api/v2/launches/lch_local/start',
]) test(`refuses unsupported journey command ${path}`, () => {
  const proof = connectionProof();
  proof.commands[0].path = path;
  assert.throws(() => validate(proof));
});

test("validates each original connection case with separate browser and provider evidence", () => {
  for (const caseId of Object.keys(connectionCaseTitles))
    assert.equal(validate(connectionProof(caseId)).caseId, caseId);
});
test("refuses missing, foreign, stale or nonterminal provider observations", () => {
  const mutations = [
    (p) => {
      delete p.after;
    },
    (p) => {
      p.after.run = "another-run";
    },
    (p) => {
      p.after.caseId = "J4-api_key";
    },
    (p) => {
      p.after.phase = "before";
    },
    ...[
      "ownerDigest",
      "targetDigest",
      "serviceDigest",
      "intentDigest",
      "fixtureDigest",
    ].map((field) => (p) => {
      p.after[field] = "f".repeat(64);
    }),
    (p) => {
      p.after.providerRevision = "f".repeat(40);
    },
    (p) => {
      p.after.mode = "device_code";
    },
    (p) => {
      p.after.observedAt = "2025-12-31T23:59:00Z";
    },
    (p) => {
      p.after.observedAt = "invalid";
    },
    (p) => {
      delete p.after.evidenceDigest;
    },
    (p) => {
      p.after.evidenceDigest = p.before.evidenceDigest;
    },
    ...["pending", "expired", "cancelled"].map((state) => (p) => {
      p.after.links[0].state = state;
    }),
    (p) => {
      p.after.links = [];
    },
    (p) => {
      p.after.links.push({ linkDigest: "f".repeat(64), state: "completed" });
    },
    (p) => {
      p.after.links.push({ ...p.after.links[0] });
    },
    (p) => {
      p.before.links.push({ linkDigest: "f".repeat(64), state: "completed" });
    },
  ];
  for (const mutate of mutations) {
    const proof = connectionProof();
    mutate(proof);
    assert.throws(() => validate(proof));
  }
});
test("compares complete inventories while retaining pre-existing owner links", () => {
  const proof = connectionProof();
  const old = { linkDigest: "f".repeat(64), state: "completed" };
  proof.before.links.push(old);
  proof.after.links.unshift({ ...old });
  assert.doesNotThrow(() => validate(proof));
});
test("refuses secrets and unreviewed fields at every observation boundary", () => {
  for (const select of [
    (p) => p,
    (p) => p.before,
    (p) => p.after,
    (p) => p.after.links[0],
    (p) => p.commands[0],
  ]) {
    const proof = connectionProof();
    select(proof).hosted_url = "https://identity.example.test/connect";
    assert.throws(() => validate(proof));
  }
});
test("requires exact connection replay and counts verification separately from starts", () => {
  for (const caseId of ["FAULT-handoff-response", "FAULT-nyxid"]) {
    const proof = connectionProof(caseId);
    const starts = proof.commands.filter(
      (row) => row.path === "/auth/connections/start",
    );
    starts[1].bodyHash = "f".repeat(64);
    assert.throws(() => validate(proof), /original command/);
    const missing = connectionProof(caseId);
    missing.commands.splice(2, 1);
    assert.throws(() => validate(missing));
  }
  const checksOnly = connectionProof();
  checksOnly.commands = checksOnly.commands.map((row) =>
    row.path === "/auth/connections/start"
      ? { ...row, path: "/auth/connections/check" }
      : row,
  );
  assert.throws(() => validate(checksOnly));
});
test("retains original Launch, visitor owner and same-tab requirements", () => {
  const mutations = [
    (p) => {
      p.commands.push({ ...p.commands[0] });
    },
    (p) => {
      p.commands.at(-1).path = "/api/v1/launches/lch_foreign/start";
    },
    (p) => {
      p.commands.push({ ...p.commands.at(-1) });
    },
    (p) => {
      p.ownerVerified = false;
    },
    (p) => {
      p.sameTab = true;
    },
    (p) => {
      p.scope = "current_image_inline";
    },
    (p) => {
      p.execution = "local_fixture";
    },
    (p) => {
      p.variant = "callback_loss";
    },
  ];
  for (const mutate of mutations) {
    const p = connectionProof();
    mutate(p);
    assert.throws(() => validate(p));
  }
  const blocked = connectionProof("FAULT-popup-blocked");
  blocked.sameTab = false;
  assert.throws(() => validate(blocked));
});
test("six migrated cases cannot stand in for the full connection outcome matrix", () => {
  const missing = missingConnectionCoverage(
    Object.keys(connectionCaseTitles).map(connectionProof),
  );
  assert.equal(missing.length, 20);
  for (const mode of ["oauth", "api_key", "device_code"])
    for (const outcome of [
      "expired",
      "cancelled",
      "callback_loss",
      "callback_replay",
      "receipt_recovery",
      "owner_switch",
    ])
      assert.ok(missing.includes(`${mode}/${outcome}`));
  assert.ok(missing.includes("service_authorization/explicit_resume"));
  assert.ok(missing.includes("provider_link/cleanup_authority"));
  assert.throws(() =>
    missingConnectionCoverage([{ mode: "oauth", variant: "expired" }]),
  );
});
