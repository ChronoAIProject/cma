import type { TriggerInlineTarget } from '@cma/client';
import type { PublisherRole, StagingFixtures } from './environment';

export function currentTriggerTarget(
  fixture: StagingFixtures,
  role: PublisherRole,
  systemPrompt = '',
): TriggerInlineTarget {
  const owner = fixture.publicJourneys.owners[role];
  const repository = fixture.publicJourneys.repository;
  return {
    agent: {
      model: owner.model,
      reasoning_effort: role === 'owner' ? 'high' : 'medium',
      permission_mode: 'guarded',
      skills: [],
      system_prompt: systemPrompt,
      services: fixture.baselineServices,
    },
    workspace: { repository: { url: repository.url, ref: repository.reference } },
  };
}
