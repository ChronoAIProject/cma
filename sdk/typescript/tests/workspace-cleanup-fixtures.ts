export const bridge = 'bridge-cleanup';
export const allocation = `bral_${'1'.repeat(64)}`;
export const target = {
  sandbox_id: 'sbx_parent',
  workspace_id: 'wks_parent',
  worktree_id: 'wkt_child',
  agent_id: 'agt_child',
};
export const request = {
  expected_bridge_revision: 3,
  expected_allocation_revision: 4,
  expected_workspace_revision: 3,
  child_cleanup_receipt_id: `bclj_${'3'.repeat(64)}`,
  action: 'remove_owned_workspace' as const,
};
export function status() {
  return {
    bridge_id: bridge,
    allocation_id: allocation,
    generation: 1,
    allocation_revision: 4,
    target,
    workspace_revision: 3,
    child_cleanup_receipt_id: request.child_cleanup_receipt_id,
    eligible: false,
    runtime_supported: true,
    receipt: {
      id: `bwcl_${'2'.repeat(64)}`,
      bridge_id: bridge,
      allocation_id: allocation,
      generation: 1,
      target,
      request,
      requested_by: { kind: 'api', subject: 'manager', delegation_id: null },
      accepted_at_ms: 200,
    },
    progress: 'requested',
    holds: ['workspace_busy'],
    observed_at_ms: 100,
    workspace_capacity_released: false,
    parent_retained: true,
    history_retained: true,
  };
}
export function response(body: unknown, status = 202) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}
