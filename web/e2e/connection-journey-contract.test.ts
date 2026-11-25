import { describe, expect, it } from 'vitest';
import {
  assertConnectionReplay,
  assertConnectionReply,
  assertProviderObservation,
  connectionStarts,
  identityDigest,
  parseConnectionJourneyFixtures,
  readConnectionIntent,
} from './connection-journey-contract';

const intent = {
  target: { type: 'launch' as const, id: 'lch_local' },
  service_id: 'sample-service',
  intent_id: '13572468-2468-1357-2468-135724681357',
};
const start = { method: 'POST', path: '/auth/connections/start', bodyHash: identityDigest(intent) };
const check = { method: 'POST', path: '/auth/connections/check', bodyHash: identityDigest({}) };
const expected = {
  run: 'local-run',
  caseId: 'J4-oauth',
  phase: 'after' as const,
  ownerDigest: identityDigest('local-owner'),
  targetDigest: identityDigest(intent.target),
  serviceDigest: identityDigest(intent.service_id),
  intentDigest: identityDigest(intent.intent_id),
  mode: 'oauth' as const,
  providerRevision: 'a'.repeat(40),
  fixtureDigest: 'b'.repeat(64),
};
const observation = () => ({
  schema: 'cma-j4-provider-observation-v1',
  ...expected,
  observedAt: '2026-01-01T00:00:00Z',
  evidenceDigest: 'c'.repeat(64),
  links: [{ linkDigest: 'd'.repeat(64), state: 'completed' }],
});

describe('current connection journey evidence', () => {
  it('separates verification POSTs, connection starts and Launch starts', () => {
    expect(
      connectionStarts([check, start, { ...start, path: '/api/v1/launches/lch_local/start' }]),
    ).toEqual([start]);
    expect(() =>
      assertConnectionReplay([check, start], [check, start, check, start, check]),
    ).not.toThrow();
  });

  it('rejects a retry that replaces the original intent', () => {
    const changed = { ...intent, intent_id: '24681357-1357-2468-1357-246813572468' };
    expect(() =>
      assertConnectionReplay([start], [start, { ...start, bodyHash: identityDigest(changed) }]),
    ).toThrow('A retry must retain the original command');
    expect(() => assertConnectionReplay([], [])).toThrow();
    expect(() => assertConnectionReplay([start], [start])).toThrow();
    expect(() => assertConnectionReplay([start], [start, start, start])).toThrow();
  });

  it('rejects another target, owner, service or intent even when the UI could continue', () => {
    expect(readConnectionIntent(intent, 'lch_local')).toEqual(intent);
    expect(() => readConnectionIntent(intent, 'lch_foreign')).toThrow();
    const reply = { ...intent, owner: 'local-owner' };
    expect(() => assertConnectionReply(reply, intent, 'local-owner')).not.toThrow();
    for (const update of [
      { owner: 'foreign' },
      { service_id: 'foreign' },
      { intent_id: 'foreign' },
    ])
      expect(() => assertConnectionReply({ ...reply, ...update }, intent, 'local-owner')).toThrow();
  });

  it('requires the provider result in addition to identical browser command hashes', () => {
    expect(() => assertProviderObservation(observation(), expected)).not.toThrow();
    for (const update of [
      { createdLinks: 2 },
      { state: 'completed' },
      { mode: 'device_code' },
      { providerRevision: 'd'.repeat(40) },
      { fixtureDigest: 'd'.repeat(64) },
      { ownerDigest: 'd'.repeat(64) },
      { targetDigest: 'd'.repeat(64) },
      { serviceDigest: 'd'.repeat(64) },
      { intentDigest: 'd'.repeat(64) },
      { hosted_url: 'https://identity.example.test/' },
    ])
      expect(() => assertProviderObservation({ ...observation(), ...update }, expected)).toThrow();
    expect(() => assertProviderObservation(undefined, expected)).toThrow();
    const incomplete = observation() as Record<string, unknown>;
    delete incomplete.links;
    expect(() => assertProviderObservation(incomplete, expected)).toThrow();
  });

  it('requires distinct mode fixtures pinned to the candidate provider version', () => {
    const fixtures = {
      schema: 'cma-j4-fixtures-v1',
      services: Object.fromEntries(
        ['oauth', 'api_key', 'device_code'].map((mode) => [
          mode,
          {
            serviceId: `service-${mode}`,
            providerRevision: 'a'.repeat(40),
            fixtureDigest: 'b'.repeat(64),
          },
        ]),
      ),
    };
    expect(parseConnectionJourneyFixtures(fixtures, 'a'.repeat(40)).services.oauth.serviceId).toBe(
      'service-oauth',
    );
    expect(() => parseConnectionJourneyFixtures(fixtures, 'c'.repeat(40))).toThrow();
    expect(() => parseConnectionJourneyFixtures(undefined, 'a'.repeat(40))).toThrow();
    const incomplete = structuredClone(fixtures);
    delete incomplete.services.device_code;
    expect(() => parseConnectionJourneyFixtures(incomplete, 'a'.repeat(40))).toThrow();
    const duplicate = structuredClone(fixtures);
    duplicate.services.device_code!.serviceId = duplicate.services.oauth!.serviceId;
    expect(() => parseConnectionJourneyFixtures(duplicate, 'a'.repeat(40))).toThrow();
  });
});
