import { fireEvent, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { AGENT_DETAIL, agentClient } from '../test/agent';
import { CmaClientProvider } from '../api/context';
import { useAgentController } from './controller';
import { AgentSurface } from './AgentSurface';

vi.mock('@novnc/novnc', () => ({
  default: class extends EventTarget {
    disconnect() {}
  },
}));

it.each([
  new DOMException('signal is aborted without reason', 'AbortError'),
  Object.assign(new Error('signal is aborted without reason'), { name: 'AbortError' }),
])(
  'shows a plain cancellation message and recovers through Try again without a command (%s)',
  async (error) => {
    const readAgent = vi.fn().mockRejectedValueOnce(error).mockResolvedValue(AGENT_DETAIL);
    const stopAgent = vi.fn(),
      deleteAgent = vi.fn();
    const client = agentClient({ readAgent, stopAgent, deleteAgent });
    function Agent() {
      const controller = useAgentController(client, AGENT_DETAIL.id);
      return (
        <AgentSurface
          agentId={AGENT_DETAIL.id}
          controller={controller}
          detailsOpen={false}
          onDetailsOpenChange={vi.fn()}
          onNavigate={vi.fn()}
        />
      );
    }
    render(
      <CmaClientProvider client={client}>
        <Agent />
      </CmaClientProvider>,
    );
    expect(await screen.findByRole('heading', { name: 'Agent unavailable' })).toBeInTheDocument();
    expect(screen.queryByText(/signal is aborted|AbortError/)).not.toBeInTheDocument();
    expect(screen.getByText(/request was interrupted/i)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Try again' }));
    expect(await screen.findByText('The repository is ready.')).toBeInTheDocument();
    expect(stopAgent).not.toHaveBeenCalled();
    expect(deleteAgent).not.toHaveBeenCalled();
  },
);
