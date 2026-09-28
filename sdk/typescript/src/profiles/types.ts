import {
  array,
  boolean,
  enumeration,
  integer,
  object,
  optional,
  positiveInteger,
  record,
  tagged,
  text,
  variant,
  type Output,
  type Parser,
} from '../schema.js';
import { ProtocolError } from '../errors.js';

export const profileKind = enumeration('agent', 'workspace', 'sandbox');
export type ProfileKind = Output<typeof profileKind>;
export const profileReference = object({ id: text, revision: positiveInteger });
export type ProfileReference = Output<typeof profileReference>;
export const profileVisibility = enumeration('private', 'public');
export type ProfileVisibility = Output<typeof profileVisibility>;
export const thinkingEffort = enumeration('minimal', 'low', 'medium', 'high', 'x_high');
export type ThinkingEffort = Output<typeof thinkingEffort>;
export const permissionLevel = enumeration('guarded', 'autonomous');
export type PermissionLevel = Output<typeof permissionLevel>;
export const modelAccess = enumeration('chronoai_managed', 'own_credentials');
export type ModelAccess = Output<typeof modelAccess>;
export const skillReference = object({
  source: enumeration('ornn'),
  id: text,
  version: optional(text),
});
export type SkillReference = Output<typeof skillReference>;

export const agentProfileContent = object({
  isolated_agent: optional(boolean),
  model: optional(text),
  thinking_effort: thinkingEffort,
  permission_level: permissionLevel,
  skills: array(skillReference),
  system_prompt: text,
});
export type AgentProfileContent = Output<typeof agentProfileContent>;

export const workspaceLocation = tagged({
  git_repository: variant('git_repository', { url: text, reference: optional(text) }),
  directory: variant('directory', { name: text }),
});
export type WorkspaceLocation = Output<typeof workspaceLocation>;
export const workspaceProfileContent = object({
  location: workspaceLocation,
  name: text,
  setup_commands: array(text),
  agent_profiles: array(profileReference),
});
export type WorkspaceProfileContent = Output<typeof workspaceProfileContent>;

export const environmentVariable = object({ name: text, value: text });
export type EnvironmentVariable = Output<typeof environmentVariable>;
export const sandboxProfileContent = object({
  cpu_millis: positiveInteger,
  memory_mib: positiveInteger,
  nyxid_services: array(text),
  agent_provider: enumeration('codex'),
  model_access: modelAccess,
  max_workspaces: positiveInteger,
  max_agents_per_workspace: positiveInteger,
  workspace_profiles: array(profileReference),
  software_install_commands: array(text),
  environment_variables: array(environmentVariable),
});
export type SandboxProfileContent = Output<typeof sandboxProfileContent>;

/** Mutable scheduling policy for Sandbox Profiles, separate from pinned content. */
export const sandboxProfileAvailability = object({
  warmup_enabled: boolean,
  warmup_count: (value: unknown) => {
    const count = positiveInteger(value);
    if (count > 5) throw new ProtocolError('warmup count must be between 1 and 5');
    return count;
  },
  always_keep_alive: boolean,
});
export type SandboxProfileAvailability = Output<typeof sandboxProfileAvailability>;

export interface ProfileContents {
  readonly agent: AgentProfileContent;
  readonly workspace: WorkspaceProfileContent;
  readonly sandbox: SandboxProfileContent;
}

export const profileRevisionSummary = object({
  revision: positiveInteger,
  digest: text,
  published_at: text,
});
export type ProfileRevisionSummary = Output<typeof profileRevisionSummary>;

export interface ProfileDraft<Content> {
  /** Sandbox Profiles only. Warmup requires a published public profile. */
  readonly availability?: SandboxProfileAvailability;
  readonly name: string;
  readonly description?: string;
  readonly content: Content;
}

export interface ProfileUpdate<Content> {
  /** Sandbox Profiles only; omission preserves the current policy. */
  readonly availability?: SandboxProfileAvailability;
  readonly draft_version: number;
  readonly name?: string;
  readonly description?: string;
  readonly content: Content;
}

export interface ProfileDocument<Content> {
  /** Present for Sandbox Profiles. */
  readonly availability?: SandboxProfileAvailability;
  readonly id: string;
  readonly owner: string;
  readonly name: string;
  readonly description: string;
  readonly visibility: ProfileVisibility;
  readonly forked_from?: ProfileReference;
  readonly uses_count: number;
  readonly draft: Content;
  readonly draft_version: number;
  readonly created_at: string;
  readonly updated_at: string;
  readonly retired_at?: string;
  readonly latest_revision?: ProfileRevisionSummary;
}

export interface ProfileRevision<Content> extends ProfileRevisionSummary {
  readonly profile_id: string;
  readonly content: Content;
}

export function profileDraft<Content>(content: Parser<Content>): Parser<ProfileDraft<Content>> {
  const metadata = object({
    name: text,
    description: optional(text),
    availability: optional(sandboxProfileAvailability),
  });
  return (value) => ({ ...metadata(value), content: content(record(value).content) });
}

export function profileUpdate<Content>(content: Parser<Content>): Parser<ProfileUpdate<Content>> {
  const metadata = object({
    draft_version: positiveInteger,
    availability: optional(sandboxProfileAvailability),
    name: optional(text),
    description: optional(text),
  });
  return (value) => ({ ...metadata(value), content: content(record(value).content) });
}

export function profileDocument<Content>(
  content: Parser<Content>,
): Parser<ProfileDocument<Content>> {
  const metadata = object({
    id: text,
    availability: optional(sandboxProfileAvailability),
    owner: text,
    name: text,
    description: text,
    visibility: profileVisibility,
    forked_from: optional(profileReference),
    uses_count: integer,
    draft_version: positiveInteger,
    created_at: text,
    updated_at: text,
    retired_at: optional(text),
    latest_revision: optional(profileRevisionSummary),
  });
  return (value) => ({ ...metadata(value), draft: content(record(value).draft) });
}

export function profileRevision<Content>(
  content: Parser<Content>,
): Parser<ProfileRevision<Content>> {
  const metadata = object({
    profile_id: text,
    revision: positiveInteger,
    digest: text,
    published_at: text,
  });
  return (value) => ({ ...metadata(value), content: content(record(value).content) });
}

export const contentParsers: { readonly [Kind in ProfileKind]: Parser<ProfileContents[Kind]> } = {
  agent: agentProfileContent,
  workspace: workspaceProfileContent,
  sandbox: sandboxProfileContent,
};
