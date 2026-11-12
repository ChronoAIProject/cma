import type { ResolvedConfig } from '../config.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { errorFromBody, request, type OperationOptions } from '../http.js';
import { requestOutcome } from '../http/outcome.js';
import { input, segment } from '../resources.js';
import { enumeration, integer, nullable, object, tagged, text, type Output } from '../schema.js';

const originalRequest = object({
  agent_id: text,
  creation_key: text,
  creation_fingerprint: text,
  failure_seq: integer,
  requested_at: text,
});
/** Preserve every original request field across uncertain outcomes. */
export type CapacityRecoveryRequest = Output<typeof originalRequest>;
const preflight = object({
  agent_id: text,
  disposition: text,
  observed_revision: integer,
  through_seq: integer,
  evaluated_at: text,
});
export type CapacityRecoveryPreflight = Output<typeof preflight>;
const status = object({
  agent_id: text,
  disposition: text,
  phase: nullable(text),
  attempts: integer,
  retry_at: nullable(text),
  operation_key: nullable(text),
});
export type CapacityRecoveryStatus = Output<typeof status>;
const outcome = tagged(
  {
    unknown: object({ outcome: enumeration('unknown') }),
    recorded: object({
      outcome: enumeration('recorded'),
      operation_key: text,
      phase: text,
      attempts: integer,
      retry_at: nullable(text),
      completed_at: nullable(text),
    }),
  },
  'outcome',
);
export type CapacityRecoveryOutcome = Output<typeof outcome> | { readonly outcome: 'known_abort' };

/** Explicit recovery operations; never activates gates or replays Agent input. */
export class CapacityRecovery {
  constructor(
    private readonly config: ResolvedConfig,
    private readonly id: string,
  ) {}
  private path(action = '') {
    return `/api/v1/agents/${segment(this.id)}/capacity-recovery${action}`;
  }
  private original(body: CapacityRecoveryRequest) {
    input(originalRequest, body);
    if (body.agent_id !== this.id)
      throw new ValidationError('recovery request belongs to another Agent');
    return body;
  }
  private identity<T extends { readonly agent_id: string }>(value: T): T {
    if (value.agent_id !== this.id)
      throw new ProtocolError('recovery result belongs to another Agent');
    return value;
  }
  /** Read metadata only; eligibility is not activation or admission. */
  async dryRun(options: OperationOptions = {}): Promise<CapacityRecoveryPreflight> {
    return this.identity(
      preflight(
        await request(this.config, this.path('/dry-run'), { ...options, expectedStatus: 200 }),
      ),
    );
  }
  async status(options: OperationOptions = {}): Promise<CapacityRecoveryStatus> {
    return this.identity(
      status(await request(this.config, this.path(), { ...options, expectedStatus: 200 })),
    );
  }
  async requestStatus(
    body: CapacityRecoveryRequest,
    options: OperationOptions = {},
  ): Promise<CapacityRecoveryStatus> {
    return this.identity(
      status(
        await request(this.config, this.path('/status'), {
          ...options,
          method: 'POST',
          body: this.original(body),
          expectedStatus: 200,
        }),
      ),
    );
  }
  /** Explicit new admission remains subject to the server's closed-by-default gate. */
  async admit(
    body: CapacityRecoveryRequest,
    options: OperationOptions = {},
  ): Promise<CapacityRecoveryOutcome> {
    return this.command('/admit', body, options);
  }
  /** Reconcile accepted work. Unknown never means safe to send input. */
  async resume(
    body: CapacityRecoveryRequest,
    options: OperationOptions = {},
  ): Promise<CapacityRecoveryOutcome> {
    return this.command('/resume', body, options);
  }
  private async command(
    action: string,
    body: CapacityRecoveryRequest,
    options: OperationOptions,
  ): Promise<CapacityRecoveryOutcome> {
    const reply = await requestOutcome(this.config, this.path(action), {
      ...options,
      method: 'POST',
      body: this.original(body),
      expectedStatus: [200, 202, 409],
    });
    if (reply.response.status === 409) {
      if (
        typeof reply.body === 'object' &&
        reply.body !== null &&
        Object.keys(reply.body).length === 1 &&
        'outcome' in reply.body &&
        reply.body.outcome === 'known_abort'
      )
        return { outcome: 'known_abort' };
      throw errorFromBody(reply.response, reply.body);
    }
    const value = outcome(reply.body);
    if ((reply.response.status === 202) !== (value.outcome === 'unknown'))
      throw new ProtocolError('recovery outcome does not match its HTTP status');
    return value;
  }
}
