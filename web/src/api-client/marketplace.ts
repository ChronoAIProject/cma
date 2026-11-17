import type { ResolvedConfig } from './config.js';
import { ProtocolError } from './errors.js';
import type { OperationOptions } from './http.js';
import {
  command,
  input,
  page,
  query,
  read,
  segment,
  type CommandOptions,
  type PageOptions,
} from './resources.js';
import { positiveInteger, type Parser } from './schema.js';
import { contentParsers, profileDocument, type ProfileKind } from './profiles/types.js';
import {
  agentMarketplaceEntry,
  marketplaceSummary,
  sandboxMarketplaceEntry,
  workspaceMarketplaceEntry,
  type MarketplaceEntries,
} from './profiles/marketplace.js';

export interface MarketplaceListOptions extends PageOptions {
  readonly q?: string;
}
const entryParsers: { readonly [Kind in ProfileKind]: Parser<MarketplaceEntries[Kind]> } = {
  agent: agentMarketplaceEntry,
  workspace: workspaceMarketplaceEntry,
  sandbox: sandboxMarketplaceEntry,
};

/** Public immutable profile revisions, resolved dependencies, and explicit forks. */
export class Marketplace<Kind extends ProfileKind> {
  readonly #path: string;
  constructor(
    private readonly config: ResolvedConfig,
    readonly kind: Kind,
  ) {
    this.#path = `/api/v1/marketplace/${kind}-profiles`;
  }

  list(options: MarketplaceListOptions = {}) {
    return read(
      this.config,
      query(this.#path, { q: options.q, cursor: options.cursor, limit: options.limit }),
      page((value) => {
        const summary = marketplaceSummary(value);
        if (summary.kind !== this.kind)
          throw new ProtocolError('Marketplace kind did not match the request');
        return summary;
      }),
      options,
    );
  }

  read(id: string, options: OperationOptions = {}): Promise<MarketplaceEntries[Kind]> {
    return read(
      this.config,
      `${this.#path}/${segment(id)}`,
      (value) => {
        const entry = entryParsers[this.kind](value);
        if (
          entry.summary.kind !== this.kind ||
          entry.summary.id !== id ||
          entry.revision.profile_id !== id ||
          entry.revision.revision !== entry.summary.latest_revision
        )
          throw new ProtocolError('Marketplace revision identity did not match the request');
        return entry;
      },
      options,
    );
  }

  /** A revision is required to keep a command pinned when retried. */
  fork(id: string, body: { readonly revision: number }, options: CommandOptions) {
    input(positiveInteger, body.revision);
    return read(
      this.config,
      `${this.#path}/${segment(id)}/fork`,
      profileDocument(contentParsers[this.kind]),
      {
        ...command(options),
        method: 'POST',
        body,
        expectedStatus: 201,
      },
    );
  }
}
