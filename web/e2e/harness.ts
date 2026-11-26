import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';
import {
  test as base,
  expect,
  type Browser,
  type BrowserContext,
  type Page,
  type Request,
  type TestInfo,
} from '@playwright/test';
import {
  fixtures,
  hooks,
  object,
  settings,
  type Role,
  type StagingFixtures,
  type Hooks,
  type TriggerFixture,
} from './environment';
import { Journal } from './journal';
import { settlesWithin, teardownRemaining, teardownWindow } from './teardown';
import { publicApiPath } from './public-api-path';
import {
  assertConnectionProof,
  connectionCaseTitles,
  identityDigest,
  type ConnectionProof,
} from '../../.github/scripts/connection-journey-evidence.mjs';
import {
  assertCredentialFree,
  assertPageSafe,
  credentialValues,
  isJsonMediaType,
} from './security';

export interface Launch {
  id: string;
  public_id: string;
  trigger_id: string;
  revision_digest: string;
  placement_id?: string;
  referrer_origin?: string;
  state: string;
  revision: number;
  event_head: number;
  first_message: string;
  failure?: { code: string; retryable: boolean };
  resolution: Record<'sandbox' | 'workspace' | 'agent', { state: string; id?: string }>;
}
export interface Command {
  method: string;
  path: string;
  bodyHash: string;
  key?: string;
}
export class Harness {
  readonly config = settings();
  readonly fixture = fixtures();
  readonly journal = new Journal(this.config.output, randomUUID());
  readonly forbidden = credentialValues(this.fixture.forbiddenValuesFile);
  readonly commands: Command[] = [];
  readonly violations: string[] = [];
  private readonly contexts: { context: BrowserContext; role: Role }[] = [];
  private readonly observations = new Set<Promise<void>>();
  private readonly requests = new Map<Request, { promise: Promise<void>; settle: () => void }>();
  private readonly opening = new Set<Promise<BrowserContext>>();
  private disposing = false;
  private disposal?: Promise<void>;
  private blocked?: string;
  private ambiguousReceipt = false;
  private identityFlow = false;
  private journeyProof?: Record<string, unknown>;
  private connectionProof?: ConnectionProof;
  constructor(
    readonly browser: Browser,
    readonly operators: Hooks,
    readonly caseId: string,
  ) {
    const visitor = this.fixture.visitors[caseId];
    if (visitor !== undefined) this.fixture.accounts.visitor = visitor;
  }
  async open(role: Role): Promise<Page> {
    if (this.disposing) throw new Error('Fixture teardown has started');
    if (role === 'visitor' && this.fixture.visitors[this.caseId] === undefined)
      throw new Error('This scenario has no isolated visitor fixture');
    const account = this.fixture.accounts[role];
    const opening = this.browser.newContext({
      storageState: account.storageState,
      serviceWorkers: 'block',
    });
    this.opening.add(opening);
    let context: BrowserContext;
    try {
      context = await opening;
    } finally {
      this.opening.delete(opening);
    }
    this.contexts.push({ context, role });
    if (this.disposing) {
      this.retain('context_open_during_teardown');
      await settlesWithin(context.close(), 2_000);
      throw new Error('Context opened during fixture teardown');
    }
    const session = await context.request.get(`${this.config.baseURL}/auth/session`);
    const identity = session.ok() ? object(await session.json()) : undefined;
    if (role === 'signup') {
      if (identity?.authenticated === true)
        throw new Error('J0 requires a fresh anonymous browser');
    } else if (identity?.subject !== account.subject || identity.authenticated !== true) {
      throw new Error(`Staging ${role} identity does not match its fixture`);
    }
    context.on('request', (request) => {
      const url = new URL(request.url());
      if (
        !this.disposing &&
        url.origin === this.config.baseURL &&
        url.pathname.startsWith('/api/')
      ) {
        let settle!: () => void;
        const promise = new Promise<void>((resolve) => {
          settle = resolve;
        });
        this.requests.set(request, { promise, settle });
      }
      if (
        url.origin === this.config.baseURL &&
        !['GET', 'HEAD', 'OPTIONS'].includes(request.method())
      )
        this.commands.push({
          method: request.method(),
          path: publicApiPath(url.pathname),
          bodyHash: createHash('sha256')
            .update(request.postData() ?? '')
            .digest('hex'),
          key: request.headers()['idempotency-key'],
        });
    });
    context.on('requestfailed', (request) => {
      if (this.requests.has(request) && !['GET', 'HEAD', 'OPTIONS'].includes(request.method()))
        this.ambiguousReceipt = true;
      this.finishRequest(request);
    });
    await context.route('**/*', async (route) => {
      const request = route.request();
      const target = new URL(request.url());
      if (
        this.disposing &&
        target.origin === this.config.baseURL &&
        target.pathname.startsWith('/api/')
      )
        return route.abort('blockedbyclient');
      if (['GET', 'HEAD', 'OPTIONS'].includes(request.method())) return route.continue();
      const url = new URL(request.url());
      if (
        url.pathname.includes('/admin') ||
        (url.origin !== this.config.baseURL && !this.identityFlow)
      ) {
        this.violations.push('out_of_scope_mutation');
        return route.abort('blockedbyclient');
      }
      if (url.origin === this.config.baseURL) {
        try {
          assertCredentialFree(request.postData() ?? '', this.forbidden);
        } catch {
          this.violations.push('credential_in_request_body');
          return route.abort('blockedbyclient');
        }
      }
      await route.continue();
    });
    context.on('response', (response) => {
      const url = new URL(response.url());
      if (url.origin !== this.config.baseURL || !url.pathname.startsWith('/api/')) return;
      const task = (async () => {
        if (!isJsonMediaType(response.headers()['content-type'] ?? '')) {
          if (!['GET', 'HEAD', 'OPTIONS'].includes(response.request().method()))
            this.ambiguousReceipt = true;
          return;
        }
        let text: string;
        try {
          text = await response.text();
        } catch (error) {
          if (
            response.request().method() === 'GET' &&
            response.request().failure()?.errorText === 'net::ERR_ABORTED'
          )
            return;
          throw error;
        }
        assertCredentialFree(text, this.forbidden);
        if (role === 'signup' && account.subject === '' && response.request().method() === 'POST') {
          const current = await context.request.get(`${this.config.baseURL}/auth/session`);
          const identity = object(await current.json());
          if (
            !current.ok() ||
            identity.authenticated !== true ||
            typeof identity.subject !== 'string'
          )
            throw new Error('New account identity cannot be bound to its Launch');
          if (
            Object.entries(this.fixture.accounts).some(
              ([name, value]) => name !== 'signup' && value.subject === identity.subject,
            )
          )
            throw new Error('J0 returned an existing fixture identity');
          account.subject = identity.subject;
          this.journal.record({
            action: 'external_account_cleanup_required',
            subject: account.subject,
          });
        }
        this.journal.observe(
          url.pathname,
          response.request().method(),
          response.status(),
          JSON.parse(text),
          account.subject,
        );
      })()
        .catch(() => {
          this.violations.push('response_inspection_failed');
        })
        .finally(() => {
          this.finishRequest(response.request());
          if (this.blocked) this.retain(this.blocked);
        });
      this.observations.add(task);
      void task.finally(() => this.observations.delete(task));
    });
    return context.newPage();
  }
  async identity<T>(operation: () => Promise<T>): Promise<T> {
    this.identityFlow = true;
    try {
      return await operation();
    } finally {
      this.identityFlow = false;
    }
  }
  proveJourney(proof: Record<string, unknown>) {
    if (!['J5', 'J6'].includes(this.caseId) || this.journeyProof)
      throw new Error('Only one current public journey proof is permitted per case');
    this.journeyProof = {
      ...proof,
      schema: 'cma-public-journey-v1',
      execution: 'configured_remote_origin',
      scope: 'current_image_inline',
      run: this.journal.run,
    };
    assertCredentialFree(JSON.stringify(this.journeyProof), this.forbidden);
  }
  async attachJourneyProof(info: TestInfo) {
    if (this.journeyProof)
      await info.attach('public-journey-proof', {
        body: JSON.stringify(this.journeyProof),
        contentType: 'application/json',
      });
    if (this.connectionProof)
      await info.attach('connection-journey-proof', {
        body: JSON.stringify(this.connectionProof),
        contentType: 'application/json',
      });
  }
  proveConnection(proof: unknown) {
    if (!Object.hasOwn(connectionCaseTitles, this.caseId) || this.connectionProof)
      throw new Error('Only one connection journey proof is permitted per connection case');
    const evidence = {
      ...object(proof),
      schema: 'cma-connection-journey-v1',
      execution: 'configured_remote_origin',
      scope: 'ordinary_service_connection',
      run: this.journal.run,
      caseId: this.caseId,
    };
    assertCredentialFree(JSON.stringify(evidence), this.forbidden);
    const verified = assertConnectionProof(evidence, {
      run: this.journal.run,
      caseId: this.caseId,
      providerRevision: this.fixture.provenance.nyxidCommit,
    });
    const service = this.fixture.connectionJourneys.services[verified.mode];
    if (
      verified.serviceDigest !== identityDigest(service.serviceId) ||
      verified.fixtureDigest !== service.fixtureDigest ||
      !this.journal.resources.some(
        (row) =>
          row.kind === 'launches' &&
          row.subject === this.fixture.accounts.visitor.subject &&
          identityDigest(row.subject) === verified.ownerDigest &&
          identityDigest({ type: 'launch', id: row.id }) === verified.targetDigest,
      )
    )
      throw new Error('Connection proof does not match the owned Launch and reviewed fixture');
    this.connectionProof = structuredClone(verified);
  }
  async get<T>(page: Page, path: string): Promise<T> {
    if (
      !publicApiPath(path).startsWith('/api/v1/') &&
      !path.startsWith('/api/image-manager/v1/') &&
      path !== '/auth/session'
    )
      throw new Error('Read path is out of scope');
    const response = await page.request.get(`${this.config.baseURL}${path}`);
    expect(response.ok(), 'Expected successful public read').toBe(true);
    const text = await response.text();
    assertCredentialFree(text, this.forbidden);
    return JSON.parse(text) as T;
  }
  async command(page: Page, method: 'POST' | 'DELETE', path: string, data?: unknown) {
    if (this.disposing) throw new Error('Fixture teardown has started');
    // Direct commands only affect resources already created by this run.
    if (
      !this.journal.resources.some(
        (row) =>
          path === `/api/v1/${row.kind}/${row.id}` ||
          path.startsWith(`/api/v1/${row.kind}/${row.id}/`),
      )
    )
      throw new Error('Direct mutation target is absent from the run-owned resource ledger');
    const response = await page.request.fetch(`${this.config.baseURL}${path}`, {
      method,
      data,
      headers: { 'Idempotency-Key': randomUUID(), Origin: this.config.baseURL },
    });
    expect(response.ok(), 'Expected accepted scoped resource command').toBe(true);
    return response;
  }
  async launch(page: Page, name: keyof StagingFixtures['triggers']) {
    return this.launchTarget(page, this.fixture.triggers[name]);
  }
  async launchTarget(page: Page, target: TriggerFixture) {
    await page.goto(`${this.config.baseURL}/t/${target.publicId}/${target.placementId}`);
    await page.locator('.public-actions button').click();
    return this.currentLaunch(page);
  }
  async currentLaunch(page: Page): Promise<Launch> {
    await page.waitForURL(/\/(launches|agents)\/[^/]+$/);
    await this.flush();
    const id = this.journal.resources.filter((row) => row.kind === 'launches').at(-1)?.id;
    if (!id) throw new Error('No admitted Launch receipt was observed');
    return this.readLaunch(page, id);
  }
  async readLaunch(page: Page, id: string) {
    const launch = await this.get<Launch>(page, `/api/v1/launches/${id}`);
    const owner = this.journal.resources.find(
      (row) => row.kind === 'launches' && row.id === id,
    )?.subject;
    if (owner) this.journal.observe(`/api/v1/launches/${id}`, 'GET', 200, launch, owner);
    return launch;
  }
  async ready(page: Page, id: string) {
    await expect
      .poll(async () => ['ready', 'completed'].includes((await this.readLaunch(page, id)).state), {
        timeout: 540_000,
        intervals: [1500, 3000, 5000],
      })
      .toBe(true);
    const launch = await this.readLaunch(page, id);
    expect(launch.resolution.agent.id).toBeTruthy();
    const destination = `${this.config.baseURL}/agents/${launch.resolution.agent.id}`;
    if (page.url() !== destination) {
      await this.flush();
      if (page.url() !== destination) await page.goto(destination);
    }
    await expect(page.locator('#agent-message')).toBeVisible();
    return launch;
  }
  async capture(page: Page, name: string) {
    await this.flush();
    expect(this.violations).toEqual([]);
    const wasDark = await page.evaluate(() => document.body.hasAttribute('data-ds-dark-theme'));
    const viewport = page.viewportSize();
    try {
      for (const theme of ['light', 'dark']) {
        await page.evaluate(
          (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
          theme === 'dark',
        );
        for (const viewport of [
          { width: 1440, height: 900 },
          { width: 390, height: 844 },
        ]) {
          await page.setViewportSize(viewport);
          await assertPageSafe(page, this.config.baseURL, this.forbidden);
          expect(
            await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth + 1),
          ).toBe(true);
          await page.screenshot({
            path: join(this.config.output, `${name}-${theme}-${viewport.width}.png`),
          });
        }
      }
    } finally {
      if (viewport) await page.setViewportSize(viewport);
      await page.evaluate(
        (dark) => document.body.toggleAttribute('data-ds-dark-theme', dark),
        wasDark,
      );
    }
  }
  async flush() {
    await Promise.all([...this.observations]);
  }
  private finishRequest(request: Request) {
    this.requests.get(request)?.settle();
    this.requests.delete(request);
  }
  private retain(reason: string) {
    this.journal.record({ action: 'cleanup_blocked', reason });
    this.blocked ??= reason;
    for (const resource of this.journal.resources)
      this.journal.record({ action: 'cleanup_blocked', ...resource, reason });
  }
  dispose(remainingMs = 10_000): Promise<void> {
    if (!this.disposal) {
      this.disposing = true;
      this.disposal = this.finish(remainingMs);
    }
    return this.disposal;
  }
  private async finish(remainingMs: number) {
    const { observationMs, releaseMs } = teardownWindow(remainingMs);
    const failures: unknown[] = [];
    try {
      const drained =
        observationMs > 0 &&
        (await settlesWithin(
          (async () => {
            await Promise.all([...this.opening]);
            await Promise.all([...this.requests.values()].map((request) => request.promise));
            await this.flush();
          })(),
          observationMs,
        ));
      if (!drained || this.blocked || this.ambiguousReceipt || this.violations.length) {
        this.retain('response_observation_incomplete');
      } else {
        const cleaned = new Set<string>();
        for (const { context, role } of [...this.contexts].reverse()) {
          try {
            const subject = this.fixture.accounts[role].subject;
            if (!cleaned.has(subject)) {
              await this.journal.cleanup(context.request, this.config.baseURL, subject);
              cleaned.add(subject);
            }
          } catch (error) {
            failures.push(error);
          }
        }
      }
    } catch (error) {
      failures.push(error);
      this.retain('response_observation_incomplete');
    } finally {
      const releases = Promise.allSettled(
        this.contexts.map(({ context }) => Promise.resolve().then(() => context.close())),
      );
      if (
        !(await settlesWithin(releases, releaseMs)) ||
        (await releases).some((row) => row.status === 'rejected')
      )
        this.retain('context_release_incomplete');
      for (const { role } of this.contexts) {
        if (role === 'signup' && this.fixture.accounts.signup.subject)
          this.journal.record({
            action: 'external_account_cleanup_required',
            subject: this.fixture.accounts.signup.subject,
          });
      }
    }
    if (this.blocked || failures.length)
      throw new Error('Scoped fixture cleanup is incomplete; consult cleanup.ndjson');
    expect(this.violations).toEqual([]);
  }
}
export const test = base.extend<{ harness: Harness }>({
  harness: async ({ browser }, runTest, info) => {
    const harness = new Harness(browser, await hooks(fixtures()), info.title.split(' ')[0]!);
    try {
      await runTest(harness);
    } finally {
      try {
        await harness.dispose(teardownRemaining(info));
      } finally {
        await harness.attachJourneyProof(info);
      }
    }
  },
});
export { expect };
