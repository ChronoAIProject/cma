export const caseIds: readonly ["boundary", "shared", "load-controls", "utc"];
export const countKeys: readonly string[];
export function evidenceHash(bytes: string | Uint8Array): string;
export function validateScenarioBinding(
  bytes: string | Uint8Array,
  expected: { run: string; case: string },
): Record<string, unknown>;
