export const conversation = {
  agent_id: 'agent-fixture',
  conversation_id: 'conversation-fixture',
  history_sha256: 'a'.repeat(64),
};
const common = {
  runtime_id: 'runtime-fixture',
  runtime_generation: 2,
  home_identity: 'home-fixture',
  boot_id: 'boot-fixture',
  runtime_process_absent: true,
  provider_plan_sha256: 'b'.repeat(64),
  conversations: [conversation],
};
export const proof = {
  ...common,
  sandbox_id: 'sbx_1',
  requires_native_quiescence: true,
  provider_plan: { diagnostic: 'private provider evidence' },
  reserved_provision: { operation_id: 'operation-fixture', generation: 3 },
  observed_provision_generation: 3,
};
export const receipt = {
  ...common,
  operation_id: 'operation-fixture',
  completed_at: '2026-11-01T00:00:00Z',
};
