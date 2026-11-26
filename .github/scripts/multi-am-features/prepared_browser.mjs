import {randomUUID} from 'node:crypto';
import {createRequire} from 'node:module';
import {fileURLToPath, pathToFileURL} from 'node:url';
import {assertOracle, check, hash, id, poll} from './cma/core.mjs';
import {withRestoration} from './failure.mjs';

const webRoot = fileURLToPath(new URL('../../../web/', import.meta.url));
const webRequire = createRequire(new URL('../../../web/package.json', import.meta.url));

function loopback(value) {
  const url = new URL(value);
  check(url.protocol === 'http:' && url.hostname === '127.0.0.1' && url.port
    && url.pathname === '/' && !url.username && !url.password && !url.search && !url.hash,
  'prepared_browser_requires_pod_forward');
  return url.origin;
}

export async function openPreparedBrowser(config) {
  check(typeof config.browser_session_cookie === 'string' && config.browser_session_cookie.length > 0,
    'prepared_browser_session_required');
  const ams = Object.fromEntries(['a', 'b'].map(am => [am, loopback(config.am_origins[am])]));
  const publicOrigin = new URL(config.public_origin);
  check(publicOrigin.protocol === 'https:' && publicOrigin.host === config.host
    && publicOrigin.href === publicOrigin.origin + '/', 'prepared_browser_invalid_public_origin');
  const {preview} = await import(pathToFileURL(webRequire.resolve('vite')));
  const {chromium} = webRequire('@playwright/test');
  const browser = await chromium.launch({headless: true, timeout: 20000});
  return {
    close: () => browser.close(),
    async forIngress(am, run) {
      check(Object.hasOwn(ams, am), 'prepared_browser_invalid_ingress');
      const server = await preview({root: webRoot, configFile: false,
        preview: {host: '127.0.0.1', port: 0, strictPort: true, proxy: {'^/(?:(?:api|auth)/|openapi\\.json(?:\\?|$))': {
          target: ams[am],
          configure(proxy) {
            proxy.on('proxyReq', (outgoing, incoming) => {
              outgoing.setHeader('Host', publicOrigin.host);
              if (incoming.headers.origin) outgoing.setHeader('Origin', publicOrigin.origin);
            });
          },
        }}}});
      let context;
      try {
        const origin = 'http://127.0.0.1:' + server.httpServer.address().port;
        context = await browser.newContext({baseURL: origin, serviceWorkers: 'block'});
        await context.route('**/*', route => new URL(route.request().url()).origin === origin
          ? route.continue() : route.abort('blockedbyclient'));
        await context.addCookies([{name: 'cma_session', value: config.browser_session_cookie, url: origin,
          httpOnly: true, secure: true, sameSite: 'Lax'}]);
        const page = await context.newPage();
        await page.goto('/', {waitUntil: 'domcontentloaded', timeout: 20000});
        const session = await page.evaluate(async () => {
          const response = await fetch('/auth/session', {credentials: 'same-origin', redirect: 'error'});
          if (!response.ok) return null;
          const value = await response.json();
          return {authenticated: value.authenticated, subject: value.subject};
        });
        check(session?.authenticated === true && session.subject === config.owner_id,
          'prepared_browser_identity_unbound');
        const info = await context.request.get('/build-info.json', {maxRedirects: 0, timeout: 5000});
        check(info.ok() && (await info.json()).revision === config.expected_api_source, 'prepared_browser_source_mismatch');
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
    const observed = waitForStatus('starting', 30000);
    this.ready = waitForStatus('ready', 180000).then(response => ({response}), error => ({error}));
    const [starting] = await Promise.all([
      observed, this.page.goto(path, {waitUntil: 'domcontentloaded', timeout: 20000}),
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
    let sendRequest;
    await adapter.prepareTurn({caseId, marker});
    await withRestoration(async () => {
      const composer = this.page.getByRole('textbox', {name: 'Message', exact: true});
      await composer.fill(input, {timeout: 15000});
      await adapter.releaseTurn({caseId});
      const [submitted] = await Promise.all([
        this.page.waitForResponse(response => {
          const request = response.request();
          return request.method() === 'POST' && new URL(response.url()).pathname === '/api/v1/agui/run'
            && request.postDataJSON()?.threadId === agentId;
        }, {timeout: 30000}),
        this.page.getByRole('button', {name: 'Send message', exact: true}).click({timeout: 15000}),
      ]);
      check(submitted.ok(), 'prepared_browser_send_refused');
      sendRequest = await requestHash(submitted);
      await this.page.getByText(answer, {exact: true}).waitFor({state: 'visible', timeout: 90000});
      const settled = await poll(() => adapter.oracle({caseId}), value => value.active_streams === 0,
        {timeout: 20000, code: 'prepared_browser_model_still_active'});
      assertOracle(settled, caseId, 1);
    }, () => adapter.releaseTurn({caseId}));
    return {answer_sha256: hash(answer), effects: 1, ready_request_sha256: readyRequest,
      send_request_sha256: sendRequest};
  }
}

async function requestHash(response) {
  const requestId = await response.headerValue('x-request-id');
  check(typeof requestId === 'string' && requestId.trim().length > 0, 'prepared_browser_request_id_missing');
  return hash(requestId);
}
