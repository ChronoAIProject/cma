import { expect, test, type Locator, type Page } from '@playwright/test';
import { cmaSession, gatewaySession, teamChannel } from '../build/fixtures/eventGateway';
import { LocalStartup, localTime } from './agent-startup-fixtures';
import { LocalUsageMetrics } from './usage-metrics.fixture';

test('LOCAL sign-out hides protected views in sibling tabs and fences late identity reads', async ({
  context,
  page: sender,
  baseURL,
}) => {
  let signedOut = false;
  let identityStarted = false;
  let finishIdentity!: () => void;
  const heldIdentity = new Promise<void>((resolve) => (finishIdentity = resolve));
  const oldSession = {
    ...cmaSession,
    subject: 'local-owner',
    capabilities: ['use_own_sandboxes', 'create_sandbox'],
  };
  const boundaries: LocalStartup[] = [];
  async function install(page: Page) {
    const boundary = new LocalStartup();
    boundaries.push(boundary);
    await boundary.route(page, baseURL!);
    await page.route(`${baseURL}/auth/session`, async (route) => {
      if (signedOut) return route.fulfill({ status: 401, json: { authenticated: false } });
      if (page === agent && holdIdentity) {
        holdIdentity = false;
        identityStarted = true;
        await heldIdentity;
      }
      return route.fulfill({ json: oldSession });
    });
  }

  const agent = await context.newPage();
  const usage = await context.newPage();
  const files = await context.newPage();
  const shared = await context.newPage();
  const gateway = await context.newPage();
  let holdIdentity = false;
  try {
    await context.addInitScript(() => {
      Object.defineProperty(window, 'WebGL2RenderingContext', { value: undefined });
    });
    await Promise.all([sender, agent, usage, files, shared, gateway].map(install));
    await agent.route('**/api/v1/agents/agt_contract/transcript', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_contract',
          through_seq: 3,
          turns: [
            {
              turn_sequence: 1,
              started_at: localTime,
              owner_message: 'A saved private message',
              agent_messages: [],
              messages: [
                {
                  sequence: 2,
                  role: 'user',
                  content: 'A saved private message',
                  appended_at: localTime,
                },
              ],
              outcome: { type: 'completed', at: localTime },
            },
          ],
        },
      }),
    );

    for (const page of [sender, gateway]) {
      await page.route('**/api/event-gateway/**', async (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path.endsWith('/auth/session'))
          return route.fulfill({
            status: signedOut ? 401 : 200,
            json: signedOut ? { code: 'unauthorized' } : gatewaySession,
          });
        if (path.endsWith('/channels'))
          return route.fulfill({ json: { items: [teamChannel], next_cursor: null } });
        return route.fulfill({ json: { items: [] } });
      });
    }
    await sender.route(`${baseURL}/auth/logout`, async (route) => {
      signedOut = true;
      await route.fulfill({ status: 303, headers: { location: '/' } });
    });
    // Model a suspended page that misses the live storage event.
    await shared.addInitScript(() => {
      Object.defineProperty(window, 'BroadcastChannel', { value: undefined });
      window.addEventListener('storage', (event) => event.stopImmediatePropagation(), true);
    });
    await files.route('**/api/v1/agents/agt_contract/files/tree**', (route) =>
      route.fulfill({
        json: {
          path: new URL(route.request().url()).searchParams.get('path') ?? '',
          entries: [
            {
              path: 'private-fixture.md',
              name: 'private-fixture.md',
              kind: 'file',
              size_bytes: 10,
              hidden: false,
            },
          ],
        },
      }),
    );
    await shared.route('**/api/v1/agents/agt_contract', (route) =>
      route.fulfill({
        json: {
          access: 'shared',
          id: 'agt_contract',
          title: 'Private shared fixture',
          status: 'ready',
          created_at: localTime,
          updated_at: localTime,
        },
      }),
    );
    await shared.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
      route.fulfill({
        json: {
          capabilities: ['view_details', 'view_history'],
          policy_revision: 1,
          revalidate_at: null,
          owner: { kind: 'person', is_current_person: false },
          shared_by: {
            subject: 'another-fixture-owner',
            display_name: 'Fixture owner',
            avatar_url: null,
          },
        },
      }),
    );
    await shared.route('**/api/v1/sandboxes', (route) => route.fulfill({ json: { items: [] } }));
    await shared.route('**/api/v2/agents/agt_contract/events/head', (route) =>
      route.fulfill({ status: 404, json: { code: 'not_found' } }),
    );
    const metrics = new LocalUsageMetrics();
    await metrics.route(usage);
    await agent.goto('/agents/agt_contract');
    await usage.goto('/usage');
    await files.goto('/agents/agt_contract');
    await shared.goto('/agents/agt_contract');
    await gateway.goto('/event-gateway');
    await sender.goto('/event-gateway');
    await files.getByRole('button', { name: 'Show Agent details', exact: true }).click();
    await files.getByRole('button', { name: 'Browse files' }).click();
    await files.getByRole('button', { name: 'Sandbox root /home/', exact: true }).click();
    const composer = agent.getByRole('textbox', { name: 'Message', exact: true });
    await composer.fill('Unsubmitted private message');
    const history = agent.getByText('A saved private message', { exact: true });
    await expect(history).toBeVisible();
    const protectedViews: Locator[] = [
      composer,
      usage.getByRole('button', { name: 'Research Sandbox', exact: true }),
      files.getByRole('button', { name: 'private-fixture.md', exact: true }),
      shared.getByRole('heading', { name: 'Private shared fixture', exact: true }),
      gateway.getByRole('treeitem', { name: new RegExp(`^${teamChannel.definition.name}`) }),
    ];
    for (const view of protectedViews) await expect(view).toBeVisible();
    holdIdentity = true;
    await agent.evaluate(() => window.dispatchEvent(new Event('online')));
    await expect.poll(() => identityStarted).toBe(true);
    const lateIdentity = agent.waitForEvent('requestfinished', {
      predicate: (request) => request.url() === `${baseURL}/auth/session`,
    });
    await sender.getByRole('button', { name: /Account menu for/ }).click();
    await sender.getByRole('menuitem', { name: 'Sign out', exact: true }).click();
    await expect.poll(() => signedOut).toBe(true);
    for (const [index, view] of protectedViews.entries())
      if (index !== 3) await expect(view).toHaveCount(0);

    await expect(protectedViews[3]!).toBeVisible();
    await shared.evaluate(() =>
      window.dispatchEvent(new PageTransitionEvent('pageshow', { persisted: true })),
    );
    await expect(protectedViews[3]!).toHaveCount(0);
    finishIdentity();
    await lateIdentity;
    await expect(agent.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await expect(composer).toHaveCount(0);
    await expect(history).toHaveCount(0);
    await expect(agent.getByRole('button', { name: 'Send message', exact: true })).toHaveCount(0);

    await usage.reload();
    await expect(usage.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await agent.goto('/agent-manager');
    await agent.goBack();
    await expect(agent.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await agent.goForward();
    await expect(agent.getByRole('button', { name: 'Sign in', exact: true })).toBeVisible();
    await expect(composer).toHaveCount(0);
    const marker = await agent.evaluate(() => localStorage.getItem('cma.auth.sign-out'));
    expect(marker).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/);
    for (const boundary of boundaries) {
      expect(boundary.writes).toEqual([]);
      expect(boundary.errors).toEqual([]);
    }
  } finally {
    finishIdentity();
  }
});
