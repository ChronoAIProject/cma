import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { AgentDetails } from './Details';
import { NamedReferences } from '../test/references';

it.each(['ready', 'stopped'] as const)(
  'holds %s Agent lifecycle controls until history hydration finishes',
  (status) => {
    const draw = (historyLoading: boolean) => (
      <AgentDetails
        detail={{ ...AGENT_DETAIL, status }}
        historyLoading={historyLoading}
        onNavigate={vi.fn()}
        onStop={vi.fn()}
        onDelete={vi.fn()}
      />
    );
    const { rerender } = render(draw(true));
    expect(screen.getByText('Loading conversation activity.')).toHaveAttribute('role', 'status');
    expect(screen.getByRole('button', { name: 'Stop' })).toBeDisabled();
    expect(screen.getByRole('button', { name: 'Delete' })).toBeDisabled();
    rerender(draw(false));
    expect(screen.queryByText('Loading conversation activity.')).not.toBeInTheDocument();
    expect(
      screen.getByRole('button', { name: status === 'ready' ? 'Stop' : 'Delete' }),
    ).toBeEnabled();
  },
);

it('shows lineage, profile, Worktree, first-message delivery, and guarded controls', async () => {
  const onStop = vi.fn();
  const actor = userEvent.setup();
  render(
    <NamedReferences>
      <AgentDetails detail={AGENT_DETAIL} onNavigate={vi.fn()} onStop={onStop} onDelete={vi.fn()} />
    </NamedReferences>,
  );
  for (const band of [
    'Sandbox',
    'Workspace',
    'Worktree',
    'Agent settings',
    'First-message delivery',
  ]) {
    expect(screen.getByRole('heading', { name: band })).toBeInTheDocument();
  }
  expect(screen.getByText('agent/one')).toBeInTheDocument();
  expect(await screen.findByRole('link', { name: 'GitHub' })).toHaveAttribute(
    'href',
    'https://nyx.chrono-ai.fun/services/catalog-github',
  );
  expect(screen.getAllByText('Completed')).toHaveLength(2);
  expect(screen.getByRole('button', { name: 'Delete' })).toBeDisabled();
  await actor.click(screen.getByRole('button', { name: 'Stop' }));
  await actor.click(screen.getByRole('button', { name: /^Stop$/ }));
  expect(onStop).toHaveBeenCalledOnce();
});

it('requires explicit acknowledgement before deleting a stopped Agent', async () => {
  const onDelete = vi.fn();
  const actor = userEvent.setup();
  render(
    <AgentDetails
      detail={{ ...AGENT_DETAIL, status: 'stopped' }}
      onNavigate={vi.fn()}
      onStop={vi.fn()}
      onDelete={onDelete}
    />,
  );
  await actor.click(screen.getByRole('button', { name: 'Delete' }));
  const confirm = screen.getByRole('button', { name: 'Delete Agent' });
  expect(confirm).toBeDisabled();
  await actor.click(
    screen.getByRole('checkbox', { name: 'I understand this Agent and Worktree will be deleted.' }),
  );
  await actor.click(confirm);
  expect(onDelete).toHaveBeenCalledOnce();
});
