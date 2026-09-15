import {
  createClient,
  type ProfileContents,
  type ProfileDocument,
  type ProfileKind,
} from '@chronoai/cma-sdk';
import { render } from '@testing-library/react';
import { vi } from 'vitest';
import { App } from '../App';
import { CmaClientProvider } from '../api/context';
import { createProfileViewsClient } from '../api/profiles';
import { createRestWebClient } from '../api/rest';
import { AuthContextProvider } from '../auth/context';
import { NOW } from './rest';

export const AUTHORING_BOUNDS = {
  min_cpu_millis: 1000,
  max_cpu_millis: 4000,
  min_memory_mib: 1024,
  max_memory_mib: 8192,
};
export const REPOSITORY = 'https://github.com/the-omega-institute/trureturing';
export const PROFILE_IDS = {
  agent: `agp_${'a'.repeat(32)}`,
  workspace: `wkp_${'b'.repeat(32)}`,
  sandbox: `sbp_${'c'.repeat(32)}`,
};
export const FORK_ID = `sbp_${'d'.repeat(32)}`;
const PREFIXES = { agent: 'agp_', workspace: 'wkp_', sandbox: 'sbp_' };
const kindOf = (id: string) =>
  (Object.keys(PREFIXES) as ProfileKind[]).find((kind) => id.startsWith(PREFIXES[kind]));
export const RESEARCH_SKILL = {
  id: 'research',
  name: 'Research',
  description: 'Repository research',
  versions: ['1.2', '1.1'],
};
export const REPOSITORY_RESULT = {
  url: REPOSITORY,
  reference: 'main',
  default_branch: 'main',
  commit_sha: 'a'.repeat(40),
};

interface RequestRecord {
  readonly url: URL;
  readonly method: string;
  readonly body: unknown;
  readonly headers: Headers;
  readonly signal: AbortSignal | null | undefined;
}
type Document = ProfileDocument<ProfileContents[ProfileKind]>;
type Intercept = (request: RequestRecord) => Response | Promise<Response> | undefined;

export function profileAuthoringTransport() {
  const records = new Map<string, Document>();
  const requests: RequestRecord[] = [];
  const unexpected: string[] = [];
  let intercept: Intercept = () => undefined;
  const summary = (kind: ProfileKind, record: Document) => ({
    kind,
    id: record.id,
    name: record.name,
    description: record.description,
    owner_display: record.owner,
    latest_revision: record.latest_revision?.revision ?? 0,
    published_at: NOW,
    uses_count: record.uses_count,
  });
  const revision = (record: Document) => ({
    profile_id: record.id,
    revision: record.latest_revision?.revision ?? 1,
    digest: record.id.slice(4).repeat(2),
    published_at: NOW,
    content: record.draft,
  });
  const fetch = vi.fn<typeof globalThis.fetch>(async (input, init) => {
    const request: RequestRecord = {
      url: new URL(String(input), 'https://cma.example.test'),
      method: init?.method ?? 'GET',
      body: init?.body === undefined ? undefined : (JSON.parse(String(init.body)) as unknown),
      headers: new Headers(init?.headers),
      signal: init?.signal,
    };
    requests.push(request);
    const override = intercept(request);
    if (override !== undefined) return override;
    const path = request.url.pathname;
    const owner = request.headers.get('authorization')?.replace('Bearer ', '') ?? 'anonymous';
    if (path === '/api/v1/sandboxes') return Response.json({ items: [] });
    if (path === '/api/v1/organizations') return Response.json({ items: [] });
    if (path === '/api/v1/profile-authoring/bounds') return Response.json(AUTHORING_BOUNDS);
    if (path === '/api/v1/profile-authoring/services')
      return Response.json([{ id: 'github', name: 'GitHub' }]);
    if (path === '/api/v1/profile-authoring/skills')
      return Response.json({ items: [RESEARCH_SKILL] });
    if (path === '/api/v1/profile-authoring/repositories/resolve') {
      const body = request.body as { url: string; reference?: string };
      return Response.json({ ...REPOSITORY_RESULT, reference: body.reference });
    }
    const match =
      /^\/api\/v1\/(marketplace\/)?(agent|workspace|sandbox)-profiles(?:\/([^/]+))?(?:\/(revisions|visibility|fork))?$/.exec(
        path,
      );
    const kind = match?.[2];
    if (kind !== 'agent' && kind !== 'workspace' && kind !== 'sandbox') {
      unexpected.push(`${request.method} ${path}`);
      throw new Error(`Unexpected authoring test route ${path}`);
    }
    const marketplace = match?.[1] !== undefined;
    const id = match?.[3];
    const action = match?.[4];
    if (request.method === 'GET' && id === undefined) {
      const items = [...records.values()].filter(
        (record) =>
          record.id.startsWith(PREFIXES[kind]) &&
          (marketplace ? record.visibility === 'public' : record.owner === owner),
      );
      return Response.json({
        items: marketplace ? items.map((record) => summary(kind, record)) : items,
      });
    }
    const record = id === undefined ? undefined : records.get(id);
    if (request.method === 'GET' && record !== undefined) {
      if (!marketplace) return Response.json(record);
      const resolved = [...records.values()].filter(
        (item) => item.id !== id && item.latest_revision !== undefined,
      );
      return Response.json({
        summary: summary(kind, record),
        revision: revision(record),
        resolved_profiles: resolved.map((item) => ({
          kind: kindOf(item.id),
          id: item.id,
          name: item.name,
          revision: item.latest_revision?.revision,
        })),
        resolved_revisions: resolved.map((item) => ({
          kind: kindOf(item.id),
          name: item.name,
          revision: revision(item),
        })),
        ...(kind === 'sandbox'
          ? { trust: { requires_confirmation: true, commands_digest: 'e'.repeat(64) } }
          : {}),
      });
    }
    if (request.method === 'POST' && id === undefined) {
      const body = request.body as {
        name: string;
        description: string;
        content: ProfileContents[ProfileKind];
      };
      const created: Document = {
        id: PROFILE_IDS[kind],
        owner,
        name: body.name,
        description: body.description,
        visibility: 'private',
        uses_count: 0,
        draft_version: 1,
        draft: body.content,
        created_at: NOW,
        updated_at: NOW,
      };
      records.set(created.id, created);
      return Response.json(created, { status: 201 });
    }
    if (record !== undefined && action === 'fork' && request.method === 'POST') {
      const forked: Document = {
        ...record,
        id: FORK_ID,
        owner,
        visibility: 'private',
        latest_revision: undefined,
        draft_version: 1,
        forked_from: { id: record.id, revision: record.latest_revision?.revision ?? 1 },
      };
      records.set(forked.id, forked);
      return Response.json(forked, { status: 201 });
    }
    if (record !== undefined && action === 'revisions' && request.method === 'POST') {
      if ((request.body as { draft_version: number }).draft_version !== record.draft_version)
        return Response.json({ status: 409, code: 'profile_version_conflict' }, { status: 409 });
      const published = {
        ...record,
        latest_revision: { revision: 1, digest: record.id.slice(4).repeat(2), published_at: NOW },
      };
      records.set(record.id, published);
      return Response.json(revision(published), { status: 201 });
    }
    if (record !== undefined && action === 'visibility' && request.method === 'PUT') {
      const shared = {
        ...record,
        visibility: (request.body as { visibility: 'public' | 'private' }).visibility,
      };
      records.set(record.id, shared);
      return Response.json(shared);
    }
    if (record !== undefined && action === undefined && request.method === 'PUT') {
      const body = request.body as {
        name: string;
        description: string;
        content: ProfileContents[ProfileKind];
        draft_version: number;
      };
      if (body.draft_version !== record.draft_version)
        return Response.json({ status: 409, code: 'profile_version_conflict' }, { status: 409 });
      const saved = {
        ...record,
        name: body.name,
        description: body.description,
        draft: body.content,
        draft_version: record.draft_version + 1,
      };
      records.set(record.id, saved);
      return Response.json(saved);
    }
    unexpected.push(`${request.method} ${path}`);
    throw new Error(`Unsupported authoring test request ${request.method} ${path}`);
  });
  return {
    records,
    requests,
    unexpected,
    fetch,
    intercept: (next: Intercept) => {
      intercept = next;
    },
    writes: () =>
      requests.filter(
        (request) =>
          request.method !== 'GET' && !request.url.pathname.endsWith('/repositories/resolve'),
      ),
    render: (owner = 'research-owner', path = '/profiles') => {
      window.history.replaceState({}, '', path);
      const sdk = createClient({ fetch, credential: () => owner });
      const auth = {
        current: async () => ({
          status: 'authenticated' as const,
          user: {
            id: owner,
            handle: owner,
            visitor: false,
            capabilities: ['manage_own_profiles' as const],
          },
        }),
        signIn: async () => {},
        signOut: async () => {},
      };
      return render(
        <AuthContextProvider provider={auth}>
          <CmaClientProvider
            client={createRestWebClient(sdk)}
            profiles={createProfileViewsClient(sdk)}
          >
            <App />
          </CmaClientProvider>
        </AuthContextProvider>,
      );
    },
  };
}
