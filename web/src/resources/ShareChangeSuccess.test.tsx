import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { fixture, original, target, operation } from './ShareChangeRecovery.fixture';
import { readShareChanges, saveShareChange } from './retainedShareChange';
import { readRevocationEvidence } from './revocationEvidence';
import { retainTaskViewer } from '../create/taskIdentity';

const scope = { owner: 'owner', kind: 'sandbox', id: target.id } as const;
beforeEach(() => {
  vi.restoreAllMocks();
  sessionStorage.clear();
  retainTaskViewer('owner');
});

for (const change of ['revoke', 'set_mode'] as const) {
  function setup() {
    const test = fixture();
    const saved = {
      key: 'original-request',
      shareId: original.id,
      revision: 1,
      organization: 'org',
      mode: change === 'revoke' ? ('org_roles' as const) : ('view' as const),
      revoked: change === 'revoke',
    };
    saveShareChange('owner', target, saved);
    Object.assign(test.state, {
      success: true,
      effective: true,
      statusUnavailable: false,
      mode: saved.mode,
      revoked: saved.revoked,
    });
    return { ...test, saved };
  }

  it(`${change} success replay retains unresolved effects through reload and clears only exact zero`, async () => {
    const test = setup();
    const mounted = test.draw();
    await userEvent.click(await screen.findByRole('button', { name: 'Retry saved share change' }));
    await screen.findByText(/Previously submitted external requests may still complete/);
    await waitFor(() => expect(readShareChanges('owner', target)).toEqual([]));
    expect(readRevocationEvidence(scope)).toEqual([expect.objectContaining({ operation })]);
    expect(test.writes).toEqual([expect.objectContaining({ key: test.saved.key, revision: 1 })]);
    mounted.unmount();
    test.state.statusUnavailable = true;
    test.draw();
    await screen.findByText(/The latest external request status could not be checked/);
    expect(readRevocationEvidence(scope)).toHaveLength(1);
    test.state.statusUnavailable = false;
    test.state.unresolved = 0;
    await userEvent.click(screen.getByRole('button', { name: 'Check external requests' }));
    await waitFor(() => expect(readRevocationEvidence(scope)).toEqual([]));
    expect(test.writes).toHaveLength(1);
  });

  it.each(['zero', 'unavailable', 'operation-save', 'history-save'] as const)(
    `${change} success preserves recovery under %s`,
    async (control) => {
      const test = setup();
      if (control === 'zero') test.state.unresolved = 0;
      if (control === 'unavailable') test.state.statusUnavailable = true;
      const setItem = Storage.prototype.setItem;
      vi.spyOn(Storage.prototype, 'setItem').mockImplementation(function (
        this: Storage,
        key,
        value,
      ) {
        if (
          (control === 'operation-save' &&
            key.startsWith('cma:resource-revocation:') &&
            value.includes(operation)) ||
          (control === 'history-save' && key.startsWith('cma:resource-revocation:observed:'))
        )
          throw new Error('storage unavailable');
        setItem.call(this, key, value);
      });
      test.draw();
      await userEvent.click(
        await screen.findByRole('button', { name: 'Retry saved share change' }),
      );
      if (control === 'zero') {
        await screen.findByText(
          change === 'revoke' ? 'Access revoked.' : 'Access change confirmed.',
        );
        expect(test.reads.length).toBeGreaterThan(0);
        expect(readShareChanges('owner', target)).toEqual([]);
        expect(readRevocationEvidence(scope)).toEqual([]);
      } else {
        await screen.findByText(
          control === 'unavailable'
            ? /Revocation could not be confirmed. Your original request is saved/
            : /saved recovery information could not be updated/,
        );
        await waitFor(() => expect(test.reads.length).toBeGreaterThan(0));
        expect(readShareChanges('owner', target)).toEqual([expect.objectContaining(test.saved)]);
        if (control !== 'operation-save')
          expect(readShareChanges('owner', target)[0]?.operation).toBe(operation);
      }
      expect(test.writes).toHaveLength(1);
    },
  );
}
