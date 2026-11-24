import { fixtures } from "./staging-authority-cli.fixture.mjs";
import { caseIds, countKeys, evidenceHash } from "./visitor-abuse-evidence.mjs";

export function campaignFixture() {
  const now = Date.parse("2026-01-02T00:02:00Z");
  const run = "tc117-synthetic-campaign";
  const before = fixtures(),
    after = fixtures();
  before.receipt.observedAt = "2026-01-01T23:58:00.000Z";
  after.request.nonce = after.receipt.nonce =
    "00000000-0000-4000-8000-000000000002";
  after.receipt.observedAt = new Date(now).toISOString();
  const context = {
    run,
    origin: before.receipt.origin,
    commit: before.receipt.commit,
    nyxidCommit: before.receipt.nyxidCommit,
    instances: ["https://one.example.test", "https://two.example.test"],
    before,
    after,
    subjects: Object.fromEntries(
      ["boundary", "a", "b", "c", "utc"].map((actor) => [
        actor,
        `synthetic-${actor}`,
      ]),
    ),
  };
  const scenarios = {};
  const journal = [];
  for (const [index, name] of caseIds.entries()) {
    const actor = ["boundary", "a", "b", "utc"][index];
    const receiptId = `synthetic-launch-${name}`;
    const counts = Object.fromEntries(countKeys.map((key) => [key, 0]));
    counts.overquotaAttempts = counts.overquotaRejects = 1;
    const requests = [
      {
        actor,
        instance: 0,
        method: "POST",
        status: 202,
        ms: 1,
        sentAt: "2026-01-01T23:59:50.000Z",
        target: "reuseA",
        receiptId,
        commandKeyDigest: "1".repeat(64),
        bodyDigest: "2".repeat(64),
      },
      { actor, instance: 1, method: "GET", status: 200, ms: 1 },
    ];
    const row = {
      schema: "cma-tc117-scenario-v1",
      campaign: "TC-117",
      run,
      case: name,
      journalRun: `journal-${name}`,
      status: "passed",
      startedAt: "2026-01-01T23:59:40.000Z",
      endedAt: "2026-01-02T00:01:00.000Z",
      origin: context.origin,
      instances: context.instances,
      N: requests.length,
      requests,
      perInstance: [0, 1].map((instance) => ({
        index: instance,
        responses: 1,
        transportErrors: 0,
      })),
      counts,
      controls: [],
      observations: {},
    };
    const owned = {
      run: row.journalRun,
      at: "2026-01-02T00:00:10.000Z",
      kind: "launches",
      id: receiptId,
      subject: context.subjects[actor],
    };
    journal.push({ ...owned, action: "created" });
    if (name === "boundary") {
      row.observations.thirdHome = {
        id: receiptId,
        state: "failed_terminal",
        code: "sandbox_limit_reached",
        live: 2,
      };
      journal.push({
        ...owned,
        action: "terminal_refusal_reconciled",
        reason: "sandbox_limit_reached_before_allocation",
      });
    } else
      journal.push({ ...owned, action: "launch_observed", state: "ready" });
    if (name === "shared")
      row.observations.shared = { attempts: 50, admitted: 5, refused: 45 };
    if (name === "utc") {
      counts.idempotencyChecks = 2;
      row.observations.utc = {
        midnight: "2026-01-02T00:00:00.000Z",
        days: { "2026-01-01": 5, "2026-01-02": 5 },
        receipts: 10,
        responseLoss: "caller_delivery_after_admission",
      };
    }
    if (name === "load-controls") {
      Object.assign(counts, {
        sideEffectChecks: 2,
        collateralAttempts: 1001,
        offered: 2,
        admitted: 1,
        refused: 1,
      });
      row.controls = [
        { kind: "stop", status: 202, ms: 10 },
        { kind: "cancel", status: 200, ms: 10 },
      ];
      row.observations.navigation = { attemptedWrites: 0, iterations: 3 };
      row.observations.load = {
        below: { offered: 1, admitted: 1, refused: 0, durationMs: 1000 },
        above: { offered: 1, admitted: 0, refused: 1, durationMs: 1000 },
      };
      row.observations.fleet = { before: 4, after: 4, ceiling: 8 };
      const agent = { ...owned, kind: "agents", id: "synthetic-agent" };
      journal.push(
        { ...agent, action: "created" },
        { ...agent, action: "stop_requested", status: 202 },
        { ...agent, action: "cleanup_accepted", status: 202 },
        { ...agent, action: "cleanup_complete" },
      );
    }
    scenarios[name] = row;
  }
  const summary = {
    schema: "cma-tc117-summary-v1",
    campaign: "TC-117",
    run,
    status: "blocked",
    scenarioStatus: "passed",
    fullCampaign: false,
    provenanceVerified: false,
    cases: caseIds.map((name) => ({
      case: `TC117-${name}`,
      status: "passed",
      retry: 0,
    })),
    evidence: [],
    counts: {},
  };
  const artifacts = {};
  const seal = () => {
    summary.evidence = caseIds.map((name) => {
      const file = `tc117-${name}.json`;
      artifacts[file] = Buffer.from(JSON.stringify(scenarios[name]));
      return {
        case: `TC117-${name}`,
        file,
        sha256: evidenceHash(artifacts[file]),
      };
    });
    summary.counts = Object.fromEntries(
      countKeys.map((key) => [
        key,
        Object.values(scenarios).reduce(
          (total, row) => total + row.counts[key],
          0,
        ),
      ]),
    );
  };
  seal();
  return { now, context, summary, scenarios, artifacts, journal, seal };
}
