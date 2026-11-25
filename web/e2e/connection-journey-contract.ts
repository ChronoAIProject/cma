import assert from 'node:assert/strict';
import type { Command } from './harness';
export {
  assertProviderObservation,
  identityDigest,
  type ConnectionProof,
  type ProviderObservation,
} from '../../.github/scripts/connection-journey-evidence.mjs';

export const connectionStartPath = '/auth/connections/start';
export const connectionCheckPath = '/auth/connections/check';
export const connectionCancelPath = '/auth/connections/cancel';
export const connectionModes = ['oauth', 'api_key', 'device_code'] as const;
export type ConnectionMode = (typeof connectionModes)[number];

export interface ConnectionJourneyFixtures {
  readonly schema: 'cma-j4-fixtures-v1';
  readonly services: Record<
    ConnectionMode,
    {
      readonly serviceId: string;
      readonly providerRevision: string;
      readonly fixtureDigest: string;
    }
  >;
}
export interface ConnectionObservationInput {
  readonly run: string;
  readonly caseId: string;
  readonly phase: 'before' | 'after';
  readonly mode: ConnectionMode;
  readonly owner: string;
  readonly target: { readonly type: 'launch'; readonly id: string };
  readonly serviceId: string;
  readonly intentId?: string;
}

export interface ConnectionIntent {
  readonly target: { readonly type: 'launch'; readonly id: string };
  readonly service_id: string;
  readonly intent_id: string;
}

function record(value: unknown): Record<string, unknown> {
  assert.ok(typeof value === 'object' && value !== null && !Array.isArray(value));
  return value as Record<string, unknown>;
}

export function parseConnectionJourneyFixtures(
  value: unknown,
  providerRevision: string,
): ConnectionJourneyFixtures {
  const root = record(value);
  assert.deepEqual(Object.keys(root).sort(), ['schema', 'services']);
  assert.equal(root.schema, 'cma-j4-fixtures-v1');
  assert.match(providerRevision, /^[a-f0-9]{40}$/);
  const services = record(root.services);
  assert.deepEqual(Object.keys(services).sort(), [...connectionModes].sort());
  const parsed = {} as ConnectionJourneyFixtures['services'];
  for (const mode of connectionModes) {
    const row = record(services[mode]);
    assert.deepEqual(Object.keys(row).sort(), ['fixtureDigest', 'providerRevision', 'serviceId']);
    assert.ok(
      typeof row.serviceId === 'string' &&
        /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/.test(row.serviceId),
    );
    assert.equal(row.providerRevision, providerRevision);
    assert.ok(typeof row.fixtureDigest === 'string' && /^[a-f0-9]{64}$/.test(row.fixtureDigest));
    parsed[mode] = { serviceId: row.serviceId, providerRevision, fixtureDigest: row.fixtureDigest };
  }
  assert.equal(
    new Set(Object.values(parsed).map((row) => row.serviceId)).size,
    connectionModes.length,
    'Each connection mode needs its own service fixture',
  );
  return { schema: 'cma-j4-fixtures-v1', services: parsed };
}

export function readConnectionIntent(value: unknown, launchId: string): ConnectionIntent {
  const body = record(value);
  const target = record(body.target);
  assert.ok(
    Object.keys(body).every((key) => ['target', 'service_id', 'intent_id', 'force'].includes(key)),
    'Unexpected connection command fields',
  );
  assert.deepEqual(Object.keys(target).sort(), ['id', 'type']);
  assert.ok(target.type === 'launch' && target.id === launchId, 'Wrong connection target');
  assert.ok(
    typeof body.service_id === 'string' &&
      /^[A-Za-z0-9][A-Za-z0-9_.:-]{0,127}$/.test(body.service_id),
    'Missing connection service',
  );
  assert.ok(
    typeof body.intent_id === 'string' &&
      /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(body.intent_id),
    'Missing stable connection intent',
  );
  assert.ok(body.force === undefined || body.force === false, 'Unexpected forced connection');
  return {
    target: { type: 'launch', id: launchId },
    service_id: body.service_id,
    intent_id: body.intent_id,
  };
}

export function assertConnectionReply(value: unknown, expected: ConnectionIntent, owner: string) {
  const reply = record(value);
  assert.ok(reply.owner === owner, 'Connection owner changed');
  assert.ok(reply.service_id === expected.service_id, 'Connection service changed');
  assert.ok(reply.intent_id === expected.intent_id, 'Connection intent changed');
}

export function assertFreshConnectionReport(
  value: unknown,
  expected: ConnectionIntent,
  owner: string,
) {
  const report = record(value);
  assert.equal(report.owner, owner, 'Connection verification owner changed');
  assert.equal(report.ready, true);
  assert.ok(Array.isArray(report.services));
  const services = report.services.map(record);
  assert.ok(
    services.some((row) => row.service === expected.service_id),
    'Required service disappeared',
  );
  assert.ok(
    services.every((row) => row.state === 'satisfied'),
    'Partial readiness cannot continue',
  );
  assert.ok(
    typeof report.checked_at === 'string' && Number.isFinite(Date.parse(report.checked_at)),
  );
}

export function connectionStarts(commands: readonly Command[]) {
  return commands.filter((row) => row.method === 'POST' && row.path === connectionStartPath);
}

export function assertConnectionReplay(before: readonly Command[], after: readonly Command[]) {
  const original = connectionStarts(before);
  const retried = connectionStarts(after);
  assert.equal(original.length, 1, 'The first connection start must be observed');
  assert.equal(retried.length, 2, 'Exactly one explicit connection retry is required');
  assert.deepEqual(retried[0], original[0], 'The original start must stay in the trace');
  assert.deepEqual(retried[1], original[0], 'A retry must retain the original command');
}
