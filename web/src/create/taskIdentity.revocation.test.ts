import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { requireTaskStorage, retainTaskViewer } from './taskIdentity';
import { retainTaskViewer as predecessorViewer } from './taskIdentity.v2.fixture';
import { readRevocations, saveRevocation } from '../resources/revocationIntent';
import { readShareChanges, saveShareChange } from '../resources/retainedShareChange';
import { readRevocationEvidence } from '../resources/revocationEvidence';

const keys = (owner: string) => [
  `cma:resource-revocation:${JSON.stringify([owner, 'agent'])}`,
  `cma:resource-revocation:${JSON.stringify([owner, 'sandbox', 'sandbox'])}`,
  `cma:resource-revocation:observed:${JSON.stringify([owner, 'sandbox', 'sandbox'])}`,
];

beforeEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
  retainTaskViewer();
});
afterEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
  retainTaskViewer();
});

for (const transition of ['account switch', 'sign-out'] as const) {
  it.each([0, 1, 2])(
    `removes foreign revocation format %s on return after predecessor ${transition}`,
    (format) => {
      predecessorViewer('owner-a');
      const oldKey = keys('owner-a')[format]!;
      sessionStorage.setItem(oldKey, 'original owner recovery bytes');
      predecessorViewer(transition === 'account switch' ? 'owner-b' : undefined);
      expect(sessionStorage.getItem(oldKey)).toBe('original owner recovery bytes');
      const currentKey = keys('owner-b')[format]!;
      sessionStorage.setItem(currentKey, 'current owner recovery bytes');
      sessionStorage.setItem('unrelated-preference', 'unchanged');

      retainTaskViewer('owner-b');

      expect(sessionStorage.getItem(oldKey)).toBeNull();
      expect(sessionStorage.getItem(currentKey)).toBe('current owner recovery bytes');
      expect(sessionStorage.getItem('unrelated-preference')).toBe('unchanged');
    },
  );
}

it.each(['owner', 'owner[with]"escaping\\and:separators'])(
  'preserves exact current-owner bytes with missing or matching markers for %s',
  (owner) => {
    const stored = [
      ...keys(owner),
      `cma:resource-revocation:future:v2:${JSON.stringify([owner, 'future-resource'])}`,
      `cma:resource-revocation:${JSON.stringify([owner, 'x'.repeat(8192)])}`,
    ];
    for (const key of stored) sessionStorage.setItem(key, '{unreadable recovery bytes');
    for (let n = 0; n < 2; n++) {
      retainTaskViewer(owner);
      for (const key of stored)
        expect(sessionStorage.getItem(key)).toBe('{unreadable recovery bytes');
      expect(() => requireTaskStorage()).not.toThrow();
    }
  },
);

it('removes foreign future namespaces and keys without a readable owner without reading their payloads', () => {
  predecessorViewer('owner');
  const invalid = [
    'cma:resource-revocation:[',
    'cma:resource-revocation:observed:["owner"]',
    'cma:resource-revocation:observed:{"owner":"owner"}',
    `cma:resource-revocation:future:${JSON.stringify(['other', 'resource'])}`,
  ];
  for (const key of invalid) sessionStorage.setItem(key, 'retained bytes');
  retainTaskViewer('owner');
  for (const key of invalid) expect(sessionStorage.getItem(key)).toBeNull();
});

it.each(['enumeration', 'deletion'] as const)(
  'refuses all revocation storage after failed %s and recovers only after cleanup',
  (fault) => {
    predecessorViewer('other');
    sessionStorage.setItem(keys('other')[0]!, 'foreign recovery');
    predecessorViewer('owner');
    const denied =
      fault === 'enumeration'
        ? vi.spyOn(Storage.prototype, 'key').mockImplementation(() => {
            throw new Error('blocked');
          })
        : vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
            throw new Error('blocked');
          });
    retainTaskViewer('owner');
    const target = { kind: 'sandbox', id: 'sandbox', name: 'Sandbox' } as const;
    const intent = { key: 'request', shareId: 'share', revision: 1 };
    expect(() => requireTaskStorage()).toThrow(/could not be isolated/);
    expect(() => readRevocations('owner', 'agent')).toThrow(/could not be isolated/);
    expect(() => saveRevocation('owner', 'agent', intent)).toThrow(/could not be isolated/);
    expect(() => readShareChanges('owner', target)).toThrow(/could not be isolated/);
    expect(() =>
      saveShareChange('owner', target, {
        ...intent,
        organization: 'org',
        mode: 'view',
        revoked: true,
      }),
    ).toThrow(/could not be isolated/);
    expect(() =>
      readRevocationEvidence({ owner: 'owner', kind: 'sandbox', id: 'sandbox' }),
    ).toThrow(/could not be isolated/);
    denied.mockRestore();
    retainTaskViewer('owner');
    expect(() => requireTaskStorage()).not.toThrow();
    expect(sessionStorage.getItem(keys('other')[0]!)).toBeNull();
    expect(readRevocations('owner', 'agent')).toEqual([]);
    expect(readShareChanges('owner', target)).toEqual([]);
  },
);

it.each(['other', undefined])(
  'retains the full namespace purge for an observed transition to %s',
  (next) => {
    retainTaskViewer('owner');
    const all = [
      ...keys('owner'),
      ...keys('other'),
      'cma:resource-revocation:unknown',
      'cma:task-draft:owner',
      'cma:agent-creation:owner',
      'cma:agent-recovery:owner',
    ];
    for (const key of all) sessionStorage.setItem(key, 'original bytes');
    sessionStorage.setItem('unrelated-preference', 'unchanged');
    retainTaskViewer(next);
    for (const key of all) expect(sessionStorage.getItem(key)).toBeNull();
    expect(sessionStorage.getItem('unrelated-preference')).toBe('unchanged');
  },
);
