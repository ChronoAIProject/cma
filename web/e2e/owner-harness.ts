import { createHash, randomUUID } from 'node:crypto';
import { join } from 'node:path';
import {
  expect,
  test as base,
  type Browser,
  type BrowserContext,
  type Page,
  type Request,
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
import { inspectOwnerStream, ownerStreamTap, streamBounds } from './owner-stream';

interface Continuation {
  agentId: string;
  worktreeId: string;
  message: string;
  request?: Request;
  runId?: string;
  writer?: WritableStreamDefaultWriter<Uint8Array>;
  headersSeen?: boolean;
  turnSequence?: number;
}

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

export function ownerConversationReadAllowed(url: URL, owned: Journal['resources']) {
  if (url.pathname === '/api/v1/agui/capabilities') return !url.search && !url.hash;
  const agents = owned.filter((row) => row.kind === 'agents');
  if (url.pathname === '/api/v1/agui/events') {
    const query = url.searchParams;
    return (
      !url.hash &&
      [...query.keys()].sort().join(',') === 'follow,starting_after,thread_id' &&
      agents.some((row) => row.id === query.get('thread_id')) &&
      /^(0|[1-9]\d*)$/.test(query.get('starting_after') ?? '') &&
      Number.isSafeInteger(Number(query.get('starting_after'))) &&
      ['true', 'false'].includes(query.get('follow') ?? '')
    );
  }
  return (
    !url.search &&
    !url.hash &&
    agents.some((row) => url.pathname === `/api/v1/agui/agents/${row.id}/sandbox`)
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
  private continuation?: Continuation;
  private readonly lifecycleCommands = new Set<string>();
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
  armContinuation(agentId: string, worktreeId: string, message: string) {
    if (
      this.continuation ||
      !this.journal.resources.some(
        (row) =>
          row.kind === 'agents' && row.id === agentId && row.subject === this.fixture.subject,
      )
    )
      throw new Error('Owner continuation requires its one run-owned Agent');
    assertCredentialFree(message, this.forbidden);
    if (message.trim() !== message || message.length === 0 || message.length > 1024)
      throw new Error('Owner continuation requires one bounded message');
    this.continuation = { agentId, worktreeId: identifier(worktreeId, 'owner Worktree'), message };
  }
  async completedContinuation() {
    await Promise.all([...this.observations]);
    expect(this.violations).toEqual([]);
    if (this.continuation?.turnSequence === undefined)
      throw new Error('Owner continuation has no inspected completed stream');
    return this.continuation.turnSequence;
  }
  private async admitContinuation(request: Request, context: BrowserContext) {
    const next = this.continuation;
    const headers = request.headers();
    const body = object(JSON.parse(request.postData() ?? ''));
    const message =
      Array.isArray(body.messages) && body.messages.length === 1 ? object(body.messages[0]) : {};
    if (
      !next ||
      next.request ||
      request.resourceType() !== 'fetch' ||
      !isJsonMediaType(headers['content-type'] ?? '') ||
      headers.origin !== this.config.baseURL ||
      headers['idempotency-key'] !== undefined ||
      Object.keys(body).sort().join(',') !== 'messages,runId,threadId' ||
      body.threadId !== next.agentId ||
      typeof body.runId !== 'string' ||
      !/^[a-zA-Z0-9_-]{1,128}$/.test(body.runId) ||
      Object.keys(message).sort().join(',') !== 'content,role' ||
      message.role !== 'user' ||
      message.content !== next.message
    )
      throw new Error('Owner conversation command is outside its single admission');
    // A correlation runId does not make an AG-UI POST idempotent. Consume before any await.
    next.request = request;
    next.runId = body.runId;
    await this.account(context);
    const agent = object(
      await ownerRead(
        context.request,
        this.config.baseURL,
        `/api/v1/agents/${next.agentId}`,
        this.forbidden,
      ),
    );
    if (
      agent.id !== next.agentId ||
      agent.status !== 'ready' ||
      object(agent.worktree).id !== next.worktreeId ||
      object(agent.sandbox).status !== 'active' ||
      !this.journal.resources.some(
        (row) =>
          row.kind === 'sandboxes' &&
          row.id === object(agent.sandbox).id &&
          row.subject === this.fixture.subject,
      ) ||
      !this.journal.resources.some(
        (row) =>
          row.kind === 'workspaces' &&
          row.id === object(agent.workspace).id &&
          row.subject === this.fixture.subject,
      )
    )
      throw new Error('Owner continuation lineage or readiness changed');
    const stream = new TransformStream<Uint8Array, Uint8Array>();
    next.writer = stream.writable.getWriter();
    const observation = inspectOwnerStream(
      new Response(stream.readable),
      next.agentId,
      this.forbidden,
      AbortSignal.timeout(streamBounds.timeoutMs),
    )
      .then((turn) => {
        next.turnSequence = turn;
      })
      .catch(() => {
        this.violations.push('conversation_stream_inspection_failed');
      })
      .finally(() => {
        void next.writer?.abort().catch(() => undefined);
      });
    this.observations.add(observation);
    void observation.finally(() => this.observations.delete(observation));
    this.journal.record({
      action: 'conversation_admission',
      agentId: next.agentId,
      correlationId: next.runId,
      retrySafe: false,
    });
  }
  async open(): Promise<Page> {
    const context = await this.browser.newContext({
      storageState: this.fixture.storageState,
      serviceWorkers: 'block',
    });
    this.contexts.push(context);
    const session = await this.account(context);
    await context.exposeBinding('__cmaOwnerStream', async ({ page }, value: unknown) => {
      try {
        const event = object(value);
        const next = this.continuation;
        if (!next?.writer || next.request?.frame().page() !== page || event.runId !== next.runId)
          throw new Error('Owner stream source mismatch');
        if (event.stage === 'headers') {
          if (
            next.headersSeen ||
            event.status !== 200 ||
            typeof event.media !== 'string' ||
            !/^text\/event-stream(?:\s*;|$)/i.test(event.media)
          )
            throw new Error('Owner stream response mismatch');
          next.headersSeen = true;
        } else if (
          event.stage === 'chunk' &&
          next.headersSeen &&
          Array.isArray(event.bytes) &&
          event.bytes.length <= 65_536 &&
          event.bytes.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255)
        ) {
          await next.writer.write(Uint8Array.from(event.bytes));
        } else if (event.stage === 'end' && next.headersSeen) await next.writer.close();
        else throw new Error('Owner stream inspection failed');
      } catch {
        this.violations.push('conversation_stream_transport_failed');
        void this.continuation?.writer?.abort().catch(() => undefined);
        throw new Error('Owner stream inspection refused');
      }
    });
    await context.addInitScript(ownerStreamTap, { origin: this.config.baseURL, ...streamBounds });
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
        (url.pathname.startsWith('/auth/') && url.pathname !== '/auth/session') ||
        (url.pathname.startsWith('/api/v1/agui/') &&
          request.method() !== 'POST' &&
          (request.method() !== 'GET' ||
            !ownerConversationReadAllowed(
              url,
              this.journal.resources.filter((row) => row.subject === this.fixture.subject),
            )))
      ) {
        this.violations.push('out_of_scope_browser_request');
        return route.abort('blockedbyclient');
      }
      if (!['GET', 'HEAD', 'OPTIONS'].includes(request.method())) {
        const conversation = url.pathname === '/api/v1/agui/run' && request.method() === 'POST';
        if (
          url.search ||
          url.hash ||
          (!conversation &&
            !ownerMutationAllowed(url.pathname, request.method(), this.journal.resources))
        ) {
          this.violations.push('out_of_scope_browser_mutation');
          return route.abort('blockedbyclient');
        }
        try {
          assertCredentialFree(request.postData() ?? '', this.forbidden);
          if (conversation) await this.admitContinuation(request, context);
          else {
            if (this.lifecycleCommands.has(url.pathname))
              throw new Error('Owner baseline refuses a repeated lifecycle command');
            this.lifecycleCommands.add(url.pathname);
            await this.account(context);
          }
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
            first_message: `Remember the marker OWNER-${this.run}. Reply once with the marker and your working directory. Do not modify files.`,
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
