import { randomBytes } from 'node:crypto';
import { mkdtempSync, rmSync } from 'node:fs';
import { request } from 'node:http';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { setImmediate } from 'node:timers/promises';
import { startProviderFixture } from './provider-fixture.mjs';
import { fixtureIdentity, scopePlan } from './provider-protocol.mjs';

export async function fixture(t, options = {}) {
  const root = mkdtempSync(join(tmpdir(), 'ordinary-work-provider-'));
  const bearer = randomBytes(32).toString('hex');
  const server = await startProviderFixture({ root, bearer, ...options });
  t.after(async () => { await server.close(); rmSync(root, { recursive: true }); });
  return { ...server, root, bearer };
}

export function mintBody() {
  const plan = scopePlan({ selected_service_ids: [fixtureIdentity.service] });
  return { name: `cma:${fixtureIdentity.deployment}:mint_${randomBytes(16).toString('hex')}`,
    scopes: 'proxy read', expires_at: new Date(Date.now() + 86400000).toISOString(),
    allowed_service_ids: plan.allowed_service_ids, allowed_node_ids: plan.allowed_node_ids,
    allow_all_services: false, allow_all_nodes: false, allow_auto_connected_services: false,
    scope_plan_digest: plan.normalized_grant_digest };
}

// Each request uses an independent HTTP connection. It does not stand in for
// an independent AM or a durable work claimant.
export function call(server, method, path, body, bearer = server.bearer) {
  let resolve;
  const result = new Promise(done => { resolve = done; });
  const req = request(server.origin + path, { method, agent: false,
    headers: { authorization: `Bearer ${bearer}`, 'content-type': 'application/json' } }, response => {
    const parts = [];
    response.on('data', part => parts.push(part));
    response.on('end', () => resolve({ status: response.statusCode,
      text: Buffer.concat(parts).toString('utf8') }));
    response.on('error', () => resolve({ transportError: true }));
  });
  req.setTimeout(5000, () => req.destroy());
  req.on('error', () => resolve({ transportError: true }));
  req.end(body === undefined ? undefined : typeof body === 'string' ? body : JSON.stringify(body));
  return { result, abort: () => req.destroy() };
}

export async function barrier(server, phase) {
  const deadline = performance.now() + 3000;
  while (performance.now() < deadline) {
    const row = server.snapshot().pending.find(item => item.phase === phase);
    if (row) return row.request;
    await setImmediate();
  }
  throw new Error('fixture_barrier_not_reached');
}
