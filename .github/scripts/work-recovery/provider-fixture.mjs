// Local-only synthetic dependencies; no real provider, cluster or process control.
import { randomBytes, timingSafeEqual } from 'node:crypto';
import { createServer } from 'node:http';
import { openLedger } from './provider-ledger.mjs';
import { fixtureIdentity, scopePlan, validateMint } from './provider-protocol.mjs';

export async function startProviderFixture({ root, bearer, port = 0, holdMs = 60000 }) {
  if (typeof bearer !== 'string' || !/^[a-f0-9]{64}$/.test(bearer)
    || !Number.isInteger(port) || port < 0 || port > 65535
    || !Number.isInteger(holdMs) || holdMs < 1 || holdMs > 120000) {
    throw new Error('invalid_fixture_configuration');
  }
  const ledger = openLedger(root);
  const pending = new Map();
  let next = ledger.nextRequest(), closing = false, failed = false;
  let modes = { preparation: 'respond', mint: 'respond', listing: 'metadata' };
  const reply = (response, status, body) => {
    if (response.destroyed) return false;
    response.writeHead(status, { 'content-type': 'application/json', 'cache-control': 'no-store' });
    response.end(JSON.stringify(body));
    return true;
  };
  function record(kind, data) {
    try { return ledger.append(kind, data); }
    catch { failed = true; throw new Error('fixture_ledger_unavailable'); }
  }
  function hold(request, phase, response, release, name) {
    if (pending.size >= 32) { failed = true; throw new Error('fixture_hold_capacity'); }
    record(phase, { request, ...(name ? { name } : {}) });
    const timer = setTimeout(() => {
      pending.delete(request);
      try {
        record('response_dropped', { request });
        response.destroy();
      } catch { response.destroy(); }
    }, holdMs);
    pending.set(request, { phase, response, release, timer });
  }
  async function readBody(request) {
    const parts = [];
    let size = 0;
    for await (const part of request) {
      size += part.length;
      if (size > 16384) throw new Error('fixture_body_too_large');
      parts.push(part);
    }
    return JSON.parse(Buffer.concat(parts).toString('utf8'));
  }
  const server = createServer(async (request, response) => {
    const ordinal = next++;
    const isMint = request.method === 'POST' && request.url === '/api/v1/api-keys';
    let accepted = false, refused = false;
    try {
      if (closing || failed) { reply(response, 503, { error: 'fixture_unavailable' }); return; }
      if (isMint) record('mint_received', { request: ordinal });
      const authorization = Buffer.from(request.headers.authorization ?? '');
      const expected = Buffer.from(`Bearer ${bearer}`);
      if (authorization.length !== expected.length || !timingSafeEqual(authorization, expected)) {
        if (isMint) { record('mint_refused', { request: ordinal }); refused = true; }
        reply(response, 401, { error: 'unauthorized' }); return;
      }
      if (request.method === 'GET' && ['/api/v1/user-services', '/api/v1/services'].includes(request.url)) {
        const send = () => reply(response, 200, [{ id: fixtureIdentity.service,
          slug: fixtureIdentity.slug, is_active: true }]);
        if (modes.preparation === 'hold') hold(ordinal, 'preparation_held', response, send);
        else send();
      } else if (request.method === 'POST' && request.url === '/api/v1/api-keys/scope-plan') {
        reply(response, 200, scopePlan(await readBody(request)));
      } else if (isMint) {
        const body = validateMint(await readBody(request));
        const responseMode = modes.mint;
        const accept = () => {
          const id = `fixture-key-${randomBytes(16).toString('hex')}`;
          // Every POST creates a fresh physical key, including identical names
          // and payloads. Artificial receiver deduplication would hide a retry.
          record('mint_accepted', { request: ordinal, name: body.name, key_id: id });
          accepted = true;
          const fullKey = `synthetic-${randomBytes(32).toString('hex')}`;
          const send = () => {
            const sent = reply(response, 200, { id, full_key: fullKey, state_version: 1,
              scopes: body.scopes, allowed_service_ids: body.allowed_service_ids,
              allowed_node_ids: body.allowed_node_ids, allow_all_services: false,
              allow_all_nodes: false, allow_auto_connected_services: false });
            // This records enqueueing a response, never proof the client saw it.
            record(sent ? 'response_sent' : 'response_dropped', { request: ordinal });
          };
          if (responseMode === 'hold_response') {
            hold(ordinal, 'response_held', response, send, body.name);
          } else send();
        };
        if (responseMode === 'hold_acceptance') {
          // A disconnected client does not cancel provider work. A controller
          // can release this after a negative listing to model late acceptance.
          hold(ordinal, 'acceptance_held', response, accept, body.name);
        } else accept();
      } else if (request.method === 'GET' && request.url === '/api/v1/api-keys') {
        if (modes.listing === 'unavailable') reply(response, 503, { error: 'unavailable' });
        else if (modes.listing === 'malformed') reply(response, 200, { keys: null });
        else reply(response, 200, { keys: modes.listing === 'empty' ? [] : ledger.keys() });
      } else if (request.method === 'DELETE'
        && /^\/api\/v1\/api-keys\/fixture-key-[a-f0-9]{32}$/.test(request.url)) {
        const id = request.url.split('/').at(-1);
        if (!ledger.keys().some(key => key.id === id)) reply(response, 404, { error: 'absent' });
        else {
          record('key_revoked', { request: ordinal, key_id: id });
          response.writeHead(204).end();
        }
      } else reply(response, 404, { error: 'unsupported_fixture_route' });
    } catch {
      // Never retain exception text, headers or bodies in output. Failure of
      // the ledger poisons this fixture; it cannot quietly restart counting.
      if (isMint && !accepted && !refused && !failed) {
        try { record('mint_refused', { request: ordinal }); } catch { failed = true; }
      }
      if (!response.headersSent) reply(response, failed ? 503 : 400, { error: 'fixture_request_refused' });
      else response.destroy();
    }
  });
  server.requestTimeout = 5000;
  server.headersTimeout = 5000;
  server.maxHeadersCount = 32;
  server.on('clientError', (_error, socket) => socket.destroy());
  try {
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(port, '127.0.0.1', () => { server.removeListener('error', reject); resolve(); });
    });
  } catch (error) { ledger.close(); throw error; }
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    identity: fixtureIdentity,
    configure(value) {
      const allowed = { preparation: ['respond', 'hold'],
        mint: ['respond', 'hold_response', 'hold_acceptance'],
        listing: ['metadata', 'empty', 'unavailable', 'malformed'] };
      if (!value || typeof value !== 'object' || Array.isArray(value)
        || Object.entries(value).some(([key, setting]) => !allowed[key]?.includes(setting))
        || closing || failed) throw new Error('invalid_fixture_mode');
      modes = { ...modes, ...value };
    },
    release(request, action = 'respond') {
      const entry = pending.get(request);
      if (!entry || !['respond', 'drop'].includes(action) || closing || failed) {
        throw new Error('invalid_fixture_release');
      }
      pending.delete(request); clearTimeout(entry.timer);
      if (action === 'drop') {
        record('response_dropped', { request }); entry.response.destroy();
      } else entry.release();
    },
    snapshot: () => ({ ...ledger.snapshot(), failed, modes: { ...modes },
      pending: [...pending].map(([request, { phase }]) => ({ request, phase })),
      caller_identity: 'unattributed; separate AM/store/process observations required' }),
    async close() {
      if (closing) return;
      closing = true;
      for (const { timer, response } of pending.values()) { clearTimeout(timer); response.destroy(); }
      const unresolved = [...pending].map(([request, { phase }]) => ({ request, phase }));
      pending.clear();
      server.closeAllConnections();
      await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
      try {
        record('fixture_closed');
        return { ...ledger.snapshot(), failed, pending_at_close: unresolved };
      } finally { ledger.close(); }
    },
  };
}
