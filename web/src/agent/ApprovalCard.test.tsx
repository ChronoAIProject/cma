import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { ApprovalCard } from './ApprovalCard';

const request = { id: 'approval_1', reason: 'Push to origin', message: 'git push origin main' };

it('names the request and offers Deny, Abort and Allow with their effects', async () => {
  const onApproval = vi.fn();
  render(<ApprovalCard approval={request} busy={false} readOnly={false} onApproval={onApproval} />);
  const card = screen.getByRole('region', { name: 'Push to origin' });
  expect(card).toHaveTextContent('Approval');
  expect(screen.getByRole('heading', { level: 2, name: 'Push to origin' })).toBeInTheDocument();
  expect(screen.getByText('git push origin main')).toBeInTheDocument();
  expect(screen.getAllByRole('button').map((button) => button.textContent)).toEqual([
    'Deny',
    'Abort',
    'Allow',
  ]);
  expect(screen.getByRole('button', { name: 'Deny' })).toHaveAccessibleDescription(
    'Refuse this request. The Agent can continue another way.',
  );
  expect(screen.getByRole('button', { name: 'Abort' })).toHaveAccessibleDescription(
    'Cancel the request without an answer.',
  );
  const allow = screen.getByRole('button', { name: 'Allow' });
  expect(allow).toHaveAccessibleDescription('Let the Agent go ahead with this request.');
  expect(allow).toHaveClass('button--primary');
  await userEvent.click(allow);
  await userEvent.click(screen.getByRole('button', { name: 'Abort' }));
  expect(onApproval.mock.calls).toEqual([
    ['approval_1', 'allow'],
    ['approval_1', 'abort'],
  ]);
});

it('holds every answer while a command is pending or input is not allowed', () => {
  render(<ApprovalCard approval={request} busy readOnly={false} onApproval={vi.fn()} />);
  for (const name of ['Deny', 'Abort', 'Allow'])
    expect(screen.getByRole('button', { name })).toBeDisabled();
});

it('tells a viewer without authority who answers, and offers no buttons', () => {
  render(<ApprovalCard approval={request} busy={false} readOnly onApproval={vi.fn()} />);
  expect(
    screen.getByText('Waiting for an authorized participant to answer this approval.'),
  ).toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
});

it('shows the recorded decision once the approval is resolved, without a detail it never had', () => {
  render(
    <ApprovalCard
      approval={{ id: 'approval_2', reason: 'confirmation', decision: 'deny' }}
      busy={false}
      readOnly
      onApproval={vi.fn()}
    />,
  );
  expect(screen.getByText('deny')).toBeInTheDocument();
  expect(screen.queryByRole('button')).not.toBeInTheDocument();
  expect(screen.queryByText(/Waiting for an authorized/)).not.toBeInTheDocument();
  expect(document.querySelector('.approval__detail')).toBeNull();
});
