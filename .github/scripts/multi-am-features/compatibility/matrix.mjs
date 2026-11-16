// M-FUNC-02 is a connected compatibility gate. This inventory never emits PASS.
export const SOURCE = "e4047c92606e2cda4117a4cb3686119c97a621d3";
export const CLIENTS = Object.freeze({
  previous: "f779e5310595fb62e19537a76f664ba52e8342f9",
  current: "4f03c35a80322673ec00df7a3b6b9399015f5a68",
});
export const LIMITS = Object.freeze({
  socketLifetimeMs: 1800000,
  legacySessionMs: 90000,
  ownedClaimMaximumMs: 15000,
  receiverChallengeMaximumMs: 10000,
  renewalIntervalMs: 3000,
  routePermitsPerProcess: 2,
  refusalMaximumMs: 1000,
  pollMinimumMs: 2000,
});

export const SCENARIOS = Object.freeze([
  {
    id: "manual-reconnect",
    samples: 20,
    directions: ["a", "b"],
    steps: [
      "Open the exact retained compiled desktop client through AM A and receive a rendered frame.",
      "Acquire input explicitly; send one unique marker and verify one receiver effect.",
      "Disconnect, switch ingress to B, manually reconnect; verify a new socket starts view-only.",
      "Repeat twenty consecutive first-pass cycles, alternating ingress; retain every attempt.",
    ],
    expected:
      "Twenty successful first attempts, no ghost input, duplicate effect or new formal task.",
  },
  {
    id: "socket-lifetime",
    samples: 1,
    directions: ["a"],
    steps: [
      "Keep one authenticated real desktop socket live while receiver renewal remains healthy.",
      "Observe frames and authority continuously for thirty real minutes; do not reconnect or shorten time.",
      "Record the server close and receiver drain, then manually reconnect through B in view-only mode.",
    ],
    expected:
      "No early expiry; server closes the original socket at the thirty-minute boundary; reconnect succeeds.",
  },
  {
    id: "legacy-90s-transition",
    samples: 1,
    directions: ["a", "b"],
    steps: [
      "Start with an authentic legacy OFF-manager desktop session whose issued/expiry receipt proves 90 seconds.",
      "The sole parent performs its approved no-overlap transition; it never seeds or edits session rows.",
      "During any unexpired unrevoked claim, require typed desktop_busy; after release/expiry, connect through the successor.",
      "Attempt old-session input after authority loss and prove zero receiver effects; retain the existing runtime/home.",
    ],
    expected:
      "Legacy claim is honored until release or natural expiry; no overlapping input authority or lost retained state.",
  },
  {
    id: "owned-lease-loss",
    samples: 1,
    directions: ["a", "b"],
    steps: [
      "Capture actual replica-mode claim and receiver challenge deadlines; validate their distinct bounds.",
      "The sole parent withholds renewal acknowledgements for this exact session using its reviewed fault control.",
      "Require old input to stop at the original receiver deadline; a late acknowledgement must not restore authority.",
      "Reconnect via the other AM only after acknowledged drain or natural claim expiry; prove one new epoch.",
    ],
    expected:
      "No input after receiver authority expiry, no early shared-claim release, view-only reconnect succeeds.",
  },
  {
    id: "capacity-handoff",
    samples: 2,
    directions: ["a", "b"],
    steps: [
      "Hold two incomplete authenticated file/browser bodies on the same AM without completing a mutation.",
      "Probe both file and browser routes: require typed 429 file_capacity in at most one second.",
      "While both permits remain held, send real desktop input and interrupt/Stop; require observed effects within one second.",
      "Drain holders, prove successful file/browser reads, then repeat on the successor after the parent-owned handoff.",
    ],
    expected:
      "Each process enforces the shared two-slot limit, controls remain responsive, and all permits are reusable.",
  },
  {
    id: "retained-runtime-handoff",
    samples: 2,
    directions: ["a", "b"],
    steps: [
      "Record exact retained native/hosted executable, boot, home, Agent, owner process and epoch.",
      "The sole parent performs A-to-B handoff; require old receiver drain before successor input.",
      "Use the retained desktop client through B, verify history/home continuity and exactly one input effect.",
      "Repeat B-to-A; stale owner/challenge/input must be refused without replacing the retained runtime.",
    ],
    expected:
      "Both handoff directions preserve runtime/home and reject stale authority with no duplicate effects.",
  },
]);

export function matrix() {
  return ["native", "hosted"].flatMap((family) =>
    ["previous", "current"].flatMap((runtime) =>
      Object.keys(CLIENTS).flatMap((client) =>
        SCENARIOS.map((scenario) => ({
          ...scenario,
          scenario: scenario.id,
          id: `M-FUNC-02/${family}/${runtime}/${client}/${scenario.id}`,
          family,
          runtime,
          client,
          writes: true,
          source: SOURCE,
        })),
      ),
    ),
  );
}

const digest = (value) =>
  typeof value === "string" && /^sha256:[a-f0-9]{64}$/.test(value);
const image = (value) =>
  typeof value === "string" && /@sha256:[a-f0-9]{64}$/.test(value);
const revision = (value) =>
  typeof value === "string" && /^[a-f0-9]{40}$/.test(value);

export function missingBindings(binding, row) {
  const missing = [];
  if (binding?.api?.source !== SOURCE || !image(binding?.api?.image))
    missing.push("exact_e4047c926_image");
  if (
    binding?.environment?.kind !== "local-kind" ||
    binding?.environment?.sole_parent !== "local-verifier-02"
  ) {
    missing.push("sole_parent_local_kind_binding");
  }
  const ams = binding?.ams;
  for (const am of ["a", "b"]) {
    const member = ams?.[am];
    if (
      !member?.ready ||
      member.draining !== false ||
      !member.pod_uid ||
      !member.instance_id ||
      member.source !== SOURCE ||
      member.image !== binding?.api?.image
    )
      missing.push(`ready_exact_AM_${am}`);
  }
  if (
    ams?.a?.pod_uid === ams?.b?.pod_uid ||
    ams?.a?.instance_id === ams?.b?.instance_id
  ) {
    missing.push("distinct_AM_processes");
  }
  const runtime = binding?.runtimes?.[row.family]?.[row.runtime];
  if (
    !image(runtime?.image) ||
    !revision(runtime?.source) ||
    !runtime?.boot_id ||
    !digest(runtime?.inventory_sha256)
  ) {
    missing.push(`retained_${row.family}_${row.runtime}_executable_inventory`);
  }
  if (!digest(runtime?.ownership_floor_receipt_sha256))
    missing.push("receiver_ownership_floor_receipt");
  const client = binding?.clients?.[row.client];
  if (
    client?.source !== CLIENTS[row.client] ||
    client?.kind !== "compiled-web-with-novnc" ||
    !digest(client?.artifact_sha256) ||
    !digest(client?.build_receipt_sha256)
  ) {
    missing.push(`full_${row.client}_desktop_executable`);
  }
  if (!digest(binding?.parent_contract_sha256))
    missing.push("parent_control_and_receiver_evidence_contract");
  if (
    row.scenario === "legacy-90s-transition" &&
    !digest(binding?.legacy_transition_contract_sha256)
  ) {
    missing.push("authentic_legacy_90s_no_overlap_transition");
  }
  return missing;
}

export function plan(binding = {}) {
  const rows = matrix().map((row) => {
    const missing = missingBindings(binding, row);
    return { ...row, status: missing.length ? "BLOCKED" : "NOT_RUN", missing };
  });
  return {
    schema: "m-func-02-compat-plan-v1",
    source: SOURCE,
    scope: "Retained runtime and desktop slice only; not the whole #1537 gate",
    limits: LIMITS,
    rows,
    counts: {
      total: rows.length,
      blocked: rows.filter((row) => row.status === "BLOCKED").length,
      passed: 0,
    },
  };
}
