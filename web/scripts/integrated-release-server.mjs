import assert from 'node:assert/strict';
import { createServer, request as httpRequest } from 'node:http';
import { readFile, writeFile, rename } from 'node:fs/promises';
import { existsSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { createServer as createViteServer, preview } from 'vite';

const directory = process.env.CMA_DESKTOP_RELAY_DIRECTORY;
assert(directory);
const ready = JSON.parse(await readFile(join(directory, 'public-ready.json'), 'utf8'));
const requests = [];
const active = new Set();
let traceWrites = Promise.resolve();
function persistTrace() {
  const body = JSON.stringify(requests, null, 2);
  traceWrites = traceWrites.then(async () => {
    await writeFile(join(directory, 'public-request-trace.tmp'), body);
    await rename(
      join(directory, 'public-request-trace.tmp'),
      join(directory, 'public-request-trace.json'),
    );
  });
}
const webRoot = resolve(import.meta.dirname, '..');
const vite = process.env.CMA_INTEGRATED_WEB_DIST
  ? await preview({
      root: webRoot,
      configLoader: 'runner',
      build: { outDir: process.env.CMA_INTEGRATED_WEB_DIST },
      preview: { host: '127.0.0.1', port: 0, open: false },
    })
  : await createViteServer({
      root: webRoot,
      configLoader: 'runner',
      server: { middlewareMode: true },
      appType: 'spa',
    });
const server = createServer((incoming, outgoing) => {
  const url = new URL(incoming.url, 'http://127.0.0.1');
  const cookies = new Map(
    (incoming.headers.cookie ?? '').split(';').map((value) => value.trim().split('=')),
  );
  const subject = cookies.get('integrated-account') ?? 'owner';
  assert(['owner', 'other'].includes(subject));
  const revoked = cookies.get('integrated-revoked') === 'true';
  const modePath = join(directory, 'policy-mode.json');
  const mode = existsSync(modePath) ? JSON.parse(readFileSync(modePath, 'utf8')).mode : 'enabled';
  assert(['enabled', 'disabled'].includes(mode));
  const publicOrigin = mode === 'disabled' ? ready.disabled_origin : ready.origin;
  assert(
    publicOrigin,
    'The actual disabled public composition must be running before policy cutover',
  );
  const json = (value) =>
    outgoing
      .writeHead(200, { 'content-type': 'application/json', 'cache-control': 'private, no-store' })
      .end(JSON.stringify(value));
  if (url.pathname === '/auth/session')
    return json({
      authenticated: true,
      subject,
      display_name: 'Integrated proof owner',
      capabilities: revoked
        ? []
        : ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox', 'manage_own_profiles'],
    });
  if (url.pathname === '/auth/connections/check')
    return json({
      owner: subject,
      ready: true,
      checked_at: new Date().toISOString(),
      services: [],
    });
  if (!url.pathname.startsWith('/api/') && url.pathname !== '/openapi.json')
    return vite.middlewares(incoming, outgoing);
  const path = (url.pathname + url.search).replace('/api/agent-manager/v1/', '/api/v1/');
  const record = {
    id: requests.length,
    method: incoming.method,
    path: url.pathname,
    started: Date.now(),
    overlapping: [...active].map((entry) => entry.id),
    scope: Object.fromEntries(
      [...url.searchParams].filter(([key]) =>
        ['runtime_incarnation', 'context_id', 'client_id', 'operation_id', 'observer'].includes(
          key,
        ),
      ),
    ),
    policyMode: mode,
    subject,
    revoked,
  };
  requests.push(record);
  active.add(record);
  persistTrace();
  const complete = () => {
    if (!active.delete(record)) return;
    Object.assign(record, {
      finished: Date.now(),
      status: outgoing.statusCode,
      responseComplete: outgoing.writableFinished,
    });
    persistTrace();
  };
  outgoing.once('finish', complete);
  outgoing.once('close', complete);
  const proxy = httpRequest(
    publicOrigin + path,
    {
      method: incoming.method,
      headers: {
        ...incoming.headers,
        authorization: 'Bearer local-sdk-conformance',
        'x-integrated-subject': subject,
        'x-integrated-revoked': String(revoked),
        host: new URL(publicOrigin).host,
      },
    },
    (response) => {
      outgoing.writeHead(response.statusCode, response.headers);
      response.pipe(outgoing);
    },
  );
  proxy.on('error', (error) => {
    console.error(error);
    if (!outgoing.headersSent) outgoing.writeHead(502);
    outgoing.end();
  });
  outgoing.on('close', () => proxy.destroy());
  incoming.pipe(proxy);
});
await new Promise((resolve) => server.listen(4319, '127.0.0.1', resolve));
console.log('Current Web and complete public proxy ready on 4319');
const close = async () => {
  server.closeAllConnections();
  await vite.close();
  await traceWrites;
  server.close(() => process.exit());
};
process.on('SIGTERM', close);
process.on('SIGINT', close);
