import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { AgentSurfaceIssue } from './AgentSurfaceStates';
import { PendingRequestPanel } from './PendingRequestPanel';

const message = 'Keep this exact message.\n包含原始文本';
const controller = {
  pendingMessage: message,
  inputAvailability: { allowed: true },
  recoverPending: vi.fn().mockResolvedValue(undefined),
};
afterEach(() => vi.restoreAllMocks());

it.each([
  { unavailable: true, starting: false, allowed: true, pending: undefined },
  { unavailable: false, starting: true, allowed: false, pending: undefined },
  { unavailable: false, starting: false, allowed: true, pending: 'send' as const },
])('preserves recovery admission %#', (state) => {
  render(
    <PendingRequestPanel
      controller={{
        ...controller,
        pending: state.pending,
        inputAvailability: { allowed: state.allowed },
      }}
      unavailable={state.unavailable}
      starting={state.starting}
    />,
  );
  expect(screen.getByRole('button', { name: 'Recover pending request' })).toBeDisabled();
});

it('only calls existing recovery on explicit input and does not pass a replacement body', () => {
  const recoverPending = vi.fn();
  render(
    <PendingRequestPanel
      controller={{ ...controller, recoverPending }}
      unavailable={false}
      starting={false}
    />,
  );
  expect(recoverPending).not.toHaveBeenCalled();
  expect(
    screen.getByText('This doesn’t mean it failed. Recovery may send the same request again.'),
  ).toBeVisible();
  fireEvent.click(screen.getByRole('button', { name: 'Recover pending request' }));
  expect(recoverPending).toHaveBeenCalledExactlyOnceWith();
});

it('keeps message receipt checking separate from pending control recovery', () => {
  const { rerender } = render(
    <PendingRequestPanel
      controller={{ ...controller, checkingInputReceipt: true }}
      unavailable={false}
      starting={false}
    />,
  );
  expect(screen.queryByRole('button', { name: 'Recover pending request' })).not.toBeInTheDocument();
  rerender(
    <PendingRequestPanel
      controller={{ ...controller, checkingInputReceipt: true, pendingControl: true }}
      unavailable={false}
      starting={false}
    />,
  );
  expect(screen.getByRole('button', { name: 'Recover pending request' })).toBeEnabled();
});

it('copies the original message only after a click and reports success after completion', async () => {
  let finish!: () => void;
  const writeText = vi.fn(
    () =>
      new Promise<void>((resolve) => {
        finish = resolve;
      }),
  );
  vi.stubGlobal('navigator', { clipboard: { writeText } });
  try {
    render(<PendingRequestPanel controller={controller} unavailable={false} starting={false} />);
    expect(writeText).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));
    expect(writeText).toHaveBeenCalledExactlyOnceWith(message);
    expect(screen.queryByText('Message copied.')).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Copy message' })).toBeDisabled();
    finish();
    expect(await screen.findByText('Message copied.')).toBeVisible();
  } finally {
    vi.unstubAllGlobals();
  }
});

it('offers selectable original text when clipboard access fails', async () => {
  vi.stubGlobal('navigator', {});
  try {
    render(<PendingRequestPanel controller={controller} unavailable={false} starting={false} />);
    fireEvent.click(screen.getByRole('button', { name: 'Copy message' }));
    await waitFor(() =>
      expect(screen.getByRole('textbox', { name: 'Message to copy' })).toHaveValue(message),
    );
    expect(screen.queryByText('Message copied.')).not.toBeInTheDocument();
    expect(screen.getByRole('textbox', { name: 'Message to copy' })).toHaveAttribute('readonly');
  } finally {
    vi.unstubAllGlobals();
  }
});

it('uses the pending panel instead of the generic incomplete-operation error for an unknown result', () => {
  const { rerender } = render(
    <AgentSurfaceIssue
      issue="The Agent operation did not complete."
      unconfirmed
      historyIncomplete={false}
      onDismiss={vi.fn()}
    />,
  );
  expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  rerender(
    <AgentSurfaceIssue
      issue="Access denied"
      unconfirmed
      historyIncomplete={false}
      onDismiss={vi.fn()}
    />,
  );
  expect(screen.getByRole('alert')).toHaveTextContent('Access denied');
});
