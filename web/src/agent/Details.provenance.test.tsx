import { act, render, screen } from '@testing-library/react';
import { expect, it, vi } from 'vitest';
import { CmaClientProvider } from '../api/context';
import { unavailableClient } from '../api/client';
import { AGENT_DETAIL } from '../test/agent';
import { AgentDetails } from './Details';

it('shows immutable creation independently of Workspace Profile provisioning and clears on close', async () => {
  const readAgentProvenance = vi
    .fn()
    .mockResolvedValueOnce({
      agent_id: AGENT_DETAIL.id,
      created_at: AGENT_DETAIL.createdAt,
      creation_origin: 'legacy_unknown',
      details: {
        created_by: null,
        trigger: null,
        channel_bridge: null,
        workspace_profile_index: 0,
      },
    })
    .mockRejectedValue(new Error('Denied'));
  const client = { ...unavailableClient, readAgentProvenance };
  const draw = (open: boolean) => (
    <CmaClientProvider client={client}>
      <AgentDetails
        open={open}
        detail={AGENT_DETAIL}
        onNavigate={vi.fn()}
        onStop={vi.fn()}
        onDelete={vi.fn()}
      />
    </CmaClientProvider>
  );
  const { rerender } = render(draw(false));
  expect(readAgentProvenance).not.toHaveBeenCalled();
  rerender(draw(true));
  expect(await screen.findByText('Creation origin unknown')).toBeVisible();
  expect(screen.getByText('Provisioning')).toBeVisible();
  expect(screen.getByText('Workspace Profile')).toBeVisible();
  rerender(draw(false));
  expect(screen.queryByText('Workspace Profile')).not.toBeInTheDocument();
  rerender(draw(true));
  expect(await screen.findByText('Creation origin unavailable')).toBeVisible();
  expect(screen.queryByText('Workspace Profile')).not.toBeInTheDocument();
  await act(async () => {});
});
