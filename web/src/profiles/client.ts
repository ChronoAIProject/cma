import type { CommandOptions, PageOptions, ReadOptions } from '../api/client';
import type { ProfileReference, ResourcePage } from '../api/types';
import type { ProfileKind } from '../routing/routes';

export type ProfileVisibility = 'private' | 'public';
export type ThinkingEffort = 'minimal' | 'low' | 'medium' | 'high' | 'x_high';
export type PermissionLevel = 'guarded' | 'autonomous';

export interface SkillReference {
  readonly source: 'ornn';
  readonly id: string;
  readonly version?: string;
}

export interface AgentProfileContent {
  readonly model?: string;
  readonly thinkingEffort: ThinkingEffort;
  readonly permissionLevel: PermissionLevel;
  readonly skills: readonly SkillReference[];
  readonly systemPrompt: string;
}

export interface ProfileRevisionSummary {
  readonly revision: number;
  readonly digest: string;
  readonly publishedAt: string;
}

export interface ProfileSummary {
  readonly id: string;
  readonly owner: string;
  readonly name: string;
  readonly description: string;
  readonly visibility: ProfileVisibility;
  readonly forkedFrom?: ProfileReference;
  readonly usesCount: number;
  readonly draftVersion: number;
  readonly createdAt: string;
  readonly updatedAt: string;
  readonly retiredAt?: string;
  readonly latestRevision?: ProfileRevisionSummary;
}

export interface ProfileDocument<Content> extends ProfileSummary {
  readonly draft: Content;
}

export interface ProfileDraft<Content> {
  readonly name: string;
  readonly description: string;
  readonly content: Content;
}

export interface ProfileRevision<Content> extends ProfileRevisionSummary {
  readonly profileId: string;
  readonly content: Content;
}

/** Public profile operations, bound to one profile kind by the SDK adapter. */
export interface ProfileEditorClient<Content> {
  readProfile(id: string, options?: ReadOptions): Promise<ProfileDocument<Content>>;
  createProfile(
    input: ProfileDraft<Content>,
    options: CommandOptions,
  ): Promise<ProfileDocument<Content>>;
  saveProfile(
    id: string,
    input: ProfileDraft<Content> & { readonly draftVersion: number },
    options: CommandOptions,
  ): Promise<ProfileDocument<Content>>;
  publishProfile(
    id: string,
    input: { readonly draftVersion: number },
    options: CommandOptions,
  ): Promise<ProfileRevision<Content>>;
  setProfileVisibility(
    id: string,
    input: { readonly visibility: ProfileVisibility },
    options: CommandOptions,
  ): Promise<ProfileDocument<Content>>;
  retireProfile(id: string, options: CommandOptions): Promise<ProfileDocument<Content>>;
}

export interface ProfilesClient {
  listProfiles(kind: ProfileKind, options?: PageOptions): Promise<ResourcePage<ProfileSummary>>;
}

export interface SkillCatalogEntry {
  readonly id: string;
  readonly name: string;
  readonly description?: string;
  readonly versions: readonly string[];
}

export interface SkillCatalogClient {
  searchSkills(query: string, options?: PageOptions): Promise<ResourcePage<SkillCatalogEntry>>;
}
