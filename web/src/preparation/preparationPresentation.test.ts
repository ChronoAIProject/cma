import { expect, it } from 'vitest';
import { derivePreparationView } from './preparationPresentation';
import type { AgentLifecycleOperationView } from '../api/client';

const receipt = { id: 'operation-one', status: 'provisioning' };
const operation: AgentLifecycleOperationView = {
  id: receipt.id,
  status: 'running',
  phase: 'workspace_preparation',
  accepted_at: '2026-11-23T09:00:00Z',
  events: [
    {
      sequence: 2,
      phase: 'workspace_preparation',
      status: 'running',
      message: 'Preparing workspace',
      occurred_at: '2026-11-23T09:03:00Z',
    },
    {
      sequence: 1,
      phase: 'sandbox_allocation',
      status: 'ready',
      message: 'Environment prepared',
      occurred_at: '2026-11-23T09:02:00Z',
    },
  ],
};

it('keeps receipt admission, durable acceptance and last real progress as separate evidence', () => {
  const result = derivePreparationView({ receipt, operation });
  expect(result).toMatchObject({
    stage: 'preparing',
    accepted: true,
    acceptedAt: operation.accepted_at,
    progressAt: '2026-11-23T09:03:00Z',
    title: 'New agent',
    notStarted: false,
  });
  expect(derivePreparationView({ receipt, operation: { ...operation } })).toEqual(result);
  expect(operation.events.map(({ sequence }) => sequence)).toEqual([2, 1]);
});

it('does not infer admission from a response read without the matching original receipt', () => {
  expect(derivePreparationView({ operation })).toMatchObject({
    stage: 'checking',
    accepted: false,
    acceptedAt: undefined,
    progressAt: undefined,
  });
  expect(
    derivePreparationView({ receipt, operation: { ...operation, id: 'other-operation' } }),
  ).toMatchObject({ accepted: true, acceptedAt: undefined, progressAt: undefined });
});

it('does not manufacture an acceptance time for old services or copy a progress timestamp', () => {
  expect(
    derivePreparationView({ receipt, operation: { ...operation, accepted_at: undefined } })
      .acceptedAt,
  ).toBeUndefined();
  expect(
    derivePreparationView({ receipt, operation: { ...operation, accepted_at: 'invalid' } })
      .acceptedAt,
  ).toBeUndefined();
});

it.each([
  ['failed', 'agent_startup', 'failed'],
  ['waiting_for_connection', 'service_connections', 'waiting'],
  ['running', 'recoverable_pending', 'stalled'],
  ['succeeded', 'completed', 'ready'],
] as const)('preserves setup outcome %s and phase %s as %s', (status, phase, stage) => {
  expect(
    derivePreparationView({ receipt, operation: { ...operation, status, phase } }),
  ).toMatchObject({ stage, animate: false, started: false });
});

it('keeps a broken setup reader separate from the retained operation and other healthy readers', () => {
  expect(
    derivePreparationView({ receipt, operation, readError: 'Setup status unavailable' }),
  ).toMatchObject({
    stage: 'interrupted',
    error: 'Setup status unavailable',
    progressAt: '2026-11-23T09:03:00Z',
    animate: false,
  });
});

it('requires explicit initial-task evidence and uses only a provided canonical title', () => {
  expect(derivePreparationView({ receipt, operation, initialTask: 'not-started' }).notStarted).toBe(
    true,
  );
  expect(derivePreparationView({ receipt, operation, initialTask: 'started' })).toMatchObject({
    stage: 'started',
    animate: false,
    notStarted: false,
  });
  expect(derivePreparationView({ receipt, canonicalTitle: 'Owner title' }).title).toBe(
    'Owner title',
  );
});
