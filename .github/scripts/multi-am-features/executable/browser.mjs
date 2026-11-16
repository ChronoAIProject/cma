// Reuse the parent's leased Chromium. Each actor gets its own context and local
// streaming proxy; the AMs, Web server and provider remain parent-owned.
import http from 'node:http';
import {createHash} from 'node:crypto';
import {NonOwnerRouter} from '../routing.mjs';
import {Blocked} from './runner.mjs';

const check = (value, code) => { if (!value) throw Error(code); };
class ReadCancelled extends Error { constructor() {super('browser_read_cancelled');} }
const hash = value => createHash('sha256').update(value).digest('hex');
const local = value => {
  const url = new URL(value);
  check(url.protocol === 'http:' && ['127.0.0.1', 'localhost', '[::1]'].includes(url.hostname)
    && !url.username && !url.password && url.pathname === '/' && !url.search && !url.hash,
  'browser_local_origin_required');
  return url.origin;
};
const safeHeaders = headers => {
  const copy = {...headers};
  for (const key of ['connection', 'proxy-connection', 'keep-alive', 'transfer-encoding']) delete copy[key];
  return copy;
};

export function createBrowserAdapter({browser, webOrigin, ams, actors, adapter, record,
  compiledSource, compiledManifest, resolveAgent, host, publicOrigin}) {
  check(typeof browser?.newContext === 'function' && typeof record === 'function'
    && typeof resolveAgent === 'function', 'browser_parent_services_required');
  const web = local(webOrigin);
  check(local(ams.a) !== local(ams.b), 'browser_two_distinct_ingresses_required');
  check(/^[a-f0-9]{40}$/.test(compiledSource ?? '')
    && /^sha256:[a-f0-9]{64}$/.test(compiledManifest ?? ''), 'compiled_web_identity_required');
  const sessions = new Map();
  const api = {
    otherAM(am) { check(['a', 'b'].includes(am), 'browser_ingress_invalid'); return am === 'a' ? 'b' : 'a'; },
    async open({actor, am, agentId}) {
      check(['a', 'b'].includes(am), 'browser_ingress_invalid');
      const identity = actors?.[actor];
      if (!identity || (!identity.bearer && !identity.storageState)) throw new Blocked('browser_actor_unavailable');
      const receipts = [], proofs = [], failures = [], pending = new Set(), sockets = new Set(), upstreams = new Set();
      let selectedAm = am, directReads = false, closed = false, context, page;
      const router = new NonOwnerRouter({adapter, record: async proof => {
        proofs.push(proof); await record({kind: 'browser_owner_proof', proof});
      }});
      function track(operation) {
        pending.add(operation);
        operation.then(() => pending.delete(operation), () => pending.delete(operation));
        return operation;
      }
      const server = http.createServer((request, response) => {
        const task = (async () => {
          check(!closed && request.url?.startsWith('/') && !request.url.startsWith('//'), 'browser_proxy_target');
          const path = new URL(request.url, 'http://fixture.invalid');
          path.pathname = path.pathname.replace(/^\/api\/agent-manager\//, '/api/');
          const isApi = path.pathname.startsWith('/api/');
          const owner = isApi ? await resolveAgent({path: path.pathname, method: request.method, agentId}) : null;
          // Resolution must distinguish collection/static routes such as
          // /agents/provenance from actual record and response aliases.
          const dispatch = async (ingress, observedOrigin) => {
            const origin = isApi ? local(observedOrigin ?? ams[ingress]) : web;
            const headers = safeHeaders(request.headers);
            headers.host = isApi && host ? host : new URL(origin).host;
            if (isApi && identity.bearer) {
              headers.authorization = 'Bearer ' + identity.bearer;
              delete headers.cookie;
            }
            if (isApi && publicOrigin) {
              headers.origin = publicOrigin; headers['x-forwarded-proto'] = new URL(publicOrigin).protocol.slice(0, -1);
            }
            const started = performance.now();
            return new Promise((resolve, reject) => {
              const outgoing = http.request(new URL(path.pathname + path.search, origin), {
                method: request.method, headers, timeout: 120000,
              });
              upstreams.add(outgoing);
              outgoing.once('close', () => upstreams.delete(outgoing));
              outgoing.once('error', reject);
              outgoing.once('timeout', () => outgoing.destroy(Error('browser_upstream_timeout')));
              outgoing.once('response', incoming => {
                const receipt = {am: ingress, serving_am: ingress, method: request.method, status: incoming.statusCode,
                  request_id: incoming.headers['x-request-id'] ?? null,
                  elapsed_ms: performance.now() - started,
                  ...(owner ? {agent_id_sha256: hash(owner)} : {})};
                if (isApi) receipts.push(receipt);
                // Keep the original upstream response and never replay, including
                // accepted writes whose later attribution fails. SSE stays streamed.
                response.writeHead(incoming.statusCode, safeHeaders(incoming.headers));
                incoming.pipe(response);
                incoming.once('error', () => response.destroy());
                track(Promise.resolve().then(() => isApi
                  ? record({kind: 'browser_http', receipt, path_sha256: hash(path.pathname)}) : undefined))
                  .catch(() => failures.push('browser_evidence_write_failed'));
                resolve({status: incoming.statusCode, body: null,
                  timing: {am: ingress, request_id: receipt.request_id, body_ms: receipt.elapsed_ms}});
              });
              const abortRead = () => outgoing.destroy(['GET', 'HEAD'].includes(request.method)
                ? new ReadCancelled() : Error('browser_mutation_outcome_unknown'));
              request.once('aborted', abortRead);
              response.once('close', () => {if (!response.writableFinished) abortRead();});
              request.pipe(outgoing);
            });
          };
          const value = owner ? directReads && ['GET', 'HEAD'].includes(request.method)
            ? await router.read(owner, selectedAm, dispatch, request.method)
            : await router.dispatch(owner, dispatch) : await dispatch(selectedAm);
          if (isApi) check(typeof value.timing.request_id === 'string'
            && value.timing.request_id.trim().length > 0, 'browser_request_id_missing');
        })();
        track(task).catch(error => {
          if (error instanceof ReadCancelled) {
            track(Promise.resolve().then(() => record({kind: 'browser_read_cancelled',
              attributed: false, response_received: response.headersSent})))
              .catch(() => failures.push('browser_evidence_write_failed'));
            response.destroy(); return;
          }
          failures.push(/^[a-z][a-z0-9_]+$/.test(error.message ?? '') ? error.message : 'browser_proxy_failed');
          if (!response.headersSent) response.writeHead(502, {'Content-Type': 'text/plain'}).end('Local fixture transport failed');
          else response.destroy();
        });
      });
      server.on('upgrade', (request, socket, head) => {
        const task = (async () => {
          check(!closed && request.url?.startsWith('/') && !request.url.startsWith('//'), 'browser_upgrade_target');
          const path = new URL(request.url, 'http://fixture.invalid');
          path.pathname = path.pathname.replace(/^\/api\/agent-manager\//, '/api/');
          check(path.pathname.startsWith('/api/'), 'browser_upgrade_api_only');
          const owner = await resolveAgent({path: path.pathname, method: 'GET', agentId});
          const dispatch = async (ingress, observedOrigin) => new Promise((resolve, reject) => {
            const origin = local(observedOrigin ?? ams[ingress]), headers = {...request.headers};
            headers.host = host ?? new URL(origin).host;
            if (identity.bearer) {headers.authorization = 'Bearer ' + identity.bearer; delete headers.cookie;}
            if (publicOrigin) headers.origin = publicOrigin;
            const started = performance.now();
            const outgoing = http.request(new URL(path.pathname + path.search, origin), {headers, timeout: 30000});
            upstreams.add(outgoing); outgoing.once('close', () => upstreams.delete(outgoing));
            outgoing.once('error', reject);
            outgoing.once('timeout', () => outgoing.destroy(Error('browser_upgrade_timeout')));
            outgoing.once('response', incoming => {incoming.resume(); reject(Error('browser_upgrade_refused'));});
            outgoing.once('upgrade', (incoming, upstream, upstreamHead) => {
              sockets.add(upstream); upstream.once('close', () => sockets.delete(upstream));
              const receipt = {am: ingress, serving_am: ingress, method: 'GET', status: 101,
                transport: 'websocket', request_id: incoming.headers['x-request-id'] ?? null,
                elapsed_ms: performance.now() - started,
                ...(owner ? {agent_id_sha256: hash(owner)} : {})};
              receipts.push(receipt);
              const lines = ['HTTP/1.1 101 Switching Protocols'];
              for (let i = 0; i < incoming.rawHeaders.length; i += 2) lines.push(incoming.rawHeaders[i] + ': ' + incoming.rawHeaders[i + 1]);
              socket.write(lines.join('\r\n') + '\r\n\r\n');
              if (upstreamHead.length) socket.write(upstreamHead);
              if (head.length) upstream.write(head);
              socket.pipe(upstream); upstream.pipe(socket);
              socket.once('error', () => upstream.destroy()); upstream.once('error', () => socket.destroy());
              socket.once('close', () => upstream.destroy()); upstream.once('close', () => socket.destroy());
              track(Promise.resolve().then(() => record({kind: 'browser_websocket', receipt})))
                .catch(() => failures.push('browser_evidence_write_failed'));
              resolve({status: 101, body: null, timing: {am: ingress, request_id: receipt.request_id}});
            });
            socket.once('close', () => outgoing.destroy()); outgoing.end();
          });
          const value = owner ? await router.dispatch(owner, dispatch) : await dispatch(selectedAm);
          check(typeof value.timing.request_id === 'string' && value.timing.request_id.trim(), 'browser_request_id_missing');
        })();
        track(task).catch(() => {failures.push('browser_upgrade_failed'); socket.destroy();});
      });
      server.on('connection', socket => {sockets.add(socket); socket.once('close', () => sockets.delete(socket));});
      await new Promise((resolve, reject) => {server.once('error', reject); server.listen(0, '127.0.0.1', resolve);});
      const origin = 'http://127.0.0.1:' + server.address().port;
      const close = async () => {
        if (closed) return; closed = true;
        try { await context?.close(); }
        finally {
          for (const outgoing of upstreams) outgoing.destroy(['GET', 'HEAD'].includes(outgoing.method)
            ? new ReadCancelled() : Error('browser_mutation_outcome_unknown'));
          for (const socket of sockets) socket.destroy();
          await new Promise(resolve => server.close(resolve));
          await Promise.allSettled([...pending]);
          // Retain receipts for evidence(page) after close; no live resources remain.
        }
      };
      try {
        context = await browser.newContext({storageState: identity.storageState,
          serviceWorkers: 'block', acceptDownloads: true,
          permissions: ['clipboard-read', 'clipboard-write']});
        await context.route('**/*', route => {
          const target = new URL(route.request().url());
          return ['data:', 'blob:'].includes(target.protocol) || target.origin === origin
            || (identity.allowedOrigins ?? []).includes(target.origin)
            ? route.continue() : route.abort('blockedbyclient');
        });
        page = await context.newPage();
        sessions.set(page, {receipts, proofs, failures, pending, close,
          authentication: identity.bearer ? 'real_am_bearer' : 'browser_session'});
        return {page, origin, async switchAM(nextAm) {
          check(!closed && ['a', 'b'].includes(nextAm), 'browser_switch_invalid');
          await Promise.allSettled([...pending]); selectedAm = nextAm; directReads = true;
        }, close};
      } catch (error) { await close(); throw error; }
    },
    async evidence(page) {
      const session = sessions.get(page); check(session, 'browser_session_unknown');
      await Promise.allSettled([...session.pending]);
      const requests = session.receipts.map(receipt => {
        const proof = session.proofs.find(value => value.request_id === receipt.request_id);
        return {...receipt, ...(proof ? {owner: proof.owner, owner_epoch: proof.owner_epoch,
          owner_stable: proof.verdict === 'PASS', owner_proof: proof} : {})};
      });
      check(session.failures.length === 0, session.failures[0] ?? 'browser_evidence_failed');
      return {schema: 'multi-am-browser-evidence-v1', real_ui: true, compiled_source: compiledSource,
        compiled_manifest: compiledManifest, authentication: session.authentication,
        secrets_redacted: true, requests};
    },
    async close() { for (const session of sessions.values()) await session.close(); sessions.clear(); },
  };
  return api;
}
