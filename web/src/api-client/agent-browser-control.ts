import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { requestBytes, type OperationOptions } from './http.js';
import { query, segment } from './resources.js';
import { array, enumeration, integer, nullable, object, text, type Output } from './schema.js';

const audit = object({
  revision: integer,
  kind: enumeration('acquired', 'input', 'returned', 'renewed', 'expired', 'unknown'),
  actor: text,
  client_id: text,
  operation_id: nullable(text),
  at: text,
});
const control = object({
  agent_id: text,
  runtime_incarnation: text,
  context_id: text,
  state: enumeration('agent', 'user', 'unknown'),
  revision: integer,
  client_id: nullable(text),
  lease_token: nullable(text),
  expires_at: nullable(text),
  lease_duration_ms: integer,
  operation_id: nullable(text),
  operation_revision: nullable(integer),
  audit: array(audit),
  audit_truncated_before: nullable(integer),
});

export type BrowserControl = Output<typeof control>;
export type BrowserInput =
  | { readonly kind: 'click'; readonly x: number; readonly y: number }
  | { readonly kind: 'text'; readonly text: string }
  | {
      readonly kind: 'key';
      readonly key:
        | 'Backspace'
        | 'Tab'
        | 'Enter'
        | 'Escape'
        | 'Home'
        | 'End'
        | 'ArrowLeft'
        | 'ArrowUp'
        | 'ArrowRight'
        | 'ArrowDown'
        | 'Delete';
    };
export interface BrowserControlScope {
  readonly runtime_incarnation: string;
  readonly context_id: string;
  readonly client_id: string;
  readonly client_key: string;
  readonly operation_id?: string;
}
export interface BrowserControlChange extends BrowserControlScope {
  readonly command: 'acquire' | 'input' | 'renew' | 'return';
  readonly operation_id: string;
  readonly expected_revision: number;
  readonly lease_token?: string;
  readonly input?: BrowserInput;
}
export interface AgentBrowserControlClient {
  read(
    agent: string,
    scope: BrowserControlScope,
    options?: OperationOptions,
  ): Promise<BrowserControl>;
  change(
    agent: string,
    input: BrowserControlChange,
    options?: OperationOptions,
  ): Promise<BrowserControl>;
}

export class AgentBrowserControl implements AgentBrowserControlClient {
  constructor(private readonly config: ResolvedConfig) {}

  read(agent: string, scope: BrowserControlScope, options: OperationOptions = {}) {
    return this.receive(agent, scope, options);
  }

  change(agent: string, input: BrowserControlChange, options: OperationOptions = {}) {
    return this.receive(agent, input, options, input);
  }

  private async receive(
    agent: string,
    scope: BrowserControlScope,
    options: OperationOptions,
    input?: BrowserControlChange,
  ) {
    const path = `/api/v1/agents/${segment(agent)}/browser/control`;
    const { client_key, ...selection } = scope;
    const body = input ? selection : undefined;
    const headers = new Headers();
    headers.set('x-cma-browser-client', client_key);
    const bytes = await requestBytes(
      this.config,
      input ? path : query(path, selection),
      { ...options, headers, method: input ? 'POST' : 'GET', body, expectedStatus: 200 },
      65536,
    );
    let value: BrowserControl;
    try {
      value = control(JSON.parse(new TextDecoder().decode(bytes)));
    } catch (cause) {
      throw new ProtocolError('Invalid browser control receipt.', { cause });
    }
    const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
    if (
      value.agent_id !== agent ||
      value.runtime_incarnation !== scope.runtime_incarnation ||
      value.context_id !== scope.context_id ||
      value.operation_id !== (scope.operation_id ?? null) ||
      value.lease_duration_ms !== 15000 ||
      value.audit.length > 128 ||
      (value.operation_id !== null &&
        (value.operation_revision === null || value.operation_revision > value.revision)) ||
      (input && value.operation_revision !== input.expected_revision + 1) ||
      (value.state === 'user' && (value.client_id === null || value.expires_at === null)) ||
      (value.expires_at !== null && !Number.isFinite(Date.parse(value.expires_at))) ||
      (value.lease_token !== null &&
        (!uuid.test(value.lease_token) ||
          value.state !== 'user' ||
          value.client_id !== scope.client_id ||
          !scope.operation_id)) ||
      value.audit.some(
        (event, index) =>
          event.revision > value.revision ||
          !Number.isFinite(Date.parse(event.at)) ||
          event.actor.length > 128 ||
          event.client_id.length > 64 ||
          (event.operation_id !== null && !uuid.test(event.operation_id)) ||
          (index > 0 && event.revision <= value.audit[index - 1]!.revision),
      )
    ) {
      throw new ProtocolError('Browser control identity or bounds changed.');
    }
    return value;
  }
}
