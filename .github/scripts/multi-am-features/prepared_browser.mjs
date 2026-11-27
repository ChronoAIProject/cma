import {randomUUID} from 'node:crypto';
import {createRequire} from 'node:module';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {assertOracle, check, hash, id, poll, safeError} from './cma/core.mjs';
import {rateLimitEvidence, withRestoration} from './failure.mjs';
import {requestPacer} from './pacing.mjs';

const webRoot = fileURLToPath(new URL('../../../web/', import.meta.url));
const webRequire = createRequire(new URL('../../../web/package.json', import.meta.url));

async function browserOperation(code, operation) {
  try {
    return await operation();
  } catch (error) {
    throw safeError(error) === 'transport_or_adapter_error' ? new Error(code, {cause: error}) : error;
  }
}

function loopback(value) {
  const url = new URL(value);
  check(url.protocol === 'http:' && url.hostname === '127.0.0.1' && url.port
    && url.pathname === '/' && !url.username && !url.password && !url.search && !url.hash,
  'prepared_browser_requires_pod_forward');
  return url.origin;
}

export async function routeFixtureBrowserRequest(route, origin) {
  if (new URL(route.request().url()).origin !== origin) return route.abort('blockedbyclient');
  return route.continue();
}

function sessionCookie(header) {
  return header?.split(';').map(part => part.trim()).find(part => part.startsWith('cma_session='))
    ?.slice('cma_session='.length);
}

export function browserApiPacing(config, freshSession) {
  const owner = config.bearer ?? config.auth?.bearer;
  const sessionCredentials = new Map([[config.browser_session_cookie, owner]]);
  if (config.recipient_browser_session_cookie && config.admin_bearer) {
    sessionCredentials.set(config.recipient_browser_session_cookie, config.admin_bearer);
  }
  let freshOwnerCookie;
  return {
    rememberOwnerSession(headers) {
      if (!freshSession) return;
      for (const header of headers ?? []) {
        const cookie = sessionCookie(header.split(';', 1)[0]);
        if (cookie) freshOwnerCookie = cookie;
      }
    },
    configurePreviewServer(server) {
      // Middleware runs before Vite proxies the original request, including
      // browser routes that forward directly and APIRequestContext requests.
      server.middlewares.use((request, response, next) => {
        if (!request.url?.startsWith('/api/')) return next();
        const cookie = sessionCookie(request.headers.cookie);
        const credential = cookie && cookie === freshOwnerCookie ? owner : sessionCredentials.get(cookie);
        if (!credential) return next();
        requestPacer.wait(credential, request.method).then(() => {
          if (!response.destroyed) next();
        }, next);
      });
    },
  };
}

export async function openPreparedBrowser(config) {
  check(typeof config.browser_session_cookie === 'string' && config.browser_session_cookie.length > 0,
    'prepared_browser_session_required');
  const ams = Object.fromEntries(['a', 'b'].map(am => [am, loopback(config.am_origins[am])]));
  const gateway = config.eg_origin === undefined ? undefined : loopback(config.eg_origin);
  const publicOrigin = new URL(config.public_origin);
  check(publicOrigin.protocol === 'https:' && publicOrigin.host === config.host
    && publicOrigin.href === publicOrigin.origin + '/', 'prepared_browser_invalid_public_origin');
  const {preview} = await import(pathToFileURL(webRequire.resolve('vite')));
  const {chromium} = webRequire('@playwright/test');
  const browser = await chromium.launch({headless: true, timeout: 20000});
  return {
    close: () => browser.close(),
    async forIngress(am, run, {freshSession = false} = {}) {
      check(Object.hasOwn(ams, am), 'prepared_browser_invalid_ingress');
      const pacing = browserApiPacing(config, freshSession);
      const proxyRoutes = {'^/(?:(?:api|auth)/|openapi\\.json(?:\\?|$))': {
        target: ams[am],
        configure(proxy) {
          proxy.on('proxyReq', (outgoing, incoming) => {
            outgoing.setHeader('Host', publicOrigin.host);
            if (incoming.headers.origin) outgoing.setHeader('Origin', publicOrigin.origin);
          });
          proxy.on('proxyRes', (incoming, request) => {
            const path = new URL(request.url, publicOrigin).pathname;
            if (incoming.statusCode === 429) {
              config.observeBrowserRateLimit?.(am,
                rateLimitEvidence(request.method, path, incoming.headers['retry-after']));
            }
            if (!freshSession) return;
            if (path === '/auth/callback' && incoming.statusCode >= 200 && incoming.statusCode < 400) {
              pacing.rememberOwnerSession(incoming.headers['set-cookie']);
            }
            if (path !== '/auth/login' || !incoming.headers.location) return;
            const authorize = new URL(incoming.headers.location, publicOrigin);
            if (authorize.origin === 'http://identity:8080' && authorize.pathname === '/authorize') {
              incoming.headers.location = '/fixture/authorize' + authorize.search;
            }
          });
        },
      }};
      if (gateway) proxyRoutes['^/event-gateway/openapi\\.json(?:\\?|$)'] = {
        target: gateway,
        rewrite: path => path.replace(/^\/event-gateway/, ''),
      };
      if (freshSession) proxyRoutes['^/fixture/authorize(?:\\?|$)'] = {
        target: loopback(config.identity_origin),
        rewrite: path => path.replace('/fixture/authorize', '/authorize'),
        configure(proxy) {
          proxy.on('proxyRes', incoming => {
            if (!incoming.headers.location) return;
            const callback = new URL(incoming.headers.location, publicOrigin);
            if (callback.origin === publicOrigin.origin && callback.pathname === '/auth/callback') {
              incoming.headers.location = callback.pathname + callback.search;
            }
          });
        },
      };
      const server = await browserOperation('prepared_browser_preview_failed', () =>
        preview({root: webRoot, configFile: false,
          plugins: [{name: 'fixture-browser-api-pacing', configurePreviewServer: pacing.configurePreviewServer}],
          preview: {host: '127.0.0.1', port: 0, strictPort: true, proxy: proxyRoutes}}));
      let context;
      try {
        const origin = 'http://127.0.0.1:' + server.httpServer.address().port;
        context = await browser.newContext({baseURL: origin, serviceWorkers: 'block'});
        await context.route('**/*', route => routeFixtureBrowserRequest(route, origin));
        if (!freshSession) {
          await context.addCookies([{name: 'cma_session', value: config.browser_session_cookie, url: origin,
            httpOnly: true, secure: true, sameSite: 'Lax'}]);
          const sessionPage = await context.newPage();
          await browserOperation('prepared_browser_session_navigation_failed', () =>
            sessionPage.goto('/', {waitUntil: 'domcontentloaded', timeout: 20000}));
          const session = await browserOperation('prepared_browser_session_read_failed', () => sessionPage.evaluate(async () => {
            const response = await fetch('/auth/session', {credentials: 'same-origin', redirect: 'error'});
            if (!response.ok) return null;
            const value = await response.json();
            return {authenticated: value.authenticated, subject: value.subject};
          }));
          check(session?.authenticated === true && session.subject === config.owner_id,
            'prepared_browser_identity_unbound');
          // The home page can prefetch an Agent or Sandbox before the test opens it.
          await sessionPage.close();
        }
        const revision = await browserOperation('prepared_browser_build_info_failed', async () => {
          const info = await context.request.get('/build-info.json', {maxRedirects: 0, timeout: 5000});
          return info.ok() ? (await info.json()).revision : null;
        });
        check(revision === config.expected_api_source, 'prepared_browser_source_mismatch');
        const page = await context.newPage();
        return await run(new PreparedAgentPage(page, origin));
      } finally {
        try { await context?.close(); }
        finally {
          server.httpServer.closeAllConnections?.();
          await new Promise((resolve, reject) => server.httpServer.close(error => error ? reject(error) : resolve()));
        }
      }
    },
  };
}

class PreparedAgentPage {
  constructor(page, origin) {
    this.page = page;
    this.origin = origin;
  }

  async openStarting(agentId) {
    check(id(agentId), 'prepared_browser_agent_identity_missing');
    const path = '/agents/' + agentId;
    const waitForStatus = (status, timeout) => this.page.waitForResponse(async response => {
      if (response.status() !== 200 || response.request().method() !== 'GET'
          || new URL(response.url()).pathname !== '/api/v1' + path) return false;
      const body = await response.json();
      return body?.id === agentId && body.status === status
        && (status !== 'ready' || body.sandbox?.status === 'active');
    }, {timeout});
    const observed = browserOperation('prepared_browser_starting_read_failed', () => waitForStatus('starting', 30000));
    this.ready = browserOperation('prepared_browser_ready_read_failed', () => waitForStatus('ready', 180000))
      .then(response => ({response}), error => ({error}));
    const [starting] = await Promise.all([
      observed, browserOperation('prepared_browser_agent_navigation_failed', () =>
        this.page.goto(path, {waitUntil: 'domcontentloaded', timeout: 20000})),
    ]);
    check(this.page.url() === this.origin + path, 'prepared_browser_open_changed_route');
    return requestHash(starting);
  }

  async sendAndRead(agentId, adapter) {
    const caseId = 'prepared-browser-' + randomUUID();
    const marker = 'WS4_' + randomUUID().replaceAll('-', '');
    const input = 'Return the exact marker ' + marker + ' without tools or file changes.';
    const answer = 'WS4 complete ' + marker;
    check(this.page.url() === this.origin + '/agents/' + agentId, 'prepared_browser_send_wrong_agent');
    const ready = await this.ready;
    if (ready?.error) throw ready.error;
    check(ready?.response, 'prepared_browser_ready_read_unproven');
    const readyRequest = await requestHash(ready.response);
    let sendRequest, sendTransport;
    await adapter.prepareTurn({caseId, marker});
    await withRestoration(async () => {
      const composer = this.page.getByRole('textbox', {name: 'Message', exact: true});
      await browserOperation('prepared_browser_composer_fill_failed', () => composer.fill(input, {timeout: 15000}));
      await adapter.releaseTurn({caseId});
      const [sent, submitted] = await Promise.all([
        browserOperation('prepared_browser_send_request_missing', () => this.page.waitForRequest(
          request => isPreparedSend(request, agentId, input), {timeout: 20000})),
        browserOperation('prepared_browser_send_response_missing', () => this.page.waitForResponse(
          response => isPreparedSend(response.request(), agentId, input), {timeout: 30000})),
        browserOperation('prepared_browser_send_click_failed', () =>
          this.page.getByRole('button', {name: 'Send message', exact: true}).click({timeout: 15000})),
      ]);
      check(submitted.request() === sent, 'prepared_browser_send_response_unbound');
      check(submitted.ok(), 'prepared_browser_send_refused');
      sendTransport = new URL(sent.url()).pathname === '/api/v1/agui/run' ? 'agui' : 'responses_v2';
      sendRequest = await requestHash(submitted);
      await browserOperation('prepared_browser_answer_render_failed', () =>
        this.page.getByText(answer, {exact: true}).waitFor({state: 'visible', timeout: 90000}));
      const settled = await poll(() => adapter.oracle({caseId}), value => value.active_streams === 0,
        {timeout: 20000, code: 'prepared_browser_model_still_active'});
      assertOracle(settled, caseId, 1);
    }, () => adapter.releaseTurn({caseId}));
    return {answer_sha256: hash(answer), effects: 1, ready_request_sha256: readyRequest,
      send_request_sha256: sendRequest, transport: sendTransport};
  }
}

export function isPreparedSend(request, agentId, input) {
  if (request.method() !== 'POST') return false;
  const path = new URL(request.url()).pathname;
  if (path === `/api/v2/agents/${agentId}/responses`) {
    const body = request.postDataJSON();
    return body?.input === input && body.stream === true;
  }
  if (path === '/api/v1/agui/run') {
    const body = request.postDataJSON();
    return body?.threadId === agentId && Array.isArray(body.messages) && body.messages.length === 1
      && body.messages[0]?.role === 'user' && body.messages[0].content === input;
  }
  return false;
}

async function requestHash(response) {
  const requestId = await response.headerValue('x-request-id');
  check(typeof requestId === 'string' && requestId.trim().length > 0, 'prepared_browser_request_id_missing');
  return hash(requestId);
}
