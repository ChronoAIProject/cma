import { describe, expect, it } from 'vitest';
import { ProtocolError, ValidationError } from '../src/index.js';
import * as schema from '../src/bridges/schema.js';
import { loadCorpus } from './public-contract/context.js';
import { transport } from './resource-fixtures.js';
const row = loadCorpus().find((item) => item.id === 'bridge-setup-create-success')!;
const create = schema.createRequest(row.request.body);
const shared = {
  sandbox_id: 'sandbox-reused',
  agent_profile: { id: 'agent-profile', revision: 3 },
  shared_trust: 'all_participants_share_filesystem_processes_and_credentials' as const,
  capacity: { keep_other_workspaces: 1, keep_other_agents_per_workspace: 2 },
};
const placements = [
  {
    ...shared,
    kind: 'workspace_per_partition' as const,
    workspace_profile: { id: 'workspace-profile', revision: 2 },
  },
  { ...shared, kind: 'worktree_agent_per_partition' as const, workspace_id: 'workspace-reused' },
];
describe('shared parent placement contract', () => {
  it.each(placements)(
    'retains explicit parent, immutable pins, trust and reserved capacity for $kind',
    async (placement) => {
      const test = transport();
      test.answer(row.response.body, 201);
      const body = {
        ...create,
        config: { ...create.config, partition: 'conversation_sender' as const, placement },
      };
      await test.client.bridges.create(body, { idempotencyKey: 'shared-parent' });
      expect(JSON.parse(String(test.calls[0]!.init.body))).toEqual(body);
      expect(schema.configuration(body.config).placement).toEqual(placement);
    },
  );
  it.each(placements)(
    'requires trust and bounded owner capacity before transport for $kind',
    (placement) => {
      const test = transport();
      for (const wrong of [
        { ...placement, shared_trust: undefined },
        { ...placement, shared_trust: 'private_per_user_isolation' },
        { ...placement, capacity: undefined },
        { ...placement, capacity: { ...placement.capacity, keep_other_workspaces: 0 } },
        { ...placement, capacity: { ...placement.capacity, keep_other_agents_per_workspace: 16 } },
        { ...placement, agent_profile: { ...placement.agent_profile, revision: 0 } },
      ]) {
        expect(() => schema.placement(wrong)).toThrow(ProtocolError);
        expect(() =>
          test.client.bridges.create(
            { ...create, config: { ...create.config, placement: wrong as never } },
            { idempotencyKey: 'invalid' },
          ),
        ).toThrow(ValidationError);
      }
      expect(test.calls).toHaveLength(0);
    },
  );
});
