// @vitest-environment node
import { describe, expect, it, vi } from 'vitest';
import {
  ApiError,
  ProtocolError,
  RequestAborted,
  TransportError,
  ValidationError,
  type V2AgentStreamFrame,
} from '../index.js';
import {
  agent,
  incarnation,
  cursor,
  pageCursor,
  eligibility,
  item,
  page,
  durable,
  live,
  gap,
  json,
  frame,
  sse,
  harness,
  kinds,
} from './agent-events-v2-fixtures.js';

describe('Agent events v2', () => {
  it('reads authenticated new-tab eligibility without interpreting false as authorization failure', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(eligibility()));
    expect(await t.api.readTransportEligibility()).toEqual(eligibility());
    const [url, options] = t.fetch.mock.calls[0]!;
    expect(String(url)).toBe('https://cma.example/gateway/api/v2/ui/transport');
    expect(new Headers(options!.headers).get('authorization')).toBe('Bearer secret');
    expect(options!.method).toBe('GET');
  });
  it.each(['enabled', 'snapshot_schema', 'events', 'live_fragments'])(
    'refuses missing eligibility %s',
    async (field) => {
      const value: Record<string, unknown> = eligibility();
      delete value[field];
      const t = harness();
      t.fetch.mockResolvedValueOnce(json(value));
      await expect(t.api.readTransportEligibility()).rejects.toBeInstanceOf(ProtocolError);
    },
  );
  it('keeps frozen-head pagination and through cursor separate while new events append', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(page(4, 0, 2))).mockResolvedValueOnce(json(page(4, 2, 2)));
    const first = await t.api.snapshot(agent, { limit: 2 });
    expect(first.next_page_cursor).toBe(pageCursor(4, 2));
    const second = await t.api.snapshot(agent, { limit: 2, pageCursor: first.next_page_cursor! });
    expect(second.through_cursor).toBe(first.through_cursor);
    expect(second.next_page_cursor).toBeNull();
    expect(new URL(String(t.fetch.mock.calls[1]![0])).searchParams.get('page_cursor')).toBe(
      first.next_page_cursor,
    );
  });
  it.each(kinds)('preserves safe $type chronology and original nullable fields', async (kind) => {
    const t = harness();
    const value = page(1);
    value.items = [item(1, kind)];
    t.fetch.mockResolvedValueOnce(json(value));
    expect((await t.api.snapshot(agent)).items[0]!.kind).toEqual(kind);
  });
  it('retains verified binding only on its original turn start', async () => {
    const value = page(1);
    value.items = [item(1, { type: 'turn_started', turn_sequence: 1 })];
    value.items[0]!.response_binding = { command_id: 'cmd', response_id: 'resp', turn_sequence: 1 };
    const t = harness();
    t.fetch.mockResolvedValueOnce(json(value));
    expect((await t.api.snapshot(agent)).items[0]!.response_binding).toEqual(
      value.items[0]!.response_binding,
    );
  });
  it.each([
    '0',
    'ag1.bad.1',
    `ag1.${incarnation}.01`,
    `ag1.${incarnation}.9007199254740992`,
    `ags1.${incarnation}.2.1`,
  ])('rejects an invalid tail cursor %s before IO', async (afterCursor) => {
    const t = harness();
    await expect(t.api.events(agent, { afterCursor, onEvent: vi.fn() })).rejects.toBeInstanceOf(
      ValidationError,
    );
    expect(t.fetch).not.toHaveBeenCalled();
  });
  it.each([0, 201, 1.5, NaN])('rejects snapshot limit %s before IO', async (limit) => {
    const t = harness();
    await expect(t.api.snapshot(agent, { limit })).rejects.toBeInstanceOf(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
  });
  it('acknowledges durable rows only after successful consumer work and preserves live separation', async () => {
    const t = harness();
    const body =
      frame('cma.agent.event', durable(1), cursor(1)) +
      frame('cma.agent.event', durable(2), cursor(2)) +
      frame('cma.agent.live', live()) +
      frame('cma.agent.live_gap', gap());
    t.fetch.mockResolvedValueOnce(sse(body, 7));
    const order: string[] = [];
    await t.api.events(agent, {
      follow: false,
      onEvent: async (event) => {
        await Promise.resolve();
        order.push(event.type);
      },
      onCursor: (value) => {
        order.push(value);
      },
    });
    expect(order).toEqual([
      'cma.agent.event',
      cursor(1),
      'cma.agent.event',
      cursor(2),
      'cma.agent.live',
      'cma.agent.live_gap',
    ]);
  });
  it('retains the prior cursor when the consumer fails and resumes by GET after that exact cursor', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      sse(
        frame('cma.agent.event', durable(2), cursor(2)) +
          frame('cma.agent.event', durable(3), cursor(3)),
      ),
    );
    let retained = cursor(1);
    const seen: number[] = [];
    await expect(
      t.api.events(agent, {
        afterCursor: retained,
        onEvent: (event) => {
          if (event.type === 'cma.agent.event') {
            if (event.data.sequence === 3) throw new Error('reducer failed');
            seen.push(event.data.sequence);
          }
        },
        onCursor: (next) => {
          retained = next;
        },
      }),
    ).rejects.toBeInstanceOf(ProtocolError);
    expect(retained).toBe(cursor(2));
    expect(t.fetch).toHaveBeenCalledOnce();
    t.fetch.mockResolvedValueOnce(sse(frame('cma.agent.event', durable(3), cursor(3))));
    await t.api.events(agent, {
      afterCursor: retained,
      follow: false,
      onEvent: (event) => {
        if (event.type === 'cma.agent.event') seen.push(event.data.sequence);
      },
      onCursor: (next) => {
        retained = next;
      },
    });
    expect(seen).toEqual([2, 3]);
    expect(retained).toBe(cursor(3));
    expect(new URL(String(t.fetch.mock.calls[1]![0])).searchParams.get('after_cursor')).toBe(
      cursor(2),
    );
    expect(
      t.fetch.mock.calls.every(
        ([, options]) => options!.method === 'GET' && options!.body === undefined,
      ),
    ).toBe(true);
  });
  it('does not continue or invent an acknowledgement when cursor persistence fails', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      sse(
        frame('cma.agent.event', durable(1), cursor(1)) +
          frame('cma.agent.event', durable(2), cursor(2)),
      ),
    );
    const onEvent = vi.fn();
    const onCursor = vi.fn(() => {
      throw new Error('storage failed');
    });
    await expect(t.api.events(agent, { onEvent, onCursor })).rejects.toBeInstanceOf(ProtocolError);
    expect(onEvent).toHaveBeenCalledOnce();
    expect(onCursor).toHaveBeenCalledOnce();
  });
  it('surfaces safe stream error without acknowledging or automatically reconnecting', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      sse(frame('cma.agent.error', { code: 'agent_read_unavailable', message: 'Unavailable' })),
    );
    const onCursor = vi.fn();
    const seen: V2AgentStreamFrame[] = [];
    await expect(
      t.api.events(agent, {
        onEvent: (event) => {
          seen.push(event);
        },
        onCursor,
      }),
    ).rejects.toBeInstanceOf(TransportError);
    expect(seen[0]!.type).toBe('cma.agent.error');
    expect(onCursor).not.toHaveBeenCalled();
    expect(t.fetch).toHaveBeenCalledOnce();
  });
  it('keeps typed HTTP refusals for missing history or changed incarnation', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      json(
        {
          type: 'about:blank',
          title: 'Conflict',
          status: 409,
          code: 'agent_incarnation_changed',
          detail: 'Agent was recreated',
        },
        409,
      ),
    );
    await expect(t.api.snapshot(agent)).rejects.toMatchObject({
      constructor: ApiError,
      status: 409,
      code: 'agent_incarnation_changed',
    });
  });
  it('aborts observation without a command or cursor callback', async () => {
    const t = harness();
    const controller = new AbortController();
    controller.abort();
    await expect(
      t.api.events(agent, { signal: controller.signal, onEvent: vi.fn() }),
    ).rejects.toBeInstanceOf(RequestAborted);
    expect(t.fetch).not.toHaveBeenCalled();
  });
  it('accepts a four-byte UTF-8 message split across transport chunks', async () => {
    const t = harness();
    t.fetch.mockResolvedValueOnce(
      sse(
        frame(
          'cma.agent.event',
          durable(1, { type: 'message', role: 'assistant', content: '🙂' }),
          cursor(1),
        ),
        1,
      ),
    );
    const seen = vi.fn();
    await t.api.events(agent, { onEvent: seen });
    expect(seen).toHaveBeenCalledOnce();
  });
});
