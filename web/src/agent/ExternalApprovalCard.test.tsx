import { act, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ExternalApprovalCard } from './ExternalApprovalCard';
import { approvalReviewUrl } from './externalApproval';
import {
  externalApprovalFixture as approval,
  externalReviewPolicyFixture as policy,
} from './externalApproval.fixture';

afterEach(() => vi.useRealTimers());

it('shows the yellow approval state and the exact trusted review link, with no native decisions', () => {
  const view = render(
    <ExternalApprovalCard approval={approval} policy={policy} active readOnly={false} />,
  );
  expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
  expect(view.container.querySelector('.character-avatar')).toHaveAttribute(
    'data-pose',
    'approval',
  );
  expect(view.container.querySelector('.character-avatar')).toHaveAttribute('data-tone', 'warning');
  expect(screen.getByRole('link', { name: 'Review and approve' })).toHaveAttribute(
    'href',
    approval.reviewUrl,
  );
  expect(screen.getByRole('link')).toHaveAttribute('rel', 'noopener noreferrer');
  expect(screen.queryByRole('button')).toBeNull();
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', approval.deadlineAt);
});

it.each([undefined, { origin: policy.origin, path: '/wrong-page' }])(
  'holds the review action without the exact configured policy %#',
  (trusted) => {
    render(<ExternalApprovalCard approval={approval} policy={trusted} active readOnly={false} />);
    expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.getByText(/review link is unavailable/)).toBeVisible();
  },
);

it.each([
  { readOnly: true, canReview: true },
  { readOnly: false, canReview: false },
])('shows a non-actionable wait to a restricted viewer %#', (rights) => {
  render(
    <ExternalApprovalCard
      approval={{ ...approval, canReview: rights.canReview }}
      policy={policy}
      active
      readOnly={rights.readOnly}
    />,
  );
  expect(screen.queryByRole('link')).toBeNull();
  expect(screen.queryByRole('button')).toBeNull();
  expect(screen.getByText(/authorized participant/)).toBeVisible();
});

it('does not end the turn or resolve approval when the clock passes the deadline or focus returns', () => {
  vi.useFakeTimers();
  vi.setSystemTime(new Date(approval.waitingSince));
  const view = render(
    <ExternalApprovalCard approval={approval} policy={policy} active readOnly={false} />,
  );
  act(() => vi.advanceTimersByTime(600_000));
  fireEvent.focus(window);
  view.rerender(
    <ExternalApprovalCard approval={{ ...approval }} policy={policy} active readOnly={false} />,
  );
  expect(screen.getByRole('heading', { name: 'Waiting for your approval' })).toBeVisible();
  expect(view.container.querySelector('time')).toHaveAttribute('datetime', approval.deadlineAt);
  expect(screen.queryByText(/This turn has ended/)).toBeNull();
});

it.each([undefined, 'approved', 'denied', 'expired'])(
  'removes the action and asking character after inactivity/resolution %#',
  (decision) => {
    const view = render(
      <ExternalApprovalCard
        approval={{ ...approval, decision }}
        policy={policy}
        active={decision !== undefined}
        readOnly={false}
      />,
    );
    expect(screen.queryByRole('link')).toBeNull();
    expect(screen.queryByRole('button')).toBeNull();
    expect(view.container.querySelector('.character-avatar')).toBeNull();
    expect(view.container.querySelector('time')).toBeNull();
  },
);

it.each([
  'http://identity.example.test/assistant/approvals',
  'https://other.example.test/assistant/approvals',
  'https://identity.example.test:444/assistant/approvals',
  'https://name@identity.example.test/assistant/approvals',
  'https://identity.example.test/assistant/approvals?redirect=https://other.example.test',
  'https://identity.example.test/assistant/approvals#credential',
  'https://identity.example.test/%66ixture-approvals',
  'https://identity.example.test/other/../assistant/approvals',
  '//identity.example.test/assistant/approvals',
  '/assistant/approvals',
  'javascript:alert(1)',
  ' https://identity.example.test/assistant/approvals',
  'https://identity.example.test\\fixture-approvals',
])('refuses an untrusted review destination %s', (candidate) => {
  expect(approvalReviewUrl(candidate, policy)).toBeUndefined();
});
