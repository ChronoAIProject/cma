import type { ExternalApprovalRequest } from './externalApproval';

export const externalApprovalFixture: ExternalApprovalRequest = {
  source: 'external',
  provider: 'nyxid',
  id: 'external-fixture',
  runId: '1',
  operationId: 'scoped-operation-fixture',
  revision: 1,
  waitingSince: '2026-01-01T00:00:00.000Z',
  deadlineAt: '2026-01-01T00:05:00.000Z',
  reason: 'Model request approval',
  canReview: true,
  reviewUrl: 'https://identity.example.test/assistant/approvals',
};
export const externalReviewPolicyFixture = {
  origin: 'https://identity.example.test',
  path: '/assistant/approvals',
};
