import { beforeEach, expect, it } from 'vitest';
import { retainTaskViewer } from '../create/taskIdentity';
import { retainPendingOperation } from './revocationObservation';
import { readRevocations, saveRevocation } from './revocationIntent';
import { readShareChanges, saveShareChange } from './retainedShareChange';

const operation = 'a'.repeat(64);
const original = { key: 'original', shareId: 'share', revision: 1 };
const target = { kind: 'sandbox', id: 'resource', name: '' } as const;
const expected = { ...original, organization: 'org', mode: 'view' as const, revoked: true };
beforeEach(() => {
  sessionStorage.clear();
  retainTaskViewer('owner');
});

for (const kind of ['agent', 'sandbox'] as const) {
  it.each(['replaced', 'missing', 'account', 'operation'] as const)(
    `${kind} late successful response preserves recovery when its original is %s`,
    (change) => {
      const scope = { owner: 'owner', kind, id: 'resource' };
      if (kind === 'agent') saveRevocation('owner', 'resource', original);
      else saveShareChange('owner', target, expected);
      const key = `cma:resource-revocation:${JSON.stringify(
        kind === 'agent' ? ['owner', 'resource'] : ['owner', 'sandbox', 'resource'],
      )}`;
      if (change === 'replaced')
        sessionStorage.setItem(key, JSON.stringify([{ ...expected, key: 'newer' }]));
      if (change === 'missing') sessionStorage.removeItem(key);
      if (change === 'account') retainTaskViewer('other');
      if (change === 'operation')
        sessionStorage.setItem(key, JSON.stringify([{ ...expected, operation: 'b'.repeat(64) }]));
      const before = sessionStorage.getItem(key);
      expect(() =>
        retainPendingOperation(
          scope,
          { ...original, operation },
          kind === 'sandbox' ? expected : undefined,
        ),
      ).toThrow();
      expect(sessionStorage.getItem(key)).toBe(before);
    },
  );

  it(`${kind} attaches the original operation without changing the saved request`, () => {
    if (kind === 'agent') saveRevocation('owner', 'resource', original);
    else saveShareChange('owner', target, expected);
    retainPendingOperation(
      { owner: 'owner', kind, id: 'resource' },
      { ...original, operation },
      kind === 'sandbox' ? expected : undefined,
    );
    expect(
      kind === 'agent' ? readRevocations('owner', 'resource') : readShareChanges('owner', target),
    ).toEqual([{ ...(kind === 'agent' ? original : expected), operation }]);
  });
}
