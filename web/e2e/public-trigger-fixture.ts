import assert from 'node:assert/strict';
import { triggerContent } from '../src/api-client/trigger-authoring/types';
import { object } from './environment';

// This boundary models TriggerRepositoryBody's strict serde wire contract.
export function materializeCurrentTriggerContent(
  value: unknown,
  expectedRepository: { url: string; ref: string },
  suffix: string,
) {
  const content = object(value);
  const inline = object(object(content.target).inline);
  const repository = object(object(inline.workspace).repository);
  assert.deepEqual(Object.keys(repository).sort(), ['ref', 'url']);
  assert.match(String(repository.ref), /^[a-f0-9]{40}$/);
  assert.deepEqual(repository, expectedRepository);
  const parsed = triggerContent(content);
  return {
    ...parsed,
    target: {
      ...parsed.target,
      sandbox_profile: { id: `generated-sandbox-${suffix}`, revision: 1 },
      workspace_profile: { id: `generated-workspace-${suffix}`, revision: 1 },
      agent_profile: { id: `generated-agent-${suffix}`, revision: 1 },
    },
  };
}
