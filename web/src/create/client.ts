import type { PageOptions, ReadOptions } from '../api/client';
import type { ProfileReference, ResourcePage } from '../api/types';
import type { ProfileKind } from '../routing/routes';
import type { ResolvedProfileRevision } from '../profiles/marketplace/client';

export type ProfileSource = 'own' | 'public' | 'default';

export interface ProfileChoice extends ProfileReference {
  readonly name: string;
  readonly source: ProfileSource;
}

export interface TrustDisclosure {
  readonly name: string;
  readonly commands: readonly string[];
  readonly environment: readonly { readonly name: string; readonly value: string }[];
}

export type CreationProfile = ResolvedProfileRevision & {
  readonly disclosures: readonly TrustDisclosure[];
  readonly requiresConfirmation: boolean;
};

export interface CreationClient {
  listProfiles(
    kind: ProfileKind,
    source: ProfileSource,
    options?: PageOptions,
  ): Promise<ResourcePage<ProfileChoice>>;
  readProfile(
    kind: ProfileKind,
    choice: ProfileChoice,
    options?: ReadOptions,
  ): Promise<CreationProfile>;
}
