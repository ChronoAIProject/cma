import { expect, type Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import {
  channelCapabilities,
  cmaSession,
  gatewaySession,
  teamChannel,
} from '../build/fixtures/eventGateway';
import type {
  ActivityPage,
  ActivityConsumer,
  ActivityDelivery,
} from '../src/services/useChannelActivity';

export const responseConsumer: ActivityConsumer = {
  consumer_id: 'sample-task',
  display_name: 'Sample task',
  provider: 'cma_codex',
  state: 'produced',
  dispatched_at: '2026-11-22T02:24:02Z',
  accepted_at: '2026-11-22T02:24:03Z',
  produced_at: '2026-11-22T02:25:00Z',
  completed_at: null,
  response_id: 'sample-response',
  agent_id: 'agt_sample',
  deliveries: [],
};
export const receipt = (
  outcome: ActivityDelivery['outcome'],
  recipient = 'sample-team',
): ActivityDelivery => ({
  step: 'answer',
  destination: 'origin',
  outcome,
  observed_at: '2026-11-22T02:25:02Z',
  confirmed_at: outcome === 'confirmed' ? '2026-11-22T02:25:02Z' : null,
  platform_message_id: null,
  recipients: [{ kind: 'lark_chat', id: recipient, thread_id: null }],
});
export function activityPage(consumers = [responseConsumer]): ActivityPage {
  return {
    cursor: 'sample-cursor',
    retention_days: 7,
    limit: 50,
    has_more: false,
    items: [
      {
        event_id: 'demo-event-01',
        kind: 'message',
        source: { type: 'nyxid_relay', platform: 'feishu' },
        actor: { id: 'synthetic-participant', display_name: 'Never displayed', kind: 'human' },
        conversation: { id: 'sample-conversation', kind: 'group' },
        occurred_at: '2026-11-22T02:24:00Z',
        received_at: '2026-11-22T02:24:01Z',
        consumers,
      },
    ],
  };
}

interface ActivityBrowserError {
  type: string;
  message: string;
  filename?: string;
  lineno?: number;
  colno?: number;
  stack?: string;
}

async function captureActivityErrors(page: Page) {
  const errors: ActivityBrowserError[] = [];
  await page.exposeBinding('__recordActivityBrowserError', (_source, error: ActivityBrowserError) => {
    errors.push(error);
  });
  await page.addInitScript(() => {
    const record = (error: ActivityBrowserError) =>
      (
        window as unknown as {
          __recordActivityBrowserError: (error: ActivityBrowserError) => Promise<void>;
        }
      ).__recordActivityBrowserError(error);
    window.addEventListener('error', (event) => {
      void record({
        type: event.type,
        message: event.message,
        filename: event.filename,
        lineno: event.lineno,
        colno: event.colno,
        stack: event.error instanceof Error ? event.error.stack : undefined,
      });
    });
    window.addEventListener('unhandledrejection', (event) => {
      void record({
        type: event.type,
        message: event.reason instanceof Error ? event.reason.message : String(event.reason),
        stack: event.reason instanceof Error ? event.reason.stack : undefined,
      });
    });
  });
  return errors;
}

// Keep each synthetic stream open after its HTTP snapshot, without native reconnect timers.
// The app still owns subscription, refresh, snapshot parsing and scope cleanup.
async function localStreams(page: Page) {
  await page.addInitScript(() => {
    class LocalActivityStream extends EventTarget {
      static CLOSED = 2;
      readyState = 0;
      onerror: (() => void) | null = null;
      private current = true;
      private abort = new AbortController();
      constructor(
        readonly url: string,
        options?: EventSourceInit,
      ) {
        super();
        void fetch(url, {
          credentials: options?.withCredentials ? 'include' : 'same-origin',
          signal: this.abort.signal,
        })
          .then(async (response) => {
            if (!response.ok) throw new Error('Synthetic activity read refused');
            const body = await response.text();
            if (!this.current) return;
            this.readyState = 1;
            for (const block of body.split('\n\n')) {
              const type = /^event: (.+)$/m.exec(block)?.[1];
              const data = /^data: (.+)$/m.exec(block)?.[1];
              if (type && data) this.dispatchEvent(new MessageEvent(type, { data }));
            }
          })
          .catch(() => {
            if (!this.current) return;
            this.readyState = LocalActivityStream.CLOSED;
            this.onerror?.();
          });
      }
      close() {
        this.current = false;
        this.readyState = LocalActivityStream.CLOSED;
        this.abort.abort();
      }
    }
    window.EventSource = LocalActivityStream as unknown as typeof EventSource;
  });
}

export async function setupActivity(page: Page, baseURL: string, theme: 'dark' | 'light' = 'dark') {
  const browserErrors = await captureActivityErrors(page);
  await page.clock.install({ time: new Date('2026-11-22T02:26:00Z') });
  await page.emulateMedia({ reducedMotion: 'reduce' });
  await page.addInitScript(
    (appearance) => localStorage.setItem('cma:appearance', appearance),
    theme,
  );
  await localStreams(page);
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const state = {
    snapshot: activityPage(),
    reads: [] as string[],
    writes: [] as string[],
    status: 200,
    hold: undefined as Promise<void> | undefined,
  };
  await page.route('**/auth/session', (route) => route.fulfill({ json: cmaSession }));
  await page.route('**/api/v1/bots?**', (route) =>
    route.fulfill({ json: { items: [], next_after: null } }),
  );
  await page.route('**/api/event-gateway/**', async (route) => {
    const request = route.request(),
      path = new URL(request.url()).pathname;
    if (request.method() !== 'GET') {
      state.writes.push(`${request.method()} ${path}`);
      return route.fulfill({ status: 405, json: { detail: 'Read-only activity fixture' } });
    }
    if (path.endsWith('/auth/session')) return route.fulfill({ json: gatewaySession });
    if (path.endsWith('/v1/channel-capabilities'))
      return route.fulfill({ json: channelCapabilities });
    if (path.endsWith('/v1/connections') || path.endsWith('/v1/channel-connections'))
      return route.fulfill({ json: { items: [] } });
    if (path.endsWith('/activity/stream')) {
      state.reads.push(path);
      const snapshot = structuredClone(state.snapshot),
        status = state.status;
      await state.hold;
      return route.fulfill({
        status,
        contentType: 'text/event-stream',
        body: `event: activity\ndata: ${JSON.stringify(snapshot)}\n\n`,
      });
    }
    if (path.endsWith('/v1/channels'))
      return route.fulfill({
        json: {
          items: [
            { ...teamChannel, definition: { ...teamChannel.definition, name: 'Example team' } },
            {
              ...teamChannel,
              channel_id: 'other-channel',
              record_id: 'other-record',
              definition: { ...teamChannel.definition, name: 'Other team' },
            },
          ],
          next_cursor: null,
        },
      });
    return route.fulfill({ status: 404, json: { detail: 'Unexpected local request' } });
  });
  await page.goto('/event-gateway');
  await expect(page.getByRole('region', { name: 'Channel workspace', exact: true })).toBeVisible();
  const showNavigation = page.getByRole('button', { name: 'Show navigation', exact: true });
  if (await showNavigation.isVisible()) await showNavigation.click();
  await page
    .getByRole('region', { name: 'Channels', exact: true })
    .getByRole('treeitem', { name: /^Example team/ })
    .click();
  await page
    .getByRole('region', { name: 'Delivery map' })
    .getByRole('button', { name: 'Activity', exact: true })
    .click();
  const view = page.getByRole('dialog').locator('.channel-activity');
  await expect(view.getByText('demo-event-01')).toBeVisible();
  await expect(view.getByText('Live activity', { exact: true })).toBeVisible();
  return { state, boundary, view, browserErrors };
}
