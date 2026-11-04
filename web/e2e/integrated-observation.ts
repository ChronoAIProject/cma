import type { Page, Request } from '@playwright/test';

export function observeJourney(page: Page) {
  const marks: Record<string, number> = {};
  const requests: {
    method: string;
    path: string;
    started: number;
    status?: number;
    finished?: number;
    failure?: string;
  }[] = [];
  const pending = new Map<Request, (typeof requests)[number]>();
  const generations = new Map<string, unknown>();
  const readings: Promise<void>[] = [];
  page.on('request', (request) => {
    const path = new URL(request.url()).pathname;
    if (!path.startsWith('/api/')) return;
    const record = { method: request.method(), path, started: Date.now() };
    pending.set(request, record);
    requests.push(record);
  });
  page.on('response', (response) => {
    const record = pending.get(response.request());
    if (record) record.status = response.status();
    if (response.ok() && new URL(response.url()).pathname.endsWith('/suggestions'))
      readings.push(
        response.json().then((value) => {
          generations.set(response.url(), { reading: value, observedAt: Date.now() });
        }),
      );
  });
  page.on('requestfinished', (request) => {
    const record = pending.get(request);
    if (record) record.finished = Date.now();
    pending.delete(request);
  });
  page.on('requestfailed', (request) => {
    const record = pending.get(request);
    if (record)
      Object.assign(record, { finished: Date.now(), failure: request.failure()?.errorText });
    pending.delete(request);
  });
  return {
    mark(name: string) {
      marks[name] = Date.now();
    },
    async result() {
      await Promise.all(readings);
      return {
        marks,
        requests,
        generations: [...generations.values()],
        confirmationMs: marks.firstReceipt! - marks.firstIntent!,
        deliveryMs: marks.downloadFinished! - marks.downloadStarted!,
        feedbackMs: marks.feedbackSaved! - marks.feedbackStarted!,
        followupMs: marks.secondCompleted! - marks.secondSent!,
        directFollowupMs:
          marks.directSent === undefined ? null : marks.directCompleted! - marks.directSent,
        denominator: {
          uniqueTaskIntents: 1,
          explicitDownloads: 1,
          negativeVotes: 1,
          reasonSets: 1,
          editedFollowups: 1,
          directFollowups: marks.directSent === undefined ? 0 : 1,
        },
        costBoundary:
          'External synthetic model usage; unknown price stays null, never a billing observation.',
      };
    },
  };
}

export function sanitizedBody(request: Request): unknown {
  const text = request.postData();
  if (!text) return text;
  let value;
  try {
    value = JSON.parse(text);
  } catch {
    return text;
  }
  if (value && typeof value === 'object') {
    for (const field of ['client_key', 'lease_token'])
      if (field in value) value[field] = '[redacted]';
  }
  return value;
}
