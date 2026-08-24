import { describe, expect, it, vi } from 'vitest';
import { createAguiTransport, parseEventStream } from '../../src/agui/wire.js';
import { toEvent } from '../../src/agui/events.js';
import type { ResponseMetadata } from '../../src/config.js';
import { ApiError, ProtocolError, RequestAborted, TimeoutError } from '../../src/errors.js';
import type { OperationOptions } from '../../src/http.js';

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
function sse(records: readonly { id?: number; event: string; data: unknown }[]): string {
  return records
    .map(
      (r) =>
        `${r.id === undefined ? '' : `id: ${String(r.id)}\n`}event: ${r.event}\ndata: ${JSON.stringify(r.data)}\n\n`,
    )
    .join('');
}

describe('what the interface serves is what this client reads', () => {
  it('reads a projected turn and advances only after a projection is complete', () => {
    const body = sse([
      { id: 1, event: 'RUN_STARTED', data: { type: 'RUN_STARTED', runId: 'r', threadId: 's' } },
      {
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
        id: 2,
        event: 'TEXT_MESSAGE_END',
        data: {
          type: 'TEXT_MESSAGE_END',
          runId: 'r',
          threadId: 's',
          messageId: 'msg_2',
        },
      },
      {
        id: 3,
        event: 'RUN_FINISHED',
        data: { type: 'RUN_FINISHED', runId: 'r', threadId: 's', outcome: { type: 'end_turn' } },
      },
    ]);

    const events = parseEventStream(body).map(toEvent);

    expect(events.map((e) => e.type)).toEqual([
      'RUN_STARTED',
      'TEXT_MESSAGE_START',
      'TEXT_MESSAGE_CONTENT',
      'TEXT_MESSAGE_END',
      'RUN_FINISHED',
    ]);
    expect(events.map((e) => e.seq)).toEqual([1, 0, 0, 2, 3]);
    expect(events.slice(1, 3).every((event) => event.ephemeral !== true)).toBe(true);
    expect(parseEventStream(body).slice(1, 3)).not.toContainEqual(
      expect.objectContaining({ 'cma:live': true }),
    );
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
          'cma:toolKind': 'read',
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
        event: 'RUN_FINISHED',
        data: { type: 'RUN_FINISHED', runId: 'r', threadId: 's' },
      },
    ]);

    const events = parseEventStream(body).map(toEvent);

    expect(events.map((e) => e.seq)).toEqual([41, 42]);
  });

  it('reports the core’s own vocabulary rather than reading it as AG-UI', () => {
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

    expect(() => parseEventStream(core)).toThrow(ProtocolError);
  });

  it('reports a frame it cannot parse', () => {
    const body =
      'id: 1\nevent: RUN_STARTED\ndata: {"type":"RUN_STARTED","runId":"r","threadId":"s"}\n' +
      '\nid: 2\nevent: RUN_STARTED\ndata: {not json\n' +
      '\nid: 3\nevent: RUN_FINISHED\ndata: {"type":"RUN_FINISHED","runId":"r","threadId":"s"}\n';

    expect(() => parseEventStream(body)).toThrow(ProtocolError);
  });
});

const tick = () => new Promise((resolve) => setTimeout(resolve, 0));

function deferred<T>() {
  let resolve!: (value: T | PromiseLike<T>) => void;
  const promise = new Promise<T>((complete) => {
    resolve = complete;
  });
  return { promise, resolve };
}

function eventStream(body: string, requestId?: string): Response {
  const response = new Response(body, {
    status: 200,
    headers: { 'content-type': 'text/event-stream; charset=utf-8' },
  });
  if (requestId !== undefined) {
    response.headers.set('x-request-id', requestId);
  }
  return response;
}

async function openBody(body: string, cursor = 0) {
  const fetch = vi.fn(() => Promise.resolve(eventStream(body)));
  const transport = createAguiTransport({
    fetch: fetch as typeof globalThis.fetch,
    cookies: false,
  });
  const seen: Record<string, unknown>[] = [];
  const drops: unknown[] = [];
  const stop = transport.open(
    'sesn_1',
    cursor,
    (frame) => seen.push(frame as Record<string, unknown>),
    (cause) => drops.push(cause),
  );
  await tick();
  await tick();
  return { drops, fetch, seen, stop };
}

describe('the live stream, not just the history read', () => {
  it('receives an unnamed frame, which is what the service sends', async () => {
    const frame = {
      type: 'TEXT_MESSAGE_CONTENT',
      runId: 'run_1',
      threadId: 'sesn_1',
      messageId: 'm1',
      delta: 'hi',
    };
    const { seen, stop } = await openBody(`id: 7\ndata: ${JSON.stringify(frame)}\n\n`);
    stop();

    expect(seen).toHaveLength(1);
    expect(seen[0]?.type).toBe('TEXT_MESSAGE_CONTENT');
    expect(seen[0]?.seq).toBe(7);
  });

  it('still receives a matching named frame during a rolling deploy', async () => {
    // Both builds run at once during a deploy, and §5 does not allow that to
    // interrupt a session. A client that read only the new shape would draw
    // nothing for whichever replica answered it.
    const frame = { type: 'RUN_STARTED', runId: 'run_1', threadId: 'sesn_1' };
    const { seen, stop } = await openBody(
      `id: 3\nevent: RUN_STARTED\ndata: ${JSON.stringify(frame)}\n\n`,
    );
    stop();

    expect(seen).toHaveLength(1);
    expect(seen[0]?.type).toBe('RUN_STARTED');
  });

  it('delivers each frame exactly once', async () => {
    // The named path and the unnamed path must not both fire for one frame.
    // Liveness deliberately bypasses the deduplication upstream, so a frame
    // delivered twice would append a piece of the message being typed twice.
    const records = sse([
      {
        id: 1,
        event: 'RUN_STARTED',
        data: { type: 'RUN_STARTED', runId: 'run_1', threadId: 'sesn_1' },
      },
      {
        id: 2,
        event: 'RUN_FINISHED',
        data: { type: 'RUN_FINISHED', runId: 'run_1', threadId: 'sesn_1' },
      },
    ]);
    const { seen, stop } = await openBody(records);
    stop();

    expect(seen).toHaveLength(2);
  });

  it('receives a kind this build has never heard of', async () => {
    // The reason the service stopped naming frames at all. An `EventSource`
    // cannot listen for a name it does not know, so a kind added by a newer
    // control plane can only arrive unnamed — and `API_AGUI.md` requires it to
    // arrive, so it can be shown as unknown rather than dropped.
    const frame = {
      type: 'SOMETHING_ADDED_LATER',
      runId: 'run_1',
      threadId: 'sesn_1',
      detail: 'x',
    };
    const { seen, stop } = await openBody(`id: 9\ndata: ${JSON.stringify(frame)}\n\n`);
    stop();

    expect(seen).toHaveLength(1);
    expect(toEvent(seen[0]).type).toBe('UNKNOWN');
  });

  it('sends the cursor in both resume channels', async () => {
    const { fetch, stop } = await openBody('', 8);
    stop();

    const [url, init] = fetch.mock.calls[0] as unknown as [string, RequestInit];
    expect(url).toContain('starting_after=8');
    expect(new Headers(init.headers).get('last-event-id')).toBe('8');
  });

  it('shares one request id across live and history reconnects and observes both', async () => {
    const responses = [
      eventStream('', 'response-agui-live'),
      eventStream('', 'response-agui-history'),
    ];
    const fetch = vi.fn<typeof globalThis.fetch>(() => {
      const response = responses.shift();
      return response === undefined
        ? Promise.reject(new Error('unexpected AG-UI request'))
        : Promise.resolve(response);
    });
    const requestIdProvider = vi.fn(() => 'caller-agui-connection-488');
    const observed: ResponseMetadata[] = [];
    const transport = createAguiTransport({
      cookies: false,
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
      responseObserver: (metadata) => {
        observed.push(metadata);
      },
    });
    const options: OperationOptions = {};
    const stop = transport.open(
      'sesn_1',
      0,
      () => undefined,
      () => undefined,
      options,
    );
    await tick();
    await tick();
    stop();

    await expect(transport.history('sesn_1', 0, options)).resolves.toEqual([]);

    expect(requestIdProvider).toHaveBeenCalledOnce();
    expect(
      fetch.mock.calls.map(([, init]) => new Headers(init?.headers).get('x-request-id')),
    ).toEqual(['caller-agui-connection-488', 'caller-agui-connection-488']);
    expect(observed).toEqual([
      { status: 200, requestId: 'response-agui-live' },
      { status: 200, requestId: 'response-agui-history' },
    ]);
  });

  it('keeps a failed handshake body inside the request deadline', async () => {
    const fetch = vi.fn((_url: string | URL | Request, init?: RequestInit) => {
      const body = new ReadableStream<Uint8Array>({
        start(controller) {
          init?.signal?.addEventListener(
            'abort',
            () => controller.error(new DOMException('aborted', 'AbortError')),
            { once: true },
          );
        },
      });
      return Promise.resolve(new Response(body, { status: 422 }));
    });
    const transport = createAguiTransport({ fetch: fetch as typeof globalThis.fetch });
    const drops: unknown[] = [];
    const stop = transport.open(
      'sesn_1',
      0,
      () => undefined,
      (cause) => drops.push(cause),
      { timeoutMs: 5 },
    );

    await vi.waitFor(() => expect(drops).toHaveLength(1));
    stop();

    expect(drops[0]).toBeInstanceOf(TimeoutError);
  });

  it('does not continue a timed-out request-id hook into a late live handshake', async () => {
    const requestId = deferred<string>();
    const requestIdProvider = vi.fn(() => requestId.promise);
    const credential = vi.fn(() => 'tok_late');
    const fetch = vi.fn();
    const transport = createAguiTransport({
      credential,
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
    });
    const drops: unknown[] = [];
    const stop = transport.open(
      'sesn_1',
      0,
      () => undefined,
      (cause) => drops.push(cause),
      { timeoutMs: 5 },
    );

    await vi.waitFor(() => expect(drops).toHaveLength(1));
    expect(drops[0]).toBeInstanceOf(TimeoutError);
    expect(requestIdProvider).toHaveBeenCalledOnce();

    requestId.resolve('caller-agui-after-timeout');
    await tick();
    stop();

    expect(credential).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });

  it('does not observe a live handshake that arrives after its deadline', async () => {
    const fetched = deferred<Response>();
    const responseObserver = vi.fn();
    const transport = createAguiTransport({
      fetch: vi.fn(() => fetched.promise) as unknown as typeof globalThis.fetch,
      responseObserver,
    });
    const drops: unknown[] = [];
    const stop = transport.open(
      'sesn_1',
      0,
      () => undefined,
      (cause) => drops.push(cause),
      { timeoutMs: 5 },
    );

    await vi.waitFor(() => expect(drops).toHaveLength(1));
    expect(drops[0]).toBeInstanceOf(TimeoutError);

    fetched.resolve(eventStream(''));
    await tick();
    stop();

    expect(responseObserver).not.toHaveBeenCalled();
  });

  it('does not read a live body after its response observer times out', async () => {
    const observer = deferred<void>();
    const observerStarted = deferred<void>();
    const responseObserver = vi.fn(() => {
      observerStarted.resolve(undefined);
      return observer.promise;
    });
    const getReader = vi.fn();
    const response = {
      body: { getReader },
      headers: new Headers({ 'content-type': 'text/event-stream' }),
      ok: true,
      status: 200,
    } as unknown as Response;
    const fetch = vi.fn(() => Promise.resolve(response));
    const transport = createAguiTransport({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver,
    });
    const drops: unknown[] = [];
    const stop = transport.open(
      'sesn_1',
      0,
      () => undefined,
      (cause) => drops.push(cause),
      { timeoutMs: 50 },
    );
    await observerStarted.promise;

    await vi.waitFor(() => expect(drops).toHaveLength(1));
    expect(drops[0]).toBeInstanceOf(TimeoutError);

    observer.resolve(undefined);
    await tick();
    stop();

    expect(responseObserver).toHaveBeenCalledOnce();
    expect(getReader).not.toHaveBeenCalled();
  });
});

describe('the history read finishes', () => {
  it('does not continue an aborted request-id hook into a late history request', async () => {
    const requestId = deferred<string>();
    const requestIdProvider = vi.fn(() => requestId.promise);
    const credential = vi.fn(() => 'tok_late');
    const fetch = vi.fn();
    const controller = new AbortController();
    const transport = createAguiTransport({
      credential,
      fetch: fetch as unknown as typeof globalThis.fetch,
      requestIdProvider,
    });
    const pending = transport.history('sesn_1', 0, { signal: controller.signal });
    await vi.waitFor(() => expect(requestIdProvider).toHaveBeenCalledOnce());

    controller.abort('cancelled while resolving request id');

    await expect(pending).rejects.toBeInstanceOf(RequestAborted);
    requestId.resolve('caller-agui-history-after-abort');
    await tick();

    expect(credential).not.toHaveBeenCalled();
    expect(fetch).not.toHaveBeenCalled();
  });

  it('does not observe a history response that arrives after cancellation', async () => {
    const fetched = deferred<Response>();
    const responseObserver = vi.fn();
    const controller = new AbortController();
    const fetch = vi.fn(() => fetched.promise);
    const transport = createAguiTransport({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver,
    });
    const pending = transport.history('sesn_1', 0, { signal: controller.signal });
    await vi.waitFor(() => expect(fetch).toHaveBeenCalledOnce());

    controller.abort('cancelled while fetching history');

    await expect(pending).rejects.toBeInstanceOf(RequestAborted);
    fetched.resolve(eventStream(''));
    await tick();

    expect(responseObserver).not.toHaveBeenCalled();
  });

  it('does not read a history body after its response observer times out', async () => {
    const observer = deferred<void>();
    const observerStarted = deferred<void>();
    const responseObserver = vi.fn(() => {
      observerStarted.resolve(undefined);
      return observer.promise;
    });
    const getReader = vi.fn();
    const response = {
      body: { getReader },
      headers: new Headers({ 'content-type': 'text/event-stream' }),
      ok: true,
      status: 200,
    } as unknown as Response;
    const fetch = vi.fn(() => Promise.resolve(response));
    const transport = createAguiTransport({
      fetch: fetch as unknown as typeof globalThis.fetch,
      responseObserver,
    });
    const pending = transport.history('sesn_1', 0, { timeoutMs: 50 });
    await observerStarted.promise;

    await expect(pending).rejects.toBeInstanceOf(TimeoutError);
    observer.resolve(undefined);
    await tick();

    expect(responseObserver).toHaveBeenCalledOnce();
    expect(getReader).not.toHaveBeenCalled();
  });

  it('retains a permanent HTTP status for the reconnect classifier', async () => {
    const fetch = vi.fn(() =>
      Promise.resolve(
        new Response(
          JSON.stringify({
            status: 404,
            code: 'not_found',
            detail: 'this session no longer exists',
          }),
          {
            status: 404,
            headers: {
              'content-type': 'application/problem+json',
              'x-request-id': 'response-agui-refusal',
            },
          },
        ),
      ),
    );
    const transport = createAguiTransport({ fetch: fetch as typeof globalThis.fetch });

    const failure = await transport.history('sesn_1', 0).catch((cause: unknown) => cause);

    expect(failure).toBeInstanceOf(ApiError);
    expect(failure).toMatchObject({
      status: 404,
      message: 'this session no longer exists',
      requestId: 'response-agui-refusal',
    });
  });

  it('asks the route to stop at the end of the log', async () => {
    // The whole defect in one assertion. Without this the route holds the
    // stream open across turns, `text()` never settles, and the connection —
    // which awaits this before it delivers anything — queues every live frame
    // behind a promise that never resolves. The surface stays empty however
    // well the rest of the path works.
    const asked: string[] = [];
    const fetch = vi.fn((url: string | URL | Request) => {
      asked.push(String(url));
      return Promise.resolve(
        eventStream(
          'id: 4\nevent: RUN_STARTED\ndata: {"type":"RUN_STARTED","runId":"r","threadId":"sesn_1"}\n\n',
        ),
      );
    });
    const transport = createAguiTransport({ fetch: fetch as typeof globalThis.fetch });
    const frames = await transport.history('sesn_1', 3);

    expect(asked[0]).toContain('follow=false');
    expect(asked[0]).toContain('starting_after=3');
    expect(frames).toHaveLength(1);
  });

  it('resolves rather than hanging, which is the property that was missing', async () => {
    const fetch = vi.fn(() => Promise.resolve(eventStream('')));
    const transport = createAguiTransport({ fetch: fetch as typeof globalThis.fetch });
    // An empty log is still an answer. A test that only checked the frames
    // would pass against a promise that never settles, so this races the
    // read against a timer and fails if the read loses.
    const read = transport.history('sesn_1', 0);
    const outcome = await Promise.race([
      read.then(() => 'resolved'),
      new Promise((r) => setTimeout(() => r('hung'), 250)),
    ]);

    expect(outcome).toBe('resolved');
  });
});
