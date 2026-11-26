// @vitest-environment node
import { createServer, type RequestListener } from 'node:http';
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { afterEach, describe, expect, it } from 'vitest';
import { TC117Client } from './visitor-abuse-client';
import { tc117Fixture, tc117Visitors } from './visitor-abuse-environment';
import { prepareEvidenceDirectory } from './security';

function localFixture() {
  const directory = mkdtempSync(join(tmpdir(), 'cma-tc117-unit-'));
  const bearerFile = join(directory, 'bearer');
  const forbidden = join(directory, 'forbidden.json');
  writeFileSync(bearerFile, 'local-fixture-credential', { mode: 0o600 });
  writeFileSync(forbidden, JSON.stringify(['private-sentinel']), { mode: 0o600 });
  const account = (subject: string) => ({ subject, bearerFile });
  const target = (id: string) => ({ id, publicId: `public-${id}`, placementId: `placement-${id}` });
  const tc117 = {
    accounts: Object.fromEntries(
      ['owner', 'admin', ...tc117Visitors].map((name) => [name, account(name)]),
    ),
    triggers: { alwaysNew: target('fresh'), reuseA: target('reuse-a'), reuseB: target('reuse-b') },
    sandboxProfile: { id: 'sandbox-profile', revision: 1 },
  };
  const document = {
    origin: 'https://staging.example.test',
    fixtureOwner: 'owner',
    forbiddenValuesFile: forbidden,
    tc117,
  };
  const manifest = join(directory, 'manifest.json');
  writeFileSync(manifest, JSON.stringify(document), { mode: 0o600 });
  const env = {
    CMA_E2E_MODE: 'mutation',
    CMA_E2E_BASE_URL: document.origin,
    CMA_E2E_MUTATION_TARGET: document.origin,
    CMA_E2E_PUBLIC_ID: 'public-fresh',
    CMA_E2E_OUTPUT_DIR: join(directory, 'output'),
    CMA_E2E_FIXTURE_OWNER: 'owner',
    CMA_E2E_FIXTURE_FILE: manifest,
  };
  return { directory, document, manifest, env, fixture: tc117Fixture(env) };
}
const disposals: (() => Promise<void>)[] = [];
afterEach(async () => {
  for (const dispose of disposals.splice(0).reverse()) await dispose();
});
async function server(handler: RequestListener) {
  const service = createServer(handler);
  await new Promise<void>((resolve) => service.listen(0, '127.0.0.1', resolve));
  disposals.push(async () => {
    service.closeAllConnections();
    await new Promise<void>((resolve) => service.close(() => resolve()));
  });
  const address = service.address();
  if (!address || typeof address === 'string') throw new Error('Local port unavailable');
  return `http://127.0.0.1:${address.port}`;
}

describe('TC-117 isolated fixture section', () => {
  it('parses a private standalone TC-117 section without adding legacy visitor cases', () => {
    const local = localFixture();
    expect(Object.keys(local.fixture.accounts)).toEqual(['owner', 'admin', ...tc117Visitors]);
    expect(local.fixture.instances).toEqual([]);
    for (const name of ['a', 'admin']) {
      const body = structuredClone(local.document);
      delete body.tc117.accounts[name];
      writeFileSync(local.manifest, JSON.stringify(body));
      expect(() => tc117Fixture(local.env)).toThrow(`tc117.accounts.${name}`);
    }
  });
  it('rejects shared subjects, wrong targets and unfrozen load', () => {
    const local = localFixture();
    const bad = structuredClone(local.document);
    bad.tc117.accounts.a!.subject = 'owner';
    writeFileSync(local.manifest, JSON.stringify(bad));
    expect(() => tc117Fixture(local.env)).toThrow('distinct identity');
    writeFileSync(
      local.manifest,
      JSON.stringify({
        ...local.document,
        tc117: { ...local.document.tc117, load: { frozen: false } },
      }),
    );
    expect(() => tc117Fixture(local.env)).toThrow(/^BLOCKED:/);
    expect(() =>
      tc117Fixture({ ...local.env, CMA_E2E_MUTATION_TARGET: 'https://other.example.test' }),
    ).toThrow('exact base origin');
  });
});

describe('TC-117 public API mechanics', () => {
  it('binds bearer requests to allowed origins, never follows redirects, and paginates public receipts', async () => {
    const auth: (string | undefined)[] = [];
    let privateReads = 0;
    const baseURL = await server((req, res) => {
      auth.push(req.headers.authorization);
      res.setHeader('content-type', 'application/json');
      if (req.url === '/redirect') {
        res.writeHead(302, { location: '/private' });
        res.end('{}');
      } else if (req.url === '/private') {
        privateReads++;
        res.end('{}');
      } else
        res.end(
          JSON.stringify({
            items: [
              {
                id: req.url!.includes('cursor') ? 'second' : 'first',
                created_at: '2026-11-13T01:00:00Z',
              },
            ],
            next_cursor: req.url!.includes('cursor') ? undefined : 'next-page',
          }),
        );
    });
    const local = localFixture();
    const client = await TC117Client.open(
      { baseURL, output: local.env.CMA_E2E_OUTPUT_DIR },
      local.fixture,
    );
    disposals.push(() => client.close(false));
    expect((await client.list('a', '/api/v1/launches')).map((row) => row.id)).toEqual([
      'first',
      'second',
    ]);
    expect((await client.send('anonymous', '/redirect')).status).toBe(302);
    await expect(
      client.send('a', '/auth/session', { origin: 'https://unlisted.example.test' }),
    ).rejects.toThrow('origin');
    expect(privateReads).toBe(0);
    expect(auth).toEqual([
      'Bearer local-fixture-credential',
      'Bearer local-fixture-credential',
      undefined,
    ]);
  });
  it('writes only counts and projected request evidence, including failures', async () => {
    const baseURL = await server((_req, res) => {
      res.setHeader('content-type', 'application/json');
      res.writeHead(503);
      res.end(
        JSON.stringify({
          code: 'upstream_unavailable',
          detail: 'unpublished private response text',
        }),
      );
    });
    const local = localFixture();
    const run = 'local-evidence-run-123456';
    prepareEvidenceDirectory(local.env.CMA_E2E_OUTPUT_DIR, run)();
    const client = await TC117Client.open(
      { baseURL, output: local.env.CMA_E2E_OUTPUT_DIR },
      local.fixture,
    );
    disposals.push(() => client.close(false));
    await client.send('a', '/api/v1/launches');
    client.counts.collateralAttempts++;
    client.counts.collateralErrors++;
    client.writeEvidence('shared', 'failed', run);
    const text = readFileSync(join(local.env.CMA_E2E_OUTPUT_DIR, 'tc117-shared.json'), 'utf8');
    expect(text).not.toContain('unpublished');
    expect(text).not.toContain('local-fixture-credential');
    expect(JSON.parse(text)).toMatchObject({
      schema: 'cma-tc117-scenario-v1',
      run,
      journalRun: client.journal.run,
      status: 'failed',
      metrics: { collateral_error_rate: 1 },
      counts: { collateralAttempts: 1, collateralErrors: 1 },
      requests: [{ status: 503 }],
    });
    expect(() => client.writeEvidence('other', 'passed', 'wrong-run-123456789')).toThrow(
      'evidence',
    );
  });
});

it('keeps scoped replay correlation as hashes and public receipt fields without persisting command keys', async () => {
  const baseURL = await server((_req, res) => {
    res.writeHead(202, { 'content-type': 'application/json' });
    res.end(
      JSON.stringify({
        id: 'launch-local',
        created_at: '2026-11-13T00:00:01Z',
        public_id: 'public-reuse-a',
        private_detail: 'private response material',
      }),
    );
  });
  const local = localFixture();
  const run = 'tc117-local-correlation-123456';
  prepareEvidenceDirectory(local.env.CMA_E2E_OUTPUT_DIR, run)();
  const client = await TC117Client.open(
    { baseURL, output: local.env.CMA_E2E_OUTPUT_DIR },
    local.fixture,
  );
  disposals.push(() => client.close(false));
  for (let i = 0; i < 2; i++)
    await client.launch('a', local.fixture.triggers.reuseA, { key: 'local-replay-probe' });
  client.writeEvidence('correlation', 'passed', run);
  const text = readFileSync(join(local.env.CMA_E2E_OUTPUT_DIR, 'tc117-correlation.json'), 'utf8');
  const rows = JSON.parse(text).requests;
  expect(rows[0].commandKeyDigest).toMatch(/^[a-f0-9]{64}$/);
  expect(rows[0].commandKeyDigest).toBe(rows[1].commandKeyDigest);
  expect(rows[0].receiptId).toBe('launch-local');
  expect(text).not.toContain('local-replay-probe');
  expect(text).not.toContain('private response material');
});

it('parses frozen budgets only with enough operator-verified egress networks to distinguish the fleet limiter', () => {
  const local = localFixture();
  const proxyFiles = Array.from({ length: 6 }, (_, i) => {
    const path = join(local.directory, `proxy-${i}.json`);
    writeFileSync(
      path,
      JSON.stringify({ server: `http://127.0.0.1:${9000 + i}`, egressNetwork: `network-${i}` }),
      { mode: 0o600 },
    );
    return path;
  });
  const load = {
    frozen: true,
    perPodPerMinute: 1200,
    perPodConcurrency: 16,
    maxServingPods: 2,
    networkPerMinute: 600,
    fleetSandboxLimit: 100,
    belowRps: 5,
    aboveRps: 200,
    durationSeconds: 30,
    proxyFiles,
  };
  const save = (files: string[]) =>
    writeFileSync(
      local.manifest,
      JSON.stringify({
        ...local.document,
        tc117: { ...local.document.tc117, load: { ...load, proxyFiles: files } },
      }),
    );
  save(proxyFiles);
  expect(tc117Fixture(local.env).load?.proxyFiles).toHaveLength(6);
  save(proxyFiles.slice(0, 2));
  expect(() => tc117Fixture(local.env)).toThrow(/^BLOCKED:/);
});

it.each([202, 429])(
  'injects caller response loss only after admission (%s), retaining scoped cleanup ownership',
  async (status) => {
    const baseURL = await server((_req, res) => {
      res.writeHead(status, { 'content-type': 'application/json' });
      res.end(JSON.stringify({ id: 'launch-delivery-loss', created_at: '2026-11-13T23:59:40Z' }));
    });
    const local = localFixture();
    prepareEvidenceDirectory(local.env.CMA_E2E_OUTPUT_DIR, 'tc117-loss-regression-123456')();
    const client = await TC117Client.open(
      { baseURL, output: local.env.CMA_E2E_OUTPUT_DIR },
      local.fixture,
    );
    disposals.push(() => client.close(false));
    await expect(
      client.launchAndLoseResponse('utc', local.fixture.triggers.reuseA, {
        key: 'local-loss-probe',
      }),
    ).rejects.toThrow(
      status === 202 ? 'injected caller response loss' : 'lost-response probe was not admitted',
    );
    expect(client.journal.resources).toEqual(
      status === 202 ? [{ kind: 'launches', id: 'launch-delivery-loss', subject: 'utc' }] : [],
    );
  },
);
