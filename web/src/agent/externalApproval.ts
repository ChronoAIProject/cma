import type { ConversationState } from './model';

export interface ExternalApprovalRequest {
  readonly source: 'external';
  readonly provider: 'nyxid';
  readonly id: string;
  readonly runId: string;
  readonly operationId: string;
  readonly revision: number;
  readonly waitingSince: string;
  readonly deadlineAt: string;
  readonly reviewUrl?: string;
  readonly canReview: boolean;
  readonly reason: string;
  readonly message?: string;
  readonly decision?: string;
  readonly order?: number;
  readonly sequence?: number;
}

/** Supplied by authenticated CMA configuration, independently of the provider receipt. */
export interface ExternalApprovalReviewPolicy {
  readonly origin: string;
  readonly path: string;
}

export function approvalReviewUrl(
  candidate: string | undefined,
  policy: ExternalApprovalReviewPolicy | undefined,
): string | undefined {
  if (!candidate || !policy) return undefined;
  try {
    const origin = new URL(policy.origin);
    if (origin.protocol !== 'https:' || origin.origin !== policy.origin) return undefined;
    if (!/^\/[a-zA-Z0-9/_-]*$/.test(policy.path) || policy.path.startsWith('//')) return undefined;
    const expected = new URL(policy.path, origin);
    const url = new URL(candidate);
    return url.protocol === 'https:' &&
      url.username === '' &&
      url.password === '' &&
      url.search === '' &&
      url.hash === '' &&
      url.origin === origin.origin &&
      url.pathname === policy.path &&
      candidate === expected.href
      ? url.href
      : undefined;
  } catch {
    return undefined;
  }
}

export function currentExternalApproval(state: ConversationState) {
  const runId = state.activeRunId;
  if (runId === undefined || state.turnOutcomes?.[runId] !== undefined) return undefined;
  return state.approvals.find(
    (approval): approval is ExternalApprovalRequest =>
      approval.source === 'external' && approval.runId === runId && approval.decision === undefined,
  );
}
