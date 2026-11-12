import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ApiError, type AccountDeletionReceipt } from '@chronoai/cma-sdk';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { AccountDeletionDialog } from './AccountDeletionDialog';
import { AccountDeletionPending } from './AccountDeletionPending';
import {
  accountDeletionStorageKey,
  createAccountDeletionRequest,
  notifyAccountDeletionInProgress,
  restoreAccountDeletion,
} from './deletion';
import type { CmaWebClient } from '../api/client';
import { unavailableClient } from '../api/client';
import { App } from '../App';
import { AuthContextProvider } from '../auth/context';
import { createMockAuthProvider } from '../auth/mock';
import { CmaClientProvider } from '../api/context';

const receipt = (
  status: 'running' | 'blocked' | 'completed' = 'running',
): AccountDeletionReceipt => ({
  id: 'acd_1',
  status,
  step: status === 'completed' ? 'verify_no_residue' : 'erase_records',
  requested_at: '2026-11-13T00:00:00Z',
  deadline_at: '2026-11-20T00:00:00Z',
  completed_at: status === 'completed' ? '2026-11-13T01:00:00Z' : null,
  steps: [
    { step: 'inventory', completed_at: '2026-11-13T00:00:01Z', counts: {} },
    { step: 'retire_sandboxes', completed_at: '2026-11-13T00:00:02Z', counts: {} },
  ],
});

function accountClient(overrides: Partial<NonNullable<CmaWebClient['accountDeletion']>> = {}) {
  return {
    ...unavailableClient,
    accountDeletion: {
      request: vi.fn().mockResolvedValue(receipt()),
      status: vi.fn().mockResolvedValue(receipt()),
      ...overrides,
    },
  } satisfies CmaWebClient;
}

afterEach(() => {
  localStorage.clear();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe('account deletion dialog', () => {
  it('keeps the confirm key disabled until acknowledgement', async () => {
    const actor = userEvent.setup();
    render(
      <AccountDeletionDialog
        client={accountClient()}
        viewerId="owner"
        initial={createAccountDeletionRequest()}
        onClose={vi.fn()}
        onAccepted={vi.fn()}
        onPending={vi.fn()}
      />,
    );
    const confirm = screen.getByRole('button', { name: 'Delete account' });
    expect(confirm).toBeDisabled();
    await actor.click(screen.getByRole('checkbox'));
    expect(confirm).toBeEnabled();
  });

  it('persists the key before posting once and reuses it after reload', async () => {
    const actor = userEvent.setup();
    const request = vi.fn(async ({ idempotencyKey }: { idempotencyKey: string }) => {
      expect(localStorage.getItem(accountDeletionStorageKey('owner'))).toContain(idempotencyKey);
      return receipt();
    });
    const client = accountClient({ request });
    const initial = createAccountDeletionRequest();
    const view = render(
      <AccountDeletionDialog
        client={client}
        viewerId="owner"
        initial={initial}
        onClose={vi.fn()}
        onAccepted={vi.fn()}
        onPending={vi.fn()}
      />,
    );
    await actor.click(screen.getByRole('checkbox'));
    await actor.click(screen.getByRole('button', { name: 'Delete account' }));
    expect(request).toHaveBeenCalledOnce();
    expect(restoreAccountDeletion('owner')?.key).toBe(initial.key);
    view.unmount();
    render(
      <AccountDeletionDialog
        client={client}
        viewerId="owner"
        initial={{ ...restoreAccountDeletion('owner')!, accepted: true }}
        restored
        onClose={vi.fn()}
        onAccepted={vi.fn()}
        onPending={vi.fn()}
      />,
    );
    await act(async () => {});
    expect(request).toHaveBeenCalledOnce();
  });

  it('uses the original key for Check request after an uncertain response', async () => {
    const actor = userEvent.setup();
    const initial = createAccountDeletionRequest();
    const request = vi
      .fn()
      .mockRejectedValueOnce(new Error('Connection lost'))
      .mockResolvedValueOnce(receipt());
    render(
      <AccountDeletionDialog
        client={accountClient({
          request,
          status: vi.fn().mockRejectedValue(new Error('Request is not available yet')),
        })}
        viewerId="owner"
        initial={initial}
        onClose={vi.fn()}
        onAccepted={vi.fn()}
        onPending={vi.fn()}
      />,
    );
    await actor.click(screen.getByRole('checkbox'));
    await actor.click(screen.getByRole('button', { name: 'Delete account' }));
    await screen.findByRole('button', { name: 'Check request' });
    await actor.click(screen.getByRole('button', { name: 'Check request' }));
    expect(request.mock.calls.map(([options]) => options.idempotencyKey)).toEqual([
      initial.key,
      initial.key,
    ]);
  });
});

describe('account deletion pending page', () => {
  it('shows progress and Sign out without account data or navigation', async () => {
    const request = { ...createAccountDeletionRequest(), accepted: true };
    localStorage.setItem(accountDeletionStorageKey('owner'), JSON.stringify(request));
    render(
      <AccountDeletionPending client={accountClient()} viewerId="owner" onSignOut={vi.fn()} />,
    );
    expect(
      await screen.findByRole('heading', { name: 'Deletion in progress' }),
    ).toBeInTheDocument();
    expect(screen.getByText('Cleanup progress', { selector: 'summary' })).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: /account menu/i })).not.toBeInTheDocument();
    expect(screen.queryByText('Welcome to CMA')).not.toBeInTheDocument();
  });

  it('does not claim deletion when a status read fails', async () => {
    const request = { ...createAccountDeletionRequest(), accepted: true };
    localStorage.setItem(accountDeletionStorageKey('owner'), JSON.stringify(request));
    render(
      <AccountDeletionPending
        client={accountClient({ status: vi.fn().mockRejectedValue(new Error('offline')) })}
        viewerId="owner"
        onSignOut={vi.fn()}
      />,
    );
    expect(
      await screen.findByText('Could not check cleanup progress. Deletion is not confirmed.'),
    ).toBeInTheDocument();
    expect(screen.queryByText(/Deleted|Done/)).not.toBeInTheDocument();
  });

  it('shows blocked delay copy without submitting a new request', async () => {
    const request = { ...createAccountDeletionRequest(), accepted: true };
    localStorage.setItem(accountDeletionStorageKey('owner'), JSON.stringify(request));
    const submit = vi.fn();
    render(
      <AccountDeletionPending
        client={accountClient({
          request: submit,
          status: vi.fn().mockResolvedValue(receipt('blocked')),
        })}
        viewerId="owner"
        onSignOut={vi.fn()}
      />,
    );
    expect(
      await screen.findByText(
        'Cleanup is delayed. Our team will handle it. You do not need to submit another request.',
      ),
    ).toBeInTheDocument();
    expect(submit).not.toHaveBeenCalled();
  });

  it('leaves the restricted page after a completed record and clears the old key', async () => {
    const request = { ...createAccountDeletionRequest(), accepted: true };
    const onCompleted = vi.fn();
    localStorage.setItem(accountDeletionStorageKey('owner'), JSON.stringify(request));
    render(
      <AccountDeletionPending
        client={accountClient({ status: vi.fn().mockResolvedValue(receipt('completed')) })}
        viewerId="owner"
        onCompleted={onCompleted}
        onSignOut={vi.fn()}
      />,
    );
    await screen.findByText('Cleanup completed and verified.');
    expect(onCompleted).toHaveBeenCalledOnce();
    expect(restoreAccountDeletion('owner')).toBeUndefined();
  });

  it('reconciles an unconfirmed key once after a not-found status before continuing', async () => {
    const request = { ...createAccountDeletionRequest(), accepted: false };
    const submit = vi.fn().mockResolvedValue(receipt());
    const status = vi
      .fn()
      .mockRejectedValueOnce(new ApiError(404, 'Missing', 'account_deletion_not_found'))
      .mockResolvedValue(receipt());
    localStorage.setItem(accountDeletionStorageKey('owner'), JSON.stringify(request));
    render(
      <AccountDeletionPending
        client={accountClient({ request: submit, status })}
        viewerId="owner"
        onSignOut={vi.fn()}
      />,
    );
    await screen.findByText('Your request was accepted. Cleanup is still in progress.');
    expect(submit).toHaveBeenCalledOnce();
    expect(submit.mock.calls[0]?.[0].idempotencyKey).toBe(request.key);
    expect(restoreAccountDeletion('owner')?.accepted).toBe(true);
  });

  it('leaves the session-ended page without retrying after a 401 status', async () => {
    const request = { ...createAccountDeletionRequest(), accepted: true };
    const status = vi
      .fn()
      .mockRejectedValue(new ApiError(401, 'Expired', 'account_deletion_requires_owner_session'));
    localStorage.setItem(accountDeletionStorageKey('owner'), JSON.stringify(request));
    render(
      <AccountDeletionPending
        client={accountClient({ status })}
        viewerId="owner"
        onSignOut={vi.fn()}
      />,
    );
    expect(
      await screen.findByText('Your sign-in session has ended. Sign out to continue.'),
    ).toBeInTheDocument();
    await act(async () => {});
    expect(status).toHaveBeenCalledOnce();
    expect(screen.getByRole('button', { name: 'Sign out' })).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Check request' })).not.toBeInTheDocument();
  });
});

describe('account deletion app transition', () => {
  it('hides account data behind the accepted dialog and transfers progress on close', async () => {
    const actor = userEvent.setup();
    render(
      <AuthContextProvider
        provider={createMockAuthProvider({
          initiallyAuthenticated: true,
          capabilities: ['delete_own_account'],
        })}
      >
        <CmaClientProvider client={accountClient()}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
    await screen.findByText('Welcome to CMA');
    await actor.click(screen.getByRole('button', { name: /account menu/i }));
    await actor.click(screen.getByRole('menuitem', { name: 'Delete account' }));
    await actor.click(screen.getByRole('checkbox'));
    await actor.click(screen.getByRole('button', { name: 'Delete account' }));
    expect(screen.queryByText('Welcome to CMA')).not.toBeInTheDocument();
    expect(screen.getByRole('dialog', { name: 'Delete account' })).toBeInTheDocument();
    await actor.click(screen.getByText('Close', { selector: 'button' }));
    expect(
      await screen.findByRole('heading', { name: 'Deletion in progress' }),
    ).toBeInTheDocument();
  });

  it('switches to the pending page when any request reports account deletion in progress', async () => {
    render(
      <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
        <CmaClientProvider client={accountClient()}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
    await screen.findByRole('heading', { name: 'Welcome to CMA' });
    notifyAccountDeletionInProgress(
      new ApiError(403, 'Deletion in progress', 'account_deletion_in_progress'),
    );
    expect(
      await screen.findByRole('heading', { name: 'Deletion in progress' }),
    ).toBeInTheDocument();
    expect(screen.queryByText('Welcome to CMA')).not.toBeInTheDocument();
  });

  it('does not let a late account read repopulate the surface after the pending transition', async () => {
    let resolve: ((value: { items: [] }) => void) | undefined;
    const listSandboxes = vi.fn(
      () => new Promise<{ items: [] }>((complete) => (resolve = complete)),
    );
    const client = { ...accountClient(), listSandboxes };
    render(
      <AuthContextProvider provider={createMockAuthProvider({ initiallyAuthenticated: true })}>
        <CmaClientProvider client={client}>
          <App />
        </CmaClientProvider>
      </AuthContextProvider>,
    );
    await screen.findByRole('heading', { name: 'Welcome to CMA' });
    notifyAccountDeletionInProgress(
      new ApiError(403, 'Deletion in progress', 'account_deletion_in_progress'),
    );
    await screen.findByRole('heading', { name: 'Deletion in progress' });
    await act(async () => resolve?.({ items: [] }));
    expect(screen.queryByRole('heading', { name: 'Welcome to CMA' })).not.toBeInTheDocument();
    expect(screen.getByRole('heading', { name: 'Deletion in progress' })).toBeInTheDocument();
  });
});
