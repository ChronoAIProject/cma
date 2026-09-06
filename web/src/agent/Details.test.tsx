import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL } from '../test/agent';
import { AgentDetails } from './Details';

it('shows lineage, profile, Worktree, first-message delivery, and guarded controls', async () => {
  const onStop = vi.fn();
  const actor = userEvent.setup();
  render(
    <AgentDetails detail={AGENT_DETAIL} onNavigate={vi.fn()} onStop={onStop} onDelete={vi.fn()} />,
  );
  for (const band of [
    'Sandbox',
    'Workspace',
    'Worktree',
    'Agent profile',
    'First-message delivery',
  ]) {
    expect(screen.getByRole('heading', { name: band })).toBeInTheDocument();
  }
  expect(screen.getByText('agent/one')).toBeInTheDocument();
  expect(screen.getByText('github')).toBeInTheDocument();
  expect(screen.getByText('ornn:typescript')).toBeInTheDocument();
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
  await actor.click(screen.getByRole('checkbox'));
  await actor.click(confirm);
  expect(onDelete).toHaveBeenCalledOnce();
});
