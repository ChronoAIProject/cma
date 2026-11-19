import type { Page } from '@playwright/test';

export interface OpeningReadTrace {
  kind: 'transcript' | 'snapshot';
  page: 'first' | 'next';
  phases: ('started' | 'received' | 'consumed' | 'aborted' | 'failed')[];
}
type TracedWindow = Window & { openingReadTrace: OpeningReadTrace[] };

/** Counts and lifecycle only; no response content or authorization is recorded. */
export async function traceOpeningReads(page: Page) {
  await page.addInitScript(() => {
    const trace: OpeningReadTrace[] = [];
    (window as unknown as TracedWindow).openingReadTrace = trace;
    const original = window.fetch.bind(window);
    window.fetch = async (input, options) => {
      const url = new URL(input instanceof Request ? input.url : String(input), location.href);
      const kind = url.pathname.endsWith('/transcript')
        ? 'transcript'
        : url.pathname.endsWith('/events/snapshot')
          ? 'snapshot'
          : undefined;
      if (!kind) return original(input, options);
      const entry: OpeningReadTrace = {
        kind,
        page: url.searchParams.has('page_cursor') ? 'next' : 'first',
        phases: ['started'],
      };
      trace.push(entry);
      const signal = options?.signal ?? (input instanceof Request ? input.signal : undefined);
      signal?.addEventListener('abort', () => entry.phases.push('aborted'), { once: true });
      try {
        const response = await original(input, options);
        entry.phases.push('received');
        if (response.body) {
          const getReader = response.body.getReader.bind(response.body);
          response.body.getReader = (() => {
            const reader = getReader();
            const read = reader.read.bind(reader);
            reader.read = async () => {
              const result = await read();
              if (result.done) entry.phases.push('consumed');
              return result;
            };
            return reader;
          }) as typeof response.body.getReader;
        }
        return response;
      } catch (error) {
        entry.phases.push('failed');
        throw error;
      }
    };
  });
}
export const openingReads = (page: Page) =>
  page.evaluate(() => (window as unknown as TracedWindow).openingReadTrace);
