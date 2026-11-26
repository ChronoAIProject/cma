import { verifiedEntryHeaders } from './public-entry-binding.fixture';
import { expect, type Page, type Request } from '@playwright/test';
import type { Launch, TriggerPreview } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';

export type Outcome = 'accepted' | 'credential-refusal' | 'forbidden' | 'unknown' | 'fenced';
export interface EntryPacket {
  id: string;
  key: string;
  owner?: string;
  body: unknown;
  submitted: boolean;
  wireBody?: string;
  recoveryRequired?: true;
  expiresAt: number;
  credentialRecovery?: string;
}

export async function entryFixture(
  page: Page,
  baseURL: string,
  mode: 'preflight' | 'command',
  outcome: Outcome = 'accepted',
) {
  const boundary = new LocalStartup();
  await boundary.route(page, baseURL);
  const previous: Launch = {
    ...recorded<Launch>('launches.json', 'launch-ready-create'),
    review: { needs: [{ type: 'owner_requires_review' }], can_start_now: false },
  };
  const accepted: Launch = {
    ...previous,
    id: 'lch_entry_accepted',
    state: 'resolving',
    revision: 2,
    event_head: 2,
    review: { needs: [], can_start_now: false },
  };
  let subject = 'local-owner';
  let holdSession = false;
  let release!: () => void;
  const gate = new Promise<void>((resolve) => {
    release = resolve;
  });
  let held: Request | undefined;
  let handled = false;
  let ended = false;
  let failed = false;
  let status: number | undefined;
  const sessions: string[] = [];
  const posts: { owner: string; key?: string; body: string | null }[] = [];
  page.on('requestfinished', (request) => {
    if (request === held) ended = true;
  });
  page.on('requestfailed', (request) => {
    if (request === held) {
      ended = true;
      failed = true;
    }
  });
  page.on('response', (response) => {
    if (response.request() === held) status = response.status();
  });
  await page.addInitScript(() => {
    window.open = () => null;
    (window as unknown as { entryDocumentId: string }).entryDocumentId = crypto.randomUUID();
  });
  await page.context().route('**/auth/login**', async (route) => {
    expect(new URL(route.request().url()).origin).toBe(baseURL);
    await route.fulfill({ contentType: 'text/html', body: '<p>Synthetic sign-in boundary</p>' });
  });
  await page.route('**/*', async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    if (url.origin !== baseURL) return route.fallback();
    const proof = verifiedEntryHeaders(request, subject);
    const reply = (body: unknown, status = 200) =>
      route.fulfill({
        status,
        headers: proof,
        contentType: 'application/json',
        body: JSON.stringify(body),
      });
    if (url.pathname === '/auth/session') {
      const returned = subject;
      const paused = holdSession;
      sessions.push(returned);
      if (paused) {
        holdSession = false;
        held = request;
        await gate;
      }
      try {
        await reply({
          authenticated: true,
          subject: returned,
          display_name: 'Local owner',
          visitor: false,
          capabilities: ['use_own_sandboxes', 'launch_triggers'],
        });
      } catch (cause) {
        if (!paused) throw cause;
        // A retired preflight may have been aborted before the synthetic reply.
      } finally {
        if (paused) handled = true;
      }
      return;
    }
    if (
      url.pathname === '/api/v2/triggers/public/public_contract/launches' &&
      request.method() === 'POST'
    ) {
      posts.push({
        owner: subject,
        key: request.headers()['idempotency-key'],
        body: request.postData(),
      });
      const first = posts.length === 1;
      if (mode === 'command' && first) {
        held = request;
        await gate;
        try {
          if (outcome === 'accepted') await reply(accepted, 201);
          else
            await reply(
              {
                code:
                  outcome === 'credential-refusal'
                    ? 'sandbox_credential_setup_required'
                    : outcome === 'unknown'
                      ? 'service_unavailable'
                      : outcome === 'fenced'
                        ? 'idempotency_recovery_required'
                        : 'forbidden',
                title: 'Synthetic refusal',
              },
              outcome === 'unknown' ? 503 : outcome === 'forbidden' ? 403 : 409,
            );
        } finally {
          handled = true;
        }
        return;
      }
      if (mode === 'preflight' && first)
        return reply({ code: 'sandbox_credential_setup_required', title: 'Sign in again' }, 409);
      if (mode === 'command' && posts.length === 2)
        return reply({ code: 'service_unavailable', title: 'Unknown result' }, 503);
      return reply(accepted, 201);
    }
    if (url.pathname === '/api/v1/triggers/public/public_contract')
      return reply({
        ...recorded<TriggerPreview>('triggers.json', 'triggers-preview'),
        always_review: true,
      });
    for (const launch of [previous, accepted]) {
      if (url.pathname === `/api/v1/launches/${launch.id}`) return reply(launch);
      if (url.pathname === `/api/v1/launches/${launch.id}/requirements`)
        return reply(launch.requirements);
      if (url.pathname === `/api/v1/agui/launches/${launch.id}/events`)
        return route.fulfill({
          contentType: 'text/event-stream',
          body: ': synthetic progress\n\n',
        });
    }
    return route.fallback();
  });
  const stored = () => page.evaluate(() => sessionStorage.getItem('cma:public-intent'));
  const packet = async () => JSON.parse((await stored()) ?? 'null') as EntryPacket | null;
  return {
    previous,
    accepted,
    posts,
    sessions,
    stored,
    packet,
    documentId: () =>
      page.evaluate(() => (window as unknown as { entryDocumentId: string }).entryDocumentId),
    setSubject: (next: string) => {
      subject = next;
    },
    holdSession: () => {
      holdSession = true;
    },
    waitHeld: () => expect.poll(() => held !== undefined).toBe(true),
    releaseGate: () => release(),
    release: async () => {
      release();
      await expect.poll(() => handled && ended).toBe(true);
      await page.evaluate(
        () => new Promise<void>((resolve) => requestAnimationFrame(() => resolve())),
      );
    },
    prepare: async () => {
      await page.goto(`/launches/${previous.id}`);
      await expect(page.getByRole('dialog')).toBeVisible();
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
      await expect(page).toHaveURL(`${baseURL}/t/public_contract`);
      await page.getByRole('button', { name: 'Try it', exact: true }).click();
      await expect.poll(() => posts.length).toBe(1);
      if (mode === 'preflight')
        await expect(page.getByRole('region', { name: 'CMA sign-in required' })).toBeVisible();
    },
    checkAccount: async () => {
      await page.getByRole('button', { name: 'Sign in to CMA', exact: true }).click();
      await page.getByRole('link', { name: 'Open CMA sign-in', exact: true }).click();
      await page.getByRole('button', { name: 'Check signed-in account', exact: true }).click();
      await expect(page.getByRole('button', { name: 'Try it', exact: true })).toBeEnabled();
    },
    finish: () => {
      if (mode === 'command') {
        expect(ended).toBe(true);
        expect(failed).toBe(false);
        expect(status).toBe(
          outcome === 'accepted'
            ? 201
            : outcome === 'unknown'
              ? 503
              : outcome === 'forbidden'
                ? 403
                : 409,
        );
      }
      expect(boundary.errors).toEqual([]);
      expect(boundary.unexpected).toEqual([]);
      expect(boundary.writes).toEqual([]);
    },
  };
}
