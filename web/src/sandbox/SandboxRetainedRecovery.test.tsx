import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { SandboxRetainedRecovery } from './SandboxRetainedRecovery';
import { saveRecoveryIntent } from './retainedRecoveryStorage';

beforeEach(() => sessionStorage.clear());
afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});
const receipt = {
  operation_id: 'recovery-operation',
  revision: 8,
  accepted_at: '2026-11-11T04:00:00Z',
};
function setup(overrides: Parameters<typeof agentClient>[0] = {}, failed = true) {
  const inspectSandboxRecovery = vi.fn(async () => ({
    disposition: 'exact_boot_replacement' as const,
    admission_enabled: true,
  }));
  const recoverSandboxRetained = vi.fn<NonNullable<CmaWebClient['recoverSandboxRetained']>>(
    async () => receipt,
  );
  const client = agentClient({ inspectSandboxRecovery, recoverSandboxRetained, ...overrides });
  const accepted = vi.fn();
  const props = {
    client,
    sandboxId: 'sbx_recovery',
    ownerId: 'owner',
    revision: 7,
    disabled: false,
    failed,
    onAccepted: accepted,
  };
  return {
    ...render(<SandboxRetainedRecovery {...props} />),
    props,
    client,
    inspectSandboxRecovery,
    recoverSandboxRetained,
    accepted,
  };
}

it('only inspects on request and requires explicit confirmation before recovery', async () => {
  const f = setup();
  expect(f.inspectSandboxRecovery).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  await screen.findByRole('dialog', { name: 'Recover on the same image' });
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Recover my Sandbox' }));
  await screen.findByText(/Recovery accepted/);
  expect(f.recoverSandboxRetained).toHaveBeenCalledExactlyOnceWith(
    'sbx_recovery',
    7,
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
  expect(f.accepted).toHaveBeenCalledOnce();
});

it.each(['disabled', 'unproved'])('never admits a %s assessment', async (reason) => {
  const f = setup({
    inspectSandboxRecovery: async () => ({
      disposition: reason === 'unproved' ? 'child_activity' : 'exact_boot_replacement',
      admission_enabled: reason !== 'disabled',
    }),
  });
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  await screen.findByText(/Recovery is not available/);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
});

it('does not submit stale consent after the parent revision changes', async () => {
  const f = setup();
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  await screen.findByRole('dialog');
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={9} />);
  expect(screen.getByRole('button', { name: 'Recover my Sandbox' })).toBeDisabled();
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
});

it('keeps the same request key through a lost reply, client recreation and lifecycle progress', async () => {
  const first = vi.fn<NonNullable<CmaWebClient['recoverSandboxRetained']>>(async () => {
    throw new TypeError('response lost');
  });
  const f = setup({ recoverSandboxRetained: first });
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Recover my Sandbox' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry recovery request' })).toBeEnabled(),
  );
  const original = first.mock.calls[0];
  f.unmount();
  const g = setup({}, false);
  fireEvent.click(screen.getByRole('button', { name: 'Retry recovery request' }));
  await screen.findByText(/Recovery accepted/);
  expect(g.recoverSandboxRetained.mock.calls[0]?.slice(0, 2)).toEqual(original?.slice(0, 2));
  expect(g.recoverSandboxRetained.mock.calls[0]?.[2].idempotencyKey).toBe(
    original?.[2].idempotencyKey,
  );
  expect(g.inspectSandboxRecovery).not.toHaveBeenCalled();
});

it('does not leak an owner request to a different owner and does not mutate on storage failure', async () => {
  saveRecoveryIntent('other-owner', 'sbx_recovery', { revision: 7, key: 'foreign-key' });
  const f = setup();
  expect(screen.queryByRole('button', { name: 'Retry recovery request' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  await screen.findByRole('dialog');
  const storage = sessionStorage;
  vi.stubGlobal('sessionStorage', {
    getItem: (key: string) => storage.getItem(key),
    setItem: () => {
      throw new Error('storage unavailable');
    },
    clear: () => storage.clear(),
  });
  fireEvent.click(screen.getByRole('button', { name: 'Recover my Sandbox' }));
  await screen.findByText(/could not be saved/);
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
});
