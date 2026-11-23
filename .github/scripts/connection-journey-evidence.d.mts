export type ConnectionMode = "oauth" | "api_key" | "device_code";
export interface ProviderObservation {
  schema: "cma-j4-provider-observation-v1";
  run: string;
  caseId: string;
  phase: "before" | "after";
  ownerDigest: string;
  targetDigest: string;
  serviceDigest: string;
  intentDigest: string | null;
  mode: ConnectionMode;
  providerRevision: string;
  fixtureDigest: string;
  observedAt: string;
  evidenceDigest: string;
  links: {
    linkDigest: string;
    state: "pending" | "completed" | "cancelled" | "expired";
  }[];
}
export type ObservationExpectation = Omit<
  ProviderObservation,
  "schema" | "observedAt" | "evidenceDigest" | "links"
>;
export interface ConnectionProof extends Omit<
  ObservationExpectation,
  "phase" | "intentDigest"
> {
  schema: "cma-connection-journey-v1";
  execution: "configured_remote_origin";
  scope: "ordinary_service_connection";
  intentDigest: string;
  variant:
    "valid" | "popup_blocked" | "start_response_loss" | "upstream_outage";
  commands: { method: string; path: string; bodyHash: string; key?: string }[];
  launchCreates: 1;
  launchStarts: 1;
  ownerVerified: true;
  sameTab: boolean;
  before: ProviderObservation;
  after: ProviderObservation;
}
export const connectionCaseTitles: Readonly<Record<string, string>>;
export function identityDigest(value: unknown): string;
export function assertProviderObservation(
  value: unknown,
  expected: ObservationExpectation,
): ProviderObservation;
export function assertConnectionProof(
  value: unknown,
  expected: Pick<ConnectionProof, "run" | "caseId" | "providerRevision">,
): ConnectionProof;
export function missingConnectionCoverage(
  proofs: readonly ConnectionProof[],
): string[];
