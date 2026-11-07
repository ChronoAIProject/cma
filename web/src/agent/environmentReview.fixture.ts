import type { AgentEnvironment } from '@chronoai/cma-sdk';
const available = {
  state: 'available',
  reason: 'agent_browser',
  scope: 'agent_browser',
  writable: false,
} as const;
export const environmentFixture: AgentEnvironment = {
  agent_id: 'agt_1',
  runtime_incarnation: 'runtime',
  policy_revision: 1,
  revalidate_at: null,
  files: { ...available, reason: 'mounted_home', scope: 'mounted_home' },
  operation_output: { ...available, reason: 'operation_records_only', scope: 'agent_turn_history' },
  browser: available,
  replay: { ...available, reason: 'operation_records_only', scope: 'agent_turn_history' },
  control: {
    state: 'unsupported',
    reason: 'not_supported_by_contract',
    scope: 'none',
    writable: false,
  },
};
