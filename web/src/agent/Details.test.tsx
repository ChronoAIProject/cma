import { render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { AgentDetails } from './Details';

const defaults = {
  model: 'gpt-6',
  reasoning_effort: 'high',
  permission_mode: 'autonomous',
} as const;

it('shows the three settings and fixed Agent key scopes without technical or lifecycle panels', async () => {
  const update = vi.fn().mockResolvedValue({ ...defaults, reasoning_effort: 'medium' });
  const client = agentClient({
    readAgentSettings: async () => defaults,
    updateAgentSettings: update,
  });
  render(
    <CmaClientProvider client={client}>
      <AgentDetails detail={AGENT_DETAIL} open />
    </CmaClientProvider>,
  );
  const panel = within(screen.getByRole('complementary', { name: 'Agent details' }));
  expect(await panel.findByLabelText('Model')).toHaveValue('gpt-6');
  expect(panel.getByLabelText('Reasoning effort')).toHaveValue('high');
  expect(panel.getByLabelText('Permission mode')).toHaveValue('autonomous');
  expect(panel.getByRole('group', { name: 'Agent key scope' })).toBeVisible();
  for (const scope of [
    'chrono-llm-public Required',
    'ornn-api Required',
    'github Already granted',
  ]) {
    const checkbox = panel.getByRole('checkbox', { name: scope });
    expect(checkbox).toBeChecked();
    expect(checkbox).toBeDisabled();
  }
  expect(panel.getAllByRole('heading').map((heading) => heading.textContent)).toEqual(['Details']);
  expect(panel.queryByText(AGENT_DETAIL.workspace.path)).not.toBeInTheDocument();
  expect(panel.queryByText(AGENT_DETAIL.worktree.branch)).not.toBeInTheDocument();
  expect(panel.queryByRole('button', { name: 'Stop' })).not.toBeInTheDocument();
  expect(panel.queryByRole('button', { name: 'Delete' })).not.toBeInTheDocument();
  await userEvent.selectOptions(panel.getByLabelText('Reasoning effort'), 'medium');
  await userEvent.click(panel.getByRole('button', { name: 'Apply settings' }));
  await panel.findByText('Settings applied to this Agent.');
  expect(update).toHaveBeenCalledWith(
    AGENT_DETAIL.id,
    { ...defaults, reasoning_effort: 'medium' },
    expect.objectContaining({ idempotencyKey: expect.any(String) }),
  );
});

it.each(['history', 'working', 'command'] as const)(
  'holds settings updates during %s and preserves the draft until the Agent is ready',
  async (busy) => {
    const update = vi.fn();
    const client = agentClient({
      readAgentSettings: async () => defaults,
      updateAgentSettings: update,
    });
    const draw = (blocked: boolean) => (
      <CmaClientProvider client={client}>
        <AgentDetails
          open
          detail={{ ...AGENT_DETAIL, status: blocked && busy === 'working' ? 'working' : 'ready' }}
          historyLoading={blocked && busy === 'history'}
          pending={blocked && busy === 'command' ? 'send' : undefined}
        />
      </CmaClientProvider>
    );
    const view = render(draw(true));
    await screen.findByLabelText('Model');
    await userEvent.selectOptions(screen.getByLabelText('Reasoning effort'), 'medium');
    expect(screen.getByRole('button', { name: 'Apply settings' })).toBeDisabled();
    expect(update).not.toHaveBeenCalled();
    view.rerender(draw(false));
    expect(screen.getByLabelText('Reasoning effort')).toHaveValue('medium');
    expect(screen.getByRole('button', { name: 'Apply settings' })).toBeEnabled();
  },
);

it('focuses the Details heading on open and restores the opener on close', async () => {
  const onClose = vi.fn();
  const client = agentClient({ readAgentSettings: async () => defaults });
  const draw = (open: boolean) => (
    <CmaClientProvider client={client}>
      <button>Open details</button>
      <AgentDetails detail={AGENT_DETAIL} open={open} onClose={onClose} />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  const opener = screen.getByRole('button', { name: 'Open details' });
  opener.focus();
  view.rerender(draw(true));
  expect(screen.getByRole('heading', { name: 'Details' })).toHaveFocus();
  await userEvent.click(screen.getByRole('button', { name: 'Close Agent details' }));
  expect(onClose).toHaveBeenCalledOnce();
  view.rerender(draw(false));
  expect(opener).toHaveFocus();
});
