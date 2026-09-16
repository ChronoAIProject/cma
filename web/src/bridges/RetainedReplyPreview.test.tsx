import { render, screen } from '@testing-library/react';
import { expect, it } from 'vitest';
import { RetainedReplyPreview } from './RetainedReplyPreview';
import { placementTransport } from './placement.test-support';

it.each(['manual', 'trigger', 'legacy_unknown'])(
  'keeps %s Agent output in CMA without offering channel reply tools',
  async (creation_origin) => {
    const test = placementTransport(undefined, () =>
      Response.json({ agent_id: 'agent-1', created_at: '2026-09-16T00:00:00Z', creation_origin }),
    );
    render(<RetainedReplyPreview client={test.client} agentId="agent-1" disabled={false} />);
    await screen.findByText(/Its output remains in CMA/);
    expect(screen.getByText(/does not give it channel reply tools/)).toBeInTheDocument();
    expect(screen.getByText(/never sent to the channel automatically/)).toBeInTheDocument();
    if (creation_origin === 'legacy_unknown')
      expect(screen.getByText(/creation origin is unknown/)).toBeInTheDocument();
  },
);

it('does not claim current reply availability from channel creation alone', async () => {
  const test = placementTransport(undefined, () =>
    Response.json({
      agent_id: 'agent-1',
      created_at: '2026-09-16T00:00:00Z',
      creation_origin: 'channel_bridge',
    }),
  );
  render(<RetainedReplyPreview client={test.client} agentId="agent-1" disabled={false} />);
  await screen.findByText(
    /Sending replies requires channel reply tools and current channel permissions/,
  );
  expect(screen.getByText(/never sent to the channel automatically/)).toBeInTheDocument();
});

it('does not use another Agent’s provenance as evidence', async () => {
  const test = placementTransport(undefined, () =>
    Response.json({
      agent_id: 'other-agent',
      created_at: '2026-09-16T00:00:00Z',
      creation_origin: 'channel_bridge',
    }),
  );
  render(<RetainedReplyPreview client={test.client} agentId="agent-1" disabled={false} />);
  await screen.findByText(/Creation origin could not be verified/);
  expect(screen.queryByText(/This Agent was originally created/)).not.toBeInTheDocument();
});
