import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { RiskConfirmation } from './RiskConfirmation';

const defaults = {
  open: true,
  title: 'Confirm action',
  description: 'This cannot be undone.',
  acknowledgeLabel: 'I understand the consequence.',
  confirmLabel: 'Confirm action',
  onCancel: vi.fn(),
  onConfirm: vi.fn(),
};

it('keeps the shared default labels, emphasis and acknowledgement requirement', () => {
  render(<RiskConfirmation {...defaults} />);
  expect(screen.getByRole('button', { name: 'Cancel' })).toHaveClass('button--outline');
  const confirm = screen.getByRole('button', { name: 'Confirm action' });
  expect(confirm).toHaveClass('button--primary');
  expect(confirm).toBeDisabled();
  fireEvent.click(screen.getByRole('checkbox'));
  expect(confirm).toBeEnabled();
});

it('can emphasize keeping an environment without removing any confirmation guards', () => {
  const onCancel = vi.fn();
  const onConfirm = vi.fn();
  const view = render(
    <RiskConfirmation
      {...defaults}
      cancelLabel="Keep environment"
      actionEmphasis="cancel"
      confirmLabel="Request deletion"
      onCancel={onCancel}
      onConfirm={onConfirm}
    />,
  );
  const keep = screen.getByRole('button', { name: 'Keep environment' });
  const remove = screen.getByRole('button', { name: 'Request deletion' });
  expect(keep).toHaveClass('button--primary');
  expect(remove).toHaveClass('button--outline');
  fireEvent.click(remove);
  expect(onConfirm).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('checkbox'));
  view.rerender(
    <RiskConfirmation
      {...defaults}
      cancelLabel="Keep environment"
      actionEmphasis="cancel"
      confirmLabel="Request deletion"
      confirmDisabled
      onCancel={onCancel}
      onConfirm={onConfirm}
    />,
  );
  expect(remove).toBeDisabled();
  fireEvent.click(keep);
  expect(onCancel).toHaveBeenCalledOnce();
  expect(onConfirm).not.toHaveBeenCalled();
  expect(screen.getByRole('checkbox')).not.toBeChecked();
});
