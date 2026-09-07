import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';
import {
  expect,
  test as base,
  type Browser,
  type BrowserContext,
  type Page,
} from '@playwright/test';
import { identifier, object } from './environment';
import { Journal } from './journal';
import {
  assertCredentialFree,
  assertPageSafe,
  credentialValues,
  isJsonMediaType,
} from './security';
import {
  assertOwnerCase,
  assertOwnerSession,
  ownerFixture,
  ownerSettings,
} from './owner-environment';
import { ownerRead } from './owner-preflight';

export function ownerMutationAllowed(path: string, method: string, owned: Journal['resources']) {
  return (
    method === 'POST' &&
    owned.some(
      (row) =>
        row.kind === 'sandboxes' &&
        [`/api/v1/sandboxes/${row.id}/pause`, `/api/v1/sandboxes/${row.id}/revive`].includes(path),
    )
  );
}

export class OwnerHarness {
  readonly config = ownerSettings();
  readonly fixture = ownerFixture(this.config);
  readonly forbidden = credentialValues(this.fixture.forbiddenValuesFile);
  readonly run = randomUUID();
  readonly journal = new Journal(this.config.output, this.run);
  readonly commands: { path: string; method: string; key?: string; bodyHash: string }[] = [];
  readonly violations: string[] = [];
  private readonly contexts: BrowserContext[] = [];
  private readonly observations = new Set<Promise<void>>();
  private unresolvedCreation = false;
  constructor(private readonly browser: Browser) {}

  async read<T>(page: Page, path: string): Promise<T> {
    return (await ownerRead(page.request, this.config.baseURL, path, this.forbidden)) as T;
  }
  private async account(context: BrowserContext) {
    const session = object(
      await ownerRead(context.request, this.config.baseURL, '/auth/session', this.forbidden),
    );
    assertOwnerSession(session, this.fixture.subject);
    return session;
  }
  async open(): Promise<Page> {
    const context = await this.browser.newContext({
      storageState: this.fixture.storageState,
      serviceWorkers: 'block',
    });
    this.contexts.push(context);
    const session = await this.account(context);
    let avatarURL: string | undefined;
    if (typeof session.avatar_url === 'string') {
      try {
        const avatar = new URL(session.avatar_url);
        if (avatar.protocol === 'https:' && !avatar.username && !avatar.password)
          avatarURL = avatar.href;
      } catch {
        // Missing or malformed cosmetic metadata cannot broaden request scope.
      }
    }
    await context.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== this.config.baseURL) {
        if (
          url.href !== avatarURL ||
          request.method() !== 'GET' ||
          request.resourceType() !== 'image'
        )
          this.violations.push('out_of_scope_browser_request');
        return route.abort('blockedbyclient');
      }
      if (
        url.pathname.includes('/admin') ||
        (url.pathname.startsWith('/auth/') && url.pathname !== '/auth/session')
      ) {
        this.violations.push('out_of_scope_browser_request');
        return route.abort('blockedbyclient');
      }
      if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method())) {
        if (!ownerMutationAllowed(url.pathname, request.method(), this.journal.resources)) {
          this.violations.push('out_of_scope_browser_mutation');
          return route.abort('blockedbyclient');
        }
        try {
          await this.account(context);
          assertCredentialFree(request.postData() ?? '', this.forbidden);
        } catch {
          this.violations.push('browser_mutation_identity_or_credential_check_failed');
          return route.abort('blockedbyclient');
        }
        const command = this.recordCommand(
          url.pathname,
          request.method(),
          request.postData() ?? '',
          request.headers()['idempotency-key'],
        );
        this.journal.record({ action: 'browser_command', ...command });
      }
      await route.continue();
    });
    context.on('response', (response) => {
      const url = new URL(response.url());
      if (
        url.origin !== this.config.baseURL ||
        !url.pathname.startsWith('/api/') ||
        !isJsonMediaType(response.headers()['content-type'] ?? '')
      )
        return;
      const observation = response
        .text()
        .then((text) => {
          assertCredentialFree(text, this.forbidden);
        })
        .catch(() => {
          this.violations.push('response_inspection_failed');
        });
      this.observations.add(observation);
      void observation.finally(() => this.observations.delete(observation));
    });
    return context.newPage();
  }
  private recordCommand(path: string, method: string, body: string, key?: string) {
    const command = {
      path,
      method,
      key,
      bodyHash: createHash('sha256').update(body).digest('hex'),
    };
    this.commands.push(command);
    return command;
  }
  async create(page: Page, kind: 'sandbox' | 'workspace' | 'agent', parentId?: string) {
    const plural =
      kind === 'sandbox' ? 'sandboxes' : kind === 'workspace' ? 'workspaces' : 'agents';
    if (this.unresolvedCreation || this.journal.resources.some((row) => row.kind === plural))
      throw new Error('E2E owner baseline refuses additional or unresolved resource creation');
    const parent = kind === 'workspace' ? 'sandboxes' : 'workspaces';
    if (
      kind !== 'sandbox' &&
      !this.journal.resources.some((row) => row.kind === parent && row.id === parentId)
    )
      throw new Error('E2E owner creation parent is not owned by this run');
    await this.account(page.context());
    const pin = this.fixture.profiles[kind];
    const path =
      kind === 'sandbox' ? '/api/v1/sandboxes' : `/api/v1/${parent}/${parentId}/${plural}`;
    const data = {
      [`${kind}_profile`]: { id: pin.id, revision: pin.revision },
      ...(kind === 'sandbox' ? { name: `Owner baseline ${this.run}` } : {}),
      ...(kind === 'agent'
        ? {
            title: 'Owner-only baseline',
            first_message: 'Reply once with your working directory. Do not modify files.',
          }
        : {}),
    };
    const body = JSON.stringify(data);
    assertCredentialFree(body, this.forbidden);
    const key = randomUUID();
    const command = this.recordCommand(path, 'POST', body, key);
    this.journal.record({ action: 'creation_requested', ...command });
    this.unresolvedCreation = true;
    const response = await page.request.post(`${this.config.baseURL}${path}`, {
      data,
      maxRedirects: 0,
      timeout: 30_000,
      headers: { Origin: this.config.baseURL, 'Idempotency-Key': key },
    });
    const text = await response.text();
    assertCredentialFree(text, this.forbidden);
    const created = object(JSON.parse(text), 'owner creation response');
    if (response.status() !== 201) {
      if (
        response.status() >= 400 &&
        response.status() < 500 &&
        response.status() !== 408 &&
        !['idempotency_in_flight', 'idempotency_recovery_required'].includes(String(created.code))
      )
        this.unresolvedCreation = false;
      throw new Error('E2E owner resource creation was not confirmed; consult the owner journal');
    }
    this.journal.observe(path, 'POST', 201, created, this.fixture.subject);
    this.unresolvedCreation = false;
    const id = identifier(created.id, 'owner-created resource ID');
    if (kind === 'workspace') expect(created.sandbox_id).toBe(parentId);
    if (kind === 'agent') expect(object(created.workspace).id).toBe(parentId);
    return id;
  }
  async capture(page: Page, name: string) {
    await Promise.all([...this.observations]);
    expect(this.violations).toEqual([]);
    const dark = await page.evaluate(() => document.body.hasAttribute('data-ds-dark-theme'));
    try {
      for (const theme of ['light', 'dark']) {
        await page.evaluate(
          (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
          theme === 'dark',
        );
        for (const width of [1440, 390]) {
          await page.setViewportSize({ width, height: 900 });
          await assertPageSafe(page, this.config.baseURL, this.forbidden);
          expect(
            await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
          ).toBe(true);
          await page.screenshot({
            path: join(this.config.output, `owner-${name}-${theme}-${width}.png`),
          });
        }
      }
    } finally {
      await page.evaluate(
        (value) => document.body.toggleAttribute('data-ds-dark-theme', value),
        dark,
      );
    }
  }
  async close(page: Page) {
    await Promise.all([...this.observations]);
    await page.context().close();
    this.contexts.splice(this.contexts.indexOf(page.context()), 1);
  }
  async dispose() {
    await Promise.all([...this.observations]);
    try {
      if (this.unresolvedCreation) {
        this.journal.record({ action: 'cleanup_blocked', reason: 'unresolved_owner_creation' });
        throw new Error(
          'E2E owner cleanup retained: an admission is unresolved; reconcile its command first',
        );
      }
      const context = this.contexts.at(-1);
      if (context !== undefined)
        await this.journal.cleanup(context.request, this.config.baseURL, this.fixture.subject);
      else if (this.journal.resources.length)
        throw new Error('E2E owner cleanup has no authenticated browser context');
    } finally {
      for (const context of this.contexts) await context.close();
    }
    expect(this.violations).toEqual([]);
  }
}

export const test = base.extend<{ owner: OwnerHarness }>({
  owner: async ({ browser }, runTest, info) => {
    assertOwnerCase(info.title);
    const owner = new OwnerHarness(browser);
    try {
      await runTest(owner);
    } finally {
      await owner.dispose();
    }
  },
});
export { expect };
