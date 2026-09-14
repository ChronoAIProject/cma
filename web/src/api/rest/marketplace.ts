import type * as Wire from '@chronoai/cma-sdk';
import type {
  MarketplaceClient,
  MarketplaceSummary,
  ResolvedProfileRevision,
} from '../../profiles/marketplace/client';
import { agentContent, sandboxContent, workspaceContent } from './profile-content';
import { profileDocument, profileRevision } from './profile-documents';

function summary(value: Wire.MarketplaceSummary): MarketplaceSummary {
  return {
    kind: value.kind,
    id: value.id,
    name: value.name,
    description: value.description,
    ownerDisplay: value.owner_display,
    latestRevision: value.latest_revision,
    publishedAt: value.published_at,
    usesCount: value.uses_count,
  };
}

function resolvedRevision(value: Wire.ResolvedProfileRevision): ResolvedProfileRevision {
  switch (value.kind) {
    case 'agent':
      return {
        kind: value.kind,
        name: value.name,
        revision: profileRevision(value.revision, agentContent.decode),
      };
    case 'workspace':
      return {
        kind: value.kind,
        name: value.name,
        revision: profileRevision(value.revision, workspaceContent.decode),
      };
    case 'sandbox':
      return {
        kind: value.kind,
        name: value.name,
        revision: profileRevision(value.revision, sandboxContent.decode),
      };
  }
}

function entryFields(value: Wire.MarketplaceEntries[Wire.ProfileKind]) {
  return {
    summary: summary(value.summary),
    resolvedProfiles: value.resolved_profiles,
    resolvedRevisions: value.resolved_revisions.map(resolvedRevision),
  };
}

export function marketplaceClient(sdk: Wire.Cma): MarketplaceClient {
  return {
    listMarketplace: async (kind, q, options) => {
      const page = await sdk.marketplace[kind].list({ ...options, q });
      return { items: page.items.map(summary), next: page.next };
    },
    readMarketplace: async (kind, id, options) => {
      switch (kind) {
        case 'agent': {
          const value = await sdk.marketplace.agent.read(id, options);
          return {
            kind,
            ...entryFields(value),
            revision: profileRevision(value.revision, agentContent.decode),
          };
        }
        case 'workspace': {
          const value = await sdk.marketplace.workspace.read(id, options);
          return {
            kind,
            ...entryFields(value),
            revision: profileRevision(value.revision, workspaceContent.decode),
          };
        }
        case 'sandbox': {
          const value = await sdk.marketplace.sandbox.read(id, options);
          return {
            kind,
            ...entryFields(value),
            revision: profileRevision(value.revision, sandboxContent.decode),
            trust: {
              requiresConfirmation: value.trust.requires_confirmation,
              commandsDigest: value.trust.commands_digest,
            },
          };
        }
      }
    },
    forkProfile: async (kind, id, input, options) => {
      switch (kind) {
        case 'agent':
          return profileDocument(
            await sdk.marketplace.agent.fork(id, input, options),
            agentContent.decode,
          );
        case 'workspace':
          return profileDocument(
            await sdk.marketplace.workspace.fork(id, input, options),
            workspaceContent.decode,
          );
        case 'sandbox':
          return profileDocument(
            await sdk.marketplace.sandbox.fork(id, input, options),
            sandboxContent.decode,
          );
      }
    },
  };
}
