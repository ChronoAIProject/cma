import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError, ValidationError } from '../src/index.js';
import {
  command,
  contents,
  marketplaceEntry,
  profile,
  revision,
  transport,
} from './resource-fixtures.js';

describe.each(['agent', 'workspace', 'sandbox'] as const)('%s profiles', (kind) => {
  it('lists, reads, creates, CAS-saves, publishes, pages revisions, shares and retires', async () => {
    const test = transport();
    const api =
      kind === 'agent'
        ? test.client.agentProfiles
        : kind === 'workspace'
          ? test.client.workspaceProfiles
          : test.client.sandboxProfiles;
    const document = profile(kind);
    const body = { name: 'Project', description: 'A test profile', content: contents[kind] };
    test.answer({ items: [document], next: 'next?&' });
    expect(await api.list({ cursor: 'cursor?&', limit: 2 })).toEqual({
      items: [document],
      next: 'next?&',
    });
    expect(test.calls.at(-1)?.url).toContain('cursor=cursor%3F%26&limit=2');
    test.answer(document);
    expect(await api.read(document.id)).toEqual(document);
    test.answer(document, 201);
    // The public generic factory keeps each kind correlated; use the matching instance here.
    if (kind === 'agent')
      await test.client.agentProfiles.create({ ...body, content: contents.agent }, command);
    if (kind === 'workspace')
      await test.client.workspaceProfiles.create({ ...body, content: contents.workspace }, command);
    if (kind === 'sandbox')
      await test.client.sandboxProfiles.create({ ...body, content: contents.sandbox }, command);
    expect(test.calls.at(-1)?.init.method).toBe('POST');
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual(body);
    test.answer(document);
    if (kind === 'agent')
      await test.client.agentProfiles.save(
        document.id,
        { ...body, content: contents.agent, draft_version: 1 },
        command,
      );
    if (kind === 'workspace')
      await test.client.workspaceProfiles.save(
        document.id,
        { ...body, content: contents.workspace, draft_version: 1 },
        command,
      );
    if (kind === 'sandbox')
      await test.client.sandboxProfiles.save(
        document.id,
        { ...body, content: contents.sandbox, draft_version: 1 },
        command,
      );
    expect(test.calls.at(-1)?.init.method).toBe('PUT');
    expect(JSON.parse(String(test.calls.at(-1)?.init.body)).draft_version).toBe(1);
    test.answer(revision(kind), 201);
    expect(await api.publish(document.id, { draft_version: 1 }, command)).toEqual(revision(kind));
    test.answer({ items: [revision(kind)] });
    expect((await api.listRevisions(document.id)).items).toEqual([revision(kind)]);
    test.answer(revision(kind));
    expect(await api.readRevision(document.id, 1)).toEqual(revision(kind));
    test.answer(document);
    await api.setVisibility(document.id, { visibility: 'public' }, command);
    expect(test.calls.at(-1)?.url).toContain('/visibility');
    await api.retire(document.id, command);
    expect(test.calls.at(-1)?.url).toContain('/retire');
    for (const call of test.calls.filter(
      ({ init }) => init.method !== 'GET' && init.method !== undefined,
    )) {
      expect(new Headers(call.init.headers).get('idempotency-key')).toBe(command.idempotencyKey);
      expect(new Headers(call.init.headers).get('x-request-id')).toBe(command.requestId);
    }
  });

  it('reads complete Marketplace revisions and forks one pinned revision', async () => {
    const test = transport();
    const api = test.client.marketplace[kind];
    const entry = marketplaceEntry(kind);
    test.answer({ items: [entry.summary] });
    expect((await api.list({ q: 'a & b', cursor: 'next?', limit: 1 })).items).toEqual([
      entry.summary,
    ]);
    expect(test.calls.at(-1)?.url).toContain('q=a+%26+b&cursor=next%3F&limit=1');
    test.answer(entry);
    expect(await api.read(entry.summary.id)).toEqual(entry);
    test.answer(profile(kind), 201);
    expect(await api.fork(entry.summary.id, { revision: 1 }, command)).toEqual(profile(kind));
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({ revision: 1 });
    expect(test.calls.at(-1)?.url).toContain(`/marketplace/${kind}-profiles/${kind}-1/fork`);
  });
});

describe('profile response boundaries', () => {
  it('preserves Agent isolation when reading and saving a profile', async () => {
    const test = transport();
    const document = {
      ...profile('agent'),
      draft: { ...contents.agent, isolated_agent: true },
    };
    test.answer(document);
    const read = await test.client.agentProfiles.read(document.id);
    expect(read.draft.isolated_agent).toBe(true);
    await test.client.agentProfiles.save(
      read.id,
      { content: read.draft, draft_version: read.draft_version },
      command,
    );
    expect(JSON.parse(String(test.calls.at(-1)?.init.body)).content.isolated_agent).toBe(true);
    test.answer(profile('agent'));
    expect(
      (await test.client.agentProfiles.read(document.id)).draft.isolated_agent,
    ).toBeUndefined();
  });
  it('preserves Sandbox availability in reads, creates and saves and validates the pool bound', async () => {
    const test = transport();
    const availability = { warmup_enabled: true, warmup_count: 5, always_keep_alive: true };
    const document = { ...profile('sandbox'), availability };
    test.answer(document);
    expect((await test.client.sandboxProfiles.read(document.id)).availability).toEqual(
      availability,
    );
    const update = { content: contents.sandbox, draft_version: 1, availability };
    await test.client.sandboxProfiles.save(document.id, update, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body)).availability).toEqual(availability);
    test.answer(document, 201);
    await test.client.sandboxProfiles.create(
      {
        name: 'Profile',
        content: contents.sandbox,
        availability: { ...availability, warmup_enabled: false },
      },
      command,
    );
    expect(JSON.parse(String(test.calls.at(-1)?.init.body)).availability.always_keep_alive).toBe(
      true,
    );
    for (const warmup_count of [0, 6, 1.5]) {
      expect(() =>
        test.client.sandboxProfiles.save(
          document.id,
          { ...update, availability: { ...availability, warmup_count } },
          command,
        ),
      ).toThrow(ValidationError);
    }
  });
  it('allows omitted create descriptions and presence-aware update metadata', async () => {
    const test = transport();
    const document = profile('agent');
    test.answer(document, 201);
    await test.client.agentProfiles.create({ name: 'Project', content: contents.agent }, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({
      name: 'Project',
      content: contents.agent,
    });
    test.answer(document);
    const update = { content: contents.agent, draft_version: document.draft_version };
    await test.client.agentProfiles.save(document.id, update, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual(update);
    await test.client.agentProfiles.save(document.id, { ...update, description: '' }, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body)).description).toBe('');
    expect(() =>
      test.client.agentProfiles.save(document.id, { ...update, name: null as never }, command),
    ).toThrow(ValidationError);
  });

  it('rejects wrong profile and revision identities, kinds and success statuses', async () => {
    const test = transport();
    test.answer(profile('agent'));
    await expect(test.client.agentProfiles.read('other')).rejects.toBeInstanceOf(ProtocolError);
    test.answer(revision('agent'));
    await expect(test.client.agentProfiles.readRevision('other', 1)).rejects.toBeInstanceOf(
      ProtocolError,
    );
    await expect(test.client.agentProfiles.readRevision('agent-1', 2)).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer({ items: [revision('agent')] });
    await expect(test.client.agentProfiles.listRevisions('other')).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer(marketplaceEntry('workspace'));
    await expect(test.client.marketplace.agent.read('workspace-1')).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer({ items: [marketplaceEntry('workspace').summary] });
    await expect(test.client.marketplace.agent.list()).rejects.toBeInstanceOf(ProtocolError);
    const entry = marketplaceEntry('agent');
    test.answer({ ...entry, summary: { ...entry.summary, latest_revision: 2 } });
    await expect(test.client.marketplace.agent.read('agent-1')).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer(profile('agent'), 202);
    await expect(
      test.client.agentProfiles.create(
        { name: 'Project', description: '', content: contents.agent },
        command,
      ),
    ).rejects.toThrow(/expected HTTP 201/);
  });

  it('keeps nested resolved profile settings and Sandbox trust evidence', async () => {
    const test = transport();
    const entry = {
      ...marketplaceEntry('sandbox'),
      resolved_profiles: [{ kind: 'workspace', id: 'workspace-1', revision: 1, name: 'Project' }],
      resolved_revisions: (['agent', 'workspace', 'sandbox'] as const).map((kind) => ({
        kind,
        name: 'Project',
        revision: revision(kind),
      })),
    };
    test.answer(entry);
    expect(await test.client.marketplace.sandbox.read('sandbox-1')).toEqual(entry);
  });

  it('validates commands before transport and preserves machine-readable conflict extensions', async () => {
    const test = transport();
    for (const revision of [0, -1, 0.5, Number.MAX_SAFE_INTEGER + 1]) {
      expect(() => test.client.agentProfiles.readRevision('agent-1', revision)).toThrow(
        ValidationError,
      );
      expect(() => test.client.marketplace.agent.fork('agent-1', { revision }, command)).toThrow(
        ValidationError,
      );
    }
    expect(() =>
      test.client.agentProfiles.save(
        'agent-1',
        { name: 'Project', description: '', content: contents.agent, draft_version: 0 },
        command,
      ),
    ).toThrow(ValidationError);
    expect(test.fetch).not.toHaveBeenCalled();
    test.answer(
      {
        status: 409,
        code: 'draft_version_conflict',
        detail: 'Draft changed',
        field: 'draft_version',
        path: 'content.model',
        reason: 'stale',
        current_version: 2,
      },
      409,
    );
    const error = await test.client.agentProfiles
      .publish('agent-1', { draft_version: 1 }, command)
      .catch((error: unknown) => error);
    expect(error).toBeInstanceOf(ApiError);
    expect(error).toMatchObject({
      field: 'draft_version',
      path: 'content.model',
      reason: 'stale',
      extensions: { current_version: 2 },
    });
    expect(Object.isFrozen((error as ApiError).extensions)).toBe(true);
    expect(test.fetch).toHaveBeenCalledOnce();
  });
});
