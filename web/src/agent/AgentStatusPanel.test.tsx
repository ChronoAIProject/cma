import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { sandboxFixture } from '../test/hierarchy';
import { AgentStatusPanel } from './AgentStatusPanel';

vi.mock('../character/CharacterAvatar', () => ({ CharacterAvatar: () => null }));
const preparing = { ...AGENT_DETAIL, status: 'starting' as const, readyAt: undefined };

it('shows preparing until a real ready snapshot arrives, with recorded details collapsed', () => {
  const props = {
    running: false,
    historyIncomplete: false,
    observation: { sandbox: sandboxFixture(), stale: false },
  };
  const { rerender } = render(<AgentStatusPanel {...props} detail={preparing} />);
  expect(screen.getByRole('heading', { name: 'Getting its cloud computer ready' })).toBeVisible();
  expect(screen.getByText('Setup details').closest('details')).not.toHaveAttribute('open');
  fireEvent.click(screen.getByText('Setup details'));
  expect(screen.getByRole('region', { name: 'Recorded setup progress' })).toHaveTextContent(
    'Preparing the agentIn progress',
  );
  rerender(<AgentStatusPanel {...props} detail={AGENT_DETAIL} />);
  expect(screen.queryByRole('region', { name: 'Agent preparation' })).not.toBeInTheDocument();
});

it('labels stale preparation honestly and uses only the actual environment observation time', () => {
  const observation = {
    sandbox: sandboxFixture(),
    stale: true,
    observedAt: '2026-01-02T03:04:05Z',
  };
  const { container, rerender } = render(
    <AgentStatusPanel
      detail={preparing}
      observation={observation}
      running={false}
      historyIncomplete={false}
    />,
  );
  expect(screen.getByText('Last confirmed: Preparing')).toBeVisible();
  expect(screen.getByRole('heading', { name: 'We can’t confirm setup progress' })).toBeVisible();
  expect(container.querySelector('time')).toHaveAttribute('datetime', observation.observedAt);
  rerender(
    <AgentStatusPanel
      detail={preparing}
      observation={{ ...observation, observedAt: undefined }}
      running={false}
      historyIncomplete={false}
    />,
  );
  expect(container.querySelector('time')).toBeNull();
  expect(screen.queryByText(/12 minutes|Ready to work/)).not.toBeInTheDocument();
});

it('ignores timestamps from another environment and invalid timestamps', () => {
  const props = { detail: preparing, running: false, historyIncomplete: false };
  const { container, rerender } = render(
    <AgentStatusPanel
      {...props}
      observation={{
        sandbox: { ...sandboxFixture(), id: 'other' },
        stale: true,
        observedAt: '2026-01-02T03:04:05Z',
      }}
    />,
  );
  expect(container.querySelector('time')).toBeNull();
  rerender(
    <AgentStatusPanel
      {...props}
      observation={{ sandbox: sandboxFixture(), stale: false, observedAt: 'invalid' }}
    />,
  );
  expect(container.querySelector('time')).toBeNull();
});

it('keeps working presentation free of invented steps and distinguishes incomplete history', () => {
  const props = {
    detail: AGENT_DETAIL,
    observation: { sandbox: sandboxFixture(), stale: false },
    running: true,
  };
  const { container, rerender } = render(<AgentStatusPanel {...props} historyIncomplete={false} />);
  expect(screen.getByRole('heading', { name: 'Working on it' })).toBeVisible();
  expect(container.querySelector('ol')).toBeNull();
  expect(container.querySelector('time')).toBeNull();
  rerender(<AgentStatusPanel {...props} historyIncomplete />);
  expect(screen.queryByRole('heading', { name: 'Working on it' })).not.toBeInTheDocument();
  expect(screen.getByText('Last confirmed: Working')).toBeVisible();
});

it('preserves the real setup failure and remediation behind details', () => {
  render(
    <AgentStatusPanel
      detail={{
        ...preparing,
        failure: {
          class: 'preparation',
          detail: 'Setup failed',
          remediation: 'Review configuration',
        },
      }}
      observation={undefined}
      running={false}
      historyIncomplete={false}
    />,
  );
  expect(screen.getByRole('heading', { name: 'Agent needs attention' })).toBeVisible();
  fireEvent.click(screen.getByText('Setup details'));
  expect(screen.getByText('Review configuration')).toBeVisible();
});

it('keeps uncertainty when a failed observation has been cleared during a longer outage', () => {
  render(
    <AgentStatusPanel
      detail={preparing}
      observation={undefined}
      running={false}
      historyIncomplete={false}
    />,
  );
  expect(screen.getByText('Last confirmed: Preparing')).toBeVisible();
  expect(
    screen.queryByRole('heading', { name: 'Getting its cloud computer ready' }),
  ).not.toBeInTheDocument();
});

it.each(['waiting_approval', 'paused', 'stopped'] as const)(
  'does not label a retained active turn as working while the Agent is %s',
  (status) => {
    render(
      <AgentStatusPanel
        detail={{ ...AGENT_DETAIL, status }}
        observation={{ sandbox: sandboxFixture(), stale: false }}
        running
        historyIncomplete={false}
      />,
    );
    expect(screen.queryByRole('region')).not.toBeInTheDocument();
  },
);
