import type {
  AgentProfileContent,
  ProfileDocument,
  ProfileEditorClient,
  ProfilesClient,
  SkillCatalogClient,
  WorkspaceProfileContent,
  SandboxProfileContent,
  CapacityBounds,
  ServiceCatalogClient,
} from '../profiles/client';
import type { MarketplaceClient, MarketplaceEntry } from '../profiles/marketplace/client';

export const AGENT_PROFILE: ProfileDocument<AgentProfileContent> = {
  id: 'agp_research',
  owner: 'owner',
  name: 'Research agent',
  description: 'Repository research and development.',
  visibility: 'private',
  usesCount: 2,
  draftVersion: 3,
  createdAt: '2026-09-01T00:00:00Z',
  updatedAt: '2026-09-06T00:00:00Z',
  latestRevision: {
    revision: 1,
    digest: 'profile-content-digest',
    publishedAt: '2026-09-04T00:00:00Z',
  },
  draft: {
    model: 'gpt-5',
    thinkingEffort: 'high',
    permissionLevel: 'guarded',
    skills: [],
    systemPrompt: 'Inspect the repository before making changes.',
  },
};

export const SKILL_CATALOG: SkillCatalogClient = {
  searchSkills: async (query) => ({
    items:
      query === 'missing'
        ? []
        : [
            {
              id: 'research',
              name: 'Research',
              description: 'Plan and verify repository changes.',
              versions: ['1.2.0', '1.1.0'],
            },
          ],
  }),
};

export function profileClient(
  overrides: Partial<ProfileEditorClient<AgentProfileContent> & ProfilesClient> = {},
): ProfileEditorClient<AgentProfileContent> & ProfilesClient {
  return {
    listProfiles: async () => ({ items: [AGENT_PROFILE] }),
    readProfile: async () => AGENT_PROFILE,
    createProfile: async (input) => ({
      ...AGENT_PROFILE,
      ...input,
      draft: input.content,
      draftVersion: 1,
      latestRevision: undefined,
    }),
    saveProfile: async (_id, input) => ({
      ...AGENT_PROFILE,
      ...input,
      draft: input.content,
      draftVersion: input.draftVersion + 1,
    }),
    publishProfile: async () => ({
      profileId: AGENT_PROFILE.id,
      revision: 2,
      digest: 'published-content-digest',
      publishedAt: '2026-09-06T00:01:00Z',
      content: AGENT_PROFILE.draft,
    }),
    setProfileVisibility: async (_id, input) => ({
      ...AGENT_PROFILE,
      visibility: input.visibility,
    }),
    retireProfile: async () => ({ ...AGENT_PROFILE, retiredAt: '2026-09-06T00:01:00Z' }),
    ...overrides,
  };
}

export const WORKSPACE_PROFILE: ProfileDocument<WorkspaceProfileContent> = {
  ...AGENT_PROFILE,
  id: 'wkp_research',
  name: 'Research workspace',
  draft: {
    location: { type: 'directory', name: 'research' },
    name: 'research',
    setupCommands: ['mkdir -p reports'],
    agentProfiles: [{ id: AGENT_PROFILE.id, revision: 1 }],
  },
};

export const SANDBOX_PROFILE: ProfileDocument<SandboxProfileContent> = {
  ...AGENT_PROFILE,
  id: 'sbp_research',
  name: 'Research sandbox',
  draft: {
    cpuMillis: 2000,
    memoryMib: 4096,
    nyxidServices: ['storage'],
    agentProvider: 'codex',
    modelAccess: 'chronoai_managed',
    maxWorkspaces: 4,
    maxAgentsPerWorkspace: 4,
    workspaceProfiles: [{ id: WORKSPACE_PROFILE.id, revision: 1 }],
    softwareInstallCommands: ['mkdir -p /home/cma/tools'],
    environmentVariables: [{ name: 'RUN_MODE', value: 'research' }],
  },
};

export const PROFILE_BOUNDS: CapacityBounds = {
  minCpuMillis: 100,
  maxCpuMillis: 8000,
  minMemoryMib: 128,
  maxMemoryMib: 16384,
};

export const SERVICE_CATALOG: ServiceCatalogClient = {
  listServices: async () => [
    { id: 'storage', name: 'Storage', description: 'Project files and artifacts.' },
  ],
};

export const OWN_PROFILES: ProfilesClient = {
  listProfiles: async (kind) => ({
    items: [
      kind === 'agent' ? AGENT_PROFILE : kind === 'workspace' ? WORKSPACE_PROFILE : SANDBOX_PROFILE,
    ],
  }),
};

export function documentClient<Content>(
  record: ProfileDocument<Content>,
  overrides: Partial<ProfileEditorClient<Content>> = {},
): ProfileEditorClient<Content> {
  return {
    readProfile: async () => record,
    createProfile: async (input) => ({
      ...record,
      name: input.name,
      description: input.description,
      draft: input.content,
      draftVersion: 1,
      latestRevision: undefined,
      visibility: 'private',
    }),
    saveProfile: async (_id, input) => ({
      ...record,
      name: input.name,
      description: input.description,
      draft: input.content,
      draftVersion: input.draftVersion + 1,
    }),
    publishProfile: async () => ({
      profileId: record.id,
      revision: 2,
      digest: 'published-content-digest',
      publishedAt: '2026-09-06T00:01:00Z',
      content: record.draft,
    }),
    setProfileVisibility: async (_id, input) => ({ ...record, visibility: input.visibility }),
    retireProfile: async () => ({ ...record, retiredAt: '2026-09-06T00:01:00Z' }),
    ...overrides,
  };
}

export const MARKETPLACE_SANDBOX: MarketplaceEntry = {
  kind: 'sandbox',
  summary: {
    kind: 'sandbox',
    id: SANDBOX_PROFILE.id,
    name: SANDBOX_PROFILE.name,
    description: 'A complete research setup.',
    ownerDisplay: 'Research team',
    latestRevision: 1,
    publishedAt: '2026-09-04T00:00:00Z',
    usesCount: 12,
  },
  revision: {
    profileId: SANDBOX_PROFILE.id,
    revision: 1,
    content: SANDBOX_PROFILE.draft,
    digest: 'sandbox-content-digest',
    publishedAt: '2026-09-04T00:00:00Z',
  },
  resolvedProfiles: [
    { kind: 'workspace', id: WORKSPACE_PROFILE.id, revision: 1, name: WORKSPACE_PROFILE.name },
    { kind: 'agent', id: AGENT_PROFILE.id, revision: 1, name: AGENT_PROFILE.name },
  ],
  resolvedRevisions: [
    {
      kind: 'workspace',
      name: WORKSPACE_PROFILE.name,
      revision: {
        profileId: WORKSPACE_PROFILE.id,
        revision: 1,
        content: WORKSPACE_PROFILE.draft,
        digest: 'workspace-content-digest',
        publishedAt: '2026-09-04T00:00:00Z',
      },
    },
    {
      kind: 'agent',
      name: AGENT_PROFILE.name,
      revision: {
        profileId: AGENT_PROFILE.id,
        revision: 1,
        content: AGENT_PROFILE.draft,
        digest: 'agent-content-digest',
        publishedAt: '2026-09-04T00:00:00Z',
      },
    },
  ],
  trust: { requiresConfirmation: true, commandsDigest: 'sandbox-content-digest' },
};

export function marketplaceClient(overrides: Partial<MarketplaceClient> = {}): MarketplaceClient {
  return {
    listMarketplace: async () => ({ items: [MARKETPLACE_SANDBOX.summary] }),
    readMarketplace: async () => MARKETPLACE_SANDBOX,
    forkProfile: async () => ({
      ...SANDBOX_PROFILE,
      id: 'sbp_forked',
      latestRevision: undefined,
      draftVersion: 1,
    }),
    ...overrides,
  };
}
