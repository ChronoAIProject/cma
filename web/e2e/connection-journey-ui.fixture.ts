import assert from 'node:assert/strict';
import { createServer } from 'node:http';
import { mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { extname, join, relative, sep } from 'node:path';
import type { Browser, Page } from '@playwright/test';
import type { Launch as LaunchDocument } from '@cma/client';
import { LocalStartup, recorded } from './agent-startup-fixtures';
import { fixtureDocument } from './environment.fixture';
import { Harness } from './harness';
import type { Hooks } from './environment';
import {
  identityDigest,
  readConnectionIntent,
  type ConnectionIntent,
  type ConnectionMode,
  type ConnectionObservationInput,
} from './connection-journey-contract';
import { wizardProvider } from './connection-journey-ui-provider.fixture';
import { publicApiPath } from './public-api-path';

// Only HTTP and provider observations are synthetic. Harness, Journal and the
// compiled application run unchanged, including the browser request recorder.
export async function connectionWizardFixture(
  browser: Browser,
  preview: string,
  caseId: string,
  buildDirectory: unknown,
  mode: ConnectionMode = 'oauth',
) {
  assert.ok(['127.0.0.1', 'localhost'].includes(new URL(preview).hostname));
  assert.equal(typeof buildDirectory, 'string');
  const directory = buildDirectory as string;
  const assets = new Map<string, Buffer>();
  for (const file of readdirSync(directory, { recursive: true, withFileTypes: true })) {
    if (!file.isFile()) continue;
    const path = join(file.parentPath, file.name);
    assets.set('/' + relative(directory, path).split(sep).join('/'), readFileSync(path));
  }
  assert.ok(assets.has('/index.html'), 'The compiled product entry must exist');
  const contentTypes: Record<string, string> = {
    '.html': 'text/html',
    '.js': 'text/javascript',
    '.css': 'text/css',
    '.svg': 'image/svg+xml',
    '.png': 'image/png',
    '.webp': 'image/webp',
    '.ico': 'image/x-icon',
    '.woff2': 'font/woff2',
    '.json': 'application/json',
  };
  const fixture = fixtureDocument();
  const selectedService = fixture.document.connectionJourneys.services[mode]!;
  const owner = `visitor-${caseId}`;
  fixture.document.visitors[caseId]!.subject = owner;
  fixture.document.provenance.nyxidCommit = 'c'.repeat(40);
  for (const service of Object.values(fixture.document.connectionJourneys.services)) {
    service.providerRevision = 'c'.repeat(40);
    service.fixtureDigest = 'b'.repeat(64);
  }
  writeFileSync(fixture.manifest, JSON.stringify(fixture.document));
  mkdirSync(fixture.env.CMA_E2E_OUTPUT_DIR, { mode: 0o700 });
  const boundary = new LocalStartup();
  boundary.sandbox.owner = owner;
  let launch: LaunchDocument = {
    ...recorded<LaunchDocument>('launches.json', 'launch-review-create'),
    id: `launch-${caseId}`,
    public_id: 'public-fixture',
    placement_id: 'placement-fixture',
    review: {
      can_start_now: false,
      needs: [{ type: 'connection', service: selectedService.serviceId }],
    },
    trust: { version: 1, disclosures: [] },
  };
  const target = { type: 'launch' as const, id: launch.id };
  let upstreamUnavailable = false;
  let independentlyConnected = false;
  let terminalState: 'expired' | 'failed' | 'cancelled' | undefined;
  const startRequests: { bytes: string; intent: ConnectionIntent }[] = [];
  const admissions: { bytes: string; intent: ConnectionIntent }[] = [];
  const cancellations: { target: typeof target; active: Omit<ConnectionIntent, 'target'> }[] = [];
  const links = new Map<string, 'pending' | 'completed'>();
  const checks: Record<string, unknown>[] = [];
  const unexpected: string[] = [];
  let creates = 0;
  let starts = 0;
  let invalidReply: 'owner' | 'intent' | 'service' | undefined;
  const session = {
    authenticated: true,
    subject: owner,
    capabilities: ['launch_triggers', 'use_own_sandboxes'],
  };
  const appRoutes = new Set([
    '/',
    '/connection-return',
    '/t/public-fixture/placement-fixture',
    `/launches/${launch.id}`,
    `/agents/${boundary.agent.id}`,
  ]);
  const server = createServer(async (request, response) => {
    try {
      if (
        !request.url?.startsWith('/') ||
        request.url.startsWith('//') ||
        request.url.includes('\\')
      ) {
        response.writeHead(400).end();
        return;
      }
      const path = publicApiPath(new URL(request.url!, 'http://127.0.0.1').pathname);
      const chunks: Buffer[] = [];
      for await (const chunk of request) chunks.push(Buffer.from(chunk));
      const bytes = Buffer.concat(chunks).toString();
      const body = JSON.parse(bytes || '{}');
      const reply = (value: unknown, status = 200) => {
        response.writeHead(status, { 'Content-Type': 'application/json' });
        response.end(JSON.stringify(value));
      };
      if (path === '/auth/session') return reply(session);
      if (path === '/auth/connections/start') {
        const intent = readConnectionIntent(body, launch.id);
        assert.equal(intent.service_id, selectedService.serviceId);
        startRequests.push({ bytes, intent });
        if (upstreamUnavailable) return reply({ code: 'upstream_unavailable' }, 503);
        admissions.push({ bytes, intent });
        if (!links.has(intent.intent_id)) links.set(intent.intent_id, 'pending');
        return reply({
          owner: invalidReply === 'owner' ? 'another-owner' : owner,
          service_id: invalidReply === 'service' ? 'another-service' : intent.service_id,
          intent_id:
            invalidReply === 'intent' ? '24681357-1357-2468-1357-246813572468' : intent.intent_id,
          hosted_url: `https://identity.example.test/connect/${intent.intent_id}`,
          expires_at: '2099-01-01T00:00:00Z',
        });
      }
      if (path === '/auth/connections/check') {
        checks.push(body);
        if (body.target?.type === 'agent') {
          assert.equal(starts, 1);
          assert.deepEqual(body.target, { type: 'agent', id: boundary.agent.id });
          return reply({ owner, ready: true, checked_at: new Date().toISOString(), services: [] });
        }
        assert.deepEqual(body.target, target);
        const active = body.active as ConnectionIntent | undefined;
        if (active) assert.equal(active.service_id, selectedService.serviceId);
        const complete =
          independentlyConnected ||
          (active !== undefined && links.get(active.intent_id) === 'completed');
        return reply({
          owner,
          ready: complete,
          checked_at: new Date().toISOString(),
          active_state:
            active === undefined
              ? undefined
              : (terminalState ?? (complete ? 'satisfied' : 'pending')),
          services: [
            {
              service: selectedService.serviceId,
              display_name: `Fixture ${mode} account`,
              kind: 'nyxid',
              state: complete ? 'satisfied' : 'missing',
              connection_mode: 'hosted',
            },
          ],
        });
      }
      if (path === '/auth/connections/cancel') {
        assert.equal(request.method, 'POST');
        assert.deepEqual(body.target, target);
        assert.equal(body.active.service_id, selectedService.serviceId);
        assert.ok(links.has(body.active.intent_id));
        cancellations.push(body);
        return reply({});
      }
      if (path === '/api/v2/triggers/public/public-fixture/launches') {
        assert.equal(request.method, 'POST');
        const verified = `v1.${Buffer.from(owner).toString('base64url')}`;
        assert.equal(request.headers['x-cma-expected-subject'], verified);
        response.setHeader('X-CMA-Verified-Subject', verified);
        creates += 1;
        return reply(launch, 201);
      }
      if (path === '/api/v1/triggers/public/public-fixture')
        return reply({
          title: 'Connection fixture',
          first_message: '',
          description: 'Connect your account to continue.',
          button_label: 'open_in_cma',
          theme: 'auto',
          publisher_display_name: 'Fixture publisher',
          what_happens: [],
          availability: 'published',
          always_review: true,
          trust: { disclosures: [] },
          technical: {
            provider: 'codex',
            effort: 'high',
            size: { cpu_millis: 2000, memory_mib: 4096 },
            services: [],
          },
        });
      if (path === `/api/v1/launches/${launch.id}/start`) {
        assert.ok(
          independentlyConnected ||
            [...links.values()].filter((state) => state === 'completed').length === 1,
        );
        starts += 1;
        launch = {
          ...launch,
          state: 'ready',
          event_head: launch.event_head + 1,
          resolution: {
            sandbox: { state: 'created', id: boundary.sandbox.id },
            workspace: { state: 'created', id: boundary.workspace.id },
            agent: { state: 'created', id: boundary.agent.id },
            choice_required: false,
          },
        };
        return reply(launch, 202);
      }
      if (path === `/api/v1/launches/${launch.id}`) return reply(launch);
      if (path === `/api/v1/sandboxes/${boundary.sandbox.id}`) return reply(boundary.sandbox);
      if (path.startsWith('/api/') || path.startsWith('/auth/')) {
        unexpected.push(`${request.method} ${path}`);
        return reply({ code: 'unexpected_fixture_request' }, 500);
      }
      const assetPath = appRoutes.has(path) ? '/index.html' : path;
      const asset = assets.get(assetPath);
      if (asset === undefined) {
        response.writeHead(404).end();
        return;
      }
      response.writeHead(200, {
        'Content-Type': contentTypes[extname(assetPath)] ?? 'application/octet-stream',
      });
      response.end(asset);
    } catch (error) {
      unexpected.push(String(error));
      response.writeHead(500).end();
    }
  });
  await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
  const address = server.address();
  assert.ok(address && typeof address !== 'string');
  const origin = `http://127.0.0.1:${address.port}`;
  const observeConnection = async (input: ConnectionObservationInput) => {
    assert.equal(input.owner, owner);
    assert.deepEqual(input.target, target);
    assert.equal(input.mode, mode);
    assert.equal(input.serviceId, selectedService.serviceId);
    assert.equal(input.caseId, caseId);
    const service = selectedService;
    return {
      schema: 'cma-j4-provider-observation-v1',
      run: input.run,
      caseId: input.caseId,
      phase: input.phase,
      mode: input.mode,
      ownerDigest: identityDigest(owner),
      targetDigest: identityDigest(target),
      serviceDigest: identityDigest(service.serviceId),
      intentDigest: input.intentId === undefined ? null : identityDigest(input.intentId),
      providerRevision: service.providerRevision,
      fixtureDigest: service.fixtureDigest,
      observedAt: new Date().toISOString(),
      evidenceDigest: identityDigest([...links]),
      links: [
        { linkDigest: identityDigest('existing-provider-link'), state: 'completed' },
        ...[...links].map(([id, state]) => ({ linkDigest: identityDigest(id), state })),
      ],
    };
  };
  // The constructor validates the normal synthetic manifest. Only its transport
  // destination is changed to the isolated loopback server for this local test.
  const previous = Object.fromEntries(
    Object.keys(fixture.env).map((key) => [key, process.env[key]]),
  );
  Object.assign(process.env, fixture.env);
  let h: Harness;
  try {
    h = new Harness(browser, { observeConnection } as Hooks, caseId);
  } finally {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key];
      else process.env[key] = value;
    }
  }
  h.config.baseURL = origin;
  let page: Page | undefined;
  const provider = wizardProvider(mode, links, origin);
  return {
    h,
    boundary,
    admissions,
    cancellations,
    startRequests,
    providerSubmissions: provider.submissions,
    links,
    checks,
    unexpected,
    target,
    get creates() {
      return creates;
    },
    get starts() {
      return starts;
    },
    set upstreamUnavailable(value: boolean) {
      upstreamUnavailable = value;
    },
    set independentlyConnected(value: boolean) {
      independentlyConnected = value;
    },
    set terminalState(value: 'expired' | 'failed' | 'cancelled') {
      terminalState = value;
    },
    set invalidReply(value: 'owner' | 'intent' | 'service') {
      invalidReply = value;
    },
    async open(blockPopup = false) {
      page = await h.open('visitor');
      page.setDefaultTimeout(5000);
      await boundary.route(page, origin);
      await page.route('**/*', (route) => {
        const path = publicApiPath(new URL(route.request().url()).pathname);
        return path === '/auth/session' ||
          path.startsWith('/auth/connections/') ||
          path.startsWith('/api/v1/launches/') ||
          path.startsWith('/api/v1/triggers/public/') ||
          path === '/api/v2/triggers/public/public-fixture/launches'
          ? route.continue()
          : route.fallback();
      });
      await page.context().route('https://identity.example.test/**', provider.route);
      await page.route('https://identity.example.test/**', provider.route);
      await page.route('**/api/v1/resource-access/agent/agt_contract', (route) =>
        route.fulfill({
          json: {
            capabilities: ['view_details', 'view_history', 'interact'],
            revalidate_at: null,
            policy_revision: 1,
          },
        }),
      );
      if (blockPopup)
        await page.addInitScript(() => {
          window.open = () => null;
        });
      return page;
    },
    async close() {
      await page
        ?.context()
        .close()
        .catch(() => undefined);
      await new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
        server.closeAllConnections();
      });
      rmSync(fixture.directory, { recursive: true, force: true });
    },
  };
}
