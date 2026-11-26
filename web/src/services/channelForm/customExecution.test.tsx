import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { ChannelEditor, type Channel } from '../ChannelEditor';
import { RelayCapabilitiesContext, type RelayDiscovery } from '../RelayCapabilities';
import { customAttachmentDescription, customExecutionRequest } from './customExecution';
import { channelDraft, channelRequest, defaultSource } from './model';

const ready: RelayDiscovery = {
  status: 'ready',
  authoring: {
    custom_durable_execution_enabled: true,
    response_recovery_providers: ['cma_codex'],
    dynamic_sources_writes_enabled: false,
    relay_raw_verification_writes_enabled: false,
    multi_consumer_writes_enabled: true,
    native_bot_mentions_writes_enabled: false,
    thread_contract_writes_enabled: false,
    platforms: [],
  },
};
const draft = { ...channelDraft(undefined, 'owner'), sources: [defaultSource('custom')] };
afterEach(() => vi.unstubAllGlobals());

it('opts eligible new Custom channels in without changing unsupported or non-Custom creates', () => {
  expect(customExecutionRequest(ready, draft)).toEqual({ durable_custom_execution: true });
  expect(
    customExecutionRequest(ready, {
      ...draft,
      sources: [...draft.sources, defaultSource('telegram')],
    }),
  ).toEqual({ durable_custom_execution: true });
  for (const discovery of [
    { status: 'legacy' as const },
    { ...ready, authoring: { ...ready.authoring!, custom_durable_execution_enabled: false } },
    { ...ready, authoring: { ...ready.authoring!, response_recovery_providers: undefined } },
  ])
    expect(customExecutionRequest(discovery, draft)).toEqual({});
  expect(customExecutionRequest(ready, { ...draft, provider: { slug: 'nyxbot' } })).toEqual({});
  expect(customExecutionRequest(ready, { ...draft, sources: [defaultSource('telegram')] })).toEqual(
    {},
  );
  expect(customExecutionRequest(ready, { ...draft, connection_ref: 'managed-telegram' })).toEqual(
    {},
  );
  for (const status of ['loading', 'failed'] as const)
    expect(() => customExecutionRequest({ status }, draft)).toThrow('Check channel support');
});

it('waits for discovery, submits the automatic choice, and retains exact uncertain-request retries', async () => {
  const fetcher = vi
    .fn()
    .mockResolvedValueOnce(
      new Response(JSON.stringify({ detail: 'Unknown result' }), { status: 503 }),
    )
    .mockResolvedValueOnce(new Response(JSON.stringify({ record_id: 'created-record' })));
  vi.stubGlobal('fetch', fetcher);
  const done = vi.fn();
  const view = (discovery: RelayDiscovery) => (
    <RelayCapabilitiesContext value={discovery}>
      <ChannelEditor ownerSubject="owner" onDone={done} denied={vi.fn()} />
    </RelayCapabilitiesContext>
  );
  const rendered = render(view({ status: 'loading' }));
  fireEvent.change(screen.getByLabelText('Channel name'), { target: { value: 'Custom events' } });
  fireEvent.change(screen.getByLabelText('Provider agent key'), {
    target: { value: 'test-only-key' },
  });
  fireEvent.click(screen.getByRole('button', { name: 'Producer 1 connection type' }));
  fireEvent.click(screen.getByRole('menuitemradio', { name: 'Custom event' }));
  fireEvent.change(screen.getByLabelText('HMAC key ID'), { target: { value: 'test-key-id' } });
  fireEvent.click(screen.getByRole('button', { name: 'Add HMAC credential' }));
  fireEvent.change(screen.getByLabelText('HMAC 1 key ID'), { target: { value: 'test-key-id' } });
  fireEvent.change(screen.getByLabelText('HMAC 1 secret'), {
    target: { value: btoa('synthetic signing material') },
  });
  expect(screen.getByRole('button', { name: 'Checking support…' })).toBeDisabled();
  expect(screen.queryByRole('checkbox', { name: /reliable delivery/i })).not.toBeInTheDocument();
  expect(fetcher).not.toHaveBeenCalled();
  const retry = vi.fn();
  rendered.rerender(view({ status: 'failed', retry }));
  expect(screen.getByRole('button', { name: 'Check support to continue' })).toBeDisabled();
  fireEvent.click(screen.getByRole('button', { name: 'Retry support check' }));
  expect(retry).toHaveBeenCalledOnce();
  expect(fetcher).not.toHaveBeenCalled();
  rendered.rerender(view(ready));
  fireEvent.click(screen.getByRole('button', { name: 'Create channel' }));
  await screen.findByText('Unknown result');
  expect(JSON.parse(fetcher.mock.calls[0]![1].body)).toMatchObject({
    durable_custom_execution: true,
  });
  rendered.rerender(view({ status: 'legacy' }));
  fireEvent.click(screen.getByRole('button', { name: 'Retry same request' }));
  await waitFor(() => expect(done).toHaveBeenCalledOnce());
  expect(fetcher.mock.calls[1]![1].body).toBe(fetcher.mock.calls[0]![1].body);
  expect(fetcher.mock.calls[1]![1].headers).toEqual(fetcher.mock.calls[0]![1].headers);
});

it('never reauthors an existing channel execution marker in a normal update', () => {
  for (const marker of [undefined, false, true]) {
    const channel = {
      channel_id: 'channel',
      record_id: 'record',
      version: 7,
      provider: { slug: 'cma_codex', level: 'l3' },
      definition: { ...draft, name: 'Saved', durable_custom_execution: marker },
      endpoints: { custom_event_url: '/events', nyxid_callback_url: '/callback' },
    } as Channel;
    const editable = channelDraft(channel);
    expect(editable).not.toHaveProperty('durable_custom_execution');
    expect(
      channelRequest({ ...editable, durable_custom_execution: true }, {}, channel),
    ).not.toHaveProperty('durable_custom_execution');
  }
});

it('describes existing-agent attachment limitations without downgrading manually selected legacy records', () => {
  const borrowed = {
    ...draft,
    provider: { slug: 'cma_codex', target: { type: 'existing_agent', agent_id: 'agt_existing' } },
  };
  expect(customAttachmentDescription(borrowed, false)).toBe(
    'Text and audio excerpts are supported. Image and file delivery depends on this channel’s agent connection.',
  );
  expect(customAttachmentDescription(borrowed, true)).toBe(
    'Supported images, files and transcripts are delivered to the agent.',
  );
  expect(customAttachmentDescription(draft, true)).toContain(
    'require an agent with channel attachment tools',
  );
  expect(
    customAttachmentDescription(
      { ...borrowed, provider: { ...borrowed.provider, slug: 'nyxbot' } },
      true,
    ),
  ).toContain('depends on this channel');
});
