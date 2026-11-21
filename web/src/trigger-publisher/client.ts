import {
  ApiError,
  type Cma,
  type ProfileContents,
  type ProfileKind,
  type ProfileReference,
  type TriggerAuthoring,
} from '@cma/client';
import type { PageOptions, ReadOptions } from '../api/client';
import type { ResourcePage } from '../api/types';

export interface PublisherProfileChoice extends ProfileReference {
  readonly name: string;
  readonly description: string;
  readonly public: boolean;
}
export type PublisherProfile = {
  readonly [Kind in ProfileKind]: PublisherProfileChoice & {
    readonly kind: Kind;
    readonly digest: string;
    readonly content: ProfileContents[Kind];
  };
}[ProfileKind];
export interface TriggerPublisherClient {
  readonly triggers: Pick<
    TriggerAuthoring,
    | 'list'
    | 'read'
    | 'create'
    | 'save'
    | 'publish'
    | 'retire'
    | 'listRevisions'
    | 'listPlacements'
    | 'createPlacement'
    | 'retirePlacement'
    | 'embed'
    | 'stats'
  >;
  listProfiles(
    kind: ProfileKind,
    source: 'own' | 'public',
    options?: PageOptions,
  ): Promise<ResourcePage<PublisherProfileChoice>>;
  readProfile(
    kind: ProfileKind,
    reference: ProfileReference,
    options?: ReadOptions,
  ): Promise<PublisherProfile>;
}

/** This adapter uses only SDK methods and reads each chosen immutable revision. */
export function createTriggerPublisherClient(sdk: Cma): TriggerPublisherClient {
  const profiles = {
    sandbox: sdk.sandboxProfiles,
    workspace: sdk.workspaceProfiles,
    agent: sdk.agentProfiles,
  };
  return {
    triggers: sdk.triggerAuthoring,
    listProfiles: async (kind, source, options) => {
      if (source === 'public') {
        const page = await sdk.marketplace[kind].list(options);
        return {
          items: page.items.map((item) => ({
            id: item.id,
            revision: item.latest_revision,
            name: item.name,
            description: item.description,
            public: true,
          })),
          next: page.next,
        };
      }
      const page = await profiles[kind].list(options);
      return {
        items: page.items.flatMap((item) =>
          item.latest_revision === undefined || item.retired_at !== undefined
            ? []
            : [
                {
                  id: item.id,
                  revision: item.latest_revision.revision,
                  name: item.name,
                  description: item.description,
                  public: item.visibility === 'public',
                },
              ],
        ),
        next: page.next,
      };
    },
    readProfile: async (kind, reference, options) => {
      let metadata: {
        readonly name: string;
        readonly description: string;
        readonly public: boolean;
      };
      try {
        const document = await profiles[kind].read(reference.id, options);
        if (document.retired_at !== undefined) throw new Error('This profile is retired.');
        metadata = {
          name: document.name,
          description: document.description,
          public: document.visibility === 'public',
        };
      } catch (error) {
        if (!(error instanceof ApiError) || (error.status !== 403 && error.status !== 404))
          throw error;
        const entry = await sdk.marketplace[kind].read(reference.id, options);
        metadata = {
          name: entry.summary.name,
          description: entry.summary.description,
          public: true,
        };
      }
      switch (kind) {
        case 'sandbox': {
          const revision = await sdk.sandboxProfiles.readRevision(
            reference.id,
            reference.revision,
            options,
          );
          return {
            ...reference,
            ...metadata,
            kind,
            digest: revision.digest,
            content: revision.content,
          };
        }
        case 'workspace': {
          const revision = await sdk.workspaceProfiles.readRevision(
            reference.id,
            reference.revision,
            options,
          );
          return {
            ...reference,
            ...metadata,
            kind,
            digest: revision.digest,
            content: revision.content,
          };
        }
        case 'agent': {
          const revision = await sdk.agentProfiles.readRevision(
            reference.id,
            reference.revision,
            options,
          );
          return {
            ...reference,
            ...metadata,
            kind,
            digest: revision.digest,
            content: revision.content,
          };
        }
      }
    },
  };
}
