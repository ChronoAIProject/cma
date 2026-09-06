import { resolveConfig, type ClientConfig, type ResolvedConfig } from './config.js';
import { readConformance, type Conformance } from './discovery.js';
import type { OperationOptions } from './http.js';
import { Agents } from './agents.js';
import { Marketplace } from './marketplace.js';
import { Profiles } from './profiles.js';
import { Sandboxes } from './sandboxes.js';
import { Workspaces, Worktrees } from './workspaces.js';

/** A configured client for the public CMA resource and discovery APIs. */
export class Cma {
  readonly #config: ResolvedConfig;
  readonly agents: Agents;
  readonly sandboxes: Sandboxes;
  readonly workspaces: Workspaces;
  readonly worktrees: Worktrees;
  readonly agentProfiles: Profiles<'agent'>;
  readonly workspaceProfiles: Profiles<'workspace'>;
  readonly sandboxProfiles: Profiles<'sandbox'>;
  readonly marketplace: {
    readonly agent: Marketplace<'agent'>;
    readonly workspace: Marketplace<'workspace'>;
    readonly sandbox: Marketplace<'sandbox'>;
  };

  constructor(config: ClientConfig = {}) {
    this.#config = resolveConfig(config);
    this.agents = new Agents(this.#config);
    this.sandboxes = new Sandboxes(this.#config);
    this.workspaces = new Workspaces(this.#config);
    this.worktrees = new Worktrees(this.#config);
    this.agentProfiles = new Profiles(this.#config, 'agent');
    this.workspaceProfiles = new Profiles(this.#config, 'workspace');
    this.sandboxProfiles = new Profiles(this.#config, 'sandbox');
    this.marketplace = {
      agent: new Marketplace(this.#config, 'agent'),
      workspace: new Marketplace(this.#config, 'workspace'),
      sandbox: new Marketplace(this.#config, 'sandbox'),
    };
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

/** Construct a configured CMA resource client. */
export function createClient(config: ClientConfig = {}): Cma {
  return new Cma(config);
}
