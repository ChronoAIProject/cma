import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import type { CmaWebClient, AgentPreferencesView } from '../api/client';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { OwnerAgentHeader } from './OwnerAgentHeader';

const preferences: AgentPreferencesView = {
  system_prompt: 'Synthetic instructions',
  prompt_pending: false,
  prompt_supported: true,
  revision: 0,
};
function header(
  client: CmaWebClient,
  viewerId = 'owner',
  readOnly = false,
  agentId = AGENT_DETAIL.id,
) {
  return (
    <CmaClientProvider client={client}>
      <OwnerAgentHeader
        detail={{ ...AGENT_DETAIL, id: agentId }}
        viewerId={viewerId}
        readOnly={readOnly}
        busy={false}
        title="Task"
        status="ready"
        breadcrumb={[]}
        detailsOpen={false}
        onDetailsOpenChange={vi.fn()}
      />
    </CmaClientProvider>
  );
}
const open = () => fireEvent.click(screen.getByRole('button', { name: 'More Agent actions' }));
const prompt = () => screen.getByRole('menuitem', { name: /^Update system prompt/ });

it.each([false, 'failed'] as const)(
  'blocks the prompt action before an unsupported editor opens: %s',
  async (supported) => {
    const readAgentPreferences = vi.fn().mockImplementation(async () => {
      if (supported === 'failed') throw new Error('Private server error');
      return { ...preferences, prompt_supported: supported };
    });
    const updateAgentPreferences = vi.fn();
    render(header(agentClient({ readAgentPreferences, updateAgentPreferences })));
    expect(readAgentPreferences).not.toHaveBeenCalled();
    open();
    expect(prompt()).toHaveAttribute('aria-disabled', 'true');
    await waitFor(() =>
      expect(prompt()).toHaveAccessibleName(
        expect.stringContaining(
          supported === false
            ? 'This runtime does not support system prompt updates'
            : 'Could not check',
        ),
      ),
    );
    fireEvent.click(prompt());
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(screen.queryByText('Private server error')).not.toBeInTheDocument();
    expect(updateAgentPreferences).not.toHaveBeenCalled();
    expect(readAgentPreferences).toHaveBeenCalledTimes(1);
  },
);

it('opens the editor only after a supported response and checks again when reopening the menu', async () => {
  let finish!: (view: AgentPreferencesView) => void;
  const readAgentPreferences = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<AgentPreferencesView>((resolve) => {
          finish = resolve;
        }),
    )
    .mockResolvedValue(preferences);
  const updateAgentPreferences = vi.fn();
  render(header(agentClient({ readAgentPreferences, updateAgentPreferences })));
  open();
  expect(prompt()).toHaveAttribute('aria-disabled', 'true');
  fireEvent.click(prompt());
  expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  await act(async () => finish(preferences));
  expect(prompt()).toHaveAttribute('aria-disabled', 'false');
  fireEvent.click(prompt());
  expect(await screen.findByRole('dialog', { name: 'Update system prompt' })).toBeVisible();
  expect(await screen.findByRole('textbox', { name: 'Agent system prompt' })).toBeEnabled();
  fireEvent.click(screen.getByRole('button', { name: 'Cancel' }));
  readAgentPreferences.mockResolvedValue({ ...preferences, prompt_supported: false });
  open();
  await waitFor(() =>
    expect(prompt()).toHaveAccessibleName(expect.stringContaining('does not support')),
  );
  expect(prompt()).toHaveAttribute('aria-disabled', 'true');
  expect(updateAgentPreferences).not.toHaveBeenCalled();
});

it.each(['owner', 'agent', 'client', 'access'] as const)(
  'rejects a delayed capability after %s changes',
  async (change) => {
    let finish!: (view: AgentPreferencesView) => void;
    let signal: AbortSignal | undefined;
    const readAgentPreferences = vi.fn((_id, options) => {
      signal = options?.signal;
      return new Promise<AgentPreferencesView>((resolve) => {
        finish = resolve;
      });
    });
    const client = agentClient({ readAgentPreferences, updateAgentPreferences: vi.fn() });
    const view = render(header(client));
    open();
    const oldSignal = signal;
    const oldFinish = finish;
    view.rerender(
      header(
        change === 'client' ? agentClient() : client,
        change === 'owner' ? 'other' : 'owner',
        change === 'access',
        change === 'agent' ? 'other-agent' : AGENT_DETAIL.id,
      ),
    );
    expect(oldSignal?.aborted).toBe(true);
    await act(async () => oldFinish(preferences));
    expect(prompt()).toHaveAttribute('aria-disabled', 'true');
    fireEvent.click(prompt());
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
  },
);
