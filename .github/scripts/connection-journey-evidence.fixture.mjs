import { identityDigest } from "./connection-journey-evidence.mjs";

export function connectionProof(caseId = "J4-oauth") {
  const mode = caseId.startsWith("J4-") ? caseId.slice(3) : "oauth";
  const variant =
    {
      "FAULT-popup-blocked": "popup_blocked",
      "FAULT-handoff-response": "start_response_loss",
      "FAULT-nyxid": "upstream_outage",
    }[caseId] ?? "valid";
  const identity = {
    run: "local-run",
    caseId,
    mode,
    ownerDigest: identityDigest("local-owner"),
    targetDigest: identityDigest({ type: "launch", id: "lch_local" }),
    serviceDigest: identityDigest("sample-service"),
    intentDigest: identityDigest("local-intent"),
    providerRevision: "a".repeat(40),
    fixtureDigest: "b".repeat(64),
  };
  const command = (path) => ({
    method: "POST",
    path,
    bodyHash: identityDigest({ path }),
  });
  const start = command("/auth/connections/start");
  const commands = [
    command("/api/v1/triggers/public/trg_local/launches"),
    command("/auth/connections/check"),
    start,
  ];
  if (["start_response_loss", "upstream_outage"].includes(variant))
    commands.push({ ...start });
  commands.push(
    command("/auth/connections/check"),
    command("/api/v1/launches/lch_local/start"),
  );
  return {
    schema: "cma-connection-journey-v1",
    execution: "configured_remote_origin",
    scope: "ordinary_service_connection",
    ...identity,
    variant,
    commands,
    launchCreates: 1,
    launchStarts: 1,
    ownerVerified: true,
    sameTab: variant === "popup_blocked",
    before: {
      schema: "cma-j4-provider-observation-v1",
      ...identity,
      phase: "before",
      intentDigest: null,
      observedAt: "2026-01-01T00:00:00Z",
      evidenceDigest: "c".repeat(64),
      links: [],
    },
    after: {
      schema: "cma-j4-provider-observation-v1",
      ...identity,
      phase: "after",
      observedAt: "2026-01-01T00:01:00Z",
      evidenceDigest: "d".repeat(64),
      links: [{ linkDigest: "e".repeat(64), state: "completed" }],
    },
  };
}
