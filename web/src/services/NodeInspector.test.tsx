import { fireEvent, render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { useState } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { NodeInspector } from './NodeInspector';
import { Section, TextField } from './channelForm/Fields';
import type { Channel } from './ChannelEditor';
import type { ChannelSelection } from './channelTopology';

const channel: Channel = {
  channel_id: 'navigation-fixture',
  record_id: 'navigation-record',
  version: 1,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: {
    name: 'Team updates',
    sources: [{ type: 'custom', auth: { type: 'hmac' } }],
    provider: { slug: 'cma_codex', target: { type: 'existing_agent', agent_id: 'existing-agent' } },
  },
  endpoints: {
    custom_event_url: 'https://example.test/events',
    nyxid_callback_url: 'https://example.test/callback',
  },
};
afterEach(() => vi.restoreAllMocks());
function Editor() {
  const [value, setValue] = useState('');
  const [extra, setExtra] = useState(false);
  return (
    <>
      <Section title="Connection" description="Choose the incoming connection.">
        <TextField label="Draft name" value={value} onChange={setValue} />
        <Section title="Nested field">
          <p>Not a second navigation destination</p>
        </Section>
      </Section>
      <button type="button" onClick={() => setExtra((current) => !current)}>
        Toggle reply settings
      </button>
      {extra ? (
        <Section title="Reply settings">
          <p>Conditional reply settings</p>
        </Section>
      ) : null}
    </>
  );
}
it('navigates actual conditional settings, excludes nested fields and retains drafts through toolsets', async () => {
  const scroll = vi.fn();
  Element.prototype.scrollIntoView = scroll;
  const tree = (selection: ChannelSelection) => (
    <NodeInspector channel={channel} id="settings" selection={selection}>
      <Editor />
    </NodeInspector>
  );
  const { rerender } = render(tree({ kind: 'channel' }));
  const nav = screen.getByRole('navigation', { name: 'Configuration sections' });
  await screen.findByRole('button', { name: 'Connection' });
  expect(within(nav).queryByRole('button', { name: 'Nested field' })).not.toBeInTheDocument();
  await userEvent.type(screen.getByLabelText('Draft name'), 'Unsaved edit');
  await userEvent.click(screen.getByRole('button', { name: 'Toggle reply settings' }));
  const destination = await within(nav).findByRole('button', { name: 'Reply settings' });
  await userEvent.click(destination);
  expect(scroll).toHaveBeenCalled();
  expect(document.activeElement).toHaveTextContent('Conditional reply settings');
  rerender(tree({ kind: 'toolsets' }));
  const toolsLink = await screen.findByRole('button', { name: 'Event tools' });
  const catalog = screen.getByRole('region', { name: 'Tool catalog' });
  expect(screen.queryByRole('heading', { name: 'Tool catalog' })).not.toBeInTheDocument();
  await userEvent.click(toolsLink);
  expect(within(catalog).getByRole('group', { name: 'Event tools' })).toHaveFocus();
  expect(
    screen.getAllByText('Available tools depend on the event, platform and permissions.'),
  ).toHaveLength(1);
  expect(within(nav).queryByRole('button', { name: 'Connection' })).not.toBeInTheDocument();
  expect(screen.getByText('Tool availability is not reported by this gateway.')).toBeVisible();
  expect(screen.queryByText('readEventContext')).not.toBeInTheDocument();
  expect(screen.getByText(/Existing agent. Uses its current tool setup./)).toBeVisible();
  rerender(tree({ kind: 'channel' }));
  expect(await screen.findByLabelText('Draft name')).toHaveValue('Unsaved edit');
  expect(within(nav).queryByRole('button', { name: 'Event tools' })).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole('button', { name: 'Toggle reply settings' }));
  await waitFor(() =>
    expect(within(nav).queryByRole('button', { name: 'Reply settings' })).not.toBeInTheDocument(),
  );
});

it('hides unavailable section links while retaining pending controls', () => {
  const tree = (available: boolean) => (
    <NodeInspector
      channel={channel}
      id="availability"
      selection={{ kind: 'channel' }}
      sendAvailable={available}
      testsAvailable={available}
      send={<input aria-label="Pending send" defaultValue="Keep original intent" />}
      tests={<p>Run check</p>}
    >
      <Editor />
    </NodeInspector>
  );
  const { rerender } = render(tree(false));
  const nav = screen.getByRole('navigation', { name: 'Configuration sections' });
  expect(within(nav).queryByRole('button', { name: 'Send message' })).not.toBeInTheDocument();
  expect(within(nav).queryByRole('button', { name: 'Tests' })).not.toBeInTheDocument();
  rerender(tree(true));
  expect(screen.getByLabelText('Pending send')).toHaveValue('Keep original intent');
  expect(within(nav).getByRole('button', { name: 'Send message' })).toBeVisible();
});

it.each([
  ['mention_or_reply_to_bot', 'mention or reply to bot'],
  [undefined, 'Not configured'],
])('shows the saved group policy %s in the producer overview', (groups, expected) => {
  const saved = {
    ...channel,
    definition: {
      ...channel.definition,
      sources: [
        {
          type: 'nyxid_relay',
          platform: 'telegram',
          admission: { type: 'scoped', senders: { type: 'open' }, chats: { type: 'open' }, groups },
        },
      ],
    },
  };
  render(
    <NodeInspector channel={saved} id="producer" selection={{ kind: 'producer', index: 0 }}>
      <Editor />
    </NodeInspector>,
  );
  expect(screen.getByText('Group rule').nextElementSibling).toHaveTextContent(expected!);
});
