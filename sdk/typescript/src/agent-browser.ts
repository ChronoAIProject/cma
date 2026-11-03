import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { requestBytes, type OperationOptions, type RequestOptions } from './http.js';
import { query, segment } from './resources.js';
import { enumeration, integer, nullable, object, text, type Output } from './schema.js';

const browserFrame = object({
  sequence: integer,
  captured_at: text,
  media_type: text,
  width: integer,
  height: integer,
  data: text,
});
const browserObservation = object({
  agent_id: text,
  runtime_incarnation: text,
  context_id: nullable(text),
  state: enumeration(
    'not_enabled',
    'unsupported',
    'loading',
    'ready',
    'live',
    'paused',
    'disconnected',
  ),
  location: text,
  frame: nullable(browserFrame),
  observers: integer,
  captures: integer,
  bytes: integer,
  min_interval_ms: integer,
  fresh_for_ms: integer,
  max_frame_bytes: integer,
  max_observers: integer,
});
export type BrowserObservation = Output<typeof browserObservation>;
export interface BrowserScope {
  readonly runtime_incarnation: string;
  readonly observer?: string;
  readonly context_id?: string;
}
export interface AgentBrowserClient {
  read(agent: string, scope: BrowserScope, options?: OperationOptions): Promise<BrowserObservation>;
  configure(
    agent: string,
    incarnation: string,
    enabled: boolean,
    options?: OperationOptions,
  ): Promise<BrowserObservation>;
  release(
    agent: string,
    scope: BrowserScope,
    options?: OperationOptions,
  ): Promise<BrowserObservation>;
}

export class AgentBrowser implements AgentBrowserClient {
  constructor(private readonly config: ResolvedConfig) {}

  read(agent: string, scope: BrowserScope, options: OperationOptions = {}) {
    return this.receive(agent, scope, options);
  }

  configure(agent: string, incarnation: string, enabled: boolean, options: OperationOptions = {}) {
    return this.receive(
      agent,
      { runtime_incarnation: incarnation },
      { ...options, method: 'PUT', body: { runtime_incarnation: incarnation, enabled } },
    );
  }

  release(agent: string, scope: BrowserScope, options: OperationOptions = {}) {
    return this.receive(agent, scope, { ...options, method: 'DELETE' });
  }

  private async receive(agent: string, scope: BrowserScope, options: RequestOptions) {
    const path = `/api/v1/agents/${segment(agent)}/browser`;
    const bytes = await requestBytes(
      this.config,
      options.method === 'PUT' ? path : query(path, { ...scope }),
      { ...options, expectedStatus: 200 },
      360000,
    );
    let value: BrowserObservation;
    try {
      value = browserObservation(JSON.parse(new TextDecoder().decode(bytes)));
    } catch (cause) {
      throw new ProtocolError('Invalid browser observation.', { cause });
    }
    if (
      value.agent_id !== agent ||
      value.runtime_incarnation !== scope.runtime_incarnation ||
      (scope.context_id !== undefined &&
        value.context_id !== scope.context_id &&
        !(
          value.frame === null && ['paused', 'not_enabled', 'unsupported'].includes(value.state)
        )) ||
      !['Blank page', 'Local page', 'Web page'].includes(value.location) ||
      value.min_interval_ms !== 1000 ||
      value.fresh_for_ms !== 3000 ||
      value.max_frame_bytes !== 262144 ||
      value.max_observers !== 16 ||
      value.observers > 16 ||
      (value.state === 'live' && (!value.frame || !value.context_id)) ||
      (value.frame &&
        (value.frame.sequence < 1 ||
          value.frame.width !== 1280 ||
          value.frame.height !== 720 ||
          value.frame.media_type !== 'image/jpeg' ||
          value.frame.data.length > 349528 ||
          !/^\/9j\/[A-Za-z0-9+/]*={0,2}$/.test(value.frame.data) ||
          !Number.isFinite(Date.parse(value.frame.captured_at))))
    )
      throw new ProtocolError('Browser observation identity or bounds changed.');
    return value;
  }
}
