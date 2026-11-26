import assert from "node:assert/strict";
import test from "node:test";
import { validateTC117Evidence } from "./visitor-abuse-campaign.mjs";
import { campaignFixture } from "./visitor-abuse-campaign.fixture.mjs";

const validate = (f) =>
  validateTC117Evidence(f.summary, f.artifacts, f.journal, f.context, f.now);
test("consistent candidate-bound artifacts remain explicitly unqualified without external authority", () => {
  const f = campaignFixture();
  const result = validate(f);
  assert.equal(result.status, "consistent");
  assert.equal(result.fullCampaign, false);
  assert.equal(result.provenanceVerified, false);
  assert.equal(result.scope, "artifact-consistency-only");
  assert.equal(result.resources, 5);
  assert.equal(result.N, 8);
});
for (const [name, change] of [
  [
    "contradictory overall status",
    (f) => {
      f.summary.status = "failed";
    },
  ],
  ["missing case", (f) => f.summary.cases.pop()],
  ["duplicated case", (f) => f.summary.cases.push(f.summary.cases[0])],
  [
    "unknown case",
    (f) => {
      f.summary.cases[0].case = "TC117-unknown";
    },
  ],
  [
    "retried case",
    (f) => {
      f.summary.cases[0].retry = 1;
    },
  ],
  [
    "skipped case",
    (f) => {
      f.summary.cases[0].status = "skipped";
    },
  ],
  [
    "cancelled runner",
    (f) => {
      f.summary.scenarioStatus = "failed";
    },
  ],
  [
    "self qualification",
    (f) => {
      f.summary.fullCampaign = true;
    },
  ],
  [
    "self provenance",
    (f) => {
      f.summary.provenanceVerified = true;
    },
  ],
  [
    "wrong run",
    (f) => {
      f.context.run = "different-run";
    },
  ],
  [
    "wrong candidate",
    (f) => {
      f.context.commit = "f".repeat(40);
    },
  ],
  [
    "wrong NyxID candidate",
    (f) => {
      f.context.nyxidCommit = "f".repeat(40);
    },
  ],
  [
    "changed candidate",
    (f) => {
      f.context.after.receipt.images.api.configDigest = `sha256:${"f".repeat(64)}`;
    },
  ],
  [
    "replayed candidate nonce",
    (f) => {
      f.context.after = structuredClone(f.context.before);
    },
  ],
  [
    "expired after observation",
    (f) => {
      f.now += 301000;
    },
  ],
  [
    "missing raw artifact",
    (f) => {
      delete f.artifacts["tc117-utc.json"];
    },
  ],
  [
    "changed raw bytes",
    (f) => {
      f.artifacts["tc117-utc.json"] = Buffer.from("{}");
    },
  ],
  [
    "extra raw artifact",
    (f) => {
      f.artifacts["extra.json"] = Buffer.from("{}");
    },
  ],
  [
    "unsafe filename",
    (f) => {
      f.summary.evidence[0].file = "../other.json";
    },
  ],
  [
    "wrong raw digest",
    (f) => {
      f.summary.evidence[0].sha256 = "0".repeat(64);
    },
  ],
  [
    "wrong totals",
    (f) => {
      f.summary.counts.overquotaAttempts++;
    },
  ],
  [
    "same serving origin",
    (f) => {
      f.context.instances[1] = f.context.instances[0];
    },
  ],
  [
    "same subject",
    (f) => {
      f.context.subjects.utc = f.context.subjects.a;
    },
  ],
])
  test(`refuses ${name}`, () => {
    const f = campaignFixture();
    change(f);
    assert.throws(() => validate(f));
  });
for (const [name, change] of [
  [
    "wrong record run",
    (f) => {
      f.scenarios.utc.run = "wrong-run";
    },
  ],
  [
    "wrong record case",
    (f) => {
      f.scenarios.utc.case = "shared";
    },
  ],
  [
    "wrong record origin",
    (f) => {
      f.scenarios.utc.origin = "https://other.example.test";
    },
  ],
  [
    "wrong record instance",
    (f) => {
      f.scenarios.utc.instances = [
        "https://other.example.test",
        f.context.instances[1],
      ];
    },
  ],
  [
    "empty samples",
    (f) => {
      f.scenarios.utc.N = 0;
      f.scenarios.utc.requests = [];
    },
  ],
  [
    "inconsistent sample count",
    (f) => {
      f.scenarios.utc.N++;
    },
  ],
  [
    "inconsistent instance totals",
    (f) => {
      f.scenarios.utc.perInstance[0].responses++;
    },
  ],
  [
    "only one instance",
    (f) => {
      f.scenarios.utc.requests[1].instance = 0;
    },
  ],
  [
    "scenario before attestation",
    (f) => {
      f.scenarios.utc.startedAt = "2026-01-01T23:57:00Z";
    },
  ],
  [
    "scenario after attestation",
    (f) => {
      f.scenarios.utc.endedAt = "2026-01-02T00:03:00Z";
    },
  ],
  [
    "no quota samples",
    (f) => {
      for (const row of Object.values(f.scenarios))
        row.counts.overquotaAttempts = row.counts.overquotaRejects = 0;
    },
  ],
  [
    "quota overshoot",
    (f) => {
      f.scenarios.shared.counts.overquotaRejects = 0;
    },
  ],
  [
    "idempotency duplicate",
    (f) => {
      f.scenarios.utc.counts.idempotentChargeDuplicates = 1;
    },
  ],
  [
    "anonymous side effect",
    (f) => {
      f.scenarios["load-controls"].counts.anonymousSideEffects = 1;
    },
  ],
  [
    "transport error",
    (f) => {
      f.scenarios.utc.requests[1].status = 0;
    },
  ],
  [
    "server error",
    (f) => {
      f.scenarios.utc.requests[1].status = 503;
    },
  ],
  [
    "negative timing",
    (f) => {
      f.scenarios.utc.requests[1].ms = -1;
    },
  ],
  [
    "unknown actor",
    (f) => {
      f.scenarios.utc.requests[0].actor = "other";
    },
  ],
  [
    "missing command identity",
    (f) => {
      delete f.scenarios.utc.requests[0].commandKeyDigest;
    },
  ],
  [
    "missing control",
    (f) => {
      f.scenarios["load-controls"].controls.pop();
    },
  ],
  [
    "slow control",
    (f) => {
      f.scenarios["load-controls"].controls[0].ms = 1001;
    },
  ],
  [
    "failed control",
    (f) => {
      f.scenarios["load-controls"].controls[0].status = 503;
    },
  ],
  [
    "collateral errors",
    (f) => {
      f.scenarios["load-controls"].counts.collateralErrors = 2;
    },
  ],
  [
    "no collateral observations",
    (f) => {
      f.scenarios["load-controls"].counts.collateralAttempts = 0;
    },
  ],
  [
    "UTC allowance overshoot",
    (f) => {
      f.scenarios.utc.observations.utc.days["2026-01-02"] = 6;
    },
  ],
  [
    "not a real UTC crossing",
    (f) => {
      f.scenarios.utc.startedAt = "2026-01-02T00:00:01Z";
    },
  ],
  [
    "navigation mutation",
    (f) => {
      f.scenarios["load-controls"].observations.navigation.attemptedWrites = 1;
    },
  ],
  [
    "fleet overshoot",
    (f) => {
      f.scenarios["load-controls"].observations.fleet.after = 9;
    },
  ],
])
  test(`refuses resealed evidence with ${name}`, () => {
    const f = campaignFixture();
    change(f);
    f.seal();
    assert.throws(() => validate(f));
  });
for (const [name, change] of [
  [
    "missing journal",
    (f) => {
      f.journal = [];
    },
  ],
  [
    "unknown journal run",
    (f) => {
      f.journal[0].run = "unknown";
    },
  ],
  [
    "wrong cleanup owner",
    (f) => {
      f.journal[0].subject = "unknown";
    },
  ],
  [
    "cleanup before creation",
    (f) => {
      [f.journal[0], f.journal[1]] = [f.journal[1], f.journal[0]];
    },
  ],
  [
    "duplicate creation",
    (f) => {
      f.journal.unshift(f.journal[0]);
    },
  ],
  [
    "unfinished resource",
    (f) => {
      f.journal = f.journal.filter((row) => row.action !== "cleanup_complete");
    },
  ],
  [
    "accepted deletion alone",
    (f) => {
      f.journal.find((row) => row.action === "cleanup_complete").action =
        "cleanup_accepted";
    },
  ],
  [
    "pending cleanup",
    (f) => {
      f.journal.find((row) => row.action === "cleanup_complete").action =
        "cleanup_pending";
    },
  ],
  [
    "unknown cleanup action",
    (f) => {
      f.journal[1].action = "unverified";
    },
  ],
  [
    "missing refusal proof",
    (f) => {
      f.journal[1].reason = "unknown";
    },
  ],
  [
    "wrong refused receipt",
    (f) => {
      f.journal[1].id = "other-launch";
    },
  ],
  [
    "missing admitted receipt",
    (f) => {
      f.journal = f.journal.filter((row) => row.run !== "journal-shared");
    },
  ],
  [
    "late journal row",
    (f) => {
      f.journal[0].at = "2026-01-02T00:05:00Z";
    },
  ],
])
  test(`refuses ${name}`, () => {
    const f = campaignFixture();
    change(f);
    assert.throws(() => validate(f));
  });

test("a newer nonterminal Launch observation invalidates an earlier terminal observation", () => {
  const f = campaignFixture();
  const terminal = f.journal.find(
    (row) => row.run === "journal-shared" && row.action === "launch_observed",
  );
  f.journal.push({ ...terminal, state: "running" });
  assert.throws(() => validate(f));
});
