import { render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { SessionSettings } from './SessionSettings';

it('loads composer settings independently of optional creation provenance', async () => {
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
      <SessionSettings agentId={AGENT_DETAIL.id} active={open} busy={false} />
    </CmaClientProvider>
  );
  const view = render(draw(false));
  expect(readAgentSettings).not.toHaveBeenCalled();
  view.rerender(draw(true));
  expect(
    await screen.findByRole('button', { name: /^Agent settings:/ }),
  ).toHaveTextContent('gpt-6 · High · Autonomous');
  expect(readAgentProvenance).not.toHaveBeenCalled();
  view.rerender(draw(false));
  expect(readAgentSettings.mock.calls[0]?.[1].signal.aborted).toBe(true);
});
