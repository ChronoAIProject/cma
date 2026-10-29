import { readFileSync } from 'node:fs';
import { describe, expect, expectTypeOf, it, vi } from 'vitest';
import {
  Cmaeg,
  type GatewayMediaStreamEvent,
  type GatewayStreamEvent,
} from '../src/cmaeg/index.js';
import { ProtocolError, TransportError, ValidationError } from '../src/errors.js';

const event = {
  specversion: '1.0' as const,
  id: 'event-1',
  source: 'urn:fixture',
  type: 'example.event',
};
const identity = { event_id: 'event-1', conversation_id: 'conversation' };
const original = {
  ...identity,
  text: 'Original answer',
  message: null,
  error: null,
  response_id: 'resp_parent',
  media_pending: true,
  media_responses_url: '/v1/channels/channel/events/event-1/media-responses',
};
const media = {
  ...identity,
  response_id: 'resp_media',
  response_url: '/api/v2/responses/resp_media',
  parent_response_id: 'resp_parent',
  delivery_id: 'delivery',
  attachment_index: 0,
  kind: 'original',
  status: 'completed',
  text: 'Attachment answer',
};
const terminal = {
  ...identity,
  text: 'Original answer',
  message: null,
  error: null,
  status: 'completed',
  media_complete: true,
};
const frame = (name: string, data: unknown) => `event: ${name}\ndata: ${JSON.stringify(data)}\n\n`;

function setup(body: string) {
  const bytes = new TextEncoder().encode(body);
  const cancel = vi.fn();
  const stream = new ReadableStream<Uint8Array>({
    start(controller) {
      for (let offset = 0; offset < bytes.length; offset += 7)
        controller.enqueue(bytes.slice(offset, offset + 7));
      controller.close();
    },
    cancel,
  });
  const fetch = vi
    .fn<typeof globalThis.fetch>()
    .mockResolvedValue(new Response(stream, { headers: { 'content-type': 'text/event-stream' } }));
  return { client: new Cmaeg({ baseUrl: 'https://gateway.example', fetch }), fetch, cancel };
}
async function collect(client: Cmaeg, mediaResponses?: boolean) {
  const values = [];
  for await (const value of client.streamEvent('channel', event, { mediaResponses }))
    values.push(value);
  return values;
}

describe('explicit attachment response streaming', () => {
  it.each(['completed', 'failed', 'incomplete', 'cancelled'] as const)(
    'retains Gateway-persisted status %s for the last supported attachment slot',
    async (status) => {
      // media_response_store preserves these four statuses; MediaArtifactClaim permits indices <32.
      const response = {
        ...media,
        attachment_index: 31,
        status,
        text: status === 'completed' ? media.text : '',
      };
      const final =
        status === 'completed'
          ? terminal
          : { ...terminal, status: 'incomplete', media_complete: false };
      const { client, fetch } = setup(
        frame('cmaeg.response.completed', original) +
          frame('cmaeg.media.response', response) +
          frame(status === 'completed' ? 'cmaeg.completed' : 'cmaeg.failed', final),
      );
      expect((await collect(client, true))[1]?.data).toEqual(response);
      expect(fetch).toHaveBeenCalledOnce();
    },
  );
  it('keeps legacy stream return types and negotiates the documented optional boolean query', () => {
    const { client, fetch } = setup('');
    expectTypeOf(client.streamEvent('channel', event)).toEqualTypeOf<
      AsyncGenerator<GatewayStreamEvent>
    >();
    expectTypeOf(client.streamEvent('channel', event, { mediaResponses: true })).toEqualTypeOf<
      AsyncGenerator<GatewayMediaStreamEvent>
    >();
    const schema = JSON.parse(
      readFileSync(new URL('../../../api/crates/cmaeg/openapi/v1.json', import.meta.url), 'utf8'),
    ) as {
      paths: Record<
        string,
        { post: { parameters: { name: string; in: string; required: boolean; schema: unknown }[] } }
      >;
    };
    expect(
      schema.paths['/channels/{channel_id}/events']?.post.parameters.find(
        (parameter) => parameter.name === 'media_responses',
      ),
    ).toMatchObject({ in: 'query', required: false, schema: { type: 'boolean' } });
    expect(fetch).not.toHaveBeenCalled();
  });
  it.each([undefined, false])(
    'preserves the legacy URL and terminal with opt-in %s',
    async (option) => {
      const { client, fetch } = setup(
        frame('cmaeg.completed', { ...identity, text: 'done', message: null, error: null }),
      );
      expect((await collect(client, option)).map((value) => value.event)).toEqual([
        'cmaeg.completed',
      ]);
      expect(String(fetch.mock.calls[0]?.[0])).toBe(
        'https://gateway.example/v1/channels/channel/events?mode=stream',
      );
      expect(fetch).toHaveBeenCalledOnce();
    },
  );

  it('observes separate responses through original completion and retains the final original text', async () => {
    const { client, fetch } = setup(
      frame('cmaeg.response.completed', original) +
        frame('cmaeg.media.response', media) +
        frame('cmaeg.completed', terminal),
    );
    const values = await collect(client, true);
    expect(values.map((value) => value.event)).toEqual([
      'cmaeg.response.completed',
      'cmaeg.media.response',
      'cmaeg.completed',
    ]);
    expect(values[1]?.data).toEqual(media);
    expect(values[2]?.data).toEqual(terminal);
    expect(String(fetch.mock.calls[0]?.[0])).toBe(
      'https://gateway.example/v1/channels/channel/events?mode=stream&media_responses=true',
    );
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('exposes bounded pending observation without replaying the admitted event', async () => {
    const { client, fetch } = setup(
      frame('cmaeg.response.completed', original) +
        frame('cmaeg.media.pending', { status: 'pending', retrieval: 'channel_activity' }),
    );
    expect((await collect(client, true)).map((value) => value.event)).toEqual([
      'cmaeg.response.completed',
      'cmaeg.media.pending',
    ]);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('retains original failure and supplementary failure without pretending the original turn failed twice', async () => {
    const error = { title: 'Unavailable', status: 503, code: 'provider_unavailable' };
    const first = setup(
      frame('cmaeg.response.failed', { ...original, text: null, error }) +
        frame('cmaeg.failed', {
          ...terminal,
          text: null,
          error,
          status: 'incomplete',
          media_complete: false,
        }),
    );
    expect((await collect(first.client, true)).map((value) => value.event)).toEqual([
      'cmaeg.response.failed',
      'cmaeg.failed',
    ]);
    const second = setup(
      frame('cmaeg.response.completed', original) +
        frame('cmaeg.media.response', { ...media, status: 'failed', text: '' }) +
        frame('cmaeg.failed', { ...terminal, status: 'incomplete', media_complete: false }),
    );
    const values = await collect(second.client, true);
    expect(values[2]?.data).toMatchObject({
      error: null,
      text: 'Original answer',
      media_complete: false,
    });
  });

  it.each([
    frame('cmaeg.media.response', { ...media, event_id: 'another' }),
    frame('cmaeg.media.response', { ...media, conversation_id: 'another' }),
    frame('cmaeg.media.response', { ...media, attachment_index: 32 }),
    frame('cmaeg.media.response', { ...media, status: 'queued' }),
    frame('cmaeg.media.response', { ...media, kind: 'caption' }),
    frame('cmaeg.response.completed', { ...original, media_pending: false }),
    frame('cmaeg.response.failed', original),
    frame('cmaeg.completed', { ...terminal, media_complete: false }),
    frame('cmaeg.failed', { ...terminal, status: 'incomplete' }),
    frame('cmaeg.media.pending', { status: 'pending', retrieval: 'untrusted_url' }),
  ])('refuses malformed or mismatched media control frames', async (bad) => {
    const { client, fetch } = setup(frame('cmaeg.response.completed', original) + bad);
    await expect(collect(client, true)).rejects.toBeInstanceOf(ProtocolError);
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('rejects unsolicited media controls in a legacy stream', async () => {
    await expect(
      collect(setup(frame('cmaeg.response.completed', original)).client),
    ).rejects.toBeInstanceOf(ProtocolError);
  });
  it('treats EOF after original completion as uncertain and never retries', async () => {
    const { client, fetch } = setup(frame('cmaeg.response.completed', original));
    await expect(collect(client, true)).rejects.toBeInstanceOf(TransportError);
    expect(fetch).toHaveBeenCalledOnce();
  });
  it('validates the opt-in before submitting a write', async () => {
    const { client, fetch } = setup('');
    await expect(collect(client, 'true' as unknown as boolean)).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(fetch).not.toHaveBeenCalled();
  });
});
