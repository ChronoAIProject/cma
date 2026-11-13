import { act, render, screen, waitFor, within } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { agentClient, AGENT_DETAIL, AGENT_USAGE } from '../test/agent';
import { AgentDetails } from './Details';
import { SessionSettings } from './SessionSettings';
it('puts Sandbox identity, resources and key scope in Details and reads resources only while open', async () => {
  const readAgentResources = vi.fn(() => new Promise<never>(() => {}));
  const client = agentClient({ readAgentResources });
  const draw = (open: boolean) => (
    <CmaClientProvider client={client}>
      <AgentDetails
        detail={AGENT_DETAIL}
        open={open}
        availability={{ state: 'live', runtimeVersion: 'runtime' }}
      />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  await act(async () => {});
  expect(readAgentResources).not.toHaveBeenCalled();
  view.rerender(draw(true));
  const machine = screen.getByRole('region', { name: 'Sandbox machine' });
  expect(within(machine).getByText(AGENT_DETAIL.sandbox.id)).toBeVisible();
  expect(within(machine).getByText('Agent key scope')).toBeVisible();
  expect(within(machine).getByText('github')).toBeVisible();
  await waitFor(() => expect(readAgentResources).toHaveBeenCalled());
});
it('keeps usage in the settings tooltip and accessible description without a composer ring', async () => {
  const client = agentClient({
    readAgentSettings: async () => ({
      model: 'gpt-6',
      reasoning_effort: 'high',
      permission_mode: 'autonomous',
    }),
  });
  render(
    <CmaClientProvider client={client}>
      <SessionSettings usage={AGENT_USAGE} agentId="agt_1" active runtimeReady busy={false} />
    </CmaClientProvider>,
  );
  const chip = await screen.findByRole('button', { name: /^Agent settings:/ });
  expect(chip).toHaveAttribute('title', expect.stringContaining('Context:'));
  expect(chip).toHaveAccessibleDescription(expect.stringContaining('120 input'));
  expect(screen.queryByRole('button', { name: 'Context usage' })).toBeNull();
});
