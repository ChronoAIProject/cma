import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { ApiError } from '@cma/client';
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
function setup(overrides: Parameters<typeof agentClient>[0] = {}) {
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
    failed: true,
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

it.each([false, true])(
  'requires a fresh assessment and consent after the same Sandbox fails again (remount: %s)',
  async (remount) => {
    const f = setup();
    f.recoverSandboxRetained.mockResolvedValueOnce(receipt).mockResolvedValueOnce({
      operation_id: 'later-recovery-operation',
      revision: 11,
      accepted_at: '2026-11-11T04:10:00Z',
    });

    fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Recover my Sandbox' }));
    await screen.findByText(/Recovery accepted/);
    const firstKey = f.recoverSandboxRetained.mock.calls[0]?.[2].idempotencyKey;
    expect(firstKey).toBeTruthy();

    f.rerender(<SandboxRetainedRecovery {...f.props} revision={9} failed={false} />);
    if (remount) {
      f.unmount();
      render(<SandboxRetainedRecovery {...f.props} revision={10} />);
    } else {
      f.rerender(<SandboxRetainedRecovery {...f.props} revision={10} />);
    }
    expect(f.inspectSandboxRecovery).toHaveBeenCalledTimes(1);
    expect(f.recoverSandboxRetained).toHaveBeenCalledTimes(1);
    await waitFor(() => expect(screen.queryByText(/Recovery accepted/)).not.toBeInTheDocument());

    fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
    const consent = await screen.findByRole('button', { name: 'Recover my Sandbox' });
    expect(f.inspectSandboxRecovery).toHaveBeenCalledTimes(2);
    expect(f.recoverSandboxRetained).toHaveBeenCalledTimes(1);
    fireEvent.click(consent);
    await screen.findByText(/Recovery accepted/);
    const secondKey = f.recoverSandboxRetained.mock.calls[1]?.[2].idempotencyKey;
    expect(secondKey).toBeTruthy();
    expect(secondKey).not.toBe(firstKey);
    expect(f.recoverSandboxRetained).toHaveBeenCalledTimes(2);
    expect(f.recoverSandboxRetained).toHaveBeenNthCalledWith(
      2,
      f.props.sandboxId,
      10,
      expect.objectContaining({ idempotencyKey: secondKey }),
    );
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
  const client = agentClient({
    recoverSandboxRetained: first,
    inspectSandboxRecovery: f.inspectSandboxRecovery,
  });
  render(<SandboxRetainedRecovery {...f.props} client={client} revision={10} failed={false} />);
  expect(first).toHaveBeenCalledTimes(1);
  expect(f.inspectSandboxRecovery).toHaveBeenCalledTimes(1);
  first.mockResolvedValueOnce(receipt);
  fireEvent.click(screen.getByRole('button', { name: 'Retry recovery request' }));
  await screen.findByText(/Recovery accepted/);
  expect(first).toHaveBeenCalledTimes(2);
  expect(first).toHaveBeenNthCalledWith(
    2,
    f.props.sandboxId,
    7,
    expect.objectContaining({ idempotencyKey: original?.[2].idempotencyKey }),
  );
  expect(f.inspectSandboxRecovery).toHaveBeenCalledTimes(1);
});

it('keeps an ambiguous repeat request across a predecessor conflict', async () => {
  const f = setup();
  const laterReceipt = {
    operation_id: 'later-recovery-operation',
    revision: 11,
    accepted_at: '2026-11-11T04:10:00Z',
  };
  f.recoverSandboxRetained
    .mockResolvedValueOnce(receipt)
    .mockRejectedValueOnce(new TypeError('response lost'))
    .mockRejectedValueOnce(new ApiError(409, 'Conflict', 'retained_recovery_authority_changed'))
    .mockResolvedValueOnce(laterReceipt);

  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Recover my Sandbox' }));
  await screen.findByText(/Recovery accepted/);
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={10} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Recover on the same image' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Recover my Sandbox' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry recovery request' })).toBeEnabled(),
  );
  const key = f.recoverSandboxRetained.mock.calls[1]?.[2].idempotencyKey;
  f.unmount();
  render(<SandboxRetainedRecovery {...f.props} revision={12} failed={false} />);
  fireEvent.click(screen.getByRole('button', { name: 'Retry recovery request' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry recovery request' })).toBeEnabled(),
  );
  expect(loadRecoveryIntent('owner', f.props.sandboxId).intent).toMatchObject({
    revision: 10,
    key,
    refused: false,
  });
  fireEvent.click(screen.getByRole('button', { name: 'Retry recovery request' }));
  await screen.findByText(/Recovery accepted/);
  expect(
    f.recoverSandboxRetained.mock.calls.slice(1).map(([, rev, opts]) => [rev, opts.idempotencyKey]),
  ).toEqual([
    [10, key],
    [10, key],
    [10, key],
  ]);
  expect(loadRecoveryIntent('owner', f.props.sandboxId).intent?.receipt).toEqual(laterReceipt);
  expect(f.inspectSandboxRecovery).toHaveBeenCalledTimes(2);
});

it('offers a new assessment when the current server confirms a lost request never committed', async () => {
  const f = setup();
  f.recoverSandboxRetained
    .mockRejectedValueOnce(new TypeError('request lost'))
    .mockRejectedValueOnce(new ApiError(409, 'Conflict', 'retained_recovery_authority_changed'))
    .mockRejectedValueOnce(new ApiError(409, 'Conflict', 'retained_recovery_revision_passed'))
    .mockResolvedValueOnce({ ...receipt, revision: 11 });
  fireEvent.click(screen.getByRole('button', { name: 'Recover on the same image' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Recover my Sandbox' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry recovery request' })).toBeEnabled(),
  );
  const firstKey = f.recoverSandboxRetained.mock.calls[0]?.[2].idempotencyKey;
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={10} />);
  fireEvent.click(screen.getByRole('button', { name: 'Retry recovery request' }));
  await waitFor(() =>
    expect(screen.getByRole('button', { name: 'Retry recovery request' })).toBeEnabled(),
  );
  fireEvent.click(screen.getByRole('button', { name: 'Retry recovery request' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Recover on the same image' }));
  const consent = await screen.findByRole('button', { name: 'Recover my Sandbox' });
  expect(f.inspectSandboxRecovery).toHaveBeenCalledTimes(2);
  expect(
    f.recoverSandboxRetained.mock.calls.map(([, revision, options]) => [
      revision,
      options.idempotencyKey,
    ]),
  ).toEqual([
    [7, firstKey],
    [7, firstKey],
    [7, firstKey],
  ]);
  fireEvent.click(consent);
  await screen.findByText(/Recovery accepted/);
  expect(f.recoverSandboxRetained).toHaveBeenCalledTimes(4);
  expect(f.recoverSandboxRetained.mock.calls[3]?.[1]).toBe(10);
  expect(f.recoverSandboxRetained.mock.calls[3]?.[2].idempotencyKey).not.toBe(firstKey);
});

it('retains an accepted receipt on stale, lower, or still-running observations', () => {
  const accepted = { revision: 7, key: 'original-key', receipt };
  saveRecoveryIntent('owner', 'sbx_recovery', accepted);
  const f = setup();
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={10} stale />);
  expect(screen.getByText(/Recovery accepted/)).toBeInTheDocument();
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={6} />);
  expect(screen.getByText(/Recovery accepted/)).toBeInTheDocument();
  const lifecycle = {
    pause: { allowed: false },
    revive: { allowed: false },
    result: {
      state: 'in_progress' as const,
      continuing: true,
      operationId: receipt.operation_id,
      actions: ['observe'] as const,
    },
  };
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={10} lifecycle={lifecycle} />);
  expect(screen.getByText(/Recovery accepted/)).toBeInTheDocument();
  expect(loadRecoveryIntent('owner', 'sbx_recovery').intent).toMatchObject(accepted);
  expect(f.inspectSandboxRecovery).not.toHaveBeenCalled();
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
});

it('preserves a replacement intent when retiring an older acknowledgment', async () => {
  saveRecoveryIntent('owner', 'sbx_recovery', { revision: 7, key: 'original-key', receipt });
  const f = setup();
  expect(screen.getByText(/Recovery accepted/)).toBeInTheDocument();
  const replacement = { revision: 10, key: 'replacement-key' };
  saveRecoveryIntent('owner', 'sbx_recovery', replacement);
  f.rerender(<SandboxRetainedRecovery {...f.props} revision={10} />);
  expect(await screen.findByRole('button', { name: 'Retry recovery request' })).toBeEnabled();
  expect(loadRecoveryIntent('owner', 'sbx_recovery').intent).toMatchObject(replacement);
  expect(f.inspectSandboxRecovery).not.toHaveBeenCalled();
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
});

it('keeps each owner’s saved intent when the viewer changes', () => {
  const first = { revision: 7, key: 'first-owner-key', receipt };
  const other = { revision: 9, key: 'other-owner-key' };
  saveRecoveryIntent('owner', 'sbx_recovery', first);
  saveRecoveryIntent('other-owner', 'sbx_recovery', other);
  const f = setup();
  expect(screen.getByText(/Recovery accepted/)).toBeInTheDocument();

  f.rerender(<SandboxRetainedRecovery {...f.props} ownerId="other-owner" revision={10} />);
  expect(screen.queryByText(/Recovery accepted/)).not.toBeInTheDocument();
  expect(screen.getByRole('button', { name: 'Retry recovery request' })).toBeEnabled();
  expect(loadRecoveryIntent('owner', f.props.sandboxId).intent).toMatchObject(first);
  expect(loadRecoveryIntent('other-owner', f.props.sandboxId).intent).toMatchObject(other);
  expect(f.inspectSandboxRecovery).not.toHaveBeenCalled();
  expect(f.recoverSandboxRetained).not.toHaveBeenCalled();
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
