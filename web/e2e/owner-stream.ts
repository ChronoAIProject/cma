import { frames } from '../../sdk/typescript/src/agui/frames';
import { object } from './environment';
import { assertCredentialFree } from './security';

export const streamBounds = { timeoutMs: 180_000, maxBytes: 8 * 1024 * 1024 };

export async function inspectOwnerStream(
  response: Response,
  agentId: string,
  forbidden: readonly string[],
  signal: AbortSignal,
) {
  if (response.body === null) throw new Error('Owner stream carried no body');
  let size = 0;
  let raw = '';
  let visible = '';
  let runId: string | undefined;
  let finished = false;
  const decoder = new TextDecoder('utf-8', { fatal: true });
  const bounded = response.body.pipeThrough(
    new TransformStream<Uint8Array, Uint8Array>({
      transform(chunk, controller) {
        size += chunk.byteLength;
        if (size > streamBounds.maxBytes) throw new Error('Owner stream exceeded its byte limit');
        raw += decoder.decode(chunk, { stream: true });
        assertCredentialFree(raw, forbidden);
        controller.enqueue(chunk);
      },
    }),
  );
  for await (const event of frames(new Response(bounded), signal, agentId)) {
    assertCredentialFree(JSON.stringify(event), forbidden);
    if (typeof event.delta === 'string') {
      visible += event.delta;
      assertCredentialFree(visible, forbidden);
    }
    if (!/^[1-9]\d*$/.test(event.runId) || !Number.isSafeInteger(Number(event.runId)))
      throw new Error('Owner stream has no durable turn identity');
    if (runId === undefined && event.type === 'RUN_STARTED') runId = event.runId;
    if (event.runId !== runId || event.type === 'RUN_ERROR')
      throw new Error('Owner stream changed turn identity or failed');
    if (event.type === 'RUN_FINISHED') {
      if (finished || object(event.outcome).type !== 'complete')
        throw new Error('Owner stream did not complete one ordinary turn');
      finished = true;
    }
  }
  if (!finished) throw new Error('Owner stream ended without a completed turn');
  return Number(runId);
}

// Only the finite conversation POST is tapped; read-only follow streams stay live.
export function ownerStreamTap(options: { origin: string; timeoutMs: number; maxBytes: number }) {
  const fetch = window.fetch.bind(window);
  const report = (event: Record<string, unknown>) =>
    (window as unknown as { __cmaOwnerStream(value: unknown): Promise<void> }).__cmaOwnerStream(
      event,
    );
  window.fetch = async (input, init) => {
    const request = new window.Request(input, init);
    const url = new URL(request.url);
    if (
      url.origin !== options.origin ||
      url.pathname !== '/api/v1/agui/run' ||
      request.method !== 'POST'
    )
      return fetch(input, init);
    const { runId } = (await request.clone().json()) as { runId: string };
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), options.timeoutMs);
    try {
      const response = await fetch(request, {
        signal: AbortSignal.any([request.signal, abort.signal]),
      });
      const inspected = response.clone();
      const observe = async () => {
        await report({
          stage: 'headers',
          runId,
          status: response.status,
          media: response.headers.get('content-type'),
        });
        const reader = inspected.body!.getReader();
        let size = 0;
        try {
          while (true) {
            const { done, value } = await reader.read();
            if (done) break;
            size += value.byteLength;
            if (size > options.maxBytes) throw new Error('Owner stream byte limit');
            for (let offset = 0; offset < value.byteLength; offset += 65_536)
              await report({
                stage: 'chunk',
                runId,
                bytes: Array.from(value.subarray(offset, offset + 65_536)),
              });
          }
          await report({ stage: 'end', runId });
        } finally {
          clearTimeout(timer);
          void reader.cancel().catch(() => undefined);
          reader.releaseLock();
        }
      };
      void observe().catch(() => {
        clearTimeout(timer);
        abort.abort();
        void report({ stage: 'failed', runId }).catch(() => undefined);
      });
      return response;
    } catch {
      clearTimeout(timer);
      abort.abort();
      await report({ stage: 'failed', runId }).catch(() => undefined);
      throw new Error('Owner conversation transport failed');
    }
  };
}
