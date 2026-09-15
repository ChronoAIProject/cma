import type { ServerResponse } from 'node:http';
import { commandStatus } from './agent-v2-command.fixture';

/** Controlled queue boundary; retained acceptance can outlive its HTTP response. */
export class LocalQueuedCancellation {
  enabled = false;
  loseReceipt = false;
  conflict = false;
  revision = 7;
  private retained?: { key: string; body: string; result: unknown };

  constructor(private readonly started: string) {}

  handle(
    method: string,
    path: string,
    response: ServerResponse,
    body?: Record<string, unknown>,
    key?: string,
  ): boolean {
    if (path !== '/api/v2/agents/agt_contract/responses/resp_1/queued-cancellation') return false;
    const json = (value: unknown, status = 200) => {
      response.writeHead(status, { 'Content-Type': 'application/json' });
      response.end(JSON.stringify(value));
    };
    if (!this.enabled) {
      json(
        {
          error: {
            type: 'invalid_request_error',
            code: 'not_found',
            message: 'Unavailable',
            param: null,
          },
        },
        404,
      );
      return true;
    }
    const source = {
      ...commandStatus(this.started),
      status: this.retained ? 'cancelled' : 'queued',
      turn_sequence: null,
      observed_cursor: null,
      delivery: null,
    };
    if (method === 'GET') {
      json({
        object: 'cma.response.queued_cancellation.observation',
        response: source,
        command_revision: this.revision,
        cancellable: this.retained === undefined,
      });
      return true;
    }
    if (method !== 'POST' || !key || !body) throw new Error('Invalid queued fixture request');
    if (this.conflict) {
      json(
        {
          error: {
            type: 'invalid_request_error',
            code: 'queued_response_cancellation_conflict',
            message: 'Cancellation observation changed',
            param: null,
          },
        },
        409,
      );
      return true;
    }
    if (this.retained) {
      if (this.retained.key !== key || this.retained.body !== JSON.stringify(body))
        throw new Error('Cancellation recovery changed its intent');
      json(this.retained.result);
      return true;
    }
    if (body.source_command_id !== 'cmd_1' || body.expected_command_revision !== this.revision)
      throw new Error('Cancellation used a foreign source');
    const result = {
      object: 'cma.response.queued_cancellation',
      cancellation: {
        ...commandStatus(this.started, true),
        turn_sequence: null,
        observed_cursor: null,
        delivery: null,
      },
      response: { ...source, status: 'cancelled' },
      target: { response_id: 'resp_1', ...body },
    };
    this.retained = { key, body: JSON.stringify(body), result };
    this.revision += 1;
    if (this.loseReceipt) {
      this.loseReceipt = false;
      response.writeHead(200, { 'Content-Type': 'application/json' });
      response.end('{');
    } else json(result);
    return true;
  }
}
