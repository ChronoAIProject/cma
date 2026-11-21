import { createClient, type ProfileKind } from '@cma/client';
import { describe, expect, it, vi } from 'vitest';
import { createTriggerPublisherClient } from './client';
import { draft, profiles } from './fixtures';

function profileDocument(kind: ProfileKind) {
  const profile = profiles[kind];
  return {
    id: profile.id,
    owner: 'owner-1',
    name: profile.name,
    description: profile.description,
    visibility: 'public',
    uses_count: 0,
    draft: profile.content,
    draft_version: 7,
    latest_revision: {
      profile_id: profile.id,
      revision: 9,
      content: profile.content,
      digest: 'digest-latest',
      published_at: draft.created_at,
    },
    created_at: draft.created_at,
    updated_at: draft.updated_at,
  };
}

describe('publisher SDK adapter', () => {
  it.each(['sandbox', 'workspace', 'agent'] as const)(
    'reads the exact selected %s revision, not the latest draft',
    async (kind) => {
      const selected = profiles[kind];
      const fetch = vi.fn(async (input: string | URL | Request) => {
        const path = new URL(String(input)).pathname;
        const value = path.endsWith(`/revisions/${String(selected.revision)}`)
          ? {
              profile_id: selected.id,
              revision: selected.revision,
              content: selected.content,
              digest: selected.digest,
              published_at: draft.created_at,
            }
          : profileDocument(kind);
        return new Response(JSON.stringify(value));
      });
      const client = createTriggerPublisherClient(
        createClient({ baseUrl: 'https://cma.example', fetch }),
      );
      expect(await client.readProfile(kind, selected)).toEqual(selected);
      expect(fetch).toHaveBeenCalledTimes(2);
      expect(String(fetch.mock.calls[1]?.[0])).toContain(`/revisions/${String(selected.revision)}`);
    },
  );

  it('falls back to public Marketplace metadata only on owner-read refusal', async () => {
    const selected = profiles.agent;
    const revision = {
      profile_id: selected.id,
      revision: selected.revision,
      content: selected.content,
      digest: selected.digest,
      published_at: draft.created_at,
    };
    const fetch = vi.fn(async (input: string | URL | Request) => {
      const path = new URL(String(input)).pathname;
      if (path.includes('/marketplace/'))
        return new Response(
          JSON.stringify({
            summary: {
              kind: 'agent',
              id: selected.id,
              name: selected.name,
              description: selected.description,
              owner_display: 'Publisher',
              latest_revision: selected.revision,
              published_at: draft.created_at,
              uses_count: 0,
            },
            revision,
            resolved_profiles: [],
            resolved_revisions: [],
          }),
        );
      if (path.includes('/revisions/')) return new Response(JSON.stringify(revision));
      return new Response(JSON.stringify({ status: 403, code: 'forbidden', detail: 'Not owned' }), {
        status: 403,
      });
    });
    const client = createTriggerPublisherClient(
      createClient({ baseUrl: 'https://cma.example', fetch }),
    );
    expect(await client.readProfile('agent', selected)).toEqual(selected);
    expect(fetch).toHaveBeenCalledTimes(3);
    fetch.mockClear();
    fetch.mockImplementation(
      async () =>
        new Response(JSON.stringify({ status: 503, detail: 'Unavailable' }), { status: 503 }),
    );
    await expect(client.readProfile('agent', selected)).rejects.toMatchObject({ status: 503 });
    expect(fetch).toHaveBeenCalledOnce();
  });

  it('filters unpublished and retired own profiles while retaining exact visible revisions and paging', async () => {
    const document = profileDocument('agent');
    const fetch = vi.fn(
      async () =>
        new Response(
          JSON.stringify({
            items: [
              document,
              { ...document, id: 'draft-only', latest_revision: undefined },
              {
                ...document,
                id: 'retired',
                latest_revision: { ...document.latest_revision, profile_id: 'retired' },
                retired_at: draft.created_at,
              },
            ],
            next: 'next-page',
          }),
        ),
    );
    const client = createTriggerPublisherClient(
      createClient({ baseUrl: 'https://cma.example', fetch }),
    );
    expect(await client.listProfiles('agent', 'own')).toEqual({
      items: [
        {
          id: document.id,
          revision: 9,
          name: document.name,
          description: document.description,
          public: true,
        },
      ],
      next: 'next-page',
    });
  });
});
