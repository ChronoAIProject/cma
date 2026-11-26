import assert from "node:assert/strict";
import test from "node:test";
import { campaignFixture } from "./visitor-abuse-campaign.fixture.mjs";
import { validateTC117Evidence } from "./visitor-abuse-campaign.mjs";

function validate(f) {
  f.seal();
  return validateTC117Evidence(
    f.summary,
    f.artifacts,
    f.journal,
    f.context,
    f.now,
  );
}
function refusal(f) {
  return f.journal.find((row) => row.action === "terminal_refusal_reconciled");
}
function readyInsteadOfRefusal(f) {
  const row = refusal(f);
  row.action = "launch_observed";
  row.state = "ready";
  delete row.reason;
}
function sharedTerminal(f) {
  return f.journal.find(
    (row) => row.run === "journal-shared" && row.action === "launch_observed",
  );
}
function loadWaves(f) {
  const row = f.scenarios["load-controls"];
  Object.assign(row.observations.load.below, {
    offered: 2,
    admitted: 2,
    refused: 0,
  });
  Object.assign(row.observations.load.above, {
    offered: 2,
    admitted: 1,
    refused: 1,
  });
  return row;
}

test("the original consistent fixture remains unqualified", () => {
  const result = validate(campaignFixture());
  assert.equal(result.status, "consistent");
  assert.equal(result.fullCampaign, false);
  assert.equal(result.provenanceVerified, false);
});
for (const [name, mutate] of [
  [
    "ordinary ready cannot replace the dedicated third-home refusal",
    readyInsteadOfRefusal,
  ],
  [
    "ordinary completed cannot replace the dedicated third-home refusal",
    (f) => {
      readyInsteadOfRefusal(f);
      f.journal.find(
        (row) =>
          row.run === "journal-boundary" && row.action === "launch_observed",
      ).state = "completed";
    },
  ],
  [
    "an unrecorded third-home ID cannot be ignored",
    (f) => {
      readyInsteadOfRefusal(f);
      f.scenarios.boundary.observations.thirdHome.id = "unrecorded-third-home";
    },
  ],
  [
    "reconciliation without the original admitted request is insufficient",
    (f) => {
      f.scenarios.boundary.requests[0].status = 429;
    },
  ],
  [
    "the original admitted request must have the boundary subject",
    (f) => {
      f.scenarios.boundary.requests[0].actor = "a";
    },
  ],
  [
    "a GET response is not original Launch admission",
    (f) => {
      f.scenarios.boundary.requests[0].method = "GET";
    },
  ],
  [
    "the refusal must belong to the same cleanup journal",
    (f) => {
      refusal(f).run = "journal-shared";
    },
  ],
  [
    "a later ready record cannot override the dedicated refusal",
    (f) => {
      const row = refusal(f);
      f.journal.push({ ...row, action: "launch_observed", state: "ready" });
    },
  ],
])
  test(`B1: ${name}`, () => {
    const f = campaignFixture();
    mutate(f);
    assert.throws(() => validate(f));
  });

test("B1: the first two homes may settle normally while the third is refused", () => {
  const f = campaignFixture();
  const boundary = f.scenarios.boundary;
  const original = boundary.requests[0];
  const created = f.journal.find(
    (row) => row.run === boundary.journalRun && row.action === "created",
  );
  for (const [index, state] of ["ready", "completed"].entries()) {
    const id = `normal-home-launch-${index}`;
    boundary.requests.push({ ...original, receiptId: id });
    f.journal.push(
      { ...created, id },
      { ...created, id, action: "launch_observed", state },
    );
  }
  boundary.N = boundary.requests.length;
  boundary.perInstance[0].responses += 2;
  assert.equal(validate(f).status, "consistent");
});

test("B2: an older terminal row cannot hide a newer running observation", () => {
  const f = campaignFixture();
  const terminal = sharedTerminal(f);
  f.journal.splice(f.journal.indexOf(terminal), 0, {
    ...terminal,
    state: "running",
    at: "2026-01-02T00:00:30.000Z",
  });
  terminal.at = "2026-01-02T00:00:20.000Z";
  assert.throws(() => validate(f));
});
test("B2: a terminal observation cannot predate resource creation", () => {
  const f = campaignFixture();
  sharedTerminal(f).at = "2026-01-02T00:00:09.000Z";
  assert.throws(() => validate(f));
});
test("B2: concurrent journals may interleave without global chronological ordering", () => {
  const f = campaignFixture();
  const shared = f.journal.filter((row) => row.run === "journal-shared");
  const utc = f.journal.filter((row) => row.run === "journal-utc");
  shared[0].at = "2026-01-02T00:00:10.000Z";
  shared[1].at = "2026-01-02T00:00:40.000Z";
  utc[0].at = "2026-01-02T00:00:20.000Z";
  utc[1].at = "2026-01-02T00:00:30.000Z";
  f.journal = [
    shared[0],
    utc[0],
    shared[1],
    utc[1],
    ...f.journal.filter(
      (row) => !["journal-shared", "journal-utc"].includes(row.run),
    ),
  ];
  assert.equal(validate(f).status, "consistent");
});
test("B2: equal timestamp entries retain their append order", () => {
  const f = campaignFixture();
  const terminal = sharedTerminal(f);
  f.journal.splice(f.journal.indexOf(terminal), 0, {
    ...terminal,
    state: "running",
  });
  assert.equal(validate(f).status, "consistent");
});

test("B3: matching offered totals cannot hide swapped admitted/refused totals", () => {
  const f = campaignFixture();
  Object.assign(loadWaves(f).counts, { offered: 4, admitted: 1, refused: 3 });
  assert.throws(() => validate(f));
});
test("B3: all three matching wave totals are preserved", () => {
  const f = campaignFixture();
  const row = loadWaves(f);
  Object.assign(row.counts, { offered: 4, admitted: 3, refused: 1 });
  const before = structuredClone(row.counts);
  const result = validate(f);
  assert.deepEqual(row.counts, before);
  assert.deepEqual(
    [result.counts.offered, result.counts.admitted, result.counts.refused],
    [4, 3, 1],
  );
});
