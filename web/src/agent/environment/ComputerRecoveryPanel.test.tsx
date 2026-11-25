import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { ComputerRecoveryPanel } from './ComputerRecoveryPanel';
import { desktopStopMessages } from '../desktopReconnect';

it.each(['unavailable', 'authorization', 'refused', 'exhausted'] as const)(
  'uses the actual %s stop reason without claiming Agent progress',
  (stop) => {
    const onReconnect = vi.fn();
    render(
      <ComputerRecoveryPanel
        stop={stop}
        error={desktopStopMessages[stop]}
        reconnectDisabled={false}
        onReconnect={onReconnect}
      />,
    );
    expect(
      screen.queryByRole('heading', { name: 'Automatic reconnect attempts have stopped' }) !== null,
    ).toBe(stop === 'exhausted');
    expect(
      screen.queryByText(/Last confirmed|work continues|nothing lost/i),
    ).not.toBeInTheDocument();
    expect(onReconnect).not.toHaveBeenCalled();
    fireEvent.click(screen.getByRole('button', { name: 'Reconnect view' }));
    expect(onReconnect).toHaveBeenCalledTimes(1);
  },
);

it('keeps disabled reconnect and the available conversation return independent', () => {
  const onReconnect = vi.fn();
  const onReturnToConversation = vi.fn();
  render(
    <ComputerRecoveryPanel
      error="Synthetic connection failure"
      reconnectDisabled
      onReconnect={onReconnect}
      onReturnToConversation={onReturnToConversation}
    />,
  );
  expect(screen.getByRole('button', { name: 'Reconnect view' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Return to conversation' }));
  expect(onReturnToConversation).toHaveBeenCalledTimes(1);
  expect(onReconnect).not.toHaveBeenCalled();
});
