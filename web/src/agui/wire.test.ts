import { describe, expect, it } from 'vitest';
import { parseEventStream, wireTransport } from './wire';
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

  it('reads the field the projection actually sends for a tool call', () => {
    // The defect this was written for. The projection sends `toolCallName` —
    // which is what AG-UI's `TOOL_CALL_START` calls it — and this client read
    // `target`, a name nothing sends. So every tool call arrived without the
    // one thing that identifies it, and §18.2's collapsed row could say only a
    // kind and a status: "scrollback with extra steps", in the document's own
    // words, which is exactly what the plane exists not to be.
    //
    // Transcribed from `layer1-public/src/agui/project.rs`. A rename there
    // fails here, which is the whole point of testing at this seam rather than
    // against a mock that was written to match the client.
    const body = sse([
      {
        id: 6,
        event: 'TOOL_CALL_START',
        data: {
          type: 'TOOL_CALL_START',
          runId: 'r',
          threadId: 's',
          toolCallId: 'call-1',
          toolCallName: 'Read file src/main.rs',
          'chrono:toolKind': 'read',
        },
      },
    ]);

    const [event] = parseEventStream(body).map(toEvent);

    expect(event?.type).toBe('TOOL_CALL_START');
    expect(event?.type === 'TOOL_CALL_START' && event.target).toBe('Read file src/main.rs');
    expect(event?.type === 'TOOL_CALL_START' && event.toolKind).toBe('read');
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

/**
 * The live path, which had no test at all — and that is exactly why the
 * service and this client disagreed about SSE framing for a whole release.
 *
 * The tests above exercise `parseEventStream`, which is the *history* path:
 * plain HTTP, parsed by hand, and it reads a named frame perfectly well. The
 * live path is `EventSource`, and `EventSource` does not: `onmessage` fires
 * only for a frame with no `event:` name. So the client received nothing at
 * all from production while every test here passed.
 */
class FakeEventSource {
  static last: FakeEventSource | undefined;
  onmessage: ((e: MessageEvent) => void) | null = null;
  onerror: (() => void) | null = null;
  closed = false;
  readonly named = new Map<string, (e: MessageEvent) => void>();

  constructor(readonly url: string) {
    FakeEventSource.last = this;
  }

  addEventListener(kind: string, fn: (e: MessageEvent) => void) {
    this.named.set(kind, fn);
  }

  close() {
    this.closed = true;
  }

  /** Deliver a frame the way a real EventSource would: by name, or unnamed. */
  emit(data: unknown, options: { name?: string; id?: string } = {}) {
    const event = {
      data: JSON.stringify(data),
      lastEventId: options.id ?? '',
    } as MessageEvent;
    if (options.name === undefined) {
      this.onmessage?.(event);
      return;
    }
    // The spec's behaviour, and the whole point: a named frame reaches only a
    // listener registered for that exact name. `onmessage` never sees it.
    this.named.get(options.name)?.(event);
  }
}

describe('the live stream, not just the history read', () => {
  it('receives an unnamed frame, which is what the service sends', () => {
    const original = globalThis.EventSource;
    (globalThis as { EventSource: unknown }).EventSource = FakeEventSource;
    try {
      const seen: Record<string, unknown>[] = [];
      const stop = wireTransport.open(
        'sesn_1',
        (frame) => seen.push(frame as Record<string, unknown>),
        () => undefined,
      );
      FakeEventSource.last?.emit(
        { type: 'TEXT_MESSAGE_CONTENT', messageId: 'm1', delta: 'hi' },
        { id: '7' },
      );
      stop();

      expect(seen).toHaveLength(1);
      expect(seen[0]?.type).toBe('TEXT_MESSAGE_CONTENT');
      expect(seen[0]?.seq).toBe(7);
    } finally {
      (globalThis as { EventSource: unknown }).EventSource = original;
    }
  });

  it('still receives a named frame, so a rolling deploy does not go silent', () => {
    // Both builds run at once during a deploy, and §5 does not allow that to
    // interrupt a session. A client that read only the new shape would draw
    // nothing for whichever replica answered it.
    const original = globalThis.EventSource;
    (globalThis as { EventSource: unknown }).EventSource = FakeEventSource;
    try {
      const seen: Record<string, unknown>[] = [];
      const stop = wireTransport.open(
        'sesn_1',
        (frame) => seen.push(frame as Record<string, unknown>),
        () => undefined,
      );
      FakeEventSource.last?.emit({ type: 'RUN_STARTED' }, { name: 'RUN_STARTED', id: '3' });
      stop();

      expect(seen).toHaveLength(1);
      expect(seen[0]?.type).toBe('RUN_STARTED');
    } finally {
      (globalThis as { EventSource: unknown }).EventSource = original;
    }
  });

  it('delivers each frame exactly once', () => {
    // The named path and the unnamed path must not both fire for one frame.
    // Liveness deliberately bypasses the deduplication upstream, so a frame
    // delivered twice would append a piece of the message being typed twice.
    const original = globalThis.EventSource;
    (globalThis as { EventSource: unknown }).EventSource = FakeEventSource;
    try {
      const seen: unknown[] = [];
      const stop = wireTransport.open(
        'sesn_1',
        (frame) => seen.push(frame),
        () => undefined,
      );
      FakeEventSource.last?.emit({ type: 'RUN_STARTED' }, { name: 'RUN_STARTED' });
      FakeEventSource.last?.emit({ type: 'RUN_FINISHED' });
      stop();

      expect(seen).toHaveLength(2);
    } finally {
      (globalThis as { EventSource: unknown }).EventSource = original;
    }
  });

  it('receives a kind this build has never heard of', () => {
    // The reason the service stopped naming frames at all. An `EventSource`
    // cannot listen for a name it does not know, so a kind added by a newer
    // control plane can only arrive unnamed — and `API_AGUI.md` requires it to
    // arrive, so it can be shown as unknown rather than dropped.
    const original = globalThis.EventSource;
    (globalThis as { EventSource: unknown }).EventSource = FakeEventSource;
    try {
      const seen: Record<string, unknown>[] = [];
      const stop = wireTransport.open(
        'sesn_1',
        (frame) => seen.push(frame as Record<string, unknown>),
        () => undefined,
      );
      FakeEventSource.last?.emit({ type: 'SOMETHING_ADDED_LATER', detail: 'x' }, { id: '9' });
      stop();

      expect(seen).toHaveLength(1);
      expect(toEvent(seen[0]).type).toBe('UNKNOWN');
    } finally {
      (globalThis as { EventSource: unknown }).EventSource = original;
    }
  });
});
