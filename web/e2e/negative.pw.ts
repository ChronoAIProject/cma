import { currentTriggerTarget } from './current-trigger-target';
import { randomUUID } from 'node:crypto';
import { test, expect } from './harness';
import { object } from './environment';
import { assertPageSafe } from './security';

test('SECURITY-negative visitor cannot create Sandboxes, profiles or Triggers', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const identity = await h.get<{ visitor: boolean }>(page, '/auth/session');
  expect(identity.visitor).toBe(true);
  const profile = h.fixture.profiles.sandbox;
  const requests: { path: string; data: unknown }[] = [
    {
      path: '/api/v1/sandboxes',
      data: {
        sandbox_profile: { id: profile.id, revision: profile.revision },
        name: 'Scoped refusal probe',
      },
    },
    {
      path: '/api/v1/agent-profiles',
      data: {
        name: 'Scoped refusal probe',
        content: {
          thinking_effort: 'medium',
          permission_level: 'guarded',
          skills: [],
          system_prompt: '',
        },
      },
    },
    {
      path: '/api/v1/triggers',
      data: {
        content: {
          presentation: {
            title: 'Scoped refusal probe',
            description: '',
            button_label: 'open_in_cma',
            theme: 'auto',
          },
          target: Object.fromEntries(
            Object.entries(h.fixture.profiles).map(([kind, value]) => [
              `${kind}_profile`,
              { id: value.id, revision: value.revision },
            ]),
          ),
          reuse: { sandbox: 'always_new', workspace: 'always_new', agent: 'always_new' },
          first_message: '',
          consent: { always_review: true },
        },
      },
    },
  ];
  const selected = h.fixture.publicJourneys.owners.owner.image;
  const image = { id: selected.id, version: selected.version, reference: selected.reference };
  requests.push(
    {
      path: '/api/v1/agents',
      data: {
        name: 'Scoped current refusal probe',
        initial_message: 'Reply once.',
        image: { id: image.id, version: image.version },
        profile: {
          model: h.fixture.publicJourneys.owners.owner.model,
          reasoning_effort: 'high',
          permission_mode: 'guarded',
          skills: [],
          system_prompt: '',
          permissions: { services: h.fixture.baselineServices },
        },
      },
    },
    {
      path: '/api/v1/triggers',
      data: {
        content: {
          presentation: {
            title: 'Scoped inline refusal probe',
            description: '',
            button_label: 'open_in_cma',
            theme: 'auto',
          },
          target: {
            inline: currentTriggerTarget(h.fixture, 'owner'),
          },
          reuse: { sandbox: 'always_new', workspace: 'always_new', agent: 'always_new' },
          first_message: 'Reply once.',
          consent: { always_review: true },
        },
      },
    },
  );
  for (const probe of requests) {
    const response = await page.request.post(`${h.config.baseURL}${probe.path}`, {
      data: probe.data,
      headers: { Origin: h.config.baseURL, 'Idempotency-Key': randomUUID() },
    });
    const body = object(await response.json());
    h.journal.observe(
      probe.path,
      'POST',
      response.status(),
      body,
      h.fixture.accounts.visitor.subject,
    );
    expect(response.status(), 'The capability boundary must refuse before provisioning').toBe(403);
    expect(typeof body.code).toBe('string');
  }
  await page.goto(`${h.config.baseURL}/t/${h.fixture.triggers.fresh.publicId}`);
  await assertPageSafe(page, h.config.baseURL, h.forbidden);
});

test('SECURITY-csrf cross-site form POST is refused without admitting a Launch', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const target = h.fixture.triggers.fresh;
  const response = await page.request.post(
    `${h.config.baseURL}/api/v1/triggers/public/${target.publicId}/launches`,
    {
      form: { start_if_ready: 'true', placement_id: target.placementId },
      headers: {
        Origin: new URL(h.fixture.embeds.html.url).origin,
        'Sec-Fetch-Site': 'cross-site',
        'Idempotency-Key': randomUUID(),
      },
    },
  );
  const body = object(await response.json());
  h.journal.observe(
    `/api/v1/triggers/public/${target.publicId}/launches`,
    'POST',
    response.status(),
    body,
    h.fixture.accounts.visitor.subject,
  );
  expect(response.status()).toBe(403);
  expect(typeof body.code).toBe('string');
});

test('G-T3 fifty explicit clicks admit at most five Launches and expose resource_quota reset', async ({
  harness: h,
}) => {
  const page = await h.open('visitor');
  const target = h.fixture.triggers.quota;
  let accepted = 0;
  let quotaSeen = false;
  for (let click = 0; click < 50; click++) {
    await page.goto(`${h.config.baseURL}/t/${target.publicId}/${target.placementId}`);
    const result = page.waitForResponse(
      (response) =>
        new URL(response.url()).pathname.endsWith(`/public/${target.publicId}/launches`) &&
        response.request().method() === 'POST',
    );
    await page.locator('.public-actions button').click();
    const response = await result;
    const body = object(await response.json());
    if (response.ok()) accepted++;
    else {
      expect(response.status()).toBe(429);
      expect(body.code).toBe('resource_quota');
      expect(Number.isFinite(Date.parse(String(body.reset_at)))).toBe(true);
      expect(Number(response.headers()['retry-after'])).toBeGreaterThan(0);
      quotaSeen = true;
    }
    await h.flush();
  }
  expect(accepted).toBeGreaterThan(0);
  expect(accepted).toBeLessThanOrEqual(5);
  expect(quotaSeen).toBe(true);
  expect(h.commands.filter((row) => row.path.endsWith('/launches'))).toHaveLength(50);
  await h.capture(page, 'g-t3-quota');
});
