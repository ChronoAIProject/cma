import { expect, it } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import type { SelectedRetiredAgent } from '../useDeletionNoticeReconciliation';
import { deriveRetiredPresentation } from './retiredPresentation';

const current = { viewerId: 'owner', authEpoch: 3, agentId: 'agt_1', authorized: true };
const observed: SelectedRetiredAgent = {
  viewerId: current.viewerId,
  authEpoch: current.authEpoch,
  detail: { ...AGENT_DETAIL, sandbox: { ...AGENT_DETAIL.sandbox, status: 'retired' } },
};

it('uses only the exact currently authorized retired lineage and keeps its real detail', () => {
  expect(deriveRetiredPresentation(observed, current)).toEqual({
    kind: 'retired',
    detail: observed.detail,
    identity: {
      viewerId: 'owner',
      authEpoch: 3,
      agentId: 'agt_1',
      worktreeId: 'wkt_1',
      workspaceId: 'wsp_1',
      sandboxId: 'sbx_1',
    },
  });
});

it.each([
  { viewerId: 'another' },
  { viewerId: undefined },
  { authEpoch: 4 },
  { authEpoch: undefined },
  { agentId: 'another' },
  { agentId: undefined },
  { authorized: false },
])('refuses changed or unconfirmed current authority: %j', (change) => {
  expect(deriveRetiredPresentation(observed, { ...current, ...change })).toEqual({
    kind: 'not-retired',
  });
});

it.each(['agentId', 'workspaceId', 'sandboxId'] as const)(
  'refuses mismatching Worktree %s',
  (field) => {
    expect(
      deriveRetiredPresentation(
        {
          ...observed,
          detail: {
            ...observed.detail,
            worktree: { ...observed.detail.worktree, [field]: 'wrong' },
          },
        },
        current,
      ),
    ).toEqual({ kind: 'not-retired' });
  },
);

it.each(['active', 'retiring', 'failed', 'paused'] as const)(
  'never calls %s a retired environment',
  (status) => {
    expect(
      deriveRetiredPresentation(
        {
          ...observed,
          detail: {
            ...observed.detail,
            status: 'deleted',
            sandbox: { ...observed.detail.sandbox, status },
          },
        },
        current,
      ),
    ).toEqual({ kind: 'not-retired' });
  },
);

it('does not derive retirement from the absence of a fresh witness', () => {
  expect(deriveRetiredPresentation(undefined, current)).toEqual({ kind: 'not-retired' });
});
