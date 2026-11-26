import { screen, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { beforeEach, expect, it, vi } from 'vitest';
import { chooseOption } from '../test/select';
import { actor, fixture, operation, original, target } from './ShareChangeRecovery.fixture';
import { readShareChanges } from './retainedShareChange';

beforeEach(() => {
  sessionStorage.clear();
  vi.restoreAllMocks();
});

const conflict = () =>
  Response.json(
    { code: 'resource_access_changed', detail: 'Sharing changed. Read the current revision.' },
    { status: 409 },
  );

async function change(kind: 'update' | 'revoke') {
  const user = userEvent.setup();
  await screen.findByText('Current access: Use organization roles');
  if (kind === 'update') {
    await chooseOption(user, 'Access for Research team', 'View only');
    await user.click(screen.getByRole('button', { name: 'Save access' }));
  } else {
    await user.click(screen.getByRole('button', { name: 'Revoke' }));
    await user.click(screen.getByRole('button', { name: 'Revoke grant' }));
  }
  return user;
}

it.each(['update', 'revoke'] as const)(
  'ends an explicitly rejected %s, refreshes and allows a fresh revoke after remount',
  async (kind) => {
    const test = fixture();
    const serve = test.fetch.getMockImplementation()!;
    let reject = true;
    test.fetch.mockImplementation(async (input, init) => {
      const response = await serve(input, init);
      if (init?.method !== 'PATCH' && init?.method !== 'DELETE') return response;
      if (reject) return conflict();
      test.state.shares = [];
      return Response.json({
        operation_id: operation,
        actor,
        at: original.created_at,
        policy_revision: 3,
        share: { ...original, revision: 3, revoked_at: original.created_at },
      });
    });
    const first = test.draw();
    await screen.findByText('Current access: Use organization roles');
    test.state.shares = [{ ...original, revision: 2 }];
    const user = await change(kind);
    await screen.findByRole('alert');
    expect(await screen.findByRole('button', { name: 'Revoke' })).toBeEnabled();
    await user.click(screen.getByRole('button', { name: 'Refresh sharing' }));
    await screen.findByText('Current access: Use organization roles');
    first.unmount();
    test.draw();
    expect(await screen.findByRole('button', { name: 'Revoke' })).toBeEnabled();
    expect(readShareChanges('owner', target)).toEqual([]);
    expect(test.writes).toHaveLength(1);
    reject = false;
    await change('revoke');
    await screen.findByText('Access revoked.');
    expect(test.writes).toHaveLength(2);
    expect(test.writes[0]!.revision).toBe(1);
    expect(test.writes[1]).toMatchObject({ method: 'DELETE', revision: 2 });
    expect(test.writes[1]!.key).toBeTruthy();
    expect(test.writes[1]!.key).not.toBe(test.writes[0]!.key);
    expect(readShareChanges('owner', target)).toEqual([]);
  },
);

it.each([
  [503, 'resource_access_changed'],
  [409, 'resource_access_unavailable'],
] as const)('retains the original request after HTTP %s %s', async (status, code) => {
  const test = fixture();
  const serve = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input, init) => {
    const response = await serve(input, init);
    return init?.method === 'PATCH'
      ? Response.json({ code, detail: 'Result unconfirmed' }, { status })
      : response;
  });
  const first = test.draw();
  await change('update');
  await screen.findByRole('alert');
  const saved = readShareChanges('owner', target);
  expect(saved).toHaveLength(1);
  first.unmount();
  test.draw();
  expect(await screen.findByRole('button', { name: 'Revoke' })).toBeDisabled();
  await userEvent.click(screen.getByRole('button', { name: 'Retry saved share change' }));
  await waitFor(() => expect(test.writes).toHaveLength(2));
  expect(test.writes[1]).toEqual(test.writes[0]);
  expect(readShareChanges('owner', target)).toEqual(saved);
});

it('does not treat an authorization read conflict as rejection of an earlier unknown write', async () => {
  const test = fixture();
  test.state.unknown = true;
  test.draw();
  await change('update');
  await screen.findByRole('alert');
  const saved = readShareChanges('owner', target);
  const serve = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input, init) =>
    new URL(String(input)).pathname.includes('/resource-access/') ? conflict() : serve(input, init),
  );
  await userEvent.click(screen.getByRole('button', { name: 'Retry saved share change' }));
  await screen.findByText('Sharing changed. Read the current revision.');
  expect(readShareChanges('owner', target)).toEqual(saved);
  expect(test.writes).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Revoke' })).toBeDisabled();
});

it('refreshes authorization after rejection and withholds commands when management access is lost', async () => {
  const test = fixture();
  const serve = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input, init) => {
    const response = await serve(input, init);
    if (init?.method !== 'PATCH') return response;
    test.state.allowed = false;
    return conflict();
  });
  test.draw();
  await change('update');
  await screen.findByText(/Only the resource owner can manage/);
  expect(readShareChanges('owner', target)).toEqual([]);
  expect(screen.queryByRole('button', { name: 'Revoke' })).not.toBeInTheDocument();
  expect(test.writes).toHaveLength(1);
});

it('keeps the record and blocks new commands if rejected-request cleanup fails', async () => {
  const test = fixture();
  const serve = test.fetch.getMockImplementation()!;
  test.fetch.mockImplementation(async (input, init) => {
    const response = await serve(input, init);
    return init?.method === 'PATCH' ? conflict() : response;
  });
  test.draw();
  vi.spyOn(Storage.prototype, 'removeItem').mockImplementation(() => {
    throw new Error('storage unavailable');
  });
  await change('update');
  await screen.findByText(/share change was rejected, but the saved request could not be cleared/);
  expect(readShareChanges('owner', target)).toHaveLength(1);
  expect(test.writes).toHaveLength(1);
  expect(screen.getByRole('button', { name: 'Revoke' })).toBeDisabled();
});
