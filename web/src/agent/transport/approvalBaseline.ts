import { ProtocolError } from '@cma/client';

const guidance = {
  policy:
    'Approval access changed. Reload this Agent to refresh its approval status. Your original request is retained.',
  overflow:
    'Approval updates could not be reconciled with conversation history. Reload this Agent. Your original request is retained.',
} as const;

/** A local read failure, never permission to retry an input or infer a terminal. */
export class ApprovalBaselineRequired extends ProtocolError {
  constructor(readonly reason: keyof typeof guidance) {
    super(guidance[reason]);
  }
}

export function approvalBaselineMessage(error: unknown): string | undefined {
  // The SDK wraps a rejected event consumer. Only this locally created, typed
  // refusal exposes guidance; arbitrary service/protocol detail stays sanitized.
  for (let depth = 0; depth < 4 && error instanceof Error; depth += 1) {
    if (error instanceof ApprovalBaselineRequired) return guidance[error.reason];
    error = error.cause;
  }
  return undefined;
}
