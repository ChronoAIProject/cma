// @vitest-environment node
import { mkdirSync, rmSync } from 'node:fs';
import type { Browser, TestInfo } from '@playwright/test';
import { afterEach, expect, it, vi } from 'vitest';
import { Harness } from './harness';
import type { Hooks } from './environment';
import { fixtureDocument } from './environment.fixture';
import { identityDigest } from '../../.github/scripts/connection-journey-evidence.mjs';

const directories: string[] = [];
afterEach(() => {
  vi.unstubAllEnvs();
  directories.splice(0).forEach((directory) => rmSync(directory, { recursive: true }));
});
function harness(caseId: string) {
  const fixture = fixtureDocument();
  directories.push(fixture.directory);
  for (const [key, value] of Object.entries(fixture.env)) vi.stubEnv(key, value);
  mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR);
  return new Harness({} as Browser, {} as Hooks, caseId);
}

function proof(h: Harness) {
  const owner = h.fixture.accounts.visitor.subject;
  h.journal.own('launches', 'owned-launch', owner);
  const service = h.fixture.connectionJourneys.services.oauth;
  const identity = {
    run: h.journal.run,
    caseId: h.caseId,
    mode: 'oauth',
    ownerDigest: identityDigest(owner),
    targetDigest: identityDigest({ type: 'launch', id: 'owned-launch' }),
    serviceDigest: identityDigest(service.serviceId),
    intentDigest: identityDigest('intent'),
    providerRevision: service.providerRevision,
    fixtureDigest: service.fixtureDigest,
  };
  const command = (path: string) => ({ method: 'POST', path, bodyHash: 'a'.repeat(64) });
  const baseline = { linkDigest: 'b'.repeat(64), state: 'completed' };
  const before = {
    ...identity,
    schema: 'cma-j4-provider-observation-v1',
    phase: 'before',
    intentDigest: null,
    observedAt: '2026-01-01T00:00:00Z',
    evidenceDigest: 'c'.repeat(64),
    links: [baseline],
  };
  const after = {
    ...identity,
    schema: before.schema,
    phase: 'after',
    observedAt: '2026-01-01T00:01:00Z',
    evidenceDigest: 'd'.repeat(64),
    links: [baseline, { linkDigest: 'e'.repeat(64), state: 'completed' }],
  };
  return {
    ...identity,
    schema: 'forged',
    execution: 'local',
    scope: 'managed_default_inline',
    run: 'other-run',
    caseId: 'J6',
    variant: 'valid',
    sameTab: false,
    launchCreates: 1,
    launchStarts: 1,
    ownerVerified: true,
    before,
    after,
    commands: [
      command('/api/v1/triggers/public/public-id/launches'),
      command('/auth/connections/start'),
      command('/auth/connections/check'),
      command('/api/v1/launches/owned-launch/start'),
    ],
  };
}

it('binds connection evidence to the actual run and case, preserving an immutable snapshot', async () => {
  const h = harness('J4-oauth');
  const evidence = proof(h);
  h.proveConnection(evidence);
  evidence.before.links[0]!.state = 'changed';
  expect(() => h.proveConnection(evidence)).toThrow('Only one connection');
  const attach = vi.fn();
  await h.attachJourneyProof({ attach } as unknown as TestInfo);
  expect(attach).toHaveBeenCalledOnce();
  expect(attach.mock.calls[0]![0]).toBe('connection-journey-proof');
  expect(JSON.parse(attach.mock.calls[0]![1].body)).toMatchObject({
    schema: 'cma-connection-journey-v1',
    execution: 'configured_remote_origin',
    scope: 'ordinary_service_connection',
    run: h.journal.run,
    caseId: 'J4-oauth',
    before: { links: [{ state: 'completed' }] },
  });
  expect(() => h.proveJourney({})).toThrow();
});

it('refuses internally consistent proofs for a different owner, fixture or unowned Launch', () => {
  for (const field of ['ownerDigest', 'serviceDigest', 'fixtureDigest'] as const) {
    const h = harness('J4-oauth');
    const evidence = proof(h);
    evidence[field] = evidence.before[field] = evidence.after[field] = '0'.repeat(64);
    expect(() => h.proveConnection(evidence)).toThrow('owned Launch and reviewed fixture');
  }
  const h = harness('J4-oauth');
  const evidence = proof(h);
  h.journal.resources.splice(0);
  expect(() => h.proveConnection(evidence)).toThrow('owned Launch and reviewed fixture');
});

it('refuses unrelated cases and never attaches rejected credential material', async () => {
  for (const caseId of ['J0', 'J5', 'J6', 'FAULT-nyxid-requirements'])
    expect(() => harness(caseId).proveConnection({})).toThrow('connection case');
  const h = harness('FAULT-handoff-response');
  expect(() => h.proveConnection({ raw: 'redacted-probe' })).toThrow('Credential material');
  const attach = vi.fn();
  await h.attachJourneyProof({ attach } as unknown as TestInfo);
  expect(attach).not.toHaveBeenCalled();
});
