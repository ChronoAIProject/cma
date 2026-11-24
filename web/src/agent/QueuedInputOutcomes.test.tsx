import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { v2CommandStatus } from '../api-client/responses-v2/controls-types';
import { QueuedInputOutcomes } from './QueuedInputOutcomes';
import {
  commandOutcome,
  queuedOutcomeText,
  type QueuedInputOutcome,
} from './transport/queuedCommandOutcomes';
import { status } from './transport/queuedReconciliation.fixture';

const outcome = (kind: QueuedInputOutcome['kind'], terminal = true): QueuedInputOutcome => ({
  commandId: 'private-command-id',
  responseId: 'private-response-id',
  kind,
  terminal,
});
it.each(Object.entries(queuedOutcomeText))(
  'renders safe %s wording without identifiers or write actions',
  (kind, text) => {
    const refresh = vi.fn();
    const { container } = render(
      <QueuedInputOutcomes
        items={[outcome(kind as QueuedInputOutcome['kind'])]}
        onRefresh={refresh}
      />,
    );
    if (kind === 'cancelled') {
      const summary = screen.getByText('Cancelled messages (1)');
      expect(summary.closest('details')).not.toHaveAttribute('open');
      expect(screen.getByText(text)).not.toBeVisible();
      fireEvent.click(summary);
    }
    expect(screen.getByText(text)).toBeVisible();
    expect(screen.getByText('Previously submitted message')).toBeVisible();
    expect(container).not.toHaveTextContent('private-command-id');
    expect(container).not.toHaveTextContent('private-response-id');
    expect(screen.queryByRole('button')).not.toBeInTheDocument();
  },
);
it('refreshes unknown status without resubmitting or editing its retained text', () => {
  const refresh = vi.fn();
  render(
    <QueuedInputOutcomes
      items={[{ ...outcome('unknown', false), input: 'Retained instruction' }]}
      onRefresh={refresh}
    />,
  );
  expect(screen.getByRole('article', { name: 'You' })).toHaveTextContent('Retained instruction');
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  expect(refresh).toHaveBeenCalledOnce();
  expect(screen.getAllByRole('button')).toHaveLength(1);
});
it('claims before-start expiry only when delivery is absent and keeps execution uncertainty explicit', () => {
  const expired = status(1, 'expired');
  expect(commandOutcome(v2CommandStatus(expired))).toEqual({ kind: 'expired', terminal: true });
  expect(
    commandOutcome(v2CommandStatus({ ...expired, turn_sequence: 1, delivery: 'dispatch_fenced' }))
      .kind,
  ).toBe('failed');
  expect(
    commandOutcome(
      v2CommandStatus({
        ...expired,
        failure: 'runtime_lost',
        turn_sequence: 1,
        delivery: 'runtime_lost',
      }),
    ).kind,
  ).toBe('uncertain');
});
