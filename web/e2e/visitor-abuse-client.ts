import { createHash, randomUUID } from 'node:crypto';
import { readFileSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';
import { request, type APIRequestContext } from '@playwright/test';
import { identifier, object } from './environment';
import { Journal } from './journal';
import {
  assertCredentialFree,
  credentialValues,
  evidenceDirectoryIsReady,
  isJsonMediaType,
} from './security';
import {
  readBearer,
  type Actor,
  type Target,
  type TC117Fixture,
  type Visitor,
} from './visitor-abuse-environment';
import {
  check,
  count,
  metrics,
  newCounts,
  receipt,
  stats,
  type ControlSample,
  type Result,
} from './visitor-abuse-model';

interface SendOptions {
  origin?: string;
  method?: 'GET' | 'POST' | 'DELETE';
  data?: unknown;
  key?: string;
  api?: APIRequestContext;
}
export class CallerResponseLost extends Error {
  constructor() {
    super('TC-117 injected caller response loss after admission');
  }
}
export class TC117Client {
  readonly counts = newCounts();
  readonly controls: ControlSample[] = [];
  readonly observations: Record<string, unknown> = {};
  readonly requests: Record<string, unknown>[] = [];
  readonly journal: Journal;
  private readonly startedAt = new Date().toISOString();
  private readonly contexts = new Map<Actor, APIRequestContext>();
  private readonly tokens = new Map<Actor, string>();
  private readonly forbidden: string[];
  private constructor(
    readonly config: { baseURL: string; output: string },
    readonly fixture: TC117Fixture,
  ) {
    this.journal = new Journal(config.output, randomUUID());
    for (const [actor, account] of Object.entries(fixture.accounts))
      this.tokens.set(actor as Actor, readBearer(account));
    this.forbidden = [...credentialValues(fixture.forbiddenValuesFile), ...this.tokens.values()];
  }
  static async open(config: { baseURL: string; output: string }, fixture: TC117Fixture) {
    const client = new TC117Client(config, fixture);
    try {
      for (const actor of ['anonymous', ...Object.keys(fixture.accounts)] as Actor[])
        client.contexts.set(actor, await request.newContext());
      return client;
    } catch {
      await client.close(false);
      throw new Error('Required E2E TC-117 request contexts are unavailable');
    }
  }
  async send(actor: Actor, path: string, options: SendOptions = {}): Promise<Result> {
    const origin = options.origin ?? this.config.baseURL;
    check(
      [this.config.baseURL, ...this.fixture.instances].includes(origin),
      'request origin is not allowlisted',
    );
    check(
      path.startsWith('/') && !path.startsWith('//') && new URL(path, origin).origin === origin,
      'request path is out of scope',
    );
    const method = options.method ?? 'GET';
    check(
      !path.startsWith('/api/v1/admin/') || method === 'GET',
      'administrative mutation is forbidden',
    );
    const commandKey = method === 'GET' ? undefined : (options.key ?? randomUUID());
    const sentAt = new Date().toISOString();
    const started = performance.now();
    let response;
    try {
      response = await (options.api ?? this.contexts.get(actor)!).fetch(`${origin}${path}`, {
        method,
        data: options.data,
        maxRedirects: 0,
        maxRetries: 0,
        timeout: 30_000,
        headers: {
          ...(actor === 'anonymous' ? {} : { Authorization: `Bearer ${this.tokens.get(actor)!}` }),
          ...(method === 'GET' ? {} : { Origin: origin, 'Idempotency-Key': commandKey! }),
        },
      });
    } catch {
      this.counts.transportErrors++;
      this.requests.push({
        actor,
        instance: this.instance(origin),
        method,
        status: 0,
        ms: performance.now() - started,
      });
      throw new Error('TC-117 public request transport failed');
    }
    try {
      const decoded: unknown = isJsonMediaType(response.headers()['content-type'] ?? '')
        ? JSON.parse(await response.text())
        : {};
      const body = Array.isArray(decoded) ? { items: decoded } : object(decoded);
      const result = {
        status: response.status(),
        body,
        retryAfter: response.headers()['retry-after'],
        ms: performance.now() - started,
      };
      // Never serialize bodies, auth headers, command keys, proxy locations or URLs.
      const code = ['resource_quota', 'idempotency_key_reused', 'sandbox_limit_reached'].includes(
        String(body.code),
      )
        ? body.code
        : undefined;
      this.requests.push({
        actor,
        instance: this.instance(origin),
        method,
        status: result.status,
        code,
        ms: result.ms,
        sentAt,
        target: Object.entries(this.fixture.triggers).find(([, target]) =>
          path.includes(`/public/${target.publicId}/launches`),
        )?.[0],
        commandKeyDigest: commandKey
          ? createHash('sha256').update(commandKey).digest('hex')
          : undefined,
        bodyDigest: commandKey
          ? createHash('sha256')
              .update(JSON.stringify(options.data ?? null))
              .digest('hex')
          : undefined,
        ...(typeof body.id === 'string' && typeof body.created_at === 'string'
          ? {
              receiptId: identifier(body.id, 'receipt id'),
              createdAt: new Date(body.created_at).toISOString(),
            }
          : {}),
        ...(body.resource === 'visitor_launches'
          ? {
              limit: count(body.limit),
              resetAt: new Date(String(body.reset_at)).toISOString(),
              retryAfter: Number(result.retryAfter) || null,
            }
          : {}),
      });
      if (actor !== 'anonymous')
        this.journal.observe(
          new URL(path, origin).pathname,
          method,
          result.status,
          body,
          this.fixture.accounts[actor].subject,
        );
      // Response sentinels are inspected in memory; the body never becomes evidence.
      if (isJsonMediaType(response.headers()['content-type'] ?? ''))
        assertCredentialFree(JSON.stringify(body), this.forbidden);
      return result;
    } finally {
      await response.dispose();
    }
  }
  private instance(origin: string) {
    return this.fixture.instances.indexOf(origin);
  }
  async read(actor: Actor, path: string, origin?: string) {
    const result = await this.send(actor, path, { origin });
    check(result.status === 200, 'required public read failed');
    return result.body;
  }
  async list(actor: Actor, path: string, origin?: string) {
    const rows: Record<string, unknown>[] = [];
    const seen = new Set<string>();
    let cursor: string | undefined;
    do {
      const url = new URL(path, this.config.baseURL);
      url.searchParams.set('limit', '200');
      if (cursor) url.searchParams.set('cursor', cursor);
      const page = await this.read(actor, `${url.pathname}${url.search}`, origin);
      check(Array.isArray(page.items), 'invalid public list page');
      rows.push(...page.items.map((row) => object(row)));
      const next = page.next_cursor ?? page.next;
      check(next == null || typeof next === 'string', 'invalid public list cursor');
      cursor = next == null ? undefined : String(next);
      if (cursor) {
        check(!seen.has(cursor) && seen.size < 1000, 'public pagination did not terminate');
        seen.add(cursor);
      }
    } while (cursor);
    return rows;
  }
  launch(actor: Visitor, target: Target, options: SendOptions = {}) {
    return this.send(actor, `/api/v1/triggers/public/${target.publicId}/launches`, {
      method: 'POST',
      data: { placement_id: target.placementId, start_if_ready: true },
      ...options,
    });
  }
  async launchAndLoseResponse(
    actor: Visitor,
    target: Target,
    options: SendOptions = {},
  ): Promise<never> {
    const admitted = await this.launch(actor, target, options);
    check([201, 202].includes(admitted.status), 'lost-response probe was not admitted');
    // Keep transport observations for cleanup; deliver no receipt to the scenario caller.
    throw new CallerResponseLost();
  }
  async settled(actor: Visitor, id: string, ready = true) {
    const deadline = Date.now() + 540_000;
    while (Date.now() < deadline) {
      const row = receipt(
        await this.read(actor, `/api/v1/launches/${identifier(id, 'Launch id')}`),
      );
      if (['ready', 'completed', 'failed_terminal', 'cancelled'].includes(String(row.state))) {
        if (ready)
          check(
            ['ready', 'completed'].includes(String(row.state)),
            'legitimate Launch did not become ready',
          );
        return row;
      }
      await new Promise((resolve) => setTimeout(resolve, 1000));
    }
    throw new Error('TC-117 Launch settlement timed out');
  }
  async snapshot(visitors: Visitor[]) {
    const launches: Record<string, string[]> = {};
    for (const actor of visitors)
      launches[actor] = (await this.list(actor, '/api/v1/launches'))
        .map((row) => identifier(row.id, 'Launch id'))
        .sort();
    const sandboxes = await this.list('admin', '/api/v1/admin/sandboxes');
    const totals: Record<string, ReturnType<typeof stats>> = {};
    for (const target of Object.values(this.fixture.triggers))
      totals[target.id] = stats(await this.read('owner', `/api/v1/triggers/${target.id}/stats`));
    return {
      launches,
      fleetLive: sandboxes.filter((row) => row.retired_at == null).length,
      sandboxes: sandboxes.map((row) => identifier(row.id, 'Sandbox id')).sort(),
      liveByVisitor: Object.fromEntries(
        visitors.map((actor) => [
          actor,
          sandboxes.filter(
            (row) => row.owner === this.fixture.accounts[actor].subject && row.retired_at == null,
          ).length,
        ]),
      ),
      totals,
    };
  }
  async control(actor: Visitor, kind: 'stop' | 'cancel', id: string, origin: string) {
    const collection = kind === 'stop' ? 'agents' : 'launches';
    check(
      this.journal.resources.some(
        (row) =>
          row.kind === collection &&
          row.id === id &&
          row.subject === this.fixture.accounts[actor].subject,
      ),
      'control target is not owned by this run',
    );
    const result = await this.send(
      actor,
      `/api/v1/${collection}/${identifier(id, 'control id')}/${kind}`,
      { method: 'POST', origin, ...(kind === 'stop' ? { data: {} } : {}) },
    );
    this.controls.push({ kind, status: result.status, ms: result.ms });
    return result;
  }
  async proxyContexts() {
    const apis: APIRequestContext[] = [];
    const endpoints = new Set<string>();
    try {
      for (const file of this.fixture.load?.proxyFiles ?? []) {
        const proxy = object(JSON.parse(readFileSync(file, 'utf8')), 'tc117 proxy');
        const url = new URL(String(proxy.server));
        check(
          ['http:', 'https:', 'socks5:'].includes(url.protocol) &&
            !url.username &&
            !url.password &&
            url.pathname === '/' &&
            !url.search &&
            !url.hash,
          'invalid private proxy endpoint',
        );
        check(
          proxy.username === undefined || typeof proxy.username === 'string',
          'invalid proxy username',
        );
        check(
          proxy.password === undefined || typeof proxy.password === 'string',
          'invalid proxy password',
        );
        const endpoint = `${url.protocol}//${url.host}`;
        check(!endpoints.has(endpoint), 'egress proxies must be distinct');
        endpoints.add(endpoint);
        apis.push(
          await request.newContext({
            proxy: {
              server: endpoint,
              username: proxy.username as string | undefined,
              password: proxy.password as string | undefined,
            },
          }),
        );
      }
      return apis;
    } catch {
      await Promise.all(apis.map((api) => api.dispose()));
      throw new Error('Required E2E TC-117 private egress proxy is unavailable');
    }
  }
  writeEvidence(name: string, status: string, run: string) {
    check(
      evidenceDirectoryIsReady(this.config.output, run),
      'evidence directory is not validated for this run',
    );
    identifier(name, 'evidence name');
    const record = {
      schema: 'cma-tc117-scenario-v1',
      campaign: 'TC-117',
      run,
      journalRun: this.journal.run,
      case: name,
      status,
      startedAt: this.startedAt,
      endedAt: new Date().toISOString(),
      origin: this.config.baseURL,
      instances: this.fixture.instances,
      N: this.requests.length,
      perInstance: this.fixture.instances.map((_, index) => ({
        index,
        responses: this.requests.filter((row) => row.instance === index && Number(row.status) > 0)
          .length,
        transportErrors: this.requests.filter((row) => row.instance === index && row.status === 0)
          .length,
      })),
      prerequisites: 'tc117-prerequisites.json',
      provenanceSource: 'staging-authority-before-and-after-attest',
      provenanceVerified: false,
      evidenceScope: 'Issue1485Task5Harness',
      authorityBinding: 'external_attestation_required',
      counts: this.counts,
      metrics: metrics(this.counts),
      controls: this.controls,
      observations: this.observations,
      requests: this.requests,
      configuration: this.fixture.load
        ? {
            ...this.fixture.load,
            proxyFiles: undefined,
            egressCount: this.fixture.load.proxyFiles.length,
          }
        : null,
      rawResults: `tc117-${name}.json`,
    };
    const text = JSON.stringify(record);
    assertCredentialFree(text, this.forbidden);
    writeFileSync(join(this.config.output, `tc117-${name}.json`), text, {
      mode: 0o600,
      flag: 'wx',
    });
  }
  async close(cleanup = true) {
    let failed = false;
    try {
      if (cleanup)
        for (const [actor, account] of Object.entries(this.fixture.accounts)) {
          if (!this.journal.resources.some((row) => row.subject === account.subject)) continue;
          const api = await request.newContext({
            extraHTTPHeaders: { Authorization: `Bearer ${this.tokens.get(actor as Actor)!}` },
          });
          try {
            await this.journal.cleanup(api, this.config.baseURL, account.subject);
          } catch {
            failed = true;
          } finally {
            await api.dispose();
          }
        }
    } finally {
      for (const api of this.contexts.values()) await api.dispose();
    }
    check(!failed, 'scoped cleanup is incomplete; consult cleanup.ndjson');
  }
}
