import { expect, type Page, type Request, type TestInfo } from '@playwright/test';
import { writeFile } from 'node:fs/promises';
import type { AgentEnvironment } from '../src/api-client/index';
import { LocalAgentV2, binding, cursor } from './agent-v2.fixture';

export type Dependency =
  'identity' | 'organizations' | 'workspace' | 'models' | 'transcript' | 'environment' | 'files';
const dependencies = new Map<string, Dependency>([
  ['/auth/session', 'identity'],
  ['/api/v1/organizations', 'organizations'],
  ['/api/v1/workspaces/wks_route', 'workspace'],
  ['/api/v1/agents/agt_contract/models', 'models'],
  ['/api/v1/agents/agt_contract/transcript', 'transcript'],
  ['/api/v2/agents/agt_contract/events/snapshot', 'transcript'],
  ['/api/v2/agents/agt_contract/events/head', 'transcript'],
  ['/api/v1/agents/agt_contract/environment', 'environment'],
  ['/api/v1/agents/agt_contract/files/tree', 'files'],
  ['/api/v1/agents/agt_contract/files/content', 'files'],
]);
const unsupported = {
  state: 'unsupported' as const,
  reason: 'not_supported_by_contract',
  scope: 'none' as const,
  writable: false,
};
const filePath = 'cma/project/.worktrees/agt_contract/notes.txt';
export const fileText = 'Synthetic recovery notes';
export const operationText = 'synthetic recorded output';

export class ComputerRecovery {
  readonly base = new LocalAgentV2();
  readonly faults = new Map<Dependency, number>();
  readonly requests: {
    dependency: Dependency;
    path: string;
    status: number;
    phase: string;
    subject: string;
    clockMs: number;
    elapsedMs: number;
  }[] = [];
  readonly writes: string[] = [];
  readonly logins: string[] = [];
  readonly cancelled: string[] = [];
  connectionChecks = 0;
  private readonly holds = new Map<Dependency, Promise<void>>();
  private readonly pending = new Set<Request>();
  private readonly releases: (() => void)[] = [];
  private readonly started = performance.now();
  phase = 'healthy';
  clockMs = 0;
  denied = false;
  environment: AgentEnvironment = {
    agent_id: 'agt_contract',
    policy_revision: 1,
    runtime_incarnation: 'b'.repeat(64),
    revalidate_at: null,
    files: {
      state: 'available',
      reason: 'authorized_mounted_home',
      scope: 'mounted_home',
      writable: true,
    },
    operation_output: {
      state: 'available',
      reason: 'recorded_output_only',
      scope: 'agent_turn_history',
      writable: false,
    },
    browser: { ...unsupported },
    replay: unsupported,
    control: unsupported,
  };

  constructor() {
    this.base.append({ type: 'turn_started', turn_sequence: 1 }, binding);
    this.base.append({
      type: 'tool_started',
      call_id: 'recovery',
      title: 'printf synthetic-output',
      kind: 'execute',
      status: 'in_progress',
    });
    this.base.append({
      type: 'tool_updated',
      call_id: 'recovery',
      title: null,
      status: 'completed',
      output: { text: operationText, truncated: false },
    });
    this.base.append({ type: 'turn_completed', turn_sequence: 1 });
  }

  count(dependency: Dependency, status?: number) {
    return this.requests.filter(
      (read) => read.dependency === dependency && (status === undefined || read.status === status),
    ).length;
  }

  hold(dependency: Dependency) {
    let release!: () => void;
    this.holds.set(
      dependency,
      new Promise<void>((resolve) => {
        release = resolve;
      }),
    );
    const finish = () => {
      this.holds.delete(dependency);
      release();
    };
    this.releases.push(finish);
    return finish;
  }

  async advance(page: Page, ms: number) {
    // Keep healthy polling responses ahead of lifecycle expiry while virtual time advances.
    await this.settle();
    while (ms > 0) {
      const step = Math.min(ms, 1000);
      this.clockMs += step;
      await page.clock.runFor(step);
      await this.settle();
      ms -= step;
    }
  }

  private async settle() {
    await expect
      .poll(() =>
        [...this.pending]
          .map((request) => new URL(request.url()).pathname)
          .filter((path) => {
            const dependency = dependencies.get(path);
            return dependency === undefined || !this.holds.has(dependency);
          }),
      )
      .toEqual([]);
  }

  async foreground(page: Page) {
    await page.evaluate(() => {
      window.dispatchEvent(new Event('focus'));
      document.dispatchEvent(new Event('visibilitychange'));
    });
  }

  async hidden(page: Page, hidden: boolean) {
    await page.evaluate((value) => {
      Object.defineProperty(document, 'hidden', { configurable: true, get: () => value });
      Object.defineProperty(document, 'visibilityState', {
        configurable: true,
        get: () => (value ? 'hidden' : 'visible'),
      });
      document.dispatchEvent(new Event('visibilitychange'));
    }, hidden);
  }

  async route(page: Page, baseURL: string) {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.clock.install();
    await page.clock.pauseAt(new Date(Date.now() + 1000));
    await this.base.route(page, baseURL);
    page.on('request', (request) => {
      const path = new URL(request.url()).pathname;
      if (
        (path.startsWith('/api/') || path.startsWith('/auth/')) &&
        !/^\/api\/v2\/agents\/[^/]+\/events$/.test(path)
      )
        this.pending.add(request);
    });
    page.on('response', (response) => {
      // Some clients reject HTTP errors without consuming their response bodies.
      if (!response.ok()) this.pending.delete(response.request());
    });
    page.on('requestfinished', (request) => this.pending.delete(request));
    page.on('requestfailed', (request) => {
      this.pending.delete(request);
      this.cancelled.push(new URL(request.url()).pathname);
    });
    await page.route('**/*', async (route) => {
      const request = route.request();
      const url = new URL(request.url());
      if (url.origin !== baseURL) return route.fallback();
      const path = url.pathname;
      if (path === '/auth/connections/check' && request.method() === 'POST') {
        this.connectionChecks++;
        return route.fallback();
      }
      if (path.startsWith('/auth/login')) {
        this.logins.push(path);
        return route.abort();
      }
      if ((path.startsWith('/api/') || path.startsWith('/auth/')) && request.method() !== 'GET') {
        this.writes.push(`${request.method()} ${path}`);
        return route.fulfill({ status: 500, json: { code: 'unexpected_local_write' } });
      }
      if (
        path.includes('/agents/agt_other') ||
        path === '/api/v1/resource-access/agent/agt_other' ||
        (this.denied && path === '/api/v1/resource-access/agent/agt_contract')
      )
        return route.fulfill({ status: 403, json: { code: 'forbidden' } });
      const dependency = dependencies.get(path);
      if (!dependency) return route.fallback();
      const status =
        this.denied && dependency !== 'identity' ? 403 : (this.faults.get(dependency) ?? 200);
      const subject = this.base.owner;
      const environment = structuredClone(this.environment);
      this.requests.push({
        dependency,
        path,
        status,
        subject,
        phase: this.phase,
        clockMs: this.clockMs,
        elapsedMs: performance.now() - this.started,
      });
      await this.holds.get(dependency);
      if (status !== 200)
        return route.fulfill({
          status,
          headers: {
            'cache-control': 'no-store',
            ...(status === 429 ? { 'retry-after': '60' } : {}),
          },
          json: {
            type: 'about:blank',
            status,
            code:
              status === 403
                ? 'forbidden'
                : dependency === 'identity'
                  ? 'identity_unavailable'
                  : 'store_failed',
            detail: 'Synthetic dependency unavailable',
            ...(dependency === 'identity'
              ? { retryable: true }
              : { extensions: { retryable: false } }),
          },
        });
      if (dependency === 'identity')
        return route.fulfill({
          json: {
            authenticated: true,
            subject,
            display_name: subject,
            capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
          },
        });
      if (dependency === 'environment') return route.fulfill({ json: environment });
      if (path.endsWith('/files/tree'))
        return route.fulfill({
          json: {
            path: url.searchParams.get('path') ?? '',
            entries: [
              {
                path: filePath,
                name: 'notes.txt',
                kind: 'file',
                size_bytes: fileText.length,
                hidden: false,
              },
            ],
          },
        });
      if (path.endsWith('/files/content'))
        return route.fulfill({
          json: {
            path: filePath,
            content_base64: Buffer.from(fileText).toString('base64'),
            size_bytes: fileText.length,
            sha256: 'a'.repeat(64),
          },
        });
      if (path.endsWith('/events/head'))
        return route.fulfill({
          json: {
            object: 'cma.agent.event_head',
            schema_version: 1,
            agent_id: 'agt_contract',
            incarnation: 'a'.repeat(64),
            through_cursor: cursor(this.base.items.length),
            active_turn: null,
          },
        });
      return route.fallback();
    });
  }

  computer(page: Page) {
    return page.getByRole('complementary', { name: 'Agent computer', exact: true });
  }

  async open(page: Page) {
    await page.getByRole('button', { name: 'Show Agent computer', exact: true }).click();
    await expect(this.computer(page)).toBeVisible();
  }

  async recovered(page: Page) {
    const computer = this.computer(page);
    await computer.getByRole('tab', { name: 'Files', exact: true }).click();
    await computer.getByRole('button', { name: 'notes.txt', exact: true }).click();
    await expect(computer.getByRole('textbox', { name: 'Contents of notes.txt' })).toHaveValue(
      fileText,
    );
    await expect(computer.getByRole('button', { name: 'New file', exact: true })).toBeEnabled();
    await computer.getByRole('tab', { name: 'Operations', exact: true }).click();
    await expect(computer.getByText(operationText, { exact: true })).toBeVisible();
    await computer.getByRole('tab', { name: 'Screen', exact: true }).click();
    await expect(
      computer.getByText('Live screen is not supported by this environment.', { exact: true }),
    ).toBeVisible();
    await expect(computer.locator('iframe, video, canvas')).toHaveCount(0);
  }

  assertSafe() {
    expect(this.writes).toEqual([]);
    expect(this.logins).toEqual([]);
    expect(this.base.writes).toEqual([]);
    expect(this.base.errors).toEqual([]);
    expect(this.base.boundary.writes).toEqual([]);
    expect(this.base.boundary.unexpected).toEqual([]);
    expect(this.base.boundary.errors).toEqual([]);
  }

  async finish(page: Page, info: TestInfo) {
    try {
      const evidence = JSON.stringify(
        {
          scope:
            'Compiled browser with synthetic public responses; not production or live shell proof',
          requests: this.requests,
          writes: this.writes,
          logins: this.logins,
          cancelled: this.cancelled,
          connectionChecks: this.connectionChecks,
        },
        null,
        2,
      );
      await writeFile(info.outputPath('requests.json'), evidence);
      if (info.status !== info.expectedStatus && !page.isClosed())
        await writeFile(info.outputPath('page.txt'), await page.locator('body').innerText());
      await info.attach('dependency-requests', { body: evidence, contentType: 'application/json' });
    } finally {
      for (const release of this.releases) release();
      await this.base.close(page);
    }
  }
}
