import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import type { Page } from '@playwright/test';
import { LocalAgentV2, cursor } from './agent-v2.fixture';
import { commandStatus } from './agent-v2-command.fixture';

const agent = 'agt_contract';
const incarnation = 'a'.repeat(64);
const eventsPath = `/api/v2/agents/${agent}/events`;
const inputPath = `/api/v2/agents/${agent}/responses`;
const receiptPath = `${inputPath}/receipt`;
const cancelPath = `${inputPath}/resp_1/controls`;
interface Write {
  ingress: number;
  path: string;
  body: string;
  key: string;
  at: number;
}
interface Read {
  ingress: number;
  after: string | null;
  at: number;
  status: number;
}

/** Two independent HTTP listeners sharing synthetic durable custody, not Mongo proof. */
export class ReplicaFixture {
  readonly base = new LocalAgentV2();
  readonly writes: Write[] = [];
  readonly receiptReads: Write[] = [];
  readonly reads: Read[] = [];
  readonly errors: string[] = [];
  readonly streams = new Set<ServerResponse>();
  readonly effects = new Map<string, string>();
  active = 0;
  faultAt = 0;
  lose: 'send' | 'cancel' | undefined;
  revealInputReceipt = true;
  tailRefusals: number[] = [];
  private origins: string[] = [];
  private baseURL = '';
  private readonly servers = [0, 1].map((ingress) =>
    createServer((request, response) => {
      void this.handle(ingress, request, response).catch((error: unknown) => {
        this.errors.push(String(error));
        response.destroy();
      });
    }),
  );

  async route(page: Page, baseURL: string) {
    this.baseURL = baseURL;
    await this.base.route(page, baseURL);
    for (const server of this.servers) {
      await new Promise<void>((resolve) => server.listen(0, '127.0.0.1', resolve));
      const address = server.address();
      if (!address || typeof address === 'string') throw new Error('Fixture listener unavailable');
      const origin = `http://127.0.0.1:${address.port}`;
      this.origins.push(origin);
      await page.route(`${origin}/**`, (route) => route.continue());
    }
    await page.route('**/api/v2/**', (route) => {
      const request = route.request();
      const url = new URL(request.url());
      const owns =
        url.pathname === eventsPath ||
        (request.method() === 'POST' &&
          [inputPath, receiptPath, cancelPath].includes(url.pathname));
      return owns
        ? route.continue({ url: `${this.origins[this.active]}${url.pathname}${url.search}` })
        : route.fallback();
    });
  }

  append(kind: Record<string, unknown>, binding: Parameters<LocalAgentV2['append']>[1] = null) {
    this.base.append(kind, binding);
    const item = this.base.items.at(-1)!;
    for (const response of this.streams) response.write(this.frame(item));
  }

  async cutMidFrame(cut: 'frame' | 'comment' = 'frame') {
    // This event exists in durable fixture custody. Interrupt ingress 0 before
    // its delivery, either within its frame or within a UTF-8 SSE comment.
    this.base.append({ type: 'message', role: 'assistant', content: 'Recovered complete message' });
    const frame = this.frame(this.base.items.at(-1)!);
    const stream = [...this.streams][0];
    if (!stream) throw new Error('No stream to interrupt');
    stream.write(
      `event: cma.agent.live\ndata: ${JSON.stringify({
        agent_id: agent,
        incarnation,
        turn_sequence: 1,
        after_sequence: this.base.items.length - 1,
        message_id: 'transient',
        kind: 'text_delta',
        delta: 'Disposable live fragment',
      })}\n\n`,
    );
    const interrupted =
      cut === 'frame'
        ? frame.slice(0, frame.length / 2)
        : Buffer.from(': keepalive 🙂').subarray(0, -2);
    await new Promise<void>((resolve, reject) =>
      stream.write(interrupted, (error) => (error ? reject(error) : resolve())),
    );
    await this.killActive();
  }

  private frame(item: LocalAgentV2['items'][number]) {
    return `id: ${item.cursor}\nevent: cma.agent.event\ndata: ${JSON.stringify({ ...item, agent_id: agent, incarnation })}\n\n`;
  }

  private async killActive() {
    const server = this.servers[this.active]!;
    this.faultAt = performance.now();
    this.active = 1;
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }

  private async handle(ingress: number, request: IncomingMessage, response: ServerResponse) {
    response.setHeader('Access-Control-Allow-Origin', this.baseURL);
    response.setHeader('Access-Control-Allow-Credentials', 'true');
    response.setHeader(
      'Access-Control-Allow-Headers',
      'content-type, x-request-id, last-event-id, idempotency-key, prefer',
    );
    response.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
    response.setHeader(
      'Access-Control-Expose-Headers',
      'x-cma-response-id, x-cma-command-id, retry-after',
    );
    if (request.method === 'OPTIONS') return response.end();
    const url = new URL(request.url!, this.baseURL);
    if (request.method === 'GET' && url.pathname === eventsPath) {
      const status = this.tailRefusals.shift() ?? 200;
      const after = url.searchParams.get('after_cursor');
      this.reads.push({ ingress, after, at: performance.now(), status });
      if (status !== 200) {
        response.setHeader('Retry-After', '1');
        return this.json(
          response,
          {
            error: {
              type: 'server_error',
              code: 'agent_owner_unavailable',
              message: 'Owner unavailable',
              param: null,
            },
          },
          status,
        );
      }
      response.writeHead(200, { 'Content-Type': 'text/event-stream' });
      response.write(': connected\n\n');
      const sequence = Number(after?.split('.').at(-1) ?? 0);
      for (const item of this.base.items.filter((item) => item.sequence > sequence))
        response.write(this.frame(item));
      this.streams.add(response);
      response.on('close', () => this.streams.delete(response));
      return;
    }
    if (request.method === 'POST' && [inputPath, receiptPath, cancelPath].includes(url.pathname)) {
      let body = '';
      for await (const chunk of request) body += String(chunk);
      const header = request.headers['idempotency-key'];
      if (typeof header !== 'string' || !header) throw new Error('Missing retained intent key');
      const captured = { ingress, path: url.pathname, body, key: header, at: performance.now() };
      if (url.pathname === receiptPath) {
        this.receiptReads.push(captured);
        const exact = this.effects.get(`${inputPath}:${header}`) === body;
        return this.json(
          response,
          this.revealInputReceipt && exact ? commandStatus(this.base.started) : null,
          200,
        );
      }
      this.writes.push(captured);
      const identity = `${url.pathname}:${header}`;
      const previous = this.effects.get(identity);
      if (previous !== undefined && previous !== body) throw new Error('Retained body changed');
      this.effects.set(identity, body);
      const lose = this.lose === (url.pathname === inputPath ? 'send' : 'cancel');
      if (lose) {
        this.lose = undefined;
        // Custody precedes the fault: a retry must recover this exact receipt.
        await this.killActive();
        return;
      }
      if (url.pathname === inputPath) {
        response.writeHead(200, {
          'Content-Type': 'text/event-stream',
          'x-cma-response-id': 'resp_1',
          'x-cma-command-id': 'cmd_1',
        });
        return response.end(': accepted\n\n');
      }
      const requestBody = JSON.parse(body) as {
        turn_sequence: number;
        expected_agent_revision: number;
      };
      return this.json(
        response,
        {
          object: 'cma.response.control',
          control: commandStatus(this.base.started, true),
          target: { response_id: 'resp_1', turn_sequence: requestBody.turn_sequence },
          expected_agent_revision: requestBody.expected_agent_revision,
        },
        202,
      );
    }
    throw new Error('Unexpected replica fixture route');
  }

  private json(response: ServerResponse, body: unknown, status: number) {
    response.writeHead(status, { 'Content-Type': 'application/json' });
    response.end(JSON.stringify(body));
  }

  async close(page: Page) {
    await this.base.close(page);
    for (const server of this.servers) {
      server.closeAllConnections();
      if (server.listening) await new Promise<void>((resolve) => server.close(() => resolve()));
    }
  }
}

export { cursor };
