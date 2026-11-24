import assert from "node:assert/strict";
import { validateAttestations } from "./staging-authority.mjs";
import {
  caseIds,
  countKeys,
  evidenceHash,
  identifier,
  instant,
  natural,
  object,
  validateScenarioBinding,
} from "./visitor-abuse-evidence.mjs";

const actors = {
  boundary: ["boundary"],
  shared: ["a"],
  "load-controls": ["b", "c"],
  utc: ["utc"],
};
const resourceKey = (row) =>
  JSON.stringify([row.run, row.kind, row.id, row.subject]);

function validateRequests(row, context) {
  for (const request of row.requests) {
    object(request);
    assert.ok(
      ["owner", "admin", "anonymous", ...Object.values(actors).flat()].includes(
        request.actor,
      ),
    );
    assert.ok(
      Number.isInteger(request.instance) &&
        [-1, 0, 1].includes(request.instance),
    );
    assert.ok(["GET", "POST", "DELETE"].includes(request.method));
    assert.ok(
      Number.isInteger(request.status) &&
        request.status >= 200 &&
        request.status < 500,
    );
    assert.ok(Number.isFinite(request.ms) && request.ms >= 0);
    if (request.sentAt !== undefined)
      assert.ok(
        instant(request.sentAt) >= instant(row.startedAt) &&
          instant(request.sentAt) <= instant(row.endedAt),
      );
    if (request.method !== "GET") {
      for (const key of ["commandKeyDigest", "bodyDigest"])
        assert.match(request[key], /^[a-f0-9]{64}$/);
    }
  }
  if (row.case !== "boundary") {
    for (const instance of [0, 1])
      assert.ok(row.requests.some((request) => request.instance === instance));
  }
  assert.equal(row.origin, context.origin);
  assert.deepEqual(row.instances, context.instances);
  const perInstance = context.instances.map((_, index) => ({
    index,
    responses: row.requests.filter((request) => request.instance === index)
      .length,
    transportErrors: 0,
  }));
  assert.deepEqual(row.perInstance, perInstance);
}

function validateMetrics(rows) {
  const totals = Object.fromEntries(countKeys.map((key) => [key, 0]));
  for (const row of rows)
    for (const key of countKeys)
      totals[key] = natural(totals[key] + row.counts[key]);
  assert.ok(
    totals.overquotaAttempts > 0 &&
      totals.overquotaRejects === totals.overquotaAttempts,
  );
  assert.ok(totals.idempotencyChecks > 0 && totals.sideEffectChecks > 0);
  assert.ok(
    totals.collateralAttempts > 0 &&
      totals.collateralErrors / totals.collateralAttempts < 0.001,
  );
  assert.ok(totals.offered > 0 && totals.admitted > 0 && totals.refused > 0);
  const utc = rows.find((row) => row.case === "utc");
  const shared = rows.find((row) => row.case === "shared");
  const boundary = rows.find((row) => row.case === "boundary");
  const load = rows.find((row) => row.case === "load-controls");
  assert.deepEqual(
    [
      boundary.observations.thirdHome.state,
      boundary.observations.thirdHome.code,
      boundary.observations.thirdHome.live,
    ],
    ["failed_terminal", "sandbox_limit_reached", 2],
  );
  assert.deepEqual(
    [
      shared.observations.shared.attempts,
      shared.observations.shared.admitted,
      shared.observations.shared.refused,
    ],
    [50, 5, 45],
  );
  const days = Object.entries(object(utc.observations.utc.days)).sort();
  assert.equal(days.length, 2);
  assert.ok(
    days.every(
      ([day, count]) => /^\d{4}-\d{2}-\d{2}$/.test(day) && count === 5,
    ),
  );
  assert.equal(Date.parse(days[1][0]) - Date.parse(days[0][0]), 86400000);
  assert.equal(utc.observations.utc.receipts, 10);
  assert.equal(
    utc.observations.utc.midnight,
    new Date(Date.parse(days[1][0])).toISOString(),
  );
  assert.equal(
    utc.observations.utc.responseLoss,
    "caller_delivery_after_admission",
  );
  const midnight = Date.parse(utc.observations.utc.midnight);
  assert.ok(
    instant(utc.startedAt) < midnight && instant(utc.endedAt) >= midnight,
  );
  for (const kind of ["stop", "cancel"]) {
    const samples = load.controls.filter((sample) => sample.kind === kind);
    assert.ok(samples.length > 0);
    assert.ok(
      samples.every(
        (sample) =>
          sample.status >= 200 &&
          sample.status < 300 &&
          Number.isFinite(sample.ms) &&
          sample.ms >= 0,
      ),
    );
    assert.ok(
      samples.map((sample) => sample.ms).sort((a, b) => a - b)[
        Math.ceil(samples.length * 0.99) - 1
      ] <= 1000,
    );
  }
  assert.equal(load.observations.navigation.attemptedWrites, 0);
  assert.ok(load.observations.navigation.iterations >= 3);
  for (const phase of ["below", "above"]) {
    const wave = object(load.observations.load[phase]);
    for (const key of ["offered", "admitted", "refused"]) natural(wave[key]);
    assert.equal(wave.offered, wave.admitted + wave.refused);
    assert.ok(
      wave.offered > 0 &&
        Number.isFinite(wave.durationMs) &&
        wave.durationMs > 0,
    );
  }
  assert.ok(
    load.observations.load.below.admitted > 0 &&
      load.observations.load.below.refused === 0,
  );
  assert.ok(load.observations.load.above.refused > 0);
  for (const key of ["offered", "admitted", "refused"]) {
    const waveTotal = natural(
      load.observations.load.below[key] + load.observations.load.above[key],
    );
    assert.equal(
      load.counts[key],
      waveTotal,
      `Load ${key} disagrees with its waves`,
    );
    assert.equal(
      totals[key],
      waveTotal,
      `Campaign ${key} disagrees with its waves`,
    );
  }
  const fleet = object(load.observations.fleet);
  for (const key of ["before", "after", "ceiling"]) natural(fleet[key]);
  assert.ok(
    fleet.ceiling > 0 &&
      fleet.before <= fleet.ceiling &&
      fleet.after <= fleet.ceiling,
  );
  return totals;
}

function validateCleanup(journal, scenarios, context) {
  assert.ok(Array.isArray(journal) && journal.length > 0);
  const owners = new Map(scenarios.map((row) => [row.journalRun, row]));
  assert.equal(owners.size, scenarios.length);
  const boundary = scenarios.find((row) => row.case === "boundary");
  const refusedId = identifier(boundary.observations.thirdHome.id);
  assert.ok(
    boundary.requests.some(
      (request) =>
        request.actor === "boundary" &&
        request.target !== undefined &&
        request.method === "POST" &&
        [201, 202].includes(request.status) &&
        request.receiptId === refusedId,
    ),
    "Third-home refusal lacks its original admitted Launch",
  );
  const refusalKey = resourceKey({
    run: boundary.journalRun,
    kind: "launches",
    id: refusedId,
    subject: context.subjects.boundary,
  });
  const created = new Map(),
    settled = new Set(),
    lastRecords = new Map();
  for (const row of journal) {
    object(row);
    const scenario = owners.get(row.run);
    assert.ok(scenario, "Unbound cleanup journal");
    assert.ok(
      instant(row.at) >= instant(scenario.startedAt) &&
        instant(row.at) <= instant(scenario.endedAt),
    );
    assert.ok(
      actors[scenario.case]
        .map((actor) => context.subjects[actor])
        .includes(identifier(row.subject)),
      "Cleanup owner is outside the scenario",
    );
    identifier(row.id);
    assert.ok(
      ["launches", "agents", "workspaces", "sandboxes"].includes(row.kind),
    );
    const key = resourceKey(row);
    const previous = lastRecords.get(key);
    assert.ok(
      !previous || instant(row.at) >= instant(previous.at),
      "Cleanup timestamp precedes the previous record for this resource",
    );
    lastRecords.set(key, row);
    if (row.action === "created") {
      assert.ok(!created.has(key), "Duplicate resource ownership");
      created.set(key, row);
      continue;
    }
    assert.ok(created.has(key), "Cleanup precedes ownership");
    settled.delete(key);
    assert.ok(
      [
        "cancel_requested",
        "launch_observed",
        "stop_requested",
        "cleanup_accepted",
        "cleanup_complete",
        "already_absent",
        "terminal_refusal_reconciled",
      ].includes(row.action),
      "Unsettled or unknown cleanup",
    );
    if (row.action === "terminal_refusal_reconciled") {
      assert.equal(scenario.case, "boundary");
      assert.equal(row.kind, "launches");
      assert.equal(row.reason, "sandbox_limit_reached_before_allocation");
      assert.equal(row.id, scenario.observations.thirdHome.id);
      settled.add(key);
    } else if (row.kind === "launches") {
      assert.ok(["cancel_requested", "launch_observed"].includes(row.action));
      if (
        row.action === "launch_observed" &&
        ["ready", "completed", "cancelled", "failed_terminal"].includes(
          row.state,
        )
      )
        settled.add(key);
    } else if (["cleanup_complete", "already_absent"].includes(row.action))
      settled.add(key);
    else if (
      row.action === "cleanup_accepted" ||
      row.action === "stop_requested"
    )
      assert.ok(row.status >= 200 && row.status < 300);
  }
  assert.ok(created.size > 0);
  assert.ok(
    created.has(refusalKey),
    "Third-home refusal lacks resource ownership",
  );
  assert.equal(
    lastRecords.get(refusalKey)?.action,
    "terminal_refusal_reconciled",
    "Third-home refusal lacks its dedicated terminal reconciliation",
  );
  for (const key of created.keys())
    assert.ok(
      settled.has(key),
      "Resource lacks a terminal cleanup observation",
    );
  for (const scenario of scenarios) {
    for (const request of scenario.requests) {
      if (
        request.target === undefined ||
        request.method !== "POST" ||
        ![201, 202].includes(request.status)
      )
        continue;
      identifier(request.receiptId);
      assert.ok(
        created.has(
          resourceKey({
            run: scenario.journalRun,
            kind: "launches",
            id: request.receiptId,
            subject: context.subjects[request.actor],
          }),
        ),
        "Accepted Launch is absent from cleanup journal",
      );
    }
  }
  return created.size;
}

// This validates artifact consistency, not the authority or truth of their producer.
export function validateTC117Evidence(
  summary,
  artifacts,
  journal,
  context,
  now = Date.now(),
) {
  object(summary);
  object(context);
  assert.equal(summary.schema, "cma-tc117-summary-v1");
  assert.equal(summary.campaign, "TC-117");
  assert.equal(summary.run, identifier(context.run));
  assert.equal(summary.scenarioStatus, "passed");
  assert.equal(summary.status, "blocked");
  assert.equal(summary.fullCampaign, false);
  assert.equal(summary.provenanceVerified, false);
  assert.ok(
    Array.isArray(context.instances) &&
      context.instances.length === 2 &&
      new Set(context.instances).size === 2,
  );
  for (const origin of [context.origin, ...context.instances]) {
    const url = new URL(origin);
    assert.ok(url.protocol === "https:" && url.origin === origin);
  }
  const subjects = ["boundary", "a", "b", "c", "utc"].map((actor) =>
    identifier(context.subjects[actor]),
  );
  assert.equal(new Set(subjects).size, subjects.length);
  validateAttestations(context.before, context.after, now);
  assert.equal(context.before.request.expected.origin, context.origin);
  assert.equal(context.before.request.expected.commit, context.commit);
  assert.equal(
    context.before.request.expected.nyxidCommit,
    context.nyxidCommit,
  );
  const names = caseIds.map((name) => `TC117-${name}`);
  assert.deepEqual(
    summary.cases.map((row) => row.case).sort(),
    [...names].sort(),
  );
  assert.deepEqual(
    summary.evidence.map((row) => row.case).sort(),
    [...names].sort(),
  );
  assert.deepEqual(
    Object.keys(artifacts).sort(),
    caseIds.map((name) => `tc117-${name}.json`).sort(),
  );
  const rows = [];
  for (const name of caseIds) {
    const result = summary.cases.find((row) => row.case === `TC117-${name}`);
    assert.equal(result.status, "passed");
    assert.equal(result.retry, 0);
    const entry = summary.evidence.find((row) => row.case === result.case);
    assert.equal(entry.file, `tc117-${name}.json`);
    const bytes = artifacts[entry.file];
    assert.equal(evidenceHash(bytes), entry.sha256, "Scenario bytes changed");
    const row = validateScenarioBinding(bytes, {
      run: context.run,
      case: name,
    });
    assert.ok(
      instant(row.startedAt) >= instant(context.before.receipt.observedAt),
    );
    assert.ok(
      instant(row.endedAt) <= instant(context.after.receipt.observedAt),
    );
    validateRequests(row, context);
    rows.push(row);
  }
  const counts = validateMetrics(rows);
  assert.deepEqual(summary.counts, counts);
  const resources = validateCleanup(journal, rows, context);
  return {
    schema: "cma-tc117-evidence-check-v1",
    run: context.run,
    status: "consistent",
    fullCampaign: false,
    provenanceVerified: false,
    scope: "artifact-consistency-only",
    counts,
    resources,
    N: rows.reduce((total, row) => natural(total + row.N), 0),
  };
}
