import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';
import { isDeepStrictEqual } from 'node:util';
import type { Page } from '@playwright/test';
import { LocalStartup } from './agent-startup-fixtures';
import { commandStatus } from './agent-v2-command.fixture';
import { LocalQueuedCancellation } from './agent-v2-queued.fixture';

const agent = 'agt_contract';
const incarnation = 'a'.repeat(64);
export const cursor = (sequence: number) => `ag1.${incarnation}.${sequence}`;
export const binding = { response_id: 'resp_1', command_id: 'cmd_1', turn_sequence: 1 };
interface EventItem {
  cursor: string;
  sequence: number;
  appended_at: string;
  kind: Record<string, unknown>;
  response_binding: typeof binding | null;
}
interface CapturedWrite {
  path: string;
  key: string | undefined;
  body: Record<string, unknown>;
}

/** Disposable local HTTP boundary; exercises the built app and real SDK parsers. */
export class LocalAgentV2 {
  readonly boundary = new LocalStartup();
  readonly started = new Date(Date.now() - 120_000).toISOString();
  readonly queued = new LocalQueuedCancellation(this.started);
  readonly items: EventItem[] = [];
  readonly writes: CapturedWrite[] = [];
  readonly receiptReads: CapturedWrite[] = [];
  readonly reads: string[] = [];
  readonly errors: string[] = [];
  readonly streams = new Set<ServerResponse>();
  enabled = true;
  externalApprovals = false;
  approvalReviewAllowed = true;
  approvalPolicyAvailable = true;
  owner = 'local-owner';
  holdInput = false;
  loseInput = false;
  revealInputReceipt = true;
  loseControl = false;
  controlConflict = false;
  holdSnapshot = false;
  snapshots = 0;
  private pendingInput?: ServerResponse;
  private readonly admittedInputs: CapturedWrite[] = [];
  private pendingSnapshot?: ServerResponse;
  private baseURL = '';
  private readonly server = createServer((request, response) => {
    void this.handle(request, response).catch((error: unknown) => {
      this.errors.push(String(error));
      response.destroy();
    });
  });

  async route(page: Page, baseURL: string) {
    this.baseURL = baseURL;
    this.boundary.sandbox = { ...this.boundary.sandbox, owner: this.owner };
    await new Promise<void>((resolve) => this.server.listen(0, '127.0.0.1', resolve));
    const address = this.server.address();
    if (!address || typeof address === 'string') throw new Error('Missing local fixture port');
    const origin = `http://127.0.0.1:${address.port}`;
    await this.boundary.route(page, baseURL);
    await page.route(`${origin}/**`, (route) => route.continue());
    await page.route('**/api/v2/**', (route) => {
      const url = new URL(route.request().url());
      return route.continue({ url: `${origin}${url.pathname}${url.search}` });
    });
    await page.route('**/auth/session', (route) =>
      route.fulfill({
        json: {
          authenticated: true,
          subject: this.owner,
          display_name: this.owner,
          capabilities: ['use_own_sandboxes', 'create_sandbox', 'delete_own_sandbox'],
        },
      }),
    );
    await page.route('**/auth/connections/check', (route) =>
      route.fulfill({
        json: {
          owner: this.owner,
          ready: true,
          checked_at: this.started,
          services: [],
        },
      }),
    );
  }

  async routeSignOut(page: Page, onSignOut: () => void) {
    await page.route(`${this.baseURL}/auth/logout`, (route) => {
      onSignOut();
      return route.fulfill({ status: 303, headers: { location: '/' } });
    });
  }

  async close(page: Page) {
    await page.close();
    for (const stream of this.streams) stream.destroy();
    this.pendingInput?.destroy();
    this.pendingSnapshot?.destroy();
    this.server.closeAllConnections();
    await new Promise<void>((resolve) => this.server.close(() => resolve()));
  }

  async suggestReply(page: Page, text: string) {
    await page.route('**/api/v2/ui/task-experience', (route) =>
      route.fulfill({
        json: {
          object: 'cma.ui.task_experience',
          schema_version: 1,
          account_id: this.owner,
          capabilities: {
            task_home_admission: false,
            suggested_followup_admission: true,
            desktop_enable: false,
            browser_takeover: false,
            replay_presentation: false,
          },
        },
      }),
    );
    await page.route('**/api/v1/agents/agt_contract/suggestions?*', (route) => {
      const scope = { turn_sequence: 1, answer_sequence: 3 };
      const query = new URL(route.request().url()).searchParams;
      if (
        query.get('turn_sequence') !== String(scope.turn_sequence) ||
        query.get('answer_sequence') !== String(scope.answer_sequence)
      )
        return route.fallback();
      return route.fulfill({
        json: {
          status: 'ready',
          set: {
            id: 'local-suggestions',
            viewerId: this.owner,
            agentId: agent,
            answerId: 'event:3',
            scope,
            source: { kind: 'template', version: '1' },
            items: [{ id: 'local-followup', text, rank: 1 }],
          },
        },
      });
    });
  }

  append(
    kind: Record<string, unknown>,
    responseBinding: typeof binding | null = null,
    appendedAt = this.started,
  ) {
    const sequence = this.items.length + 1;
    const item = {
      cursor: cursor(sequence),
      sequence,
      appended_at: appendedAt,
      kind,
      response_binding: responseBinding,
    };
    this.items.push(item);
    for (const stream of this.streams)
      this.frame(stream, 'cma.agent.event', { ...item, agent_id: agent, incarnation }, item.cursor);
  }

  live(kind: 'text_delta' | 'reasoning_summary_delta', delta: string, turnSequence = 1) {
    for (const stream of this.streams)
      this.frame(stream, 'cma.agent.live', {
        agent_id: agent,
        incarnation,
        turn_sequence: turnSequence,
        after_sequence: this.items.length,
        message_id: kind === 'text_delta' ? 'live_text' : 'live_reasoning',
        kind,
        delta,
      });
  }

  endTails() {
    for (const stream of this.streams) stream.end();
  }

  gap(turnSequence: number | null, reason: 'lagged' | 'reconnected' = 'lagged') {
    for (const stream of this.streams)
      this.frame(stream, 'cma.agent.live_gap', {
        agent_id: agent,
        incarnation,
        turn_sequence: turnSequence,
        frames: reason === 'lagged' ? 1 : null,
        reason,
      });
  }

  acceptInput() {
    if (!this.pendingInput) throw new Error('No input is pending');
    this.receipt(this.pendingInput);
    this.pendingInput = undefined;
    this.holdInput = false;
  }

  /** Whether the app has requested a held snapshot page that is not yet released. */
  get snapshotPending() {
    return this.pendingSnapshot !== undefined;
  }

  releaseSnapshot() {
    if (!this.pendingSnapshot) throw new Error('No snapshot page is pending');
    this.json(this.pendingSnapshot, this.snapshot(this.items.slice(1)));
    this.pendingSnapshot = undefined;
    this.holdSnapshot = false;
  }

  private snapshot(items = this.items, next: string | null = null) {
    return {
      object: 'cma.agent.event_page',
      schema_version: 1,
      agent_id: agent,
      incarnation,
      through_cursor: cursor(this.items.length),
      items,
      next_page_cursor: next,
    };
  }

  private json(response: ServerResponse, body: unknown, status = 200) {
    response.writeHead(status, { 'Content-Type': 'application/json' });
    response.end(JSON.stringify(body));
  }

  private frame(response: ServerResponse, type: string, data: unknown, id?: string) {
    response.write(
      `${id === undefined ? '' : `id: ${id}\n`}event: ${type}\ndata: ${JSON.stringify(data)}\n\n`,
    );
  }

  admitInput(write: CapturedWrite) {
    this.admittedInputs.push(write);
    this.queued.admit(write.body.input);
  }

  private receipt(response: ServerResponse) {
    response.writeHead(200, {
      'Content-Type': 'text/event-stream',
      'x-cma-response-id': 'resp_1',
      'x-cma-command-id': 'cmd_1',
    });
    response.end(': accepted\n\n');
  }

  private async handle(request: IncomingMessage, response: ServerResponse) {
    if (
      this.externalApprovals &&
      String(request.headers.prefer ?? '')
        .split(',')
        .some((token) => token.trim() === 'cma-external-approval-v1')
    ) {
      response.setHeader('Preference-Applied', 'cma-external-approval-v1');
      if (this.approvalPolicyAvailable)
        response.setHeader('CMA-Approval-Review-Origin', 'https://identity.example.test');
      response.setHeader(
        'CMA-External-Approval-Action',
        this.approvalReviewAllowed ? 'review' : 'none',
      );
    }
    response.setHeader('Access-Control-Allow-Origin', this.baseURL);
    response.setHeader('Access-Control-Allow-Credentials', 'true');
    response.setHeader(
      'Access-Control-Allow-Headers',
      'content-type, x-request-id, last-event-id, idempotency-key, prefer',
    );
    response.setHeader('Access-Control-Allow-Methods', 'GET, POST, OPTIONS');
    response.setHeader(
      'Access-Control-Expose-Headers',
      'x-cma-response-id, x-cma-command-id, Preference-Applied, CMA-Approval-Review-Origin, CMA-External-Approval-Action',
    );
    if (request.method === 'OPTIONS') return response.end();
    const url = new URL(request.url!, 'http://localhost');
    const path = url.pathname;
    if (request.method === 'POST') {
      let source = '';
      for await (const chunk of request) source += String(chunk);
      const body = JSON.parse(source) as Record<string, unknown>;
      const header = request.headers['idempotency-key'];
      const key = typeof header === 'string' ? header : undefined;
      const captured = { path, body, key };
      if (path === `/api/v2/agents/${agent}/responses/receipt`) {
        this.receiptReads.push(captured);
        const admitted = this.admittedInputs.some(
          (write) =>
            write.path === `/api/v2/agents/${agent}/responses` &&
            key &&
            write.key === key &&
            isDeepStrictEqual(write.body, body),
        );
        return this.json(
          response,
          this.revealInputReceipt && admitted ? commandStatus(this.started) : null,
        );
      }
      this.writes.push(captured);
      if (this.queued.handle('POST', path, response, body, key)) return;
      if (path === `/api/v2/agents/${agent}/responses`) {
        this.admitInput(captured);
        if (this.loseInput) {
          this.loseInput = false;
          response.writeHead(200, { 'Content-Type': 'text/event-stream' });
          return response.end('event: response.created\ndata: {\n\n');
        }
        if (this.holdInput) {
          this.pendingInput = response;
          return;
        }
        return this.receipt(response);
      }
      if (path === `/api/v2/agents/${agent}/responses/resp_1/controls`) {
        if (this.controlConflict)
          return this.json(
            response,
            {
              error: {
                type: 'invalid_request_error',
                code: 'response_control_conflict',
                message: 'Fresh observation changed',
                param: null,
              },
            },
            409,
          );
        if (this.loseControl) {
          this.loseControl = false;
          response.writeHead(202, { 'Content-Type': 'application/json' });
          return response.end('{');
        }
        return this.json(
          response,
          {
            object: 'cma.response.control',
            control: commandStatus(this.started, true),
            target: { response_id: 'resp_1', turn_sequence: body.turn_sequence },
            expected_agent_revision: body.expected_agent_revision,
          },
          202,
        );
      }
    } else if (request.method === 'GET') {
      this.reads.push(`${path}${url.search}`);
      // Older boundary cases deliberately exercise optional capability fallback.
      if (path === '/api/v2/ui/task-experience')
        return this.json(
          response,
          { code: 'not_found', detail: 'Optional capability unavailable.' },
          404,
        );
      if (path === `/api/v2/agents/${agent}/events/head`)
        return this.json(
          response,
          {
            error: {
              type: 'server_error',
              code: 'agent_read_unavailable',
              message: 'Optional live head unavailable.',
              param: null,
            },
          },
          503,
        );
      if (this.queued.handle('GET', path, response)) return;
      if (path === `/api/v2/agents/${agent}/queued-inputs`)
        return this.json(response, {
          object: 'cma.agent.queued_inputs',
          agent_id: agent,
          data: this.queued.items(),
          next_after: null,
        });
      if (path === '/api/v2/ui/transport')
        return this.json(response, {
          object: 'cma.ui.transport',
          schema_version: 1,
          enabled: this.enabled,
          selection: 'new_tabs',
          input: 'responses_v2',
          snapshot_schema: 1,
          event_schema: 1,
          control_schema: 1,
          live_fragments: true,
          snapshot: '/api/v2/agents/{agent_id}/events/snapshot',
          events: '/api/v2/agents/{agent_id}/events',
          controls: '/api/v2/agents/{agent_id}/responses/{response_id}/controls',
          command_status: '/api/v2/commands/{command_id}',
        });
      if (path === `/api/v2/agents/${agent}/events/snapshot`) {
        this.snapshots += 1;
        if (this.holdSnapshot) {
          if (url.searchParams.has('page_cursor')) {
            this.pendingSnapshot = response;
            return;
          }
          return this.json(
            response,
            this.snapshot(this.items.slice(0, 1), `ags1.${incarnation}.${this.items.length}.1`),
          );
        }
        return this.json(response, this.snapshot());
      }
      if (path === `/api/v2/agents/${agent}/events`) {
        response.writeHead(200, { 'Content-Type': 'text/event-stream' });
        response.write(': connected\n\n');
        const after = Number(url.searchParams.get('after_cursor')?.split('.').at(-1) ?? 0);
        for (const item of this.items.filter((item) => item.sequence > after))
          this.frame(
            response,
            'cma.agent.event',
            { ...item, agent_id: agent, incarnation },
            item.cursor,
          );
        this.streams.add(response);
        response.on('close', () => this.streams.delete(response));
        return;
      }
      if (path === `/api/v2/agents/${agent}/responses/resp_1/controls`)
        return this.json(response, {
          object: 'cma.response.controls',
          response: commandStatus(this.started),
          target: { response_id: 'resp_1', turn_sequence: 1 },
          agent_revision: 10,
          target_active: true,
          runtime_support: { cancel: true, steer: true, resolve_approval: true },
        });
      if (path === '/api/v2/commands/cmd_control')
        return this.json(response, commandStatus(this.started, true));
      if (path === '/api/v2/commands/cmd_1') {
        const turn = this.items.find((item) => item.response_binding?.command_id === 'cmd_1');
        const sequence = turn?.response_binding?.turn_sequence;
        const completed = this.items.some(
          (item) => item.kind.type === 'turn_completed' && item.kind.turn_sequence === sequence,
        );
        return this.json(response, {
          ...commandStatus(this.started),
          status: turn === undefined ? 'queued' : completed ? 'completed' : 'in_progress',
          turn_sequence: sequence ?? null,
          observed_cursor: turn === undefined ? null : this.items.length,
          delivery: turn === undefined ? null : 'execution_observed',
        });
      }
    }
    this.errors.push(`Unexpected ${request.method} ${path}`);
    response.writeHead(404);
    response.end();
  }
}
