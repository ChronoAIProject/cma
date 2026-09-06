import { describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
} from '../src/errors.js';
import { Launches, repeatChoiceFrom } from '../src/launch/index.js';
import { createClient } from '../src/client.js';
import { launchEventKind } from '../src/launch/events.js';
import { command, transport } from './resource-fixtures.js';
import { launchFixture, previewFixture } from './launch-fixtures.js';

describe('public entry resources', () => {
  it('binds preview, admission, owner reads, review, connections and command keys', async () => {
    const test = transport();
    const api = new Launches(test.client.config);
    test.answer(previewFixture);
    expect(await api.preview('public/example')).toEqual(previewFixture);
    expect(test.calls.at(-1)?.url).toContain('public%2Fexample');
    test.answer(launchFixture, 201);
    await api.createOrResume(
      'public-example',
      { start_if_ready: true, choice: 'resume', placement_id: 'plc_readme' },
      command,
    );
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({
      start_if_ready: true,
      choice: 'resume',
      placement_id: 'plc_readme',
    });
    test.answer({ ...launchFixture, state: 'resolving' }, 202);
    await api.start(
      launchFixture.id,
      { acknowledgements: [], trust_confirmed_digest: 'review-one' },
      command,
    );
    test.answer({ ...launchFixture, state: 'cancelled' });
    await api.cancel(launchFixture.id, command);
    test.answer(launchFixture);
    expect(await api.read(launchFixture.id)).toEqual(launchFixture);
    test.answer({ items: [launchFixture], next_cursor: 'more' });
    expect(await api.list({ cursor: 'old', limit: 4 })).toEqual({
      items: [launchFixture],
      next: 'more',
    });
    test.answer({
      services: [
        { service: 'code', state: 'missing', display_name: 'Code host', connection_mode: 'hosted' },
      ],
      checked_at: launchFixture.created_at,
    });
    expect((await api.requirements(launchFixture.id)).services[0]?.display_name).toBe('Code host');
    const created = {
      handoff_id: 'handoff-one',
      hosted_url: 'https://identity.example.test/connect',
      expires_at: launchFixture.created_at,
    };
    test.answer(created, 201);
    expect(
      await api.createConnection(
        { launch_id: launchFixture.id, service_id: 'code', intent_id: 'intent-one' },
        command,
      ),
    ).toEqual(created);
    test.answer({
      ...created,
      launch_id: launchFixture.id,
      service_id: 'code',
      state: 'satisfied',
      updated_at: launchFixture.updated_at,
    });
    expect((await api.readConnection('handoff-one')).state).toBe('satisfied');
    for (const call of test.calls.filter(({ init }) => init.method === 'POST'))
      expect(new Headers(call.init.headers).get('idempotency-key')).toBe(command.idempotencyKey);
  });

  it('validates identity, closed enums, event ordering and cursors', async () => {
    const test = transport();
    const api = test.client.launches;
    const event = {
      launch_id: launchFixture.id,
      seq: 2,
      appended_at: launchFixture.created_at,
      kind: { type: 'created' },
    };
    test.answer({ launch_id: launchFixture.id, events: [event], next_cursor: 2 });
    expect((await api.events(launchFixture.id, { after: 1 })).events).toEqual([event]);
    for (const page of [
      { launch_id: 'other', events: [event], next_cursor: 2 },
      { launch_id: launchFixture.id, events: [event, event], next_cursor: 2 },
      { launch_id: launchFixture.id, events: [event], next_cursor: 3 },
    ]) {
      test.answer(page);
      await expect(api.events(launchFixture.id, { after: 1 })).rejects.toBeInstanceOf(
        ProtocolError,
      );
    }
    test.answer({ ...launchFixture, id: 'other' });
    await expect(api.read(launchFixture.id)).rejects.toBeInstanceOf(ProtocolError);
    test.answer({ ...launchFixture, state: 'invented' });
    await expect(api.read(launchFixture.id)).rejects.toBeInstanceOf(ProtocolError);
    test.answer({
      handoff_id: 'other',
      launch_id: launchFixture.id,
      service_id: 'code',
      state: 'pending',
      expires_at: launchFixture.created_at,
      updated_at: launchFixture.updated_at,
    });
    await expect(api.readConnection('wanted')).rejects.toBeInstanceOf(ProtocolError);
    expect(() =>
      api.start(launchFixture.id, { acknowledgements: [] }, { idempotencyKey: '' }),
    ).toThrow(ValidationError);
    expect(() => launchEventKind({ type: 'invented' })).toThrow(ProtocolError);
  });

  it('extracts repeat choices from structured errors including a null title', () => {
    const existing = {
      agent_id: 'agt_previous',
      title: null,
      created_at: launchFixture.created_at,
      first_message_excerpt: 'Inspect the project',
    };
    expect(
      repeatChoiceFrom(
        new ApiError(409, 'Choose', 'trigger_repeat_choice_required', undefined, undefined, {
          existing,
        }),
      ),
    ).toEqual(existing);
    expect(repeatChoiceFrom(new Error('trigger_repeat_choice_required'))).toBeUndefined();
    expect(repeatChoiceFrom(new ApiError(409, 'Choose', 'other'))).toBeUndefined();
    expect(() =>
      repeatChoiceFrom(new ApiError(409, 'Choose', 'trigger_repeat_choice_required')),
    ).toThrow(ProtocolError);
  });
});

describe('progress companion', () => {
  const event = (seq: number, id = launchFixture.id) => ({
    launch_id: id,
    seq,
    appended_at: launchFixture.created_at,
    kind: { type: 'created' },
  });
  const wire = (seq: number, id?: string) =>
    `id: ${seq}\ndata: ${JSON.stringify(event(seq, id))}\n\n`;
  const sdk = (body: string, media = 'text/event-stream') =>
    createClient({
      baseUrl: 'https://cma.example.test',
      fetch: vi.fn(async () => new Response(body, { headers: { 'content-type': media } })),
    });

  it('delivers durable events in order from the requested cursor', async () => {
    const client = sdk(wire(2) + wire(3));
    const seen: number[] = [];
    await client.launches.stream(launchFixture.id, {
      after: 1,
      onEvent: async (value) => {
        seen.push(value.seq);
      },
    });
    expect(seen).toEqual([2, 3]);
    expect(client.config.fetch).toHaveBeenCalledWith(
      expect.stringContaining('starting_after=1'),
      expect.objectContaining({ method: 'GET' }),
    );
  });

  it.each([
    wire(1),
    wire(2, 'other'),
    wire(2) + wire(2),
    'id: 2\ndata: malformed\n\n',
    'id: 3\ndata: {}\n\n',
  ])('rejects malformed and nonadvancing event data', async (body) => {
    await expect(
      sdk(body).launches.stream(launchFixture.id, { after: 1, onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(ProtocolError);
  });

  it('validates media, cursor, consumer acknowledgement and cancellation', async () => {
    await expect(
      sdk('{}', 'application/json').launches.stream(launchFixture.id, { onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(ProtocolError);
    await expect(
      sdk('').launches.stream(launchFixture.id, { after: -1, onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(ValidationError);
    const consumer = vi.fn().mockRejectedValue(new Error('not acknowledged'));
    await expect(
      sdk(wire(1) + wire(2)).launches.stream(launchFixture.id, { onEvent: consumer }),
    ).rejects.toThrow('not acknowledged');
    expect(consumer).toHaveBeenCalledOnce();
    const controller = new AbortController();
    controller.abort();
    await expect(
      sdk('').launches.stream(launchFixture.id, { signal: controller.signal, onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(RequestAborted);
  });

  it('normalizes invalid UTF-8, transport interruption, and in-flight cancellation', async () => {
    const from = (body: BodyInit) =>
      createClient({
        baseUrl: 'https://cma.example.test',
        fetch: vi.fn(
          async () => new Response(body, { headers: { 'content-type': 'text/event-stream' } }),
        ),
      });
    await expect(
      from(new Uint8Array([255])).launches.stream(launchFixture.id, { onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(ProtocolError);
    const failed = new ReadableStream({
      start(controller) {
        controller.error(new TypeError('offline'));
      },
    });
    await expect(
      from(failed).launches.stream(launchFixture.id, { onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(TransportError);
    const controller = new AbortController();
    const cancel = vi.fn();
    const stream = new ReadableStream({
      start(source) {
        source.enqueue(new TextEncoder().encode(wire(1)));
      },
      cancel,
    });
    const acknowledge = vi.fn(() => controller.abort());
    await expect(
      from(stream).launches.stream(launchFixture.id, {
        signal: controller.signal,
        onEvent: acknowledge,
      }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(acknowledge).toHaveBeenCalledOnce();
    expect(cancel).toHaveBeenCalledOnce();
  });
});
