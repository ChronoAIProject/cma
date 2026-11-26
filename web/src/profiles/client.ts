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

export type WorkspaceLocation =
  | { readonly type: 'repository_snapshot'; readonly url: string; readonly reference?: string }
  | { readonly type: 'git_repository'; readonly url: string; readonly reference?: string }
  | { readonly type: 'directory'; readonly name: string };

export interface WorkspaceProfileContent {
  readonly location: WorkspaceLocation;
  readonly name: string;
  readonly setupCommands: readonly string[];
  readonly agentProfiles: readonly ProfileReference[];
}

export interface SandboxProfileContent {
  readonly cpuMillis: number;
  readonly memoryMib: number;
  readonly nyxidServices: readonly string[];
  readonly agentProvider: 'codex';
  readonly modelAccess: 'chronoai_managed' | 'own_credentials';
  readonly maxWorkspaces: number;
  readonly maxAgentsPerWorkspace: number;
  readonly workspaceProfiles: readonly ProfileReference[];
  readonly softwareInstallCommands: readonly string[];
  readonly environmentVariables: readonly { readonly name: string; readonly value: string }[];
}

export interface ProfileContents {
  readonly agent: AgentProfileContent;
  readonly workspace: WorkspaceProfileContent;
  readonly sandbox: SandboxProfileContent;
}

export interface CapacityBounds {
  readonly minCpuMillis: number;
  readonly maxCpuMillis: number;
  readonly minMemoryMib: number;
  readonly maxMemoryMib: number;
}

export interface CapacityBoundsClient {
  readCapacityBounds(options?: ReadOptions): Promise<CapacityBounds>;
}

export interface ProfileAuthoringClient
  extends CapacityBoundsClient, RepositoryResolver, ServiceCatalogClient, SkillCatalogClient {}

/** Optional public authoring capabilities; no first-party-only transport is implied. */
export interface RepositoryResolver {
  resolveRepository(
    input: Extract<WorkspaceLocation, { type: 'git_repository' }>,
    options?: ReadOptions,
  ): Promise<{ readonly url: string; readonly reference?: string }>;
}

export interface ServiceCatalogClient {
  listServices(options?: ReadOptions): Promise<readonly ServiceCatalogEntry[]>;
}

export interface ServiceCatalogEntry {
  readonly id: string;
  readonly catalogId?: string;
  readonly detailUrl?: string;
  readonly name: string;
  readonly description?: string;
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
  readonly availability?: SandboxProfileAvailability;
  readonly draft: Content;
}

export interface ProfileDraft<Content> {
  readonly availability?: SandboxProfileAvailability;
  readonly name: string;
  readonly description: string;
  readonly content: Content;
}

export interface ProfileRevision<Content> extends ProfileRevisionSummary {
  readonly profileId: string;
  readonly content: Content;
}

export interface SandboxProfileAvailability {
  readonly warmupEnabled: boolean;
  readonly warmupCount: number;
  readonly alwaysKeepAlive: boolean;
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
