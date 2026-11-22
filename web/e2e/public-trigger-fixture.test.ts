// @vitest-environment node
import { readFileSync } from 'node:fs';
import { expect, it } from 'vitest';
import { materializeCurrentTriggerContent } from './public-trigger-fixture';
import { assertInlineRequest, assertInlineTarget } from './public-journey-contracts';

const body = JSON.parse(
  readFileSync(
    new URL(
      '../../api/crates/layer1-public/tests/fixtures/trigger-inline/pinned-request.json',
      import.meta.url,
    ),
    'utf8',
  ),
);
const repository = { url: 'https://github.com/example/repository', ref: 'a'.repeat(40) };

it('uses the same pinned body as the backend owner-positive and visitor-negative contract', () => {
  const materialized = materializeCurrentTriggerContent(body.content, repository, 'fixture');
  assertInlineRequest(body.content.target, body.content.target.inline);
  assertInlineTarget(materialized.target, body.content.target.inline);
  expect(materialized.target.inline?.workspace?.repository).toEqual(repository);
  expect(materialized.target.agent_profile.id).toBe('generated-agent-fixture');
});
for (const invalid of [
  { url: repository.url, reference: repository.ref },
  { ...repository, reference: repository.ref },
  { ...repository, unknown: true },
  { ...repository, ref: 'b'.repeat(40) },
  { ...repository, ref: 'main' },
])
  it('rejects a wrong wire field, unknown field or changed commit at the synthetic backend', () => {
    const content = structuredClone(body.content);
    content.target.inline.workspace.repository = invalid;
    expect(() => materializeCurrentTriggerContent(content, repository, 'fixture')).toThrow();
  });
