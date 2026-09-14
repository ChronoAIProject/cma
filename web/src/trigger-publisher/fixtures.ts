import { vi } from 'vitest';
import type {
  ProfileKind,
  TriggerContent,
  TriggerDocument,
  TriggerPlacement,
} from '@chronoai/cma-sdk';
import descriptor from '../../../api/crates/layer1-public/tests/fixtures/trigger-embed/descriptor.json';
import type { PublisherProfile, TriggerPublisherClient } from './client';

export { descriptor };
export const profiles: Record<ProfileKind, PublisherProfile> = {
  sandbox: {
    kind: 'sandbox',
    id: 'sbp_standard',
    revision: 2,
    name: 'Standard',
    description: 'A connected development sandbox',
    public: true,
    digest: 'a'.repeat(64),
    content: {
      cpu_millis: 2000,
      memory_mib: 4096,
      nyxid_services: ['github'],
      agent_provider: 'codex',
      model_access: 'chronoai_managed',
      max_workspaces: 4,
      max_agents_per_workspace: 4,
      workspace_profiles: [{ id: 'wkp_trureturing', revision: 3 }],
      software_install_commands: ['corepack enable'],
      environment_variables: [{ name: 'PROJECT_MODE', value: 'development' }],
    },
  },
  workspace: {
    kind: 'workspace',
    id: 'wkp_trureturing',
    revision: 3,
    name: 'trureturing',
    description: 'The trureturing repository',
    public: true,
    digest: 'b'.repeat(64),
    content: {
      name: 'trureturing',
      location: {
        type: 'git_repository',
        url: 'https://github.com/chronoai/trureturing',
        reference: 'main',
      },
      setup_commands: ['npm ci'],
      agent_profiles: [{ id: 'agp_codex', revision: 4 }],
    },
  },
  agent: {
    kind: 'agent',
    id: 'agp_codex',
    revision: 4,
    name: 'Codex',
    description: 'A guarded coding agent',
    public: true,
    digest: 'c'.repeat(64),
    content: {
      model: 'gpt-5-codex',
      thinking_effort: 'high',
      permission_level: 'guarded',
      skills: [],
      system_prompt: '',
    },
  },
};
export const content: TriggerContent = {
  presentation: {
    title: 'trureturing',
    description: 'Contribute to trureturing',
    button_label: 'open_in_cma',
    theme: 'auto',
  },
  target: {
    sandbox_profile: { id: 'sbp_standard', revision: 2 },
    workspace_profile: { id: 'wkp_trureturing', revision: 3 },
    agent_profile: { id: 'agp_codex', revision: 4 },
  },
  reuse: { sandbox: 'reuse_existing', workspace: 'reuse_existing', agent: 'ask' },
  first_message: 'Inspect the project',
  consent: { always_review: true },
};
export const draft: TriggerDocument = {
  id: 'trg_project',
  draft: content,
  draft_version: 1,
  availability: 'draft',
  revision_count: 0,
  placements: [],
  stats: { launches_total: 0, agents_ready_total: 0 },
  created_at: '2026-09-07T00:00:00Z',
  updated_at: '2026-09-07T00:00:00Z',
};
export const placement: TriggerPlacement = {
  id: 'plc_readme',
  label: 'README',
  source_url: 'https://github.com/chronoai/trureturing',
};
export const published: TriggerDocument = {
  ...draft,
  public_id: 'pub_example',
  availability: 'published',
  revision_count: 1,
  placements: [placement],
};

export function publisherFixture(initial?: TriggerDocument) {
  let stored = initial;
  let placementCount = 0;
  const triggers = {
    list: vi.fn(async () => ({ items: stored === undefined ? [] : [stored] })),
    read: vi.fn(async () => stored ?? draft),
    create: vi.fn(async (body: { readonly content: TriggerContent }) => {
      stored = { ...draft, draft: body.content };
      return stored;
    }),
    save: vi.fn(
      async (
        _id: string,
        body: { readonly content: TriggerContent; readonly draft_version: number },
      ) => {
        stored = {
          ...(stored ?? draft),
          draft: body.content,
          draft_version: body.draft_version + 1,
        };
        return stored;
      },
    ),
    publish: vi.fn(async (id: string, body: { readonly draft_version: number }) => {
      if (body.draft_version !== (stored ?? draft).draft_version)
        throw new Error('Unexpected draft version');
      stored = {
        ...(stored ?? draft),
        public_id: 'pub_example',
        availability: 'published',
        revision_count: (stored?.revision_count ?? 0) + 1,
      };
      return {
        trigger_id: id,
        public_id: 'pub_example',
        availability: 'published' as const,
        revision: stored.revision_count,
        content: stored.draft,
        digest: 'd'.repeat(64),
        published_at: draft.created_at,
      };
    }),
    retire: vi.fn(async () => {
      stored = { ...(stored ?? published), availability: 'retired', retired_at: draft.created_at };
      return stored;
    }),
    listRevisions: vi.fn(async () => []),
    listPlacements: vi.fn(async () => stored?.placements ?? []),
    createPlacement: vi.fn(
      async (_id: string, body: { readonly label: string; readonly source_url: string }) => {
        const value = { ...body, id: placementCount++ === 0 ? 'plc_readme' : 'plc_web' };
        stored = { ...(stored ?? draft), placements: [...(stored?.placements ?? []), value] };
        return value;
      },
    ),
    retirePlacement: vi.fn(async (_id: string, id: string) => {
      const value = { ...placement, id, retired_at: draft.created_at };
      stored = {
        ...(stored ?? draft),
        placements: (stored?.placements ?? []).map((row) => (row.id === id ? value : row)),
      };
      return value;
    }),
    embed: vi.fn(async () => descriptor),
    stats: vi.fn(async () => ({
      totals: { launches_total: 12, agents_ready_total: 9 },
      placements: [{ placement_id: 'plc_readme', launches_total: 7, agents_ready_total: 5 }],
    })),
  };
  const client: TriggerPublisherClient = {
    triggers,
    listProfiles: vi.fn(async (kind: ProfileKind) => ({ items: [profiles[kind]] })),
    readProfile: vi.fn(
      async (kind: ProfileKind, reference: { readonly id: string; readonly revision: number }) => ({
        ...profiles[kind],
        id: reference.id,
        revision: reference.revision,
      }),
    ),
  };
  return { client, triggers, stored: () => stored };
}
