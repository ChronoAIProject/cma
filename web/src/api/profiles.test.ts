/** @vitest-environment node */
import { describe, expect, it } from 'vitest';
import { ApiError } from '@cma/client';
import {
  NOW,
  PROFILE_CONTENTS,
  restTransport,
  wireMarketplace,
  wireProfile,
  wireRevision,
} from '../test/rest';
import { profileFailure } from '../profiles/failure';
import { createProfileViewsClient } from './profiles';

const options = { idempotencyKey: 'profile-command-819' };

describe('public profile web binding', () => {
  it('round-trips mutable Sandbox availability through the public SDK without changing content', async () => {
    const test = restTransport();
    const api = createProfileViewsClient(test.sdk).sandbox;
    const availability = { warmup_enabled: true, warmup_count: 5, always_keep_alive: true };
    const wire = { ...wireProfile('sandbox'), availability };
    test.reply(wire);
    const profile = await api.readProfile('sandbox-1');
    expect(profile.availability).toEqual({
      warmupEnabled: true,
      warmupCount: 5,
      alwaysKeepAlive: true,
    });
    test.reply(wire);
    await api.saveProfile(
      profile.id,
      {
        name: profile.name,
        description: profile.description,
        content: profile.draft,
        draftVersion: profile.draftVersion,
        availability: profile.availability,
      },
      options,
    );
    expect(test.body()).toMatchObject({ availability, content: PROFILE_CONTENTS.sandbox });
  });
  it('binds public authoring reads and a read-only repository resolve without a command key', async () => {
    const test = restTransport();
    const authoring = createProfileViewsClient(test.sdk).authoring;
    const signal = new AbortController().signal;
    test.reply({
      min_cpu_millis: 1000,
      max_cpu_millis: 4000,
      min_memory_mib: 1024,
      max_memory_mib: 8192,
    });
    expect(await authoring?.readCapacityBounds({ signal })).toEqual({
      minCpuMillis: 1000,
      maxCpuMillis: 4000,
      minMemoryMib: 1024,
      maxMemoryMib: 8192,
    });
    test.reply([{ id: 'github', name: 'GitHub', description: 'Repository access' }]);
    expect(await authoring?.listServices({ signal })).toEqual([
      { id: 'github', name: 'GitHub', description: 'Repository access' },
    ]);
    const skills = {
      items: [{ id: 'research', name: 'Research', versions: ['1.2', '1.1'] }],
      next: 'more',
    };
    test.reply(skills);
    expect(await authoring?.searchSkills('research & tools', { cursor: 'first', signal })).toEqual(
      skills,
    );
    const url = new URL(String(test.fetch.mock.calls.at(-1)?.[0]), 'https://cma.example.test');
    expect(url.pathname).toBe('/api/v1/profile-authoring/skills');
    expect(url.searchParams.get('q')).toBe('research & tools');
    expect(url.searchParams.get('cursor')).toBe('first');
    const repository = {
      url: 'https://github.com/example/research',
      default_branch: 'main',
      commit_sha: 'a'.repeat(40),
    };
    test.reply(repository);
    expect(
      await authoring?.resolveRepository(
        { type: 'git_repository', url: repository.url },
        { signal },
      ),
    ).toEqual(repository);
    expect(test.body()).toEqual({ url: repository.url });
    expect(test.fetch.mock.calls.at(-1)?.[1]?.method).toBe('POST');
    expect(new Headers(test.fetch.mock.calls.at(-1)?.[1]?.headers).has('idempotency-key')).toBe(
      false,
    );
  });

  it('maps all owner lists and Agent draft mutations, publication, sharing and retirement', async () => {
    const test = restTransport();
    const views = createProfileViewsClient(test.sdk);
    for (const kind of ['agent', 'workspace', 'sandbox'] as const) {
      test.reply({ items: [wireProfile(kind)], next: 'next' });
      expect(await views.profiles.listProfiles(kind, { cursor: 'first' })).toMatchObject({
        next: 'next',
        items: [
          {
            id: `${kind}-1`,
            draftVersion: 3,
            createdAt: NOW,
            usesCount: 2,
            latestRevision: { publishedAt: NOW },
          },
        ],
      });
    }
    test.reply(wireProfile('agent'));
    const original = await views.agent.readProfile('agent-1');
    expect(original.draft).toEqual({
      model: 'gpt-5',
      thinkingEffort: 'high',
      permissionLevel: 'guarded',
      skills: PROFILE_CONTENTS.agent.skills,
      systemPrompt: PROFILE_CONTENTS.agent.system_prompt,
    });
    const input = {
      name: original.name,
      description: original.description,
      content: original.draft,
    };
    test.reply(wireProfile('agent'), 201);
    await views.agent.createProfile(input, options);
    expect(test.body()).toEqual({
      name: input.name,
      description: input.description,
      content: PROFILE_CONTENTS.agent,
    });
    test.reply({ ...wireProfile('agent'), draft_version: 4 });
    expect(
      await views.agent.saveProfile('agent-1', { ...input, draftVersion: 3 }, options),
    ).toMatchObject({ draftVersion: 4 });
    expect(test.body()).toMatchObject({ draft_version: 3 });
    test.reply(wireRevision('agent'), 201);
    expect(await views.agent.publishProfile('agent-1', { draftVersion: 4 }, options)).toEqual({
      profileId: 'agent-1',
      revision: 1,
      digest: 'digest-1',
      publishedAt: NOW,
      content: original.draft,
    });
    expect(test.body()).toEqual({ draft_version: 4 });
    test.reply({ ...wireProfile('agent'), visibility: 'public' });
    expect(
      await views.agent.setProfileVisibility('agent-1', { visibility: 'public' }, options),
    ).toMatchObject({ visibility: 'public' });
    test.reply({ ...wireProfile('agent'), retired_at: NOW });
    expect(await views.agent.retireProfile('agent-1', options)).toMatchObject({ retiredAt: NOW });
  });

  it('round-trips complete Workspace and Sandbox settings and strips view-only reference digests', async () => {
    const test = restTransport();
    const views = createProfileViewsClient(test.sdk);
    test.reply(wireProfile('workspace'));
    const workspace = await views.workspace.readProfile('workspace-1');
    expect(workspace.draft).toEqual({
      location: PROFILE_CONTENTS.workspace.location,
      name: 'project',
      setupCommands: ['mkdir -p reports'],
      agentProfiles: [{ id: 'agent-1', revision: 2 }],
    });
    test.reply(wireProfile('workspace'), 201);
    await views.workspace.createProfile(
      {
        name: workspace.name,
        description: workspace.description,
        content: {
          ...workspace.draft,
          agentProfiles: workspace.draft.agentProfiles.map((reference) => ({
            ...reference,
            digest: 'view-only',
          })),
        },
      },
      options,
    );
    expect(test.body()).toMatchObject({ content: PROFILE_CONTENTS.workspace });
    test.reply(wireProfile('sandbox'));
    const sandbox = await views.sandbox.readProfile('sandbox-1');
    expect(sandbox.draft).toEqual({
      cpuMillis: 2000,
      memoryMib: 4096,
      nyxidServices: ['storage'],
      agentProvider: 'codex',
      modelAccess: 'own_credentials',
      maxWorkspaces: 4,
      maxAgentsPerWorkspace: 2,
      workspaceProfiles: [{ id: 'workspace-1', revision: 1 }],
      softwareInstallCommands: ['mkdir -p /home/cma/tools'],
      environmentVariables: [{ name: 'RUN_MODE', value: 'research' }],
    });
    test.reply(wireProfile('sandbox'));
    await views.sandbox.saveProfile(
      'sandbox-1',
      {
        name: sandbox.name,
        description: sandbox.description,
        draftVersion: 3,
        content: {
          ...sandbox.draft,
          workspaceProfiles: sandbox.draft.workspaceProfiles.map((reference) => ({
            ...reference,
            digest: 'view-only',
          })),
        },
      },
      options,
    );
    expect(test.body()).toEqual({
      name: sandbox.name,
      description: sandbox.description,
      draft_version: 3,
      content: PROFILE_CONTENTS.sandbox,
    });
  });

  it.each(['agent', 'workspace', 'sandbox'] as const)(
    'maps complete %s Marketplace entries and pinned forks',
    async (kind) => {
      const test = restTransport();
      const views = createProfileViewsClient(test.sdk);
      const wire = wireMarketplace(kind);
      test.reply({ items: [wire.summary], next: 'next' });
      expect(
        await views.marketplace.listMarketplace(kind, 'research & tools', { cursor: 'first' }),
      ).toMatchObject({
        next: 'next',
        items: [{ kind, latestRevision: 1, publishedAt: NOW, ownerDisplay: 'Research team' }],
      });
      expect(String(test.fetch.mock.calls.at(-1)?.[0])).toContain('q=research+%26+tools');
      test.reply(wire);
      const value = await views.marketplace.readMarketplace(kind, `${kind}-1`);
      expect(value.kind).toBe(kind);
      expect(value.resolvedRevisions.map((value) => value.revision.profileId)).toEqual([
        'agent-1',
        'workspace-1',
        'sandbox-1',
      ]);
      expect(value.resolvedProfiles).toEqual(wire.resolved_profiles);
      if (value.kind === 'sandbox')
        expect(value.trust).toEqual({ requiresConfirmation: true, commandsDigest: 'digest-1' });
      test.reply(wireProfile(kind), 201);
      expect(
        await views.marketplace.forkProfile(kind, `${kind}-1`, { revision: 1 }, options),
      ).toMatchObject({ id: `${kind}-1`, draftVersion: 3 });
      expect(test.body()).toEqual({ revision: 1 });
      expect(new Headers(test.fetch.mock.calls.at(-1)?.[1]?.headers).get('idempotency-key')).toBe(
        options.idempotencyKey,
      );
    },
  );

  it('passes structured SDK validation extensions to field-level profile feedback', async () => {
    const test = restTransport();
    const views = createProfileViewsClient(test.sdk);
    test.reply(
      {
        status: 422,
        code: 'profile_invalid',
        detail: 'Invalid draft',
        field: 'system_prompt',
        reason: 'system_prompt_too_long',
      },
      422,
    );
    const error = await views.agent
      .publishProfile('agent-1', { draftVersion: 3 }, options)
      .catch((error: unknown) => error);
    expect(error).toBeInstanceOf(ApiError);
    expect(profileFailure(error)).toMatchObject({
      uncertain: false,
      fields: { systemPrompt: 'Shorten the system prompt to fit 32 KB of text.' },
    });
  });
});
