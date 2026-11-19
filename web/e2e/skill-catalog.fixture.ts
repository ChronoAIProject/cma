import { expect, type Page, type Route } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';

export const privateBody = '<html>synthetic-private-body</html>';
export const skill = { id: 'review', name: 'Review', versions: ['1.2', '1.1'] };
export const secondSkill = { id: 'search', name: 'Search', versions: ['2.0'] };
export const problem = (route: Route, status: number, code: string) =>
  route.fulfill({
    status,
    headers: { 'Retry-After': '1' },
    json: { status, code, detail: privateBody, message: privateBody, request_id: 'local-request' },
  });

export class SkillCatalogFixture extends LocalStartup {
  readonly catalogReads: { query: string | null; cursor: string | null }[] = [];
  readonly installs: { key?: string; body: unknown }[] = [];
  readonly consoleMessages: string[] = [];
  installedReads = 0;
  installState: 'pending' | 'installed' | undefined;
  loseInstallResponse = false;
  catalog: (route: Route) => Promise<void> = (route) => route.fulfill({ json: { items: [skill] } });

  override async route(page: Page, baseURL: string) {
    await super.route(page, baseURL);
    page.on('console', (message) => this.consoleMessages.push(message.text()));
    await page.route('**/api/v1/agents/agt_contract/skills', (route) => {
      if (route.request().method() === 'POST') {
        this.installs.push({
          key: route.request().headers()['idempotency-key'],
          body: route.request().postDataJSON(),
        });
        if (this.loseInstallResponse) {
          this.loseInstallResponse = false;
          return route.abort('connectionfailed');
        }
        this.installState = 'pending';
      } else this.installedReads += 1;
      return route.fulfill({
        status: route.request().method() === 'POST' ? 202 : 200,
        json: {
          items: [
            {
              id: 'saved',
              name: 'Saved skill',
              version: '1.0',
              source: 'ornn',
              status: 'installed',
            },
            ...(this.installState
              ? [{ id: skill.id, version: '1.2', source: 'ornn', status: this.installState }]
              : []),
          ],
          pending: this.installState === 'pending',
          can_install: this.installState !== 'pending',
        },
      });
    });
    await page.route('**/api/v1/profile-authoring/skills*', (route) => {
      expect(route.request().method()).toBe('GET');
      expect(route.request().headers()['idempotency-key']).toBeUndefined();
      const params = new URL(route.request().url()).searchParams;
      this.catalogReads.push({ query: params.get('q'), cursor: params.get('cursor') });
      return this.catalog(route);
    });
    await page.route('**/api/v1/agents/agt_contract/transcript', (route) =>
      route.fulfill({
        json: {
          agent_id: 'agt_contract',
          through_seq: 4,
          turns: [
            {
              turn_sequence: 1,
              started_at: '2026-10-20T16:30:00Z',
              owner_message: 'Review the code.',
              agent_messages: ['Review complete.'],
              messages: [],
              outcome: { type: 'completed', at: '2026-10-20T16:31:00Z' },
            },
          ],
        },
      }),
    );
  }

  assertReadBoundary() {
    expect(this.writes).toEqual([]);
    expect(this.unexpected).toEqual([]);
    expect(this.errors).toEqual([]);
    expect(this.consoleMessages.join('\n')).not.toContain('synthetic-private-body');
  }
}

export async function openSkills(page: Page) {
  await page.getByRole('button', { name: 'Show Agent details' }).click();
  const details = page.getByRole('complementary', { name: 'Agent details' });
  const summary = details.getByText('Add skills', { exact: true });
  if (!(await summary.evaluate((element) => element.closest('details')?.open)))
    await summary.click();
  return details;
}
