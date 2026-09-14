import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { request, type OperationOptions } from './http.js';

/** Probe declared refusals; these methods do not implement either protocol. */
export class UnsupportedInterfaces {
  constructor(private readonly config: ResolvedConfig) {}

  mcp(options: OperationOptions = {}): Promise<never> {
    return requireRefusal(this.config, '/api/v1/mcp', 'POST', options);
  }

  a2a(options: OperationOptions = {}): Promise<never> {
    return requireRefusal(this.config, '/api/v1/a2a', 'POST', options);
  }

  agentCard(options: OperationOptions = {}): Promise<never> {
    return requireRefusal(
      { ...this.config, credential: undefined, cookies: false },
      '/.well-known/agent-card.json',
      'GET',
      options,
    );
  }
}

export async function requireRefusal(
  config: ResolvedConfig,
  path: string,
  method: string,
  options: OperationOptions,
): Promise<never> {
  await request(config, path, { ...options, method });
  throw new ProtocolError('An unsupported operation unexpectedly returned a successful response');
}
