import { resolveConfig, type ClientConfig, type ResolvedConfig } from './config.js';
import { readConformance, type Conformance } from './discovery.js';
import type { OperationOptions } from './http.js';

/** A configured client for the public CMA discovery boundary. */
export class Cma {
  readonly #config: ResolvedConfig;

  constructor(config: ClientConfig = {}) {
    this.#config = resolveConfig(config);
  }

  /** The resolved transport configuration held by this client. */
  get config(): ResolvedConfig {
    return this.#config;
  }

  /** Read the anonymous deployment state exposed during the rebuild. */
  readConformance(options: OperationOptions = {}): Promise<Conformance> {
    return readConformance(this.#config, options);
  }
}

/** Construct a configured CMA discovery client. */
export function createClient(config: ClientConfig = {}): Cma {
  return new Cma(config);
}
