import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AccessStatusPanel } from './AccessStatusPanel';
import { ScopedNavigation } from './ScopedNavigation';
import { createClient } from '@cma/client';

const choices = {
  options: [
    { value: '', label: 'Personal' },
    { value: 'shared', label: 'Shared' },
  ],
  unavailable: true,
};

it('keeps the selected scope visible and disables repeated retry while the existing read is pending', () => {
  const reload = vi.fn();
  const close = vi.fn();
  const { rerender } = render(
    <AccessStatusPanel choices={{ ...choices, reload }} scope="shared" onClose={close} />,
  );
  expect(screen.getByRole('heading', { name: 'Shared' })).toBeVisible();
  expect(screen.getByRole('alert')).toHaveTextContent('Your organizations didn’t load');
  fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
  expect(reload).toHaveBeenCalledOnce();
  rerender(
    <AccessStatusPanel
      choices={{ ...choices, reload, loading: true }}
      scope="shared"
      onClose={close}
    />,
  );
  expect(screen.getByRole('button', { name: 'Try again' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Back to your current view' }));
  expect(close).toHaveBeenCalledOnce();
});

it('opens locally and returns focus without navigating or losing the current draft', () => {
  const navigate = vi.fn();
  render(
    <ScopedNavigation
      client={createClient({ fetch: vi.fn() })}
      personal={<textarea aria-label="Current draft" defaultValue="Retain my draft" />}
      scope=""
      choices={choices}
      onNavigate={navigate}
    />,
  );
  const trigger = screen.getByRole('button', { name: 'Review access' });
  trigger.focus();
  fireEvent.click(trigger);
  expect(screen.getByRole('dialog', { name: 'Access' })).toBeVisible();
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  expect(screen.getByLabelText('Current draft')).toHaveValue('Retain my draft');
  expect(trigger).toHaveFocus();
  expect(navigate).not.toHaveBeenCalled();
});

it('returns focus to access after retry restores organization choices', () => {
  const client = createClient({ fetch: vi.fn() });
  const navigate = vi.fn();
  const view = (unavailable: boolean) => (
    <ScopedNavigation
      client={client}
      personal={<textarea aria-label="Current draft" defaultValue="Retain my draft" />}
      scope=""
      choices={{ ...choices, unavailable }}
      onNavigate={navigate}
    />
  );
  const { rerender } = render(view(true));
  const trigger = screen.getByRole('button', { name: 'Review access' });
  trigger.focus();
  fireEvent.click(trigger);
  rerender(view(false));
  expect(screen.getByRole('status')).toHaveTextContent('Organization choices are available');
  fireEvent.keyDown(screen.getByRole('dialog'), { key: 'Escape' });
  expect(trigger).toHaveFocus();
  expect(screen.getByLabelText('Current draft')).toHaveValue('Retain my draft');
  expect(navigate).not.toHaveBeenCalled();
});
