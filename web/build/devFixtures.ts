import type { Plugin } from 'vite';
import { agentId, agentReads, eventsAfter, profileRead } from './fixtures/agent';
import {
  channelActivityFrame,
  channelCapabilities,
  cmaSession,
  gatewaySession,
  localOwner,
  teamChannel,
} from './fixtures/eventGateway';

/*
 * `npm run dev:fixtures` — the Vite dev server answers `/auth` and `/api`
 * from local fixtures so pages can be reviewed without a CMA backend, NyxID
 * sign-in or production data. It exists only in `vite serve --mode fixtures`;
 * production builds, tests and plain `npm run dev` never load it.
 *
 * Coverage is deliberately page by page: Event Gateway (`/event-gateway`) and
 * one Agent conversation (`/agents/agt_contract`). A read without a fixture
 * returns a typed 404 and is logged once, so a missing fixture is visible
 * rather than silently rendered as HTML. Writes are refused: nothing persists.
 */

export type FixtureReply =
  { readonly status: number; readonly json: unknown } | { readonly stream: string };

let reads: Readonly<Record<string, unknown>> | undefined;
function exactReads(): Readonly<Record<string, unknown>> {
  reads ??= {
    // Shell
    '/auth/session': cmaSession,
    '/auth/connections/check': {
      owner: localOwner.subject,
      ready: true,
      checked_at: '2026-10-27T00:00:00Z',
      services: [],
    },
    '/api/v1/bots': { items: [], next_after: null },
    '/api/v1/organizations': { items: [] },
    // Event Gateway
    '/api/event-gateway/auth/session': gatewaySession,
    '/api/event-gateway/v1/channel-capabilities': channelCapabilities,
    '/api/event-gateway/v1/connections': { items: [] },
    '/api/event-gateway/v1/channels': { items: [teamChannel], next_cursor: null },
    // Agent Manager
    ...agentReads(),
  };
  return reads;
}

const activityStream = /^\/api\/event-gateway\/v1\/channels\/[^/]+\/activity\/stream$/;

/** The fixture answer for one request, or `undefined` to leave it to Vite. */
export function fixtureReply(method: string, path: string, search = ''): FixtureReply | undefined {
  if (path !== '/auth' && !path.startsWith('/auth/') && !path.startsWith('/api/')) return undefined;
  // A POST that only reports connection readiness; it changes nothing.
  if (path === '/auth/connections/check') return { status: 200, json: exactReads()[path] };
  if (method !== 'GET' && method !== 'HEAD')
    return {
      status: 501,
      json: {
        code: 'fixture_read_only',
        detail: `Local fixture mode does not accept ${method} ${path}.`,
      },
    };
  if (activityStream.test(path)) return { stream: channelActivityFrame };
  // The finished conversation has no live frames; history comes from the events below.
  if (path === '/api/v1/agui/events') return { stream: ': fixture replay has no new frames\n\n' };
  // A server without v2 transport selection keeps the established Agent transport.
  if (path === '/api/v2/ui/transport')
    return { status: 404, json: { code: 'not_found', detail: 'Unknown optional endpoint.' } };
  if (path === `/api/v1/agents/${agentId}/events`)
    return {
      status: 200,
      json: eventsAfter(Number(new URLSearchParams(search).get('after') ?? 0)),
    };
  const json = exactReads()[path] ?? profileRead(path);
  return json === undefined
    ? {
        status: 404,
        json: { code: 'fixture_missing', detail: `No local fixture for GET ${path}.` },
      }
    : { status: 200, json };
}

export function devFixtures(): Plugin {
  return {
    name: 'cma-dev-fixtures',
    apply: 'serve',
    configureServer(server) {
      const { logger } = server.config;
      const reported = new Set<string>();
      logger.info('  CMA fixture mode: /auth and /api are answered from web/build/fixtures.');
      server.middlewares.use((request, response, next) => {
        const url = new URL(request.url ?? '/', 'http://fixtures.invalid');
        const reply = fixtureReply(request.method ?? 'GET', url.pathname, url.search);
        if (reply === undefined) return next();
        if ('stream' in reply) {
          // Held open like the real stream; a closed one makes EventSource reconnect in a loop.
          response.writeHead(200, {
            'content-type': 'text/event-stream',
            'cache-control': 'no-cache',
          });
          response.write(reply.stream);
          return;
        }
        const key = `${reply.status} ${request.method} ${url.pathname}`;
        if (reply.status >= 400 && !reported.has(key)) {
          reported.add(key);
          logger.warn(`  [fixtures] ${key}`);
        }
        response.writeHead(reply.status, { 'content-type': 'application/json' });
        response.end(JSON.stringify(reply.json));
      });
    },
  };
}
