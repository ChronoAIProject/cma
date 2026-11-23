import { act, renderHook, screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { chooseOption } from '../test/select';
import { fixture, original, target, operation, actor } from './ShareChangeRecovery.fixture';
import { readShareChanges, saveShareChange } from './retainedShareChange';
import { useShareChangeRecovery } from './useShareChangeRecovery';
import { retainTaskViewer } from '../create/taskIdentity';
import { readRevocationEvidence } from './revocationEvidence';

beforeEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});
async function narrow() {
  const user = userEvent.setup();
  await screen.findByText('Current access: Use organization roles');
  await chooseOption(user, 'Access for Research team', 'View only');
  await user.click(screen.getByRole('button', { name: 'Save access' }));
  return user;
}
it('preserves the exact mode, revision and key after a lost response and remount with a newer grant', async () => {
  const test = fixture();
  test.state.unknown = true;
  const first = test.draw();
  await narrow();
  await screen.findByRole('alert');
  expect(readShareChanges('owner', target)).toEqual([
    {
      key: test.writes[0]!.key,
      shareId: 'share',
      revision: 1,
      organization: 'org',
      mode: 'view',
      revoked: false,
    },
  ]);
  first.unmount();
  test.state.shares = [{ ...original, revision: 2 }];
  test.state.unknown = false;
  test.draw();
  await screen.findByRole('button', { name: 'Retry saved share change' });
  expect(test.writes).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Revoke' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry saved share change' }));
  await screen.findByRole('button', { name: 'Check revocation' });
  expect(test.writes[1]).toEqual(test.writes[0]);
  expect(readShareChanges('owner', target)[0]!.operation).toBe(operation);
});
it('resumes a pending access change without sending and rejects a receipt for a different mode', async () => {
  const test = fixture();
  const first = test.draw();
  await narrow();
  await screen.findByRole('button', { name: 'Check revocation' });
  first.unmount();
  test.state.statusUnavailable = false;
  test.state.effective = true;
  test.state.wrongReceipt = true;
  test.draw();
  await screen.findByRole('button', { name: 'Check revocation' });
  expect(test.writes).toHaveLength(1);
  expect(readShareChanges('owner', target)).toHaveLength(1);
  test.state.wrongReceipt = false;
  await userEvent.click(screen.getByRole('button', { name: 'Check revocation' }));
  await screen.findByText(
    'Access change confirmed. Previously submitted external requests may still complete.',
  );
  expect(readShareChanges('owner', target)).toEqual([]);
  expect(test.writes).toHaveLength(1);
});
it('recovers a revoke even when the active grant list no longer includes it', async () => {
  const test = fixture();
  test.state.mode = 'org_roles';
  test.state.revoked = true;
  const first = test.draw();
  await screen.findByText('Current access: Use organization roles');
  await userEvent.click(screen.getByRole('button', { name: 'Revoke' }));
  await userEvent.click(screen.getByRole('button', { name: 'Revoke grant' }));
  await screen.findByRole('button', { name: 'Check revocation' });
  first.unmount();
  test.state.shares = [];
  test.state.statusUnavailable = false;
  test.state.effective = true;
  test.draw();
  await screen.findByText(
    'Access revoked. Previously submitted external requests may still complete.',
  );
  expect(test.writes).toHaveLength(1);
  expect(readShareChanges('owner', target)).toEqual([]);
});
it.each(['revoke', 'set_mode'] as const)(
  'retains confirmed %s external-effect evidence after remount when status is unavailable',
  async (change) => {
    const test = fixture();
    test.state.revoked = change === 'revoke';
    if (change === 'revoke') test.state.mode = 'org_roles';
    const first = test.draw();
    if (change === 'revoke') {
      await screen.findByText('Current access: Use organization roles');
      await userEvent.click(screen.getByRole('button', { name: 'Revoke' }));
      await userEvent.click(screen.getByRole('button', { name: 'Revoke grant' }));
    } else await narrow();
    await screen.findByRole('button', { name: 'Check revocation' });
    test.state.statusUnavailable = false;
    test.state.effective = true;
    test.state.shares = change === 'revoke' ? [] : [{ ...original, revision: 2, mode: 'view' }];
    await userEvent.click(screen.getByRole('button', { name: 'Check revocation' }));
    await screen.findByText(
      /Access (revoked|change confirmed)\. Previously submitted external requests may still complete\./,
    );
    first.unmount();
    const previousReads = test.reads.length;
    test.state.statusUnavailable = true;
    test.draw();
    await screen.findByText(/Previously submitted external requests may still complete/);
    await waitFor(() => expect(test.reads.length).toBeGreaterThan(previousReads));
    expect(test.writes).toHaveLength(1);
    if (change === 'set_mode') expect(screen.getByRole('button', { name: 'Revoke' })).toBeEnabled();
  },
);
it('withholds writes when storage is unavailable', async () => {
  const test = fixture();
  test.draw();
  await screen.findByText('Current access: Use organization roles');
  vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  await narrow();
  await screen.findByRole('alert');
  expect(test.writes).toEqual([]);
});
it('allows a later mode change while preserving independent evidence from the earlier operation', async () => {
  const test = fixture();
  const second = 'b'.repeat(64);
  let firstResolved = false;
  let firstEffective = false;
  vi.spyOn(test.client.resourceRevocations!, 'status').mockImplementation(async (op) => {
    if (!firstEffective) throw new Error('unavailable');
    const later = op === second;
    return {
      operation_id: op,
      selector: original.selector,
      share_id: original.id,
      expected_revision: later ? 2 : 1,
      state: 'effective',
      participants: 1,
      stopped: 1,
      unresolved_effects: !later && firstResolved ? 0 : 1,
      deadline_unix_ms: 11000,
      deadline_exceeded: false,
      receipt: {
        operation_id: op,
        actor,
        at: original.created_at,
        policy_revision: later ? 3 : 2,
        share: { ...original, revision: later ? 3 : 2, mode: later ? 'org_roles' : 'view' },
      },
    };
  });
  test.draw();
  const user = await narrow();
  await screen.findByRole('button', { name: 'Check revocation' });
  firstEffective = true;
  test.state.shares = [{ ...original, revision: 2, mode: 'view' }];
  await user.click(screen.getByRole('button', { name: 'Check revocation' }));
  await screen.findByText('Current access: View only');
  expect(readShareChanges('owner', target)).toEqual([]);
  expect(screen.getByRole('button', { name: 'Revoke' })).toBeEnabled();
  test.state.operation = second;
  await chooseOption(user, 'Access for Research team', 'Use organization roles');
  test.state.shares = [{ ...original, revision: 3 }];
  await user.click(screen.getByRole('button', { name: 'Save access' }));
  await screen.findByText('Current access: Use organization roles');
  await waitFor(() =>
    expect(screen.getAllByRole('button', { name: 'Check external requests' })).toHaveLength(2),
  );
  expect(test.writes).toHaveLength(2);
  expect(test.writes[1]!.key).not.toBe(test.writes[0]!.key);
  expect(test.writes[1]!.revision).toBe(2);
  const scope = { owner: 'owner', kind: 'sandbox', id: target.id } as const;
  expect(readRevocationEvidence(scope).map((row) => row.operation)).toEqual([operation, second]);
  firstResolved = true;
  await user.click(screen.getAllByRole('button', { name: 'Check external requests' })[0]!);
  await waitFor(() =>
    expect(readRevocationEvidence(scope).map((row) => row.operation)).toEqual([second]),
  );
  expect(test.writes).toHaveLength(2);
});
it('keeps confirmed status and archived evidence if pending cleanup fails, without resending', async () => {
  const test = fixture();
  test.draw();
  await narrow();
  await screen.findByRole('button', { name: 'Check revocation' });
  test.state.statusUnavailable = false;
  test.state.effective = true;
  vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  await userEvent.click(screen.getByRole('button', { name: 'Check revocation' }));
  await screen.findByText(
    /The access change is confirmed, but saved recovery information could not be updated/,
  );
  expect(readShareChanges('owner', target)).toHaveLength(1);
  expect(test.writes).toHaveLength(1);
  expect(test.reads).toHaveLength(3);
});
it('isolates resource and owner identities and clears saved changes on account change', async () => {
  retainTaskViewer('owner');
  const saved = {
    key: 'original',
    shareId: 'share',
    revision: 1,
    organization: 'org',
    mode: 'view' as const,
    revoked: false,
    operation,
  };
  saveShareChange('owner', target, saved);
  expect(readShareChanges('other', target)).toEqual([]);
  expect(readShareChanges('owner', { ...target, id: 'other' })).toEqual([]);
  const test = fixture();
  test.draw('other');
  await screen.findByText('Current access: Use organization roles');
  expect(screen.queryByLabelText('Saved share change')).not.toBeInTheDocument();
  expect(test.reads).toEqual([]);
  retainTaskViewer('other');
  expect(readShareChanges('owner', target)).toEqual([]);
});
it('retains pending identity but stops observing when management authority is lost', async () => {
  const test = fixture();
  test.draw();
  await narrow();
  await screen.findByRole('button', { name: 'Check revocation' });
  test.state.allowed = false;
  await userEvent.click(screen.getByRole('button', { name: 'Refresh sharing' }));
  await screen.findByText(/Only the resource owner can manage/);
  expect(screen.queryByRole('button', { name: 'Check revocation' })).not.toBeInTheDocument();
  expect(readShareChanges('owner', target)).toHaveLength(1);
  await waitFor(() => expect(test.writes).toHaveLength(1));
});

it('refuses malformed saved changes before another write', async () => {
  const test = fixture();
  sessionStorage.setItem('cma:resource-revocation:["owner","sandbox","sandbox"]', '{');
  test.draw();
  await narrow();
  await screen.findByRole('alert');
  expect(test.writes).toEqual([]);
});
it('cannot replace the original request identity or intent while it is unresolved', () => {
  const saved = {
    key: 'original',
    shareId: 'share',
    revision: 1,
    organization: 'org',
    mode: 'view' as const,
    revoked: false,
  };
  saveShareChange('owner', target, saved);
  for (const replacement of [
    { key: 'replacement' },
    { revision: 2 },
    { organization: 'other' },
    { mode: 'org_roles' as const },
    { revoked: true },
  ]) {
    expect(() => saveShareChange('owner', target, { ...saved, ...replacement })).toThrow();
    expect(readShareChanges('owner', target)).toEqual([saved]);
  }
  saveShareChange('owner', target, { ...saved, operation });
  expect(() => saveShareChange('owner', target, saved)).toThrow();
  expect(() => saveShareChange('owner', target, { ...saved, operation: 'b'.repeat(64) })).toThrow();
});

it('confirms a legacy success without a pending service or retained recovery request', async () => {
  const test = fixture();
  vi.spyOn(test.client.resourceAccess, 'updateShare').mockResolvedValue({
    operation_id: operation,
    actor,
    at: original.created_at,
    policy_revision: 2,
    share: { ...original, revision: 2, mode: 'view' },
  });
  test.draw();
  await narrow();
  await screen.findByText('Access change confirmed.');
  expect(readShareChanges('owner', target)).toEqual([]);
  expect(test.reads).toEqual([]);
});
it('retains the original command after a timeout and ignores a late success', async () => {
  const test = fixture();
  const saved = {
    key: 'original',
    shareId: 'share',
    revision: 1,
    organization: 'org',
    mode: 'view' as const,
    revoked: false,
  };
  const receipt = {
    operation_id: operation,
    actor,
    at: original.created_at,
    policy_revision: 2,
    share: { ...original, revision: 2, mode: 'view' as const },
  };
  let resolve!: (value: typeof receipt) => void;
  vi.spyOn(test.client.resourceAccess, 'updateShare').mockReturnValue(
    new Promise((done) => {
      resolve = done;
    }),
  );
  const refresh = vi.fn();
  const hook = renderHook(() =>
    useShareChangeRecovery(test.client, target, 'owner', true, refresh),
  );
  vi.useFakeTimers();
  try {
    let pending!: Promise<void>;
    await act(async () => {
      pending = hook.result.current.send(saved);
    });
    expect(test.client.resourceAccess.updateShare).toHaveBeenCalledOnce();
    await act(async () => {
      await vi.advanceTimersByTimeAsync(15_000);
    });
    expect(hook.result.current.busy).toBe(false);
    expect(hook.result.current.error).toContain('could not be confirmed');
    await act(async () => {
      resolve(receipt);
      await pending;
    });
    expect(readShareChanges('owner', target)).toEqual([saved]);
    expect(hook.result.current.notice).toBeUndefined();
    expect(refresh).not.toHaveBeenCalled();
  } finally {
    vi.useRealTimers();
  }
});
