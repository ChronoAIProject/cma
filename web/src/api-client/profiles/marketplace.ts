import {
  array,
  boolean,
  enumeration,
  integer,
  object,
  positiveInteger,
  tagged,
  text,
  type Output,
} from '../schema.js';
import {
  agentProfileContent,
  profileKind,
  profileRevision,
  sandboxProfileContent,
  workspaceProfileContent,
} from './types.js';

export const marketplaceSummary = object({
  kind: profileKind,
  id: text,
  name: text,
  description: text,
  owner_display: text,
  latest_revision: positiveInteger,
  published_at: text,
  uses_count: integer,
});
export type MarketplaceSummary = Output<typeof marketplaceSummary>;
export const resolvedProfileName = object({
  kind: profileKind,
  id: text,
  revision: positiveInteger,
  name: text,
});
export type ResolvedProfileName = Output<typeof resolvedProfileName>;
export const resolvedProfileRevision = tagged(
  {
    agent: object({
      kind: enumeration('agent'),
      name: text,
      revision: profileRevision(agentProfileContent),
    }),
    workspace: object({
      kind: enumeration('workspace'),
      name: text,
      revision: profileRevision(workspaceProfileContent),
    }),
    sandbox: object({
      kind: enumeration('sandbox'),
      name: text,
      revision: profileRevision(sandboxProfileContent),
    }),
  },
  'kind',
);
export type ResolvedProfileRevision = Output<typeof resolvedProfileRevision>;
export const sandboxProfileTrust = object({
  requires_confirmation: boolean,
  commands_digest: text,
});
export type SandboxProfileTrust = Output<typeof sandboxProfileTrust>;
const entryFields = {
  summary: marketplaceSummary,
  resolved_profiles: array(resolvedProfileName),
  resolved_revisions: array(resolvedProfileRevision),
};
export const agentMarketplaceEntry = object({
  ...entryFields,
  revision: profileRevision(agentProfileContent),
});
export type AgentMarketplaceEntry = Output<typeof agentMarketplaceEntry>;
export const workspaceMarketplaceEntry = object({
  ...entryFields,
  revision: profileRevision(workspaceProfileContent),
});
export type WorkspaceMarketplaceEntry = Output<typeof workspaceMarketplaceEntry>;
export const sandboxMarketplaceEntry = object({
  ...entryFields,
  revision: profileRevision(sandboxProfileContent),
  trust: sandboxProfileTrust,
});
export type SandboxMarketplaceEntry = Output<typeof sandboxMarketplaceEntry>;
export interface MarketplaceEntries {
  readonly agent: AgentMarketplaceEntry;
  readonly workspace: WorkspaceMarketplaceEntry;
  readonly sandbox: SandboxMarketplaceEntry;
}
