import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { PreparationView } from './PreparationView';
import { derivePreparationView } from './preparationPresentation';

it('shows one original request and keeps recovery actions outside collapsed setup details', () => {
  const retry = vi.fn();
  const model = derivePreparationView({
    receipt: { id: 'operation' },
    readError: 'Last setup read failed',
  });
  render(
    <PreparationView
      model={model}
      message="The exact original request"
      actions={<button onClick={retry}>Retry setup status</button>}
      details={<p>Recorded provisioning stages</p>}
      notices={<p role="alert">Conversation updates interrupted</p>}
    />,
  );
  expect(screen.getAllByText('The exact original request')).toHaveLength(1);
  expect(screen.getByRole('heading', { level: 1, name: 'New agent' })).toBeVisible();
  expect(screen.getByText('Acceptance time unavailable')).toBeVisible();
  expect(screen.getByText('Last setup read failed')).toBeVisible();
  expect(screen.getByText('Conversation updates interrupted')).toBeVisible();
  expect(screen.getByRole('button', { name: 'Retry setup status' }).closest('details')).toBeNull();
  expect(retry).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry setup status' }));
  expect(retry).toHaveBeenCalledOnce();
});

it('does not claim acceptance or a task that has not started without evidence', () => {
  render(<PreparationView model={derivePreparationView({})} message="Unknown request" />);
  expect(screen.queryByText('Request accepted')).toBeNull();
  expect(screen.getByText('Checking request acceptance')).toBeVisible();
  expect(screen.queryByText(/It hasn’t started yet/)).toBeNull();
});

it('exits the preparation surface when real task-start evidence is available', () => {
  render(
    <PreparationView
      model={derivePreparationView({ initialTask: 'started' })}
      message="Already started"
    />,
  );
  expect(screen.queryByRole('region', { name: 'Task preparation' })).toBeNull();
  expect(screen.queryByText('Already started')).toBeNull();
});

it('makes the original acceptance timestamp focusable and never turns progress into acceptance', () => {
  const model = derivePreparationView({
    receipt: { id: 'operation' },
    operation: {
      id: 'operation',
      status: 'running',
      phase: 'sandbox_allocation',
      events: [],
      accepted_at: '2026-11-23T09:00:00Z',
    },
  });
  const { container } = render(<PreparationView model={model} />);
  const time = container.querySelector('time')!;
  expect(time).toHaveAttribute('dateTime', '2026-11-23T09:00:00Z');
  expect(time).toHaveAttribute('tabindex', '0');
  expect(time.getAttribute('title')).toBe(time.getAttribute('aria-label'));
  expect(screen.queryByText(/Last progress/)).toBeNull();
});
