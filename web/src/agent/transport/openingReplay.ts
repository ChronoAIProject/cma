import type { Cma } from '@cma/client';

// Both server paths accept 1..200 rows and retain the 4 MiB response bound.
export const OPENING_EVENT_PAGE_LIMIT = 200;

/** Only an access-classified, freshly admitted opening supplies this handle. */
export function prepareOpeningReplay(
  selectedEvents: Promise<Cma['agentEventsV2'] | undefined>,
  agent: string,
  signal: AbortSignal,
  assertCurrent: () => void,
) {
  const selected = selectedEvents.then((events) => {
    signal.throwIfAborted();
    assertCurrent();
    return events;
  });
  const firstPage = selected.then((events) =>
    events?.snapshot(agent, {
      limit: OPENING_EVENT_PAGE_LIMIT,
      signal,
      externalApprovals: true,
      messageBindings: true,
      toolOutput: true,
      retainedOutput: true,
    }),
  );
  const head = selected.then((events) => events?.head(agent, { signal, externalApprovals: true }));
  // Preserve both failures for the loader; abandoning speculative navigation
  // must not leave an unhandled rejection or cause a replacement request.
  void Promise.allSettled([firstPage, head]);
  return { firstPage, head };
}
