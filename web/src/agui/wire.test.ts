import { describe, expect, it } from 'vitest';
import { parseEventStream } from './wire';
import { toEvent } from './events';

/**
 * The seam between what the interface serves and what this client reads.
 *
 * Every other chat test drives `MockSession`, whose frames are already
 * AG-UI-shaped. That proves the surface renders AG-UI; it cannot prove the
 * wire delivers it — and it did not. The client asked a REST route for
 * `?stream=agui`, a parameter that route does not have, and received the
 * core's own vocabulary: `{"seq":…,"session_id":…,"event":{"type":…}}`, with
 * no top-level `type`. Every frame parsed as unknown, so against production
 * this surface could draw nothing but unknown rows.
 *
 * So these tests hold the shapes rather than a mock. The bodies below are the
 * projection's own output, transcribed from `layer1-public/src/agui/project.rs`
 * — if that file renames a field, these fail, which is the whole point.
 */

/** An SSE body exactly as the replay route writes it. */
function sse(records: readonly { id: number; event: string; data: unknown }[]): string {
  return records
    .map((r) => `id: ${String(r.id)}\nevent: ${r.event}\ndata: ${JSON.stringify(r.data)}\n`)
    .join('\n');
}

describe('what the interface serves is what this client reads', () => {
  it('reads a projected turn without falling through to unknown', () => {
    const body = sse([
      { id: 1, event: 'RUN_STARTED', data: { type: 'RUN_STARTED', runId: 'r', threadId: 's' } },
      {
        id: 2,
        event: 'TEXT_MESSAGE_START',
        data: {
          type: 'TEXT_MESSAGE_START',
          runId: 'r',
          threadId: 's',
          messageId: 'msg_2',
          role: 'assistant',
        },
      },
      {
        id: 3,
        event: 'TEXT_MESSAGE_CONTENT',
        data: {
          type: 'TEXT_MESSAGE_CONTENT',
          runId: 'r',
          threadId: 's',
          messageId: 'msg_2',
          delta: 'carried across',
        },
      },
      {
        id: 4,
        event: 'RUN_FINISHED',
        data: { type: 'RUN_FINISHED', runId: 'r', threadId: 's', outcome: { type: 'end_turn' } },
      },
    ]);

    const events = parseEventStream(body).map(toEvent);

    expect(events.map((e) => e.type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'RUN_FINISHED',
    ]);
    expect(events.some((e) => e.type === 'UNKNOWN')).toBe(false);
  });

  it('takes the sequence from the SSE id, because the body deliberately has none', () => {
    // `AguiEvent.seq` is `#[serde(skip)]` in the projection, so a client is
    // never handed a field that looks like protocol and is not. Everything
    // above depends on `seq` for deduplication and for the cursor it resumes
    // on, so reading it off the body — which is what this client did — leaves
    // every frame at zero and silently breaks both.
    const body = sse([
      { id: 41, event: 'RUN_STARTED', data: { type: 'RUN_STARTED', runId: 'r', threadId: 's' } },
      {
        id: 42,
        event: 'TURN_COMPLETED',
        data: { type: 'RUN_FINISHED', runId: 'r', threadId: 's' },
      },
    ]);

    const events = parseEventStream(body).map(toEvent);

    expect(events.map((e) => e.seq)).toEqual([41, 42]);
  });

  it('refuses to read the core’s own vocabulary as if it were this protocol', () => {
    // The exact frame the old route served. It must not quietly become
    // something: an unknown row is the honest reading, and is what told us
    // the client was pointed at the wrong route.
    const core = sse([
      {
        id: 9,
        event: 'message',
        data: { seq: 9, session_id: 'sesn_1', event: { type: 'progress', stage: 'ready' } },
      },
    ]);

    const events = parseEventStream(core).map(toEvent);

    expect(events[0]?.type).toBe('UNKNOWN');
  });

  it('keeps reading after a frame it cannot parse', () => {
    const body =
      'id: 1\nevent: RUN_STARTED\ndata: {"type":"RUN_STARTED","runId":"r","threadId":"s"}\n' +
      '\nid: 2\nevent: RUN_STARTED\ndata: {not json\n' +
      '\nid: 3\nevent: RUN_FINISHED\ndata: {"type":"RUN_FINISHED","runId":"r","threadId":"s"}\n';

    const events = parseEventStream(body).map(toEvent);

    expect(events.map((e) => e.type)).toEqual(['RUN_STARTED', 'RUN_FINISHED']);
  });
});
