import { StrictMode } from 'react';
import { act, render, screen, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelToolsets } from './ChannelToolsets';
import { RelayCapabilitiesProvider, type AgentToolCapability } from './RelayCapabilities';
import type { Channel } from './ChannelEditor';

const channel: Channel = {
  channel_id: 'tools-channel',
  record_id: 'tools-record',
  version: 1,
  provider: { slug: 'cma_codex', level: 'l2' },
  definition: {
    name: 'Team events',
    sources: [],
    provider: { target: { type: 'existing_agent', agent_id: 'retained-agent' } },
    routing: {
      consumers: [{ id: 'pending', name: 'Reviewer', pending: true }],
    },
  },
  endpoints: { custom_event_url: '', nyxid_callback_url: '' },
};
const context: AgentToolCapability = {
  name: 'readEventContext',
  description: 'Read the recorded event and supported delivery features.',
  read_only: true,
  destructive: false,
  idempotent: true,
  open_world: false,
};
const response = (value: unknown, status = 200) => new Response(JSON.stringify(value), { status });
function Catalog({
  owner = 'alice',
  visible = true,
  current = channel,
}: {
  owner?: string;
  visible?: boolean;
  current?: Channel;
}) {
  return (
    <StrictMode>
      <RelayCapabilitiesProvider key={owner} denied={vi.fn()}>
        {visible ? <ChannelToolsets channel={current} /> : null}
      </RelayCapabilitiesProvider>
    </StrictMode>
  );
}
const catalog = () => screen.getByRole('group', { name: 'Event tools' });
afterEach(() => vi.unstubAllGlobals());

it('shows only the advertised tools and server descriptions, reusing discovery across navigation', async () => {
  const futureTool = {
    ...context,
    name: 'readLarkResource',
    description: 'Read a connected resource only when the caller is authorized.',
  };
  const destructiveTool = {
    ...context,
    name: 'deleteMessage',
    description: 'Delete a confirmed message subject to platform limits.',
    read_only: false,
    destructive: true,
    open_world: true,
  };
  const fetcher = vi.fn().mockResolvedValue(
    response({
      agent_tools: [context, futureTool, destructiveTool],
      relay_authoring: { platforms: [], native_media_writes_enabled: true },
    }),
  );
  vi.stubGlobal('fetch', fetcher);
  const view = render(<Catalog />);
  await screen.findByText('readEventContext');
  expect(within(catalog()).getAllByRole('listitem')).toHaveLength(3);
  expect(within(catalog()).getByText(context.description)).toBeVisible();
  expect(within(catalog()).getByText('readLarkResource')).toBeVisible();
  expect(within(catalog()).getByText(futureTool.description)).toBeVisible();
  expect(
    within(catalog()).getByText('Can make changes · May remove or replace data'),
  ).toBeVisible();
  expect(within(catalog()).getAllByText('Read only')).toHaveLength(2);
  for (const missing of ['readEventAttachment', 'replyToEvent', 'setMessageReaction'])
    expect(within(catalog()).queryByText(missing)).not.toBeInTheDocument();
  expect(screen.getByText(/does not confirm installation in an agent/)).toBeVisible();
  expect(screen.getByText(/Existing agent. Its current tool setup is preserved/)).toBeVisible();
  expect(screen.getByText('Connection pending. Tool availability is not confirmed.')).toBeVisible();

  view.rerender(<Catalog visible={false} />);
  view.rerender(<Catalog />);
  expect(screen.getByText('readEventContext')).toBeVisible();
  expect(fetcher).toHaveBeenCalledOnce();
  expect(fetcher.mock.calls[0]![0]).toBe('/api/event-gateway/v1/channel-capabilities');
  expect(fetcher.mock.calls[0]![1]).toMatchObject({
    credentials: 'same-origin',
    cache: 'no-store',
  });
});

it.each([
  ['no sources', [], 'Requires a direct Discord source', 'Requires a direct WhatsApp source'],
  [
    'relay sources',
    [
      { type: 'nyxid_relay', platform: 'discord' },
      { type: 'nyxid_relay', platform: 'whatsapp' },
    ],
    'Requires a direct Discord source',
    'Requires a direct WhatsApp source',
  ],
  [
    'missing consent',
    [{ type: 'discord' }, { type: 'whatsapp', forms_enabled: false }],
    'Enable Discord thread management',
    'Enable WhatsApp Forms',
  ],
])(
  'keeps unrelated tools discoverable with setup hints for %s',
  async (_, sources, discordHint, whatsappHint) => {
    const names = [
      'readEventContext',
      'listEventThreads',
      'createEventThread',
      'manageThread',
      'readEventHistory',
      'prepareFormReply',
      'readFormPreparation',
      'futureTool',
    ];
    vi.stubGlobal(
      'fetch',
      vi
        .fn()
        .mockResolvedValue(response({ agent_tools: names.map((name) => ({ ...context, name })) })),
    );
    render(<Catalog current={{ ...channel, definition: { ...channel.definition, sources } }} />);
    await screen.findByText('readEventContext');
    const other = screen.getByText('Other advertised tools (5)').closest('details')!;
    expect(other).not.toHaveAttribute('open');
    for (const name of ['readEventContext', 'readFormPreparation', 'futureTool']) {
      expect(screen.getByText(name)).toBeVisible();
      expect(other).not.toContainElement(screen.getByText(name));
    }
    await userEvent.click(screen.getByText('Other advertised tools (5)'));
    expect(within(other).getAllByRole('listitem')).toHaveLength(5);
    expect(within(other).getAllByText(discordHint)).not.toHaveLength(0);
    expect(within(other).getByText(whatsappHint)).toBeVisible();
    expect(within(other).getByText('readEventHistory')).toBeVisible();
    expect(
      screen.getByText('Connection pending. Tool availability is not confirmed.'),
    ).toBeVisible();
  },
);

it('uses saved source consents without treating them as confirmed execution or installation', async () => {
  const names = [
    'readEventContext',
    'listEventThreads',
    'createEventThread',
    'manageThread',
    'readEventHistory',
    'prepareFormReply',
  ];
  const fetcher = vi
    .fn()
    .mockResolvedValue(response({ agent_tools: names.map((name) => ({ ...context, name })) }));
  vi.stubGlobal('fetch', fetcher);
  const current = {
    ...channel,
    definition: {
      ...channel.definition,
      sources: [
        { type: 'discord', thread_lifecycle: true, conversation_history: true },
        { type: 'whatsapp', forms_enabled: true },
      ],
    },
  };
  const view = render(<Catalog current={current} />);
  await screen.findByText('prepareFormReply');
  expect(within(catalog()).getAllByRole('listitem')).toHaveLength(names.length);
  expect(screen.queryByText(/Other advertised tools/)).not.toBeInTheDocument();
  expect(screen.getByText(/Support depends on the event, platform and permissions/)).toBeVisible();
  expect(screen.getByText(/does not confirm installation in an agent/)).toBeVisible();
  view.rerender(
    <Catalog
      current={{
        ...current,
        definition: {
          ...current.definition,
          sources: [
            { type: 'discord', thread_lifecycle: true, conversation_history: false },
            { type: 'whatsapp', forms_enabled: false },
          ],
        },
      }}
    />,
  );
  const other = screen.getByText('Other advertised tools (2)').closest('details')!;
  expect(within(other).getByText('readEventHistory')).toBeInTheDocument();
  expect(within(other).getByText('prepareFormReply')).toBeInTheDocument();
  expect(within(other).queryByText('listEventThreads')).not.toBeInTheDocument();
  expect(screen.getByText('Enable Discord conversation history')).toBeInTheDocument();
  view.rerender(<Catalog current={channel} />);
  expect(screen.getByText('Other advertised tools (5)')).toBeVisible();
  expect(fetcher).toHaveBeenCalledOnce();
});

it.each([
  ['missing', { relay_authoring: { platforms: [], native_media_writes_enabled: true } }, 200],
  ['null', { agent_tools: null }, 200],
  ['older gateway', {}, 404],
])(
  'keeps %s tool availability unknown instead of inferring supported tools',
  async (_, value, status) => {
    const fetcher = vi.fn().mockResolvedValue(response(value, status as number));
    vi.stubGlobal('fetch', fetcher);
    render(<Catalog />);
    expect(
      await screen.findByText('Tool availability is not reported by this gateway.'),
    ).toBeVisible();
    expect(within(catalog()).queryByRole('list')).not.toBeInTheDocument();
    expect(within(catalog()).queryByText(/advertises no agent tools/)).not.toBeInTheDocument();
    expect(fetcher).toHaveBeenCalledOnce();
  },
);

it('distinguishes an explicitly empty catalog from an older gateway', async () => {
  vi.stubGlobal('fetch', vi.fn().mockResolvedValue(response({ agent_tools: [] })));
  render(<Catalog />);
  expect(await screen.findByText('This gateway advertises no agent tools.')).toBeVisible();
  expect(
    screen.queryByText('Tool availability is not reported by this gateway.'),
  ).not.toBeInTheDocument();
  expect(within(catalog()).queryByRole('list')).not.toBeInTheDocument();
});

it('shows loading until the shared response and accepts a catalog without authoring metadata', async () => {
  let complete!: (value: Response) => void;
  const fetcher = vi.fn().mockImplementation(
    () =>
      new Promise<Response>((resolve) => {
        complete = resolve;
      }),
  );
  vi.stubGlobal('fetch', fetcher);
  render(<Catalog />);
  expect(within(catalog()).getByRole('status')).toHaveTextContent(
    'Checking available agent tools…',
  );
  expect(within(catalog()).queryByRole('list')).not.toBeInTheDocument();
  await act(async () => complete(response({ agent_tools: [context] })));
  expect(within(catalog()).getByText('readEventContext')).toBeVisible();
  expect(within(catalog()).queryByRole('status')).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledOnce();
});

it('keeps failures explicit and retries only on user action', async () => {
  const fetcher = vi
    .fn()
    .mockRejectedValueOnce(new Error('offline'))
    .mockResolvedValueOnce(response({ agent_tools: [context] }));
  vi.stubGlobal('fetch', fetcher);
  const view = render(<Catalog />);
  await screen.findByText('Agent tool availability could not be loaded.');
  expect(within(catalog()).queryByRole('list')).not.toBeInTheDocument();
  view.rerender(<Catalog />);
  expect(fetcher).toHaveBeenCalledOnce();
  await userEvent.click(screen.getByRole('button', { name: 'Retry capability check' }));
  expect(await screen.findByText('readEventContext')).toBeVisible();
  expect(screen.queryByRole('button', { name: 'Retry capability check' })).not.toBeInTheDocument();
  expect(fetcher).toHaveBeenCalledTimes(2);
});

it('discards a previous owner catalog response after identity changes', async () => {
  let complete!: (value: Response) => void;
  const fetcher = vi
    .fn()
    .mockImplementationOnce(
      () =>
        new Promise<Response>((resolve) => {
          complete = resolve;
        }),
    )
    .mockResolvedValueOnce(response({ agent_tools: [] }));
  vi.stubGlobal('fetch', fetcher);
  const view = render(<Catalog />);
  view.rerender(<Catalog owner="bob" />);
  await screen.findByText('This gateway advertises no agent tools.');
  await act(async () => complete(response({ agent_tools: [context] })));
  expect(screen.queryByText('readEventContext')).not.toBeInTheDocument();
  expect(screen.getByText('This gateway advertises no agent tools.')).toBeVisible();
  expect(fetcher).toHaveBeenCalledTimes(2);
});
