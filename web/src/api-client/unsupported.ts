import { readAgentCard } from './agent-card.js';
import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import { request, type OperationOptions } from './http.js';

/** Protocol refusal probes and the retained provider-discovery alias. */
export class UnsupportedInterfaces {
  constructor(private readonly config: ResolvedConfig) {}

  mcp(options: OperationOptions = {}): Promise<never> {
    return requireRefusal(this.config, '/api/v1/mcp', 'POST', options);
  }

  a2a(options: OperationOptions = {}): Promise<never> {
    return requireRefusal(this.config, '/api/v1/a2a', 'POST', options);
  }

  /** Compatibility alias for the public provider card; does not advertise A2A messages. */
  agentCard(options: OperationOptions = {}) {
    return readAgentCard(this.config, options);
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
