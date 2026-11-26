import { expect, type Locator, type Page, type TestInfo } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { environments, resourceRow, insideViewport } from './resource-lifecycle.fixture';

export class RedesignDeletionFixture {
  readonly boundary = new LocalStartup();
  readonly commands: { path: string; key?: string; body: string | null }[] = [];
  readonly reads: string[] = [];
  readFailure = 0;
  loseResponse = false;
  pendingReads = 0;

  constructor() {
    this.boundary.agent = { ...this.boundary.agent, title: 'Sample task', status: 'stopped' };
    this.boundary.workspace = {
      ...this.boundary.workspace,
      name: 'sample-project',
      agents: [
        { ...this.boundary.workspace.agents[0]!, title: 'Sample task', status: 'stopped' },
        {
          ...this.boundary.workspace.agents[0]!,
          id: 'agt_sample_notes',
          title: 'Sample notes',
          status: 'stopped',
        },
      ],
      agent_count: 2,
    };
    this.boundary.sandbox = {
      ...this.boundary.sandbox,
      name: 'Example environment',
      workspaces: [this.boundary.workspace],
      agent_count: 2,
      lifecycle: { pause: { allowed: true }, revive: { allowed: false, reason: 'not_paused' } },
    };
  }

  async route(page: Page, baseURL: string, accepted?: boolean) {
    await page.clock.install({ time: new Date('2026-11-22T10:00:00Z') });
    await page.clock.pauseAt(new Date('2026-11-22T10:00:01Z'));
    await page.emulateMedia({ reducedMotion: 'reduce' });
    await this.boundary.route(page, baseURL);
    await page.route('**/api/v1/agents/agt_sample_notes/channel-usage', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_sample_notes',
          channel_usage: {
            active: false,
            has_history: false,
            first_used_at: null,
            last_used_at: null,
          },
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_sample_notes/provenance', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_sample_notes',
          created_at: this.boundary.agent.created_at,
          creation_origin: 'legacy_unknown',
        },
      }),
    );
    if (accepted !== undefined)
      await page.addInitScript(
        ({ sandboxId, accepted }) => {
          localStorage.setItem(
            'cma:deletion:local-owner',
            JSON.stringify({
              action: { kind: 'delete_sandbox', sandboxId },
              key: 'synthetic-original-delete-reference',
              accepted,
            }),
          );
        },
        { sandboxId: this.boundary.sandbox.id, accepted },
      );
    await page.route(`**/api/v1/sandboxes/${this.boundary.sandbox.id}**`, async (route) => {
      const request = route.request();
      const path = new URL(request.url()).pathname;
      if (request.method() === 'DELETE') {
        this.commands.push({
          path,
          key: request.headers()['idempotency-key'],
          body: request.postData(),
        });
        if (this.loseResponse) return route.abort('connectionfailed');
        this.boundary.sandbox = { ...this.boundary.sandbox, status: 'retiring' };
        return route.fulfill({ status: 202, json: this.boundary.sandbox });
      }
      if (
        request.method() !== 'GET' ||
        ![
          `/api/v1/sandboxes/${this.boundary.sandbox.id}`,
          `/api/v1/sandboxes/${this.boundary.sandbox.id}/events`,
        ].includes(path)
      )
        return route.fallback();
      this.reads.push(path);
      this.pendingReads++;
      try {
        if (this.readFailure)
          return await route.fulfill({
            status: this.readFailure,
            json: { code: 'synthetic_unavailable', detail: 'Synthetic status unavailable' },
          });
        return await route.fulfill({ json: path.endsWith('/events') ? [] : this.boundary.sandbox });
      } finally {
        this.pendingReads--;
      }
    });
  }

  async openConfirmation(page: Page) {
    await page.goto('/agent-manager');
    await environments(page);
    const target = resourceRow(page, 'sandbox', this.boundary.sandbox.id);
    await target.hover();
    await target.getByRole('button', { name: /^Options for / }).click();
    await page.getByRole('menuitem', { name: 'Delete', exact: true }).click();
    const dialog = page.getByRole('dialog', { name: 'Delete Sandbox', exact: true });
    await expect(dialog.getByText('Sample notes', { exact: true })).toBeVisible();
    return dialog;
  }

  async openStatus(page: Page) {
    await page.goto('/agent-manager');
    await page.getByRole('button', { name: 'View deletion status', exact: true }).click();
    await page
      .getByRole('button', { name: 'View Sandbox deletion status (1)', exact: true })
      .click();
    const dialog = page.getByRole('dialog', { name: 'Deleting Sandbox', exact: true });
    await expect(dialog).toBeVisible();
    return dialog;
  }

  async settled() {
    await expect.poll(() => this.pendingReads).toBe(0);
  }
  assertNoUnexpected() {
    expect(this.boundary.writes).toEqual([]);
    expect(this.boundary.unexpected).toEqual([]);
    expect(this.boundary.errors).toEqual([]);
  }
}

export async function captureDeletion(
  page: Page,
  dialog: Locator,
  info: TestInfo,
  name: string,
  theme: string,
  width: number,
  zoom: number,
) {
  await insideViewport(dialog);
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1)).toBe(
    true,
  );
  const overflow = await dialog.evaluate(
    (element) => element.scrollWidth > element.clientWidth + 1,
  );
  expect(overflow).toBe(false);
  await page.screenshot({ path: info.outputPath(`${name}-${theme}-${width}-${zoom}.png`) });
  await info.attach('screen-context', {
    contentType: 'application/json',
    body: Buffer.from(
      JSON.stringify({
        screen: name,
        theme,
        viewport: page.viewportSize(),
        zoom,
        entry: new URL(page.url()).pathname,
        fixture: 'synthetic local deletion',
        host: 'existing dialog and production shell',
      }),
    ),
  });
}
