import type {
  AgentProfileContent,
  ProfileDocument,
  ProfileEditorClient,
  ProfilesClient,
  SkillCatalogClient,
} from '../profiles/client';

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
