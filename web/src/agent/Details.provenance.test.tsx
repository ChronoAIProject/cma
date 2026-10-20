import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { AgentDetails } from './Details';

it('keeps Details independent of optional creation provenance', async () => {
  const readAgentProvenance = vi.fn().mockRejectedValue(new Error('Denied'));
  const readAgentSettings = vi.fn().mockResolvedValue({
    model: 'gpt-6',
    reasoning_effort: 'high',
    permission_mode: 'autonomous',
  });
  const client = agentClient({
    readAgentProvenance,
    readAgentSettings,
    updateAgentSettings: vi.fn(),
  });
  const draw = (open: boolean) => (
    <CmaClientProvider client={client}>
      <AgentDetails open={open} detail={AGENT_DETAIL} />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  expect(readAgentSettings).not.toHaveBeenCalled();
  view.rerender(draw(true));
  expect(screen.queryByLabelText('Model')).not.toBeInTheDocument();
  expect(readAgentProvenance).not.toHaveBeenCalled();
  view.rerender(draw(false));
  expect(readAgentSettings).not.toHaveBeenCalled();
});
