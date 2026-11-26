// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { TriggerAuthoring, type TriggerContentInput } from '../trigger-authoring/index.js';
import { ProtocolError, ValidationError } from '../errors.js';
import { command, transport } from './resource-fixtures.js';

const pin = 'a'.repeat(40);
const fixture = JSON.parse(
  readFileSync(
    new URL(
      '../../../../api/crates/layer1-public/tests/fixtures/trigger-inline/pinned-request.json',
      import.meta.url,
    ),
    'utf8',
  ),
) as { content: TriggerContentInput };
const content = fixture.content;
const materialized = {
  ...content,
  target: {
    ...content.target,
    sandbox_profile: { id: 'sbp_generated', revision: 1 },
    workspace_profile: { id: 'wsp_generated', revision: 1 },
    agent_profile: { id: 'agp_generated', revision: 1 },
  },
};
const document = {
  id: 'trg_test',
  draft: materialized,
  draft_version: 1,
  availability: 'draft',
  revision_count: 0,
  placements: [],
  stats: { launches_total: 0, agents_ready_total: 0 },
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z',
};

it('create, edit, publish and read preserve the exact Trigger ref pin with generated profiles', async () => {
  const t = transport();
  const api = new TriggerAuthoring(t.client.config);
  t.answer(document, 201);
  const created = await api.create({ content }, command);
  expect(JSON.parse(String(t.calls.at(-1)?.init.body))).toEqual({ content });
  expect(created.draft.target.inline?.workspace?.repository).toEqual({
    url: 'https://github.com/example/repository',
    ref: pin,
  });
  expect(created.draft.target.sandbox_profile.id).toBe('sbp_generated');
  t.answer(document);
  await api.save(document.id, { content: created.draft, draft_version: 1 }, command);
  expect(
    JSON.parse(String(t.calls.at(-1)?.init.body)).content.target.inline.workspace.repository,
  ).toEqual({ url: 'https://github.com/example/repository', ref: pin });
  const revision = {
    revision: 1,
    content: materialized,
    digest: 'b'.repeat(64),
    published_at: document.created_at,
  };
  t.answer(
    { ...revision, trigger_id: document.id, public_id: 'pub_test', availability: 'published' },
    201,
  );
  const published = await api.publish(document.id, { draft_version: 1 }, command);
  expect(published.content.target.inline?.workspace?.repository?.ref).toBe(pin);
  t.answer([revision]);
  expect(
    (await api.listRevisions(document.id))[0]?.content.target.inline?.workspace?.repository?.ref,
  ).toBe(pin);
});

for (const repository of [
  { url: 'https://github.com/example/repository', reference: pin },
  { url: 'https://github.com/example/repository', ref: pin, reference: pin },
  { url: 'https://github.com/example/repository', ref: pin, extra: 'unknown' },
])
  it('refuses invalid Trigger repository fields before transport and when decoding a response', async () => {
    const t = transport();
    const api = new TriggerAuthoring(t.client.config);
    const invalid = {
      ...content,
      target: { inline: { ...content.target.inline!, workspace: { repository } } },
    };
    expect(() => api.create({ content: invalid }, command)).toThrow(ValidationError);
    expect(t.fetch).not.toHaveBeenCalled();
    t.answer({ ...document, draft: invalid });
    await expect(api.read(document.id)).rejects.toBeInstanceOf(ProtocolError);
  });
