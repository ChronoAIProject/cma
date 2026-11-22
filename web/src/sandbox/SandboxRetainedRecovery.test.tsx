import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import type { CmaWebClient } from '../api/client';
import { agentClient } from '../test/agent';
import { SandboxRetainedRecovery } from './SandboxRetainedRecovery';
import { loadRecoveryIntent, saveRecoveryIntent } from './retainedRecoveryStorage';

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

it.each(['exact_boot_replacement', 'exact_suspended_runtime'] as const)(
  'requires explicit consent for %s and sends the stored key once',
  async (disposition) => {
    const inspect = vi.fn(async () => ({ disposition, admission_enabled: true }));
    const f = setup({ inspectSandboxRecovery: inspect });
    expect(inspect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
    await screen.findByRole('dialog', { name: 'Recover on the same image' });
    expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
    expect(sessionStorage.length).toBe(0);
    fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
    expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
    expect(sessionStorage.length).toBe(0);
    fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
    await screen.findByRole('dialog', { name: 'Recover on the same image' });
    fireEvent.click(screen.getByRole('button', { name: 'Recover my Sandbox' }));
    await screen.findByText(/Recovery accepted/);
    const intent = loadRecoveryIntent('owner', 'sbx_recovery').intent;
    expect(intent?.key).toBeTruthy();
    expect(f.recoverSandboxRetained).toHaveBeenCalledExactlyOnceWith(
      'sbx_recovery',
      7,
      expect.objectContaining({ idempotencyKey: intent?.key }),
    );
    expect(f.accepted).toHaveBeenCalledOnce();
  },
);

it.each([
  { disposition: 'exact_boot_replacement', admission_enabled: false },
  { disposition: 'exact_suspended_runtime', admission_enabled: false },
  { disposition: 'child_activity', admission_enabled: true },
] as const)('never admits an ineligible $disposition assessment', async (assessment) => {
  const f = setup({
    inspectSandboxRecovery: async () => assessment,
  });
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  await screen.findByText(/Recovery is not available/);
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
});

it.each(['exact_boot_replacement', 'exact_suspended_runtime'] as const)(
  'does not submit stale %s consent after the parent revision changes',
  async (disposition) => {
    const f = setup({
      inspectSandboxRecovery: async () => ({ disposition, admission_enabled: true }),
    });
    fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
    await screen.findByRole('dialog');
    f.rerender(<SandboxRetainedRecovery {...f.props} revision={9} />);
    const confirmation = screen.getByRole('button', { name: 'Recover my Sandbox' });
    expect(confirmation).toBeDisabled();
    fireEvent.click(confirmation);
    expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
    expect(sessionStorage.length).toBe(0);
  },
);

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
