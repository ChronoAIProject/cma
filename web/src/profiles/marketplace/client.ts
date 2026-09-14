import type { CommandOptions, PageOptions, ReadOptions } from '../../api/client';
import type { ProfileReference, ResourcePage } from '../../api/types';
import type { ProfileKind } from '../../routing/routes';
import type { ProfileContents, ProfileDocument, ProfileRevision } from '../client';

export interface MarketplaceSummary {
  readonly kind: ProfileKind;
  readonly id: string;
  readonly name: string;
  readonly description: string;
  readonly ownerDisplay: string;
  readonly latestRevision: number;
  readonly publishedAt: string;
  readonly usesCount: number;
}

export type ResolvedProfileRevision = {
  [Kind in ProfileKind]: {
    readonly kind: Kind;
    readonly name: string;
    readonly revision: ProfileRevision<ProfileContents[Kind]>;
  };
}[ProfileKind];

export interface ResolvedProfileName extends ProfileReference {
  readonly kind: ProfileKind;
  readonly name: string;
}

export interface ProfileTrust {
  readonly requiresConfirmation: boolean;
  readonly commandsDigest: string;
}

export type MarketplaceEntry = {
  [Kind in ProfileKind]: {
    readonly kind: Kind;
    readonly summary: MarketplaceSummary;
    readonly revision: ProfileRevision<ProfileContents[Kind]>;
    readonly resolvedProfiles: readonly ResolvedProfileName[];
    readonly resolvedRevisions: readonly ResolvedProfileRevision[];
  } & (Kind extends 'sandbox' ? { readonly trust: ProfileTrust } : object);
}[ProfileKind];

export interface MarketplaceClient {
  listMarketplace(
    kind: ProfileKind,
    query: string,
    options?: PageOptions,
  ): Promise<ResourcePage<MarketplaceSummary>>;
  readMarketplace(kind: ProfileKind, id: string, options?: ReadOptions): Promise<MarketplaceEntry>;
  forkProfile(
    kind: ProfileKind,
    id: string,
    input: { readonly revision: number },
    options: CommandOptions,
  ): Promise<ProfileDocument<ProfileContents[ProfileKind]>>;
}

export interface ProfileUse {
  readonly kind: ProfileKind;
  readonly profile: ProfileReference;
  readonly entry: MarketplaceEntry;
}
