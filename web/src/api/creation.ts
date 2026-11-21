import type { Cma, ProfileKind, ProfileReference } from '@cma/client';
import type { ReadOptions } from './client';
import type {
  CreationClient,
  CreationProfile,
  ProfileChoice,
  TrustDisclosure,
} from '../create/client';
import type {
  MarketplaceEntry,
  ProfileUse,
  ResolvedProfileRevision,
} from '../profiles/marketplace/client';
import { agentContent, sandboxContent, workspaceContent } from './rest/profile-content';
import { profileRevision } from './rest/profile-documents';
import { marketplaceClient } from './rest/marketplace';

function disclosure(value: ResolvedProfileRevision): TrustDisclosure | undefined {
  if (value.kind === 'agent') return undefined;
  const commands =
    value.kind === 'sandbox'
      ? value.revision.content.softwareInstallCommands
      : value.revision.content.setupCommands;
  const environment = value.kind === 'sandbox' ? value.revision.content.environmentVariables : [];
  return commands.length === 0 && environment.length === 0
    ? undefined
    : { name: value.name, commands, environment };
}

function withDisclosures(
  value: ResolvedProfileRevision,
  revisions: readonly ResolvedProfileRevision[],
  requiresConfirmation: boolean,
): CreationProfile {
  return {
    ...value,
    requiresConfirmation,
    disclosures: revisions.flatMap((revision) => {
      const item = disclosure(revision);
      return item === undefined ? [] : [item];
    }),
  };
}

export function fromMarketplace(entry: MarketplaceEntry): CreationProfile {
  if (entry.kind === 'sandbox' && entry.trust.commandsDigest !== entry.revision.digest)
    throw new Error('Profile trust digest did not match the reviewed revision.');
  const top = {
    kind: entry.kind,
    name: entry.summary.name,
    revision: entry.revision,
  } as ResolvedProfileRevision;
  return withDisclosures(
    top,
    [top, ...entry.resolvedRevisions],
    entry.kind === 'sandbox'
      ? entry.trust.requiresConfirmation
      : entry.kind === 'workspace' && entry.revision.content.setupCommands.length > 0,
  );
}

/** The handoff retains the reviewed immutable body, not another latest-revision read. */
export function fromProfileUse(selection: ProfileUse): CreationProfile {
  if (
    selection.kind !== selection.entry.kind ||
    selection.profile.id !== selection.entry.revision.profileId ||
    selection.profile.revision !== selection.entry.revision.revision
  )
    throw new Error('Profile selection changed.');
  return fromMarketplace(selection.entry);
}

export function createCreationClient(sdk: Cma): CreationClient {
  const resources = {
    agent: sdk.agentProfiles,
    workspace: sdk.workspaceProfiles,
    sandbox: sdk.sandboxProfiles,
  };
  const marketplace = marketplaceClient(sdk);
  async function own(
    kind: ProfileKind,
    reference: ProfileReference,
    name: string,
    options?: ReadOptions,
  ): Promise<ResolvedProfileRevision> {
    switch (kind) {
      case 'agent':
        return {
          kind,
          name,
          revision: profileRevision(
            await sdk.agentProfiles.readRevision(reference.id, reference.revision, options),
            agentContent.decode,
          ),
        };
      case 'workspace':
        return {
          kind,
          name,
          revision: profileRevision(
            await sdk.workspaceProfiles.readRevision(reference.id, reference.revision, options),
            workspaceContent.decode,
          ),
        };
      case 'sandbox':
        return {
          kind,
          name,
          revision: profileRevision(
            await sdk.sandboxProfiles.readRevision(reference.id, reference.revision, options),
            sandboxContent.decode,
          ),
        };
    }
  }
  async function publicRevision(kind: ProfileKind, choice: ProfileChoice, options?: ReadOptions) {
    const top = await own(kind, choice, choice.name, options);
    const nested: ResolvedProfileRevision[] = [];
    if (top.kind === 'sandbox') {
      for (const reference of top.revision.content.workspaceProfiles) {
        nested.push(await own('workspace', reference, 'Workspace profile', options));
      }
    }
    const all = [top, ...nested];
    return withDisclosures(
      top,
      all,
      all.some((value) => disclosure(value) !== undefined),
    );
  }
  return {
    listProfiles: async (kind, source, options) => {
      if (source === 'default') {
        const defaults = await sdk.readDefaults(options);
        return { items: [{ ...defaults[`${kind}_profile`], name: 'Deployment default', source }] };
      }
      if (source === 'public') {
        const page = await marketplace.listMarketplace(kind, '', options);
        return {
          items: page.items.map((item) => ({
            id: item.id,
            revision: item.latestRevision,
            name: item.name,
            source,
          })),
          next: page.next,
        };
      }
      const page = await resources[kind].list(options);
      return {
        items: page.items.flatMap((item) =>
          item.latest_revision === undefined || item.retired_at !== undefined
            ? []
            : [{ id: item.id, revision: item.latest_revision.revision, name: item.name, source }],
        ),
        next: page.next,
      };
    },
    readProfile: async (kind, choice, options) => {
      if (choice.source !== 'own') return publicRevision(kind, choice, options);
      const top = await own(kind, choice, choice.name, options);
      const nested: ResolvedProfileRevision[] = [];
      let requiresConfirmation = false;
      if (top.kind === 'sandbox') {
        for (const ref of top.revision.content.workspaceProfiles) {
          try {
            const document = await sdk.workspaceProfiles.read(ref.id, options);
            nested.push(await own('workspace', ref, document.name, options));
          } catch (error) {
            if (options?.signal?.aborted) throw error;
            const value = await publicRevision(
              'workspace',
              { ...ref, name: 'Workspace', source: 'public' },
              options,
            );
            requiresConfirmation ||= value.requiresConfirmation;
            nested.push(value);
          }
        }
      }
      // Include nested setup even in an owned outer profile: ownership is not transitive.
      return withDisclosures(top, [top, ...nested], requiresConfirmation);
    },
  };
}
