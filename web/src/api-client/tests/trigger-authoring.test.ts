// @vitest-environment node
import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import { ApiError, ProtocolError, ValidationError } from '../errors.js';
import {
  TriggerAuthoring,
  type TriggerContent,
  type TriggerContentInput,
} from '../trigger-authoring/index.js';
import { command, transport } from './resource-fixtures.js';

const content: TriggerContent = {
  presentation: {
    title: 'trureturing',
    description: 'Work on trureturing',
    button_label: 'open_in_cma',
    theme: 'auto',
  },
  target: {
    sandbox_profile: { id: 'sp_standard', revision: 2 },
    workspace_profile: { id: 'wp_trureturing', revision: 3 },
    agent_profile: { id: 'ap_codex', revision: 4 },
  },
  reuse: { sandbox: 'reuse_existing', workspace: 'reuse_existing', agent: 'ask' },
  first_message: 'Inspect the project',
  consent: { always_review: true },
};
const placement = {
  id: 'plc_readme',
  source_url: 'https://github.com/chronoai/trureturing',
  label: 'README',
};
const document = {
  id: 'trg_project',
  draft: content,
  draft_version: 1,
  availability: 'draft',
  revision_count: 0,
  placements: [],
  stats: { launches_total: 0, agents_ready_total: 0 },
  created_at: '2026-09-06T00:00:00Z',
  updated_at: '2026-09-06T00:00:00Z',
};
const revision = { revision: 1, content, digest: 'digest-1', published_at: '2026-09-06T00:00:00Z' };

describe('Trigger authoring', () => {
  it('binds every owner route and translates the Trigger-specific page cursor', async () => {
    const test = transport();
    const api = new TriggerAuthoring(test.client.config);
    test.answer({ items: [document], next_cursor: 'more?&' });
    expect(await api.list({ cursor: 'old?&', limit: 2 })).toEqual({
      items: [document],
      next: 'more?&',
    });
    expect(test.calls.at(-1)?.url).toContain('cursor=old%3F%26&limit=2');
    test.answer(document);
    expect(await api.read(document.id)).toEqual(document);
    test.answer(document, 201);
    await api.create({ content }, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({ content });
    test.answer(document);
    await api.save(document.id, { content, draft_version: 1 }, command);
    expect(test.calls.at(-1)?.init.method).toBe('PUT');
    test.answer(
      { trigger_id: document.id, public_id: 'pub_project', availability: 'published', ...revision },
      201,
    );
    await api.publish(document.id, { draft_version: 1 }, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({ draft_version: 1 });
    test.answer([revision]);
    expect(await api.listRevisions(document.id)).toEqual([revision]);
    test.answer([placement]);
    expect(await api.listPlacements(document.id, { includeRetired: true })).toEqual([placement]);
    expect(test.calls.at(-1)?.url).toContain('include_retired=true');
    test.answer(placement, 201);
    await api.createPlacement(
      document.id,
      { source_url: placement.source_url, label: placement.label },
      command,
    );
    test.answer({ ...placement, retired_at: document.created_at });
    await api.retirePlacement(document.id, placement.id, command);
    expect(test.calls.at(-1)?.url).toContain('/placements/plc_readme/retire');
    test.answer({ totals: document.stats, placements: [] });
    expect(await api.stats(document.id)).toEqual({ totals: document.stats, placements: [] });
    test.answer({ ...document, availability: 'retired' });
    await api.retire(document.id, command);
    test.answer(undefined, 204);
    await api.delete(document.id, command);
    expect(test.calls.at(-1)?.init.method).toBe('DELETE');
    for (const call of test.calls.filter(
      ({ init }) => init.method !== 'GET' && init.method !== undefined,
    ))
      expect(new Headers(call.init.headers).get('idempotency-key')).toBe(command.idempotencyKey);
  });

  it('preserves the API golden snippets, SRI and CSP byte for byte', async () => {
    const test = transport();
    const golden: unknown = JSON.parse(
      readFileSync(
        new URL(
          '../../../../api/crates/layer1-public/tests/fixtures/trigger-embed/descriptor.json',
          import.meta.url,
        ),
        'utf8',
      ),
    );
    test.answer(golden);
    expect(await new TriggerAuthoring(test.client.config).embed(document.id, 'plc_readme')).toEqual(
      golden,
    );
    expect(test.calls.at(-1)?.url).toContain('/embed?placement_id=plc_readme');
  });

  it('decodes inline targets with legacy profile sentinels', async () => {
    const test = transport();
    const inlineDocument = {
      ...document,
      draft: {
        ...content,
        target: {
          sandbox_profile: { id: '', revision: 0 },
          workspace_profile: { id: '', revision: 0 },
          agent_profile: { id: '', revision: 0 },
          inline: {
            image: { id: 'private-codex', version: 'v3' },
            agent: {
              model: 'gpt-5-codex',
              reasoning_effort: 'medium',
              permission_mode: 'guarded',
              skills: [],
              system_prompt: 'Inspect the project.',
              services: ['api-github'],
            },
          },
        },
      },
    };
    test.answer(inlineDocument);
    const parsed = await new TriggerAuthoring(test.client.config).read(document.id);
    expect(parsed.draft.target.inline?.image?.id).toBe('private-codex');
    expect(parsed.draft.target.sandbox_profile).toEqual({ id: '', revision: 1 });
    expect(parsed.draft.target.agent_profile).toEqual({ id: '', revision: 1 });
  });

  it('rejects mismatched identities and invalid mutations before transport', async () => {
    const test = transport();
    const api = new TriggerAuthoring(test.client.config);
    expect(() => api.save(document.id, { content, draft_version: 0 }, command)).toThrow(
      ValidationError,
    );
    expect(() => api.create({ content }, { idempotencyKey: '' })).toThrow(ValidationError);
    expect(test.fetch).not.toHaveBeenCalled();
    test.answer(document);
    await expect(api.read('trg_other')).rejects.toBeInstanceOf(ProtocolError);
    test.answer(
      { trigger_id: 'trg_other', public_id: 'pub_project', availability: 'published', ...revision },
      201,
    );
    await expect(api.publish(document.id, { draft_version: 1 }, command)).rejects.toBeInstanceOf(
      ProtocolError,
    );
    test.answer(placement);
    await expect(api.retirePlacement(document.id, 'plc_other', command)).rejects.toBeInstanceOf(
      ProtocolError,
    );
  });

  it('creates and saves an inline Agent without sending legacy profile references', async () => {
    const test = transport();
    const api = new TriggerAuthoring(test.client.config);
    const inline: TriggerContentInput = {
      ...content,
      target: {
        inline: {
          agent: {
            reasoning_effort: 'medium',
            permission_mode: 'guarded',
            skills: [],
            system_prompt: 'Inspect the project.',
            services: [],
          },
          workspace: { repository: { url: 'https://github.com/example/repo', ref: 'main' } },
        },
      },
    };
    test.answer({ ...document, draft: inline }, 201);
    const created = await api.create({ content: inline }, command);
    expect(created.draft.target.inline).toEqual(inline.target.inline);
    expect(created.draft.target.agent_profile).toEqual({ id: '', revision: 1 });
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({ content: inline });
    test.answer({ ...document, draft: inline });
    await api.save(document.id, { content: inline, draft_version: 1 }, command);
    expect(JSON.parse(String(test.calls.at(-1)?.init.body))).toEqual({
      content: inline,
      draft_version: 1,
    });
  });

  it('rejects incomplete legacy targets and named profiles with zero revision', async () => {
    const test = transport();
    const api = new TriggerAuthoring(test.client.config);
    for (const field of ['sandbox_profile', 'workspace_profile', 'agent_profile'] as const) {
      const missing: Partial<TriggerContent['target']> = { ...content.target };
      delete missing[field];
      for (const target of [
        missing,
        { ...content.target, [field]: { id: '', revision: 0 } },
        { ...content.target, [field]: { id: 'profile_one', revision: 0 } },
      ]) {
        test.answer({ ...document, draft: { ...content, target } });
        await expect(api.read(document.id)).rejects.toBeInstanceOf(ProtocolError);
      }
    }
  });

  it('retains private-profile refusals and command keys for caller reconciliation', async () => {
    const test = transport();
    const api = new TriggerAuthoring(test.client.config);
    test.answer(
      {
        status: 422,
        code: 'private_profile_reference',
        detail: 'Publish the profile first',
        field: 'target.workspace_profile',
        reason: 'private',
      },
      422,
    );
    const invoke = () => api.publish(document.id, { draft_version: 1 }, command);
    await expect(invoke()).rejects.toMatchObject({
      field: 'target.workspace_profile',
      reason: 'private',
    });
    await expect(invoke()).rejects.toBeInstanceOf(ApiError);
    expect(new Headers(test.calls.at(-1)?.init.headers).get('idempotency-key')).toBe(
      command.idempotencyKey,
    );
  });
});
